//! MTAS authority for the ExternalToolSurface machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all ExternalToolSurface state mutations flow through the machine authority.
//! Handwritten shell code calls [`ExternalToolSurfaceAuthority::apply`] and
//! executes returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/external_tool_surface.rs`:
//!
//! - 2 global phases: Operating, Shutdown
//! - 12 inputs: StageAdd, StageRemove, StageReload, ApplyBoundary,
//!   PendingSucceeded, PendingFailed, CallStarted, CallFinished,
//!   FinalizeRemovalClean, FinalizeRemovalForced, SnapshotAligned, Shutdown
//! - canonical per-surface lifecycle state plus staged ordering, pending
//!   lineage, and snapshot publication epochs
//! - 6 effects: ScheduleSurfaceCompletion, RefreshVisibleSurfaceSet,
//!   EmitExternalToolDelta, CloseSurfaceConnection, RejectSurfaceCall

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// SurfaceId newtype
// ---------------------------------------------------------------------------

/// Newtype for surface identifiers (server names in MCP context).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SurfaceId(pub String);

impl fmt::Display for SurfaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SurfaceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SurfaceId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

// ---------------------------------------------------------------------------
// Per-surface enums
// ---------------------------------------------------------------------------

/// Base lifecycle state per surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceBaseState {
    Absent,
    Active,
    Removing,
    Removed,
}

impl fmt::Display for SurfaceBaseState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => f.write_str("Absent"),
            Self::Active => f.write_str("Active"),
            Self::Removing => f.write_str("Removing"),
            Self::Removed => f.write_str("Removed"),
        }
    }
}

/// Pending async operation per surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingSurfaceOp {
    None,
    Add,
    Reload,
}

/// Staged operation per surface (intent recorded, not yet applied).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StagedSurfaceOp {
    None,
    Add,
    Remove,
    Reload,
}

/// Delta operation type for lifecycle notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDeltaOperation {
    None,
    Add,
    Remove,
    Reload,
}

impl fmt::Display for SurfaceDeltaOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Add => f.write_str("Add"),
            Self::Remove => f.write_str("Remove"),
            Self::Reload => f.write_str("Reload"),
        }
    }
}

/// Delta phase for lifecycle notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDeltaPhase {
    None,
    Pending,
    Applied,
    Draining,
    Failed,
    Forced,
}

impl fmt::Display for SurfaceDeltaPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Pending => f.write_str("Pending"),
            Self::Applied => f.write_str("Applied"),
            Self::Draining => f.write_str("Draining"),
            Self::Failed => f.write_str("Failed"),
            Self::Forced => f.write_str("Forced"),
        }
    }
}

// ---------------------------------------------------------------------------
// TurnNumber newtype
// ---------------------------------------------------------------------------

/// Turn number for tracking when operations were applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnNumber(pub u64);

// ---------------------------------------------------------------------------
// Global phase
// ---------------------------------------------------------------------------

/// Global phase of the external tool surface machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalToolSurfacePhase {
    Operating,
    Shutdown,
}

// ---------------------------------------------------------------------------
// Typed input enum
// ---------------------------------------------------------------------------

/// Typed inputs for the ExternalToolSurface machine.
///
/// Shell code classifies raw commands into these typed inputs, then calls
/// [`ExternalToolSurfaceAuthority::apply`]. The authority decides transition
/// legality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalToolSurfaceInput {
    StageAdd {
        surface_id: SurfaceId,
    },
    StageRemove {
        surface_id: SurfaceId,
    },
    StageReload {
        surface_id: SurfaceId,
    },
    ApplyBoundary {
        surface_id: SurfaceId,
        applied_at_turn: TurnNumber,
    },
    PendingSucceeded {
        surface_id: SurfaceId,
        operation: SurfaceDeltaOperation,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        applied_at_turn: TurnNumber,
    },
    PendingFailed {
        surface_id: SurfaceId,
        operation: SurfaceDeltaOperation,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        applied_at_turn: TurnNumber,
    },
    CallStarted {
        surface_id: SurfaceId,
    },
    CallFinished {
        surface_id: SurfaceId,
    },
    FinalizeRemovalClean {
        surface_id: SurfaceId,
        applied_at_turn: TurnNumber,
    },
    FinalizeRemovalForced {
        surface_id: SurfaceId,
        applied_at_turn: TurnNumber,
    },
    SnapshotAligned {
        snapshot_epoch: u64,
    },
    Shutdown,
}

// ---------------------------------------------------------------------------
// Typed effect enum
// ---------------------------------------------------------------------------

/// Effects emitted by ExternalToolSurface transitions.
///
/// Shell code receives these from [`ExternalToolSurfaceAuthority::apply`] and
/// is responsible for executing the side effects (spawning tasks, closing
/// connections, emitting events, rebuilding caches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalToolSurfaceEffect {
    /// Shell should spawn a background connection task for this surface.
    ScheduleSurfaceCompletion {
        surface_id: SurfaceId,
        operation: SurfaceDeltaOperation,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
        applied_at_turn: TurnNumber,
    },
    /// Shell should rebuild its visible tool cache from authority state.
    RefreshVisibleSurfaceSet { snapshot_epoch: u64 },
    /// Shell should emit a lifecycle delta notification.
    EmitExternalToolDelta {
        surface_id: SurfaceId,
        operation: SurfaceDeltaOperation,
        phase: SurfaceDeltaPhase,
        persisted: bool,
        applied_at_turn: TurnNumber,
    },
    /// Shell should close the connection for this surface.
    CloseSurfaceConnection { surface_id: SurfaceId },
    /// Shell should reject the in-flight call attempt with the given reason.
    RejectSurfaceCall {
        surface_id: SurfaceId,
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error returned when a transition is not legal from the current state.
#[derive(Debug, Clone)]
pub struct ExternalToolSurfaceError {
    pub input_name: String,
    pub reason: String,
}

impl fmt::Display for ExternalToolSurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ExternalToolSurface transition '{}' rejected: {}",
            self.input_name, self.reason
        )
    }
}

impl std::error::Error for ExternalToolSurfaceError {}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the ExternalToolSurface authority.
#[derive(Debug)]
pub struct ExternalToolSurfaceTransition {
    /// Name of the transition that fired (for diagnostics/tracing).
    pub transition_name: String,
    /// The global phase after the transition.
    pub phase: ExternalToolSurfacePhase,
    /// Effects to be executed by shell code.
    pub effects: Vec<ExternalToolSurfaceEffect>,
}

// ---------------------------------------------------------------------------
// Canonical machine state (per-surface maps)
// ---------------------------------------------------------------------------

/// Removal timing information for a surface in `Removing` state.
///
/// Authority-owned data that drives the clean-vs-forced removal decision.
#[derive(Debug, Clone, Copy)]
pub struct RemovalTimingInfo {
    pub draining_since: Instant,
    pub timeout_at: Instant,
    pub applied_at_turn: TurnNumber,
}

/// Canonical machine-owned state for ExternalToolSurface.
///
/// All per-surface lifecycle truth lives here. Shell code may read these
/// fields (for cache rebuilding, diagnostics) but must never write them
/// directly.
#[derive(Debug, Clone)]
struct ExternalToolSurfaceFields {
    known_surfaces: BTreeSet<SurfaceId>,
    visible_surfaces: BTreeSet<SurfaceId>,
    base_state: HashMap<SurfaceId, SurfaceBaseState>,
    pending_op: HashMap<SurfaceId, PendingSurfaceOp>,
    staged_op: HashMap<SurfaceId, StagedSurfaceOp>,
    staged_intent_sequence: HashMap<SurfaceId, u64>,
    next_staged_intent_sequence: u64,
    pending_task_sequence: HashMap<SurfaceId, u64>,
    pending_lineage_sequence: HashMap<SurfaceId, u64>,
    next_pending_task_sequence: u64,
    inflight_calls: HashMap<SurfaceId, u64>,
    last_delta_operation: HashMap<SurfaceId, SurfaceDeltaOperation>,
    last_delta_phase: HashMap<SurfaceId, SurfaceDeltaPhase>,
    snapshot_epoch: u64,
    snapshot_aligned_epoch: u64,
    /// Removal timing per surface (set when entering Removing, cleared on finalization).
    removal_timing: HashMap<SurfaceId, RemovalTimingInfo>,
}

impl ExternalToolSurfaceFields {
    fn new() -> Self {
        Self {
            known_surfaces: BTreeSet::new(),
            visible_surfaces: BTreeSet::new(),
            base_state: HashMap::new(),
            pending_op: HashMap::new(),
            staged_op: HashMap::new(),
            staged_intent_sequence: HashMap::new(),
            next_staged_intent_sequence: 1,
            pending_task_sequence: HashMap::new(),
            pending_lineage_sequence: HashMap::new(),
            next_pending_task_sequence: 1,
            inflight_calls: HashMap::new(),
            last_delta_operation: HashMap::new(),
            last_delta_phase: HashMap::new(),
            snapshot_epoch: 0,
            snapshot_aligned_epoch: 0,
            removal_timing: HashMap::new(),
        }
    }

    // --- Helper functions matching the schema's HelperSchema ---

    fn surface_base(&self, id: &SurfaceId) -> SurfaceBaseState {
        self.base_state
            .get(id)
            .copied()
            .unwrap_or(SurfaceBaseState::Absent)
    }

    fn pending_op(&self, id: &SurfaceId) -> PendingSurfaceOp {
        self.pending_op
            .get(id)
            .copied()
            .unwrap_or(PendingSurfaceOp::None)
    }

    fn staged_op(&self, id: &SurfaceId) -> StagedSurfaceOp {
        self.staged_op
            .get(id)
            .copied()
            .unwrap_or(StagedSurfaceOp::None)
    }

    fn inflight_call_count(&self, id: &SurfaceId) -> u64 {
        self.inflight_calls.get(id).copied().unwrap_or(0)
    }

    fn staged_intent_sequence(&self, id: &SurfaceId) -> u64 {
        self.staged_intent_sequence.get(id).copied().unwrap_or(0)
    }

    fn pending_task_sequence(&self, id: &SurfaceId) -> u64 {
        self.pending_task_sequence.get(id).copied().unwrap_or(0)
    }

    fn pending_lineage_sequence(&self, id: &SurfaceId) -> u64 {
        self.pending_lineage_sequence.get(id).copied().unwrap_or(0)
    }

    fn is_visible(&self, id: &SurfaceId) -> bool {
        self.visible_surfaces.contains(id)
    }
}

// ---------------------------------------------------------------------------
// Sealed mutator trait
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for ExternalToolSurface state mutation.
///
/// Only [`ExternalToolSurfaceAuthority`] implements this. Handwritten code
/// cannot create alternative implementations, ensuring single-source-of-truth
/// semantics for lifecycle state.
pub trait ExternalToolSurfaceMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(
        &mut self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for ExternalToolSurface state.
///
/// Holds the canonical global phase + per-surface field maps and delegates
/// all transitions through the encoded transition table.
pub struct ExternalToolSurfaceAuthority {
    phase: ExternalToolSurfacePhase,
    fields: ExternalToolSurfaceFields,
    /// Default removal timeout duration (used when entering Removing state).
    removal_timeout: Duration,
}

/// Default removal timeout duration (30 seconds).
const DEFAULT_AUTHORITY_REMOVAL_TIMEOUT: Duration = Duration::from_secs(30);

impl sealed::Sealed for ExternalToolSurfaceAuthority {}

impl ExternalToolSurfaceAuthority {
    /// Create a new authority in the initial Operating phase with empty state.
    pub fn new() -> Self {
        Self {
            phase: ExternalToolSurfacePhase::Operating,
            fields: ExternalToolSurfaceFields::new(),
            removal_timeout: DEFAULT_AUTHORITY_REMOVAL_TIMEOUT,
        }
    }

    /// Create a new authority with a custom removal timeout duration.
    pub fn with_removal_timeout(removal_timeout: Duration) -> Self {
        Self {
            removal_timeout,
            ..Self::new()
        }
    }

    /// Set the removal timeout duration while the machine is operating.
    pub fn set_removal_timeout(
        &mut self,
        removal_timeout: Duration,
    ) -> Result<(), ExternalToolSurfaceError> {
        self.require_operating("set_removal_timeout")?;
        self.removal_timeout = removal_timeout;
        Ok(())
    }

    /// Current global phase.
    pub fn phase(&self) -> ExternalToolSurfacePhase {
        self.phase
    }

    /// Read the base state for a surface (Absent if unknown).
    pub fn surface_base(&self, id: &SurfaceId) -> SurfaceBaseState {
        self.fields.surface_base(id)
    }

    /// Read the pending operation for a surface.
    pub fn pending_op(&self, id: &SurfaceId) -> PendingSurfaceOp {
        self.fields.pending_op(id)
    }

    /// Read the staged intent sequence for a surface.
    pub fn staged_intent_sequence(&self, id: &SurfaceId) -> u64 {
        self.fields.staged_intent_sequence(id)
    }

    /// Read the current pending task sequence for a surface.
    pub fn pending_task_sequence(&self, id: &SurfaceId) -> u64 {
        self.fields.pending_task_sequence(id)
    }

    /// Read the lineage sequence attached to the current pending task.
    pub fn pending_lineage_sequence(&self, id: &SurfaceId) -> u64 {
        self.fields.pending_lineage_sequence(id)
    }

    /// Read the staged operation for a surface.
    pub fn staged_op(&self, id: &SurfaceId) -> StagedSurfaceOp {
        self.fields.staged_op(id)
    }

    /// Read the inflight call count for a surface.
    pub fn inflight_call_count(&self, id: &SurfaceId) -> u64 {
        self.fields.inflight_call_count(id)
    }

    /// Check if a surface is in the visible set.
    pub fn is_visible(&self, id: &SurfaceId) -> bool {
        self.fields.is_visible(id)
    }

    /// Iterate over all known surface IDs.
    pub fn known_surfaces(&self) -> impl Iterator<Item = &SurfaceId> {
        self.fields.known_surfaces.iter()
    }

    /// Iterate over all visible surface IDs.
    pub fn visible_surfaces(&self) -> impl Iterator<Item = &SurfaceId> {
        self.fields.visible_surfaces.iter()
    }

    /// Iterate over staged intents in canonical sequence order.
    pub fn staged_intents_in_order(&self) -> Vec<(SurfaceId, StagedSurfaceOp, u64)> {
        let mut staged = self
            .fields
            .known_surfaces
            .iter()
            .filter_map(|surface_id| {
                let staged_op = self.fields.staged_op(surface_id);
                if staged_op == StagedSurfaceOp::None {
                    return None;
                }
                Some((
                    surface_id.clone(),
                    staged_op,
                    self.fields.staged_intent_sequence(surface_id),
                ))
            })
            .collect::<Vec<_>>();
        staged.sort_by_key(|(_, _, sequence)| *sequence);
        staged
    }

    /// Get removal timing info for a surface (only set while in Removing state).
    pub fn removal_timing(&self, id: &SurfaceId) -> Option<RemovalTimingInfo> {
        self.fields.removal_timing.get(id).copied()
    }

    /// Return all surface IDs that are currently in Removing state.
    pub fn removing_surfaces(&self) -> impl Iterator<Item = &SurfaceId> {
        self.fields.base_state.iter().filter_map(|(id, state)| {
            if *state == SurfaceBaseState::Removing {
                Some(id)
            } else {
                None
            }
        })
    }

    /// Return true if any surface has a pending async operation or staged op.
    pub fn has_pending_or_staged(&self) -> bool {
        self.fields
            .pending_op
            .values()
            .any(|op| *op != PendingSurfaceOp::None)
            || self
                .fields
                .staged_op
                .values()
                .any(|op| *op != StagedSurfaceOp::None)
    }

    /// Return the count of surfaces with pending async operations.
    pub fn pending_count(&self) -> usize {
        self.fields
            .pending_op
            .values()
            .filter(|op| **op != PendingSurfaceOp::None)
            .count()
    }

    /// Return all surface IDs that are currently waiting on async completion.
    pub fn pending_surfaces(&self) -> impl Iterator<Item = &SurfaceId> {
        self.fields.pending_op.iter().filter_map(|(id, op)| {
            if *op == PendingSurfaceOp::None {
                None
            } else {
                Some(id)
            }
        })
    }

    /// Current published snapshot epoch requested by the authority.
    pub fn snapshot_epoch(&self) -> u64 {
        self.fields.snapshot_epoch
    }

    fn advance_snapshot_epoch(fields: &mut ExternalToolSurfaceFields) -> u64 {
        if fields.snapshot_epoch == fields.snapshot_aligned_epoch {
            fields.snapshot_epoch = fields.snapshot_epoch.saturating_add(1);
        }
        fields.snapshot_epoch
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: &ExternalToolSurfaceInput,
    ) -> Result<
        (
            ExternalToolSurfacePhase,
            ExternalToolSurfaceFields,
            Vec<ExternalToolSurfaceEffect>,
            String,
        ),
        ExternalToolSurfaceError,
    > {
        let mut fields = self.fields.clone();
        let mut effects: Vec<ExternalToolSurfaceEffect> = Vec::new();

        match input {
            // ----------------------------------------------------------
            // StageAdd: Operating -> Operating (no guards)
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::StageAdd { surface_id } => {
                self.require_operating("StageAdd")?;
                fields.known_surfaces.insert(surface_id.clone());
                fields
                    .staged_op
                    .insert(surface_id.clone(), StagedSurfaceOp::Add);
                fields
                    .staged_intent_sequence
                    .insert(surface_id.clone(), fields.next_staged_intent_sequence);
                fields.next_staged_intent_sequence =
                    fields.next_staged_intent_sequence.saturating_add(1);
                Ok((
                    ExternalToolSurfacePhase::Operating,
                    fields,
                    effects,
                    "StageAdd".into(),
                ))
            }

            // ----------------------------------------------------------
            // StageRemove: Operating -> Operating (no guards)
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::StageRemove { surface_id } => {
                self.require_operating("StageRemove")?;
                fields.known_surfaces.insert(surface_id.clone());
                fields
                    .staged_op
                    .insert(surface_id.clone(), StagedSurfaceOp::Remove);
                fields
                    .staged_intent_sequence
                    .insert(surface_id.clone(), fields.next_staged_intent_sequence);
                fields.next_staged_intent_sequence =
                    fields.next_staged_intent_sequence.saturating_add(1);
                Ok((
                    ExternalToolSurfacePhase::Operating,
                    fields,
                    effects,
                    "StageRemove".into(),
                ))
            }

            // ----------------------------------------------------------
            // StageReload: Operating -> Operating
            // Guard: surface_is_active
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::StageReload { surface_id } => {
                self.require_operating("StageReload")?;
                if self.fields.surface_base(surface_id) != SurfaceBaseState::Active {
                    return Err(ExternalToolSurfaceError {
                        input_name: "StageReload".into(),
                        reason: format!(
                            "surface '{}' is not Active (is {:?})",
                            surface_id,
                            self.fields.surface_base(surface_id)
                        ),
                    });
                }
                fields.known_surfaces.insert(surface_id.clone());
                fields
                    .staged_op
                    .insert(surface_id.clone(), StagedSurfaceOp::Reload);
                fields
                    .staged_intent_sequence
                    .insert(surface_id.clone(), fields.next_staged_intent_sequence);
                fields.next_staged_intent_sequence =
                    fields.next_staged_intent_sequence.saturating_add(1);
                Ok((
                    ExternalToolSurfacePhase::Operating,
                    fields,
                    effects,
                    "StageReload".into(),
                ))
            }

            // ----------------------------------------------------------
            // ApplyBoundary: dispatches to 4 sub-transitions based on
            // staged_op and base_state.
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::ApplyBoundary {
                surface_id,
                applied_at_turn,
            } => {
                self.require_operating("ApplyBoundary")?;
                let staged = self.fields.staged_op(surface_id);
                let base = self.fields.surface_base(surface_id);
                let pending = self.fields.pending_op(surface_id);

                if pending != PendingSurfaceOp::None {
                    return Err(ExternalToolSurfaceError {
                        input_name: "ApplyBoundary".into(),
                        reason: format!(
                            "cannot apply staged boundary for surface '{surface_id}' while pending operation {pending:?} is unresolved"
                        ),
                    });
                }

                match staged {
                    StagedSurfaceOp::Add => {
                        // Guard: base_state_accepts_add (Absent | Active | Removed)
                        if !matches!(
                            base,
                            SurfaceBaseState::Absent
                                | SurfaceBaseState::Active
                                | SurfaceBaseState::Removed
                        ) {
                            return Err(ExternalToolSurfaceError {
                                input_name: "ApplyBoundary".into(),
                                reason: format!(
                                    "cannot add surface '{surface_id}' in base state {base:?}"
                                ),
                            });
                        }
                        fields.known_surfaces.insert(surface_id.clone());
                        let staged_intent_sequence = self.fields.staged_intent_sequence(surface_id);
                        let pending_task_sequence = fields.next_pending_task_sequence;
                        fields.next_pending_task_sequence =
                            fields.next_pending_task_sequence.saturating_add(1);
                        fields
                            .pending_op
                            .insert(surface_id.clone(), PendingSurfaceOp::Add);
                        fields
                            .pending_task_sequence
                            .insert(surface_id.clone(), pending_task_sequence);
                        fields
                            .pending_lineage_sequence
                            .insert(surface_id.clone(), staged_intent_sequence);
                        fields
                            .staged_op
                            .insert(surface_id.clone(), StagedSurfaceOp::None);
                        fields.staged_intent_sequence.remove(surface_id);
                        fields
                            .last_delta_operation
                            .insert(surface_id.clone(), SurfaceDeltaOperation::Add);
                        fields
                            .last_delta_phase
                            .insert(surface_id.clone(), SurfaceDeltaPhase::Pending);
                        effects.push(ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Add,
                            pending_task_sequence,
                            staged_intent_sequence,
                            applied_at_turn: *applied_at_turn,
                        });
                        effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Add,
                            phase: SurfaceDeltaPhase::Pending,
                            persisted: false,
                            applied_at_turn: *applied_at_turn,
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "ApplyBoundaryAdd".into(),
                        ))
                    }

                    StagedSurfaceOp::Reload => {
                        // Guard: reload_requires_active_base
                        if base != SurfaceBaseState::Active {
                            return Err(ExternalToolSurfaceError {
                                input_name: "ApplyBoundary".into(),
                                reason: format!(
                                    "cannot reload surface '{surface_id}' in base state {base:?}"
                                ),
                            });
                        }
                        fields.known_surfaces.insert(surface_id.clone());
                        let staged_intent_sequence = self.fields.staged_intent_sequence(surface_id);
                        let pending_task_sequence = fields.next_pending_task_sequence;
                        fields.next_pending_task_sequence =
                            fields.next_pending_task_sequence.saturating_add(1);
                        fields
                            .pending_op
                            .insert(surface_id.clone(), PendingSurfaceOp::Reload);
                        fields
                            .pending_task_sequence
                            .insert(surface_id.clone(), pending_task_sequence);
                        fields
                            .pending_lineage_sequence
                            .insert(surface_id.clone(), staged_intent_sequence);
                        fields
                            .staged_op
                            .insert(surface_id.clone(), StagedSurfaceOp::None);
                        fields.staged_intent_sequence.remove(surface_id);
                        fields
                            .last_delta_operation
                            .insert(surface_id.clone(), SurfaceDeltaOperation::Reload);
                        fields
                            .last_delta_phase
                            .insert(surface_id.clone(), SurfaceDeltaPhase::Pending);
                        effects.push(ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Reload,
                            pending_task_sequence,
                            staged_intent_sequence,
                            applied_at_turn: *applied_at_turn,
                        });
                        effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Reload,
                            phase: SurfaceDeltaPhase::Pending,
                            persisted: false,
                            applied_at_turn: *applied_at_turn,
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "ApplyBoundaryReload".into(),
                        ))
                    }

                    StagedSurfaceOp::Remove => {
                        if base == SurfaceBaseState::Active {
                            // ApplyBoundaryRemoveDraining
                            let draining_since = Instant::now();
                            fields.known_surfaces.insert(surface_id.clone());
                            fields
                                .staged_op
                                .insert(surface_id.clone(), StagedSurfaceOp::None);
                            fields.staged_intent_sequence.remove(surface_id);
                            fields
                                .pending_op
                                .insert(surface_id.clone(), PendingSurfaceOp::None);
                            fields.pending_task_sequence.insert(surface_id.clone(), 0);
                            fields
                                .pending_lineage_sequence
                                .insert(surface_id.clone(), 0);
                            fields
                                .base_state
                                .insert(surface_id.clone(), SurfaceBaseState::Removing);
                            fields.removal_timing.insert(
                                surface_id.clone(),
                                RemovalTimingInfo {
                                    draining_since,
                                    timeout_at: draining_since + self.removal_timeout,
                                    applied_at_turn: *applied_at_turn,
                                },
                            );
                            fields
                                .last_delta_operation
                                .insert(surface_id.clone(), SurfaceDeltaOperation::Remove);
                            fields
                                .last_delta_phase
                                .insert(surface_id.clone(), SurfaceDeltaPhase::Draining);
                            fields.visible_surfaces.remove(surface_id);
                            let snapshot_epoch = Self::advance_snapshot_epoch(&mut fields);
                            effects.push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet {
                                snapshot_epoch,
                            });
                            effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                                surface_id: surface_id.clone(),
                                operation: SurfaceDeltaOperation::Remove,
                                phase: SurfaceDeltaPhase::Draining,
                                persisted: false,
                                applied_at_turn: *applied_at_turn,
                            });
                            Ok((
                                ExternalToolSurfacePhase::Operating,
                                fields,
                                effects,
                                "ApplyBoundaryRemoveDraining".into(),
                            ))
                        } else {
                            // ApplyBoundaryRemoveNoop — not active, clear staged
                            fields.known_surfaces.insert(surface_id.clone());
                            fields
                                .staged_op
                                .insert(surface_id.clone(), StagedSurfaceOp::None);
                            fields.staged_intent_sequence.remove(surface_id);
                            fields
                                .pending_op
                                .insert(surface_id.clone(), PendingSurfaceOp::None);
                            fields.pending_task_sequence.insert(surface_id.clone(), 0);
                            fields
                                .pending_lineage_sequence
                                .insert(surface_id.clone(), 0);
                            Ok((
                                ExternalToolSurfacePhase::Operating,
                                fields,
                                effects,
                                "ApplyBoundaryRemoveNoop".into(),
                            ))
                        }
                    }

                    StagedSurfaceOp::None => Err(ExternalToolSurfaceError {
                        input_name: "ApplyBoundary".into(),
                        reason: format!("no staged operation for surface '{surface_id}'"),
                    }),
                }
            }

            // ----------------------------------------------------------
            // PendingSucceeded: dispatches to Add or Reload
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::PendingSucceeded {
                surface_id,
                operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => {
                self.require_operating("PendingSucceeded")?;
                let pending = self.fields.pending_op(surface_id);
                let expected_operation = match pending {
                    PendingSurfaceOp::Add => SurfaceDeltaOperation::Add,
                    PendingSurfaceOp::Reload => SurfaceDeltaOperation::Reload,
                    PendingSurfaceOp::None => SurfaceDeltaOperation::None,
                };

                if *operation != expected_operation {
                    return Err(ExternalToolSurfaceError {
                        input_name: "PendingSucceeded".into(),
                        reason: format!(
                            "pending operation for surface '{surface_id}' does not match completion operation {operation}"
                        ),
                    });
                }
                if self.fields.pending_task_sequence(surface_id) != *pending_task_sequence {
                    return Err(ExternalToolSurfaceError {
                        input_name: "PendingSucceeded".into(),
                        reason: format!(
                            "pending task sequence mismatch for surface '{surface_id}': expected {}, got {}",
                            self.fields.pending_task_sequence(surface_id),
                            pending_task_sequence
                        ),
                    });
                }
                if self.fields.pending_lineage_sequence(surface_id) != *staged_intent_sequence {
                    return Err(ExternalToolSurfaceError {
                        input_name: "PendingSucceeded".into(),
                        reason: format!(
                            "pending lineage mismatch for surface '{surface_id}': expected {}, got {}",
                            self.fields.pending_lineage_sequence(surface_id),
                            staged_intent_sequence
                        ),
                    });
                }

                match pending {
                    PendingSurfaceOp::Add => {
                        fields.known_surfaces.insert(surface_id.clone());
                        fields
                            .pending_op
                            .insert(surface_id.clone(), PendingSurfaceOp::None);
                        fields.pending_task_sequence.insert(surface_id.clone(), 0);
                        fields
                            .pending_lineage_sequence
                            .insert(surface_id.clone(), 0);
                        fields
                            .base_state
                            .insert(surface_id.clone(), SurfaceBaseState::Active);
                        fields
                            .last_delta_operation
                            .insert(surface_id.clone(), SurfaceDeltaOperation::Add);
                        fields
                            .last_delta_phase
                            .insert(surface_id.clone(), SurfaceDeltaPhase::Applied);
                        fields.visible_surfaces.insert(surface_id.clone());
                        let snapshot_epoch = Self::advance_snapshot_epoch(&mut fields);
                        effects.push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet {
                            snapshot_epoch,
                        });
                        effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Add,
                            phase: SurfaceDeltaPhase::Applied,
                            persisted: true,
                            applied_at_turn: *applied_at_turn,
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "PendingSucceededAdd".into(),
                        ))
                    }
                    PendingSurfaceOp::Reload => {
                        fields.known_surfaces.insert(surface_id.clone());
                        fields
                            .pending_op
                            .insert(surface_id.clone(), PendingSurfaceOp::None);
                        fields.pending_task_sequence.insert(surface_id.clone(), 0);
                        fields
                            .pending_lineage_sequence
                            .insert(surface_id.clone(), 0);
                        fields
                            .base_state
                            .insert(surface_id.clone(), SurfaceBaseState::Active);
                        fields
                            .last_delta_operation
                            .insert(surface_id.clone(), SurfaceDeltaOperation::Reload);
                        fields
                            .last_delta_phase
                            .insert(surface_id.clone(), SurfaceDeltaPhase::Applied);
                        fields.visible_surfaces.insert(surface_id.clone());
                        let snapshot_epoch = Self::advance_snapshot_epoch(&mut fields);
                        effects.push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet {
                            snapshot_epoch,
                        });
                        effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Reload,
                            phase: SurfaceDeltaPhase::Applied,
                            persisted: true,
                            applied_at_turn: *applied_at_turn,
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "PendingSucceededReload".into(),
                        ))
                    }
                    PendingSurfaceOp::None => Err(ExternalToolSurfaceError {
                        input_name: "PendingSucceeded".into(),
                        reason: format!("no pending operation for surface '{surface_id}'"),
                    }),
                }
            }

            // ----------------------------------------------------------
            // PendingFailed: dispatches to Add or Reload
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::PendingFailed {
                surface_id,
                operation,
                pending_task_sequence,
                staged_intent_sequence,
                applied_at_turn,
            } => {
                self.require_operating("PendingFailed")?;
                let pending = self.fields.pending_op(surface_id);
                let expected_operation = match pending {
                    PendingSurfaceOp::Add => SurfaceDeltaOperation::Add,
                    PendingSurfaceOp::Reload => SurfaceDeltaOperation::Reload,
                    PendingSurfaceOp::None => SurfaceDeltaOperation::None,
                };

                if *operation != expected_operation {
                    return Err(ExternalToolSurfaceError {
                        input_name: "PendingFailed".into(),
                        reason: format!(
                            "pending operation for surface '{surface_id}' does not match failed operation {operation}"
                        ),
                    });
                }
                if self.fields.pending_task_sequence(surface_id) != *pending_task_sequence {
                    return Err(ExternalToolSurfaceError {
                        input_name: "PendingFailed".into(),
                        reason: format!(
                            "pending task sequence mismatch for surface '{surface_id}': expected {}, got {}",
                            self.fields.pending_task_sequence(surface_id),
                            pending_task_sequence
                        ),
                    });
                }
                if self.fields.pending_lineage_sequence(surface_id) != *staged_intent_sequence {
                    return Err(ExternalToolSurfaceError {
                        input_name: "PendingFailed".into(),
                        reason: format!(
                            "pending lineage mismatch for surface '{surface_id}': expected {}, got {}",
                            self.fields.pending_lineage_sequence(surface_id),
                            staged_intent_sequence
                        ),
                    });
                }

                match pending {
                    PendingSurfaceOp::Add => {
                        fields.known_surfaces.insert(surface_id.clone());
                        fields
                            .pending_op
                            .insert(surface_id.clone(), PendingSurfaceOp::None);
                        fields.pending_task_sequence.insert(surface_id.clone(), 0);
                        fields
                            .pending_lineage_sequence
                            .insert(surface_id.clone(), 0);
                        fields
                            .last_delta_operation
                            .insert(surface_id.clone(), SurfaceDeltaOperation::Add);
                        fields
                            .last_delta_phase
                            .insert(surface_id.clone(), SurfaceDeltaPhase::Failed);
                        effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Add,
                            phase: SurfaceDeltaPhase::Failed,
                            persisted: true,
                            applied_at_turn: *applied_at_turn,
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "PendingFailedAdd".into(),
                        ))
                    }
                    PendingSurfaceOp::Reload => {
                        fields.known_surfaces.insert(surface_id.clone());
                        fields
                            .pending_op
                            .insert(surface_id.clone(), PendingSurfaceOp::None);
                        fields.pending_task_sequence.insert(surface_id.clone(), 0);
                        fields
                            .pending_lineage_sequence
                            .insert(surface_id.clone(), 0);
                        fields
                            .last_delta_operation
                            .insert(surface_id.clone(), SurfaceDeltaOperation::Reload);
                        fields
                            .last_delta_phase
                            .insert(surface_id.clone(), SurfaceDeltaPhase::Failed);
                        effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                            surface_id: surface_id.clone(),
                            operation: SurfaceDeltaOperation::Reload,
                            phase: SurfaceDeltaPhase::Failed,
                            persisted: true,
                            applied_at_turn: *applied_at_turn,
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "PendingFailedReload".into(),
                        ))
                    }
                    PendingSurfaceOp::None => Err(ExternalToolSurfaceError {
                        input_name: "PendingFailed".into(),
                        reason: format!("no pending operation for surface '{surface_id}'"),
                    }),
                }
            }

            // ----------------------------------------------------------
            // CallStarted: dispatches to Active, RejectRemoving, or
            // RejectUnavailable
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::CallStarted { surface_id } => {
                self.require_operating("CallStarted")?;
                let base = self.fields.surface_base(surface_id);
                fields.known_surfaces.insert(surface_id.clone());

                match base {
                    SurfaceBaseState::Active => {
                        let count = self.fields.inflight_call_count(surface_id);
                        fields.inflight_calls.insert(surface_id.clone(), count + 1);
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "CallStartedActive".into(),
                        ))
                    }
                    SurfaceBaseState::Removing => {
                        effects.push(ExternalToolSurfaceEffect::RejectSurfaceCall {
                            surface_id: surface_id.clone(),
                            reason: "surface_draining".into(),
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "CallStartedRejectWhileRemoving".into(),
                        ))
                    }
                    _ => {
                        effects.push(ExternalToolSurfaceEffect::RejectSurfaceCall {
                            surface_id: surface_id.clone(),
                            reason: "surface_unavailable".into(),
                        });
                        Ok((
                            ExternalToolSurfacePhase::Operating,
                            fields,
                            effects,
                            "CallStartedRejectWhileUnavailable".into(),
                        ))
                    }
                }
            }

            // ----------------------------------------------------------
            // CallFinished: dispatches to Active or Removing
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::CallFinished { surface_id } => {
                self.require_operating("CallFinished")?;
                let base = self.fields.surface_base(surface_id);
                let count = self.fields.inflight_call_count(surface_id);

                if !matches!(base, SurfaceBaseState::Active | SurfaceBaseState::Removing) {
                    return Err(ExternalToolSurfaceError {
                        input_name: "CallFinished".into(),
                        reason: format!(
                            "surface '{surface_id}' is not Active or Removing (is {base:?})"
                        ),
                    });
                }
                if count == 0 {
                    return Err(ExternalToolSurfaceError {
                        input_name: "CallFinished".into(),
                        reason: format!("surface '{surface_id}' has no inflight calls"),
                    });
                }

                fields.known_surfaces.insert(surface_id.clone());
                fields.inflight_calls.insert(surface_id.clone(), count - 1);

                let transition_name = match base {
                    SurfaceBaseState::Active => "CallFinishedActive",
                    SurfaceBaseState::Removing => "CallFinishedRemoving",
                    other => {
                        return Err(ExternalToolSurfaceError {
                            input_name: "CallFinished".into(),
                            reason: format!(
                                "surface '{surface_id}' entered invalid base state during CallFinished ({other:?})"
                            ),
                        });
                    }
                };
                Ok((
                    ExternalToolSurfacePhase::Operating,
                    fields,
                    effects,
                    transition_name.into(),
                ))
            }

            // ----------------------------------------------------------
            // FinalizeRemovalClean: Removing -> Removed
            // Guards: surface_is_removing, no_inflight_calls_remain
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::FinalizeRemovalClean {
                surface_id,
                applied_at_turn,
            } => {
                self.require_operating("FinalizeRemovalClean")?;
                let base = self.fields.surface_base(surface_id);
                if base != SurfaceBaseState::Removing {
                    return Err(ExternalToolSurfaceError {
                        input_name: "FinalizeRemovalClean".into(),
                        reason: format!("surface '{surface_id}' is not Removing (is {base:?})"),
                    });
                }
                let count = self.fields.inflight_call_count(surface_id);
                if count != 0 {
                    return Err(ExternalToolSurfaceError {
                        input_name: "FinalizeRemovalClean".into(),
                        reason: format!("surface '{surface_id}' still has {count} inflight calls"),
                    });
                }

                fields.known_surfaces.insert(surface_id.clone());
                fields
                    .base_state
                    .insert(surface_id.clone(), SurfaceBaseState::Removed);
                fields
                    .pending_op
                    .insert(surface_id.clone(), PendingSurfaceOp::None);
                fields.pending_task_sequence.insert(surface_id.clone(), 0);
                fields
                    .pending_lineage_sequence
                    .insert(surface_id.clone(), 0);
                fields
                    .last_delta_operation
                    .insert(surface_id.clone(), SurfaceDeltaOperation::Remove);
                fields
                    .last_delta_phase
                    .insert(surface_id.clone(), SurfaceDeltaPhase::Applied);
                fields.visible_surfaces.remove(surface_id);
                fields.removal_timing.remove(surface_id);

                effects.push(ExternalToolSurfaceEffect::CloseSurfaceConnection {
                    surface_id: surface_id.clone(),
                });
                let snapshot_epoch = Self::advance_snapshot_epoch(&mut fields);
                effects
                    .push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch });
                effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id: surface_id.clone(),
                    operation: SurfaceDeltaOperation::Remove,
                    phase: SurfaceDeltaPhase::Applied,
                    persisted: true,
                    applied_at_turn: *applied_at_turn,
                });

                Ok((
                    ExternalToolSurfacePhase::Operating,
                    fields,
                    effects,
                    "FinalizeRemovalClean".into(),
                ))
            }

            // ----------------------------------------------------------
            // FinalizeRemovalForced: Removing -> Removed (timeout path)
            // Guard: surface_is_removing (no inflight guard — forced)
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::FinalizeRemovalForced {
                surface_id,
                applied_at_turn,
            } => {
                self.require_operating("FinalizeRemovalForced")?;
                let base = self.fields.surface_base(surface_id);
                if base != SurfaceBaseState::Removing {
                    return Err(ExternalToolSurfaceError {
                        input_name: "FinalizeRemovalForced".into(),
                        reason: format!("surface '{surface_id}' is not Removing (is {base:?})"),
                    });
                }

                fields.known_surfaces.insert(surface_id.clone());
                fields
                    .base_state
                    .insert(surface_id.clone(), SurfaceBaseState::Removed);
                fields
                    .pending_op
                    .insert(surface_id.clone(), PendingSurfaceOp::None);
                fields.pending_task_sequence.insert(surface_id.clone(), 0);
                fields
                    .pending_lineage_sequence
                    .insert(surface_id.clone(), 0);
                fields.inflight_calls.insert(surface_id.clone(), 0);
                fields
                    .last_delta_operation
                    .insert(surface_id.clone(), SurfaceDeltaOperation::Remove);
                fields
                    .last_delta_phase
                    .insert(surface_id.clone(), SurfaceDeltaPhase::Forced);
                fields.visible_surfaces.remove(surface_id);
                fields.removal_timing.remove(surface_id);

                effects.push(ExternalToolSurfaceEffect::CloseSurfaceConnection {
                    surface_id: surface_id.clone(),
                });
                let snapshot_epoch = Self::advance_snapshot_epoch(&mut fields);
                effects
                    .push(ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch });
                effects.push(ExternalToolSurfaceEffect::EmitExternalToolDelta {
                    surface_id: surface_id.clone(),
                    operation: SurfaceDeltaOperation::Remove,
                    phase: SurfaceDeltaPhase::Forced,
                    persisted: true,
                    applied_at_turn: *applied_at_turn,
                });

                Ok((
                    ExternalToolSurfacePhase::Operating,
                    fields,
                    effects,
                    "FinalizeRemovalForced".into(),
                ))
            }

            // ----------------------------------------------------------
            // SnapshotAligned: acknowledge an atomically published projection snapshot.
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::SnapshotAligned { snapshot_epoch } => {
                self.require_operating("SnapshotAligned")?;
                if *snapshot_epoch != self.fields.snapshot_epoch {
                    return Err(ExternalToolSurfaceError {
                        input_name: "SnapshotAligned".into(),
                        reason: format!(
                            "snapshot epoch mismatch: expected {}, got {}",
                            self.fields.snapshot_epoch, snapshot_epoch
                        ),
                    });
                }
                if self.fields.snapshot_epoch <= self.fields.snapshot_aligned_epoch {
                    return Err(ExternalToolSurfaceError {
                        input_name: "SnapshotAligned".into(),
                        reason: format!(
                            "snapshot epoch {} is already aligned at {}",
                            self.fields.snapshot_epoch, self.fields.snapshot_aligned_epoch
                        ),
                    });
                }
                fields.snapshot_aligned_epoch = *snapshot_epoch;
                Ok((
                    ExternalToolSurfacePhase::Operating,
                    fields,
                    effects,
                    "SnapshotAligned".into(),
                ))
            }

            // ----------------------------------------------------------
            // Shutdown: Operating|Shutdown -> Shutdown (clear all)
            // ----------------------------------------------------------
            ExternalToolSurfaceInput::Shutdown => {
                // Shutdown is valid from both Operating and Shutdown.
                fields.known_surfaces.clear();
                fields.visible_surfaces.clear();
                fields.base_state.clear();
                fields.pending_op.clear();
                fields.staged_op.clear();
                fields.staged_intent_sequence.clear();
                fields.next_staged_intent_sequence = 1;
                fields.pending_task_sequence.clear();
                fields.pending_lineage_sequence.clear();
                fields.next_pending_task_sequence = 1;
                fields.inflight_calls.clear();
                fields.last_delta_operation.clear();
                fields.last_delta_phase.clear();
                fields.snapshot_epoch = 0;
                fields.snapshot_aligned_epoch = 0;
                fields.removal_timing.clear();
                Ok((
                    ExternalToolSurfacePhase::Shutdown,
                    fields,
                    effects,
                    "Shutdown".into(),
                ))
            }
        }
    }

    /// Check global phase is Operating.
    fn require_operating(&self, input_name: &str) -> Result<(), ExternalToolSurfaceError> {
        if self.phase != ExternalToolSurfacePhase::Operating {
            return Err(ExternalToolSurfaceError {
                input_name: input_name.into(),
                reason: "machine is in Shutdown phase".into(),
            });
        }
        Ok(())
    }

    /// Check schema and keyspace invariants (debug builds only).
    #[cfg(debug_assertions)]
    fn check_invariants(&self) {
        debug_assert!(
            self.fields.snapshot_aligned_epoch <= self.fields.snapshot_epoch,
            "invariant: snapshot_aligned_epoch must not exceed snapshot_epoch"
        );

        for surface_id in &self.fields.visible_surfaces {
            debug_assert!(
                self.fields.known_surfaces.contains(surface_id),
                "invariant: visible surface '{surface_id}' must be in known_surfaces"
            );
        }

        for (label, keys) in [
            (
                "base_state",
                self.fields.base_state.keys().cloned().collect::<Vec<_>>(),
            ),
            (
                "pending_op",
                self.fields.pending_op.keys().cloned().collect::<Vec<_>>(),
            ),
            (
                "staged_op",
                self.fields.staged_op.keys().cloned().collect::<Vec<_>>(),
            ),
            (
                "staged_intent_sequence",
                self.fields
                    .staged_intent_sequence
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            (
                "pending_task_sequence",
                self.fields
                    .pending_task_sequence
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            (
                "pending_lineage_sequence",
                self.fields
                    .pending_lineage_sequence
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            (
                "inflight_calls",
                self.fields
                    .inflight_calls
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            (
                "last_delta_operation",
                self.fields
                    .last_delta_operation
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            (
                "last_delta_phase",
                self.fields
                    .last_delta_phase
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            (
                "removal_timing",
                self.fields
                    .removal_timing
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
        ] {
            for surface_id in keys {
                debug_assert!(
                    self.fields.known_surfaces.contains(&surface_id),
                    "invariant: key '{surface_id}' in map {label} must be in known_surfaces"
                );
            }
        }

        for surface_id in &self.fields.known_surfaces {
            let base = self.fields.surface_base(surface_id);
            let pending = self.fields.pending_op(surface_id);
            let staged = self.fields.staged_op(surface_id);
            let inflight = self.fields.inflight_call_count(surface_id);
            let visible = self.fields.is_visible(surface_id);
            let staged_intent_sequence = self.fields.staged_intent_sequence(surface_id);
            let pending_task_sequence = self.fields.pending_task_sequence(surface_id);
            let pending_lineage_sequence = self.fields.pending_lineage_sequence(surface_id);
            let last_phase = self
                .fields
                .last_delta_phase
                .get(surface_id)
                .copied()
                .unwrap_or(SurfaceDeltaPhase::None);
            let last_op = self
                .fields
                .last_delta_operation
                .get(surface_id)
                .copied()
                .unwrap_or(SurfaceDeltaOperation::None);

            // 1. removing_or_removed_surfaces_are_not_visible
            if matches!(base, SurfaceBaseState::Removing | SurfaceBaseState::Removed) {
                debug_assert!(
                    !visible,
                    "invariant: removing/removed surface '{surface_id}' must not be visible"
                );
            }

            // 2. visible_membership_matches_active_base_state
            debug_assert_eq!(
                visible,
                base == SurfaceBaseState::Active,
                "invariant: visible membership for '{surface_id}' must match Active base state (visible={visible}, base={base:?})"
            );

            // 3. removing_surfaces_have_no_pending_add_or_reload
            if base == SurfaceBaseState::Removing {
                debug_assert_eq!(
                    pending,
                    PendingSurfaceOp::None,
                    "invariant: removing surface '{surface_id}' must have no pending op"
                );
            }

            // 4. removed_surfaces_only_allow_pending_none_or_add
            if base == SurfaceBaseState::Removed {
                debug_assert!(
                    matches!(pending, PendingSurfaceOp::None | PendingSurfaceOp::Add),
                    "invariant: removed surface '{surface_id}' can only have None or Add pending (has {pending:?})"
                );
            }

            // 5. inflight_calls_only_exist_for_active_or_removing_surfaces
            if inflight > 0 {
                debug_assert!(
                    matches!(base, SurfaceBaseState::Active | SurfaceBaseState::Removing),
                    "invariant: surface '{surface_id}' has {inflight} inflight calls but base is {base:?}"
                );
            }

            // 6. reload_pending_requires_active_base_state
            if pending == PendingSurfaceOp::Reload {
                debug_assert_eq!(
                    base,
                    SurfaceBaseState::Active,
                    "invariant: surface '{surface_id}' has Reload pending but base is {base:?}"
                );
            }

            // 7. removed_surfaces_have_zero_inflight_calls
            if base == SurfaceBaseState::Removed {
                debug_assert_eq!(
                    inflight, 0,
                    "invariant: removed surface '{surface_id}' must have zero inflight calls (has {inflight})"
                );
            }

            // 8. forced_delta_phase_is_always_a_remove_delta
            if last_phase == SurfaceDeltaPhase::Forced {
                debug_assert_eq!(
                    last_op,
                    SurfaceDeltaOperation::Remove,
                    "invariant: surface '{surface_id}' has Forced phase but last op is {last_op:?}"
                );
            }

            // 9. staged_intent_sequence exists iff staged op is not None.
            if staged == StagedSurfaceOp::None {
                debug_assert_eq!(
                    staged_intent_sequence, 0,
                    "invariant: staged sequence for '{surface_id}' must be zero when staged op is None (is {staged_intent_sequence})"
                );
            } else {
                debug_assert!(
                    staged_intent_sequence > 0,
                    "invariant: staged sequence for '{surface_id}' must be non-zero when staged op is {staged:?}"
                );
            }

            // 10. pending task/lineage sequences exist iff pending op is not None.
            if pending == PendingSurfaceOp::None {
                debug_assert_eq!(
                    pending_task_sequence, 0,
                    "invariant: pending task sequence for '{surface_id}' must be zero when pending op is None (is {pending_task_sequence})"
                );
                debug_assert_eq!(
                    pending_lineage_sequence, 0,
                    "invariant: pending lineage sequence for '{surface_id}' must be zero when pending op is None (is {pending_lineage_sequence})"
                );
            } else {
                debug_assert!(
                    pending_task_sequence > 0,
                    "invariant: pending task sequence for '{surface_id}' must be non-zero when pending op is {pending:?}"
                );
                debug_assert!(
                    pending_lineage_sequence > 0,
                    "invariant: pending lineage sequence for '{surface_id}' must be non-zero when pending op is {pending:?}"
                );
            }

            // 11. removal timing exists iff base state is Removing.
            let has_removal_timing = self.fields.removal_timing.contains_key(surface_id);
            if base == SurfaceBaseState::Removing {
                debug_assert!(
                    has_removal_timing,
                    "invariant: removing surface '{surface_id}' must have removal timing"
                );
            } else {
                debug_assert!(
                    !has_removal_timing,
                    "invariant: non-removing surface '{surface_id}' must not have removal timing"
                );
            }
        }
    }
}

impl Default for ExternalToolSurfaceAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalToolSurfaceMutator for ExternalToolSurfaceAuthority {
    fn apply(
        &mut self,
        input: ExternalToolSurfaceInput,
    ) -> Result<ExternalToolSurfaceTransition, ExternalToolSurfaceError> {
        let (next_phase, next_fields, effects, transition_name) = self.evaluate(&input)?;

        // Commit: update canonical state.
        self.phase = next_phase;
        self.fields = next_fields;

        // Verify invariants in debug builds.
        #[cfg(debug_assertions)]
        self.check_invariants();

        Ok(ExternalToolSurfaceTransition {
            transition_name,
            phase: next_phase,
            effects,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::redundant_clone)]
mod tests {
    use super::*;

    fn make_authority() -> ExternalToolSurfaceAuthority {
        ExternalToolSurfaceAuthority::new()
    }

    fn turn(n: u64) -> TurnNumber {
        TurnNumber(n)
    }

    fn pending_feedback(
        auth: &ExternalToolSurfaceAuthority,
        id: &str,
        applied_at_turn: u64,
    ) -> (SurfaceId, SurfaceDeltaOperation, u64, u64, TurnNumber) {
        let sid = SurfaceId::from(id);
        let operation = match auth.pending_op(&sid) {
            PendingSurfaceOp::Add => SurfaceDeltaOperation::Add,
            PendingSurfaceOp::Reload => SurfaceDeltaOperation::Reload,
            PendingSurfaceOp::None => SurfaceDeltaOperation::None,
        };
        (
            sid.clone(),
            operation,
            auth.pending_task_sequence(&sid),
            auth.pending_lineage_sequence(&sid),
            turn(applied_at_turn),
        )
    }

    fn pending_succeeded_input(
        auth: &ExternalToolSurfaceAuthority,
        id: &str,
        applied_at_turn: u64,
    ) -> ExternalToolSurfaceInput {
        let (surface_id, operation, pending_task_sequence, staged_intent_sequence, applied_at_turn) =
            pending_feedback(auth, id, applied_at_turn);
        ExternalToolSurfaceInput::PendingSucceeded {
            surface_id,
            operation,
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn,
        }
    }

    fn pending_failed_input(
        auth: &ExternalToolSurfaceAuthority,
        id: &str,
        applied_at_turn: u64,
    ) -> ExternalToolSurfaceInput {
        let (surface_id, operation, pending_task_sequence, staged_intent_sequence, applied_at_turn) =
            pending_feedback(auth, id, applied_at_turn);
        ExternalToolSurfaceInput::PendingFailed {
            surface_id,
            operation,
            pending_task_sequence,
            staged_intent_sequence,
            applied_at_turn,
        }
    }

    /// Helper: stage add + apply boundary + pending succeeded -> Active.
    fn add_surface_active(auth: &mut ExternalToolSurfaceAuthority, id: &str, t: u64) {
        let sid = SurfaceId::from(id);
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("stage add");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(t),
        })
        .expect("apply boundary add");
        auth.apply(pending_succeeded_input(auth, id, t))
            .expect("pending succeeded");
    }

    // ---- StageAdd / StageRemove / StageReload ----

    #[test]
    fn stage_add_records_intent() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        let t = auth
            .apply(ExternalToolSurfaceInput::StageAdd {
                surface_id: sid.clone(),
            })
            .expect("stage add");
        assert_eq!(t.transition_name, "StageAdd");
        assert_eq!(auth.staged_op(&sid), StagedSurfaceOp::Add);
        assert!(auth.known_surfaces().any(|s| *s == sid));
    }

    #[test]
    fn stage_remove_records_intent() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        assert_eq!(auth.staged_op(&sid), StagedSurfaceOp::Remove);
    }

    #[test]
    fn stage_reload_requires_active_base() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        let result = auth.apply(ExternalToolSurfaceInput::StageReload { surface_id: sid });
        assert!(result.is_err(), "reload on non-active should fail");
    }

    #[test]
    fn stage_reload_succeeds_when_active() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        let t = auth
            .apply(ExternalToolSurfaceInput::StageReload {
                surface_id: sid.clone(),
            })
            .expect("stage reload");
        assert_eq!(t.transition_name, "StageReload");
        assert_eq!(auth.staged_op(&sid), StagedSurfaceOp::Reload);
    }

    // ---- ApplyBoundary ----

    #[test]
    fn apply_boundary_add_emits_schedule_and_delta() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("stage add");
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                applied_at_turn: turn(5),
            })
            .expect("apply add");
        assert_eq!(t.transition_name, "ApplyBoundaryAdd");
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::ScheduleSurfaceCompletion { operation, .. }
                if *operation == SurfaceDeltaOperation::Add
        )));
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::EmitExternalToolDelta { phase, persisted, .. }
                if *phase == SurfaceDeltaPhase::Pending && !persisted
        )));
        assert_eq!(auth.pending_op(&sid), PendingSurfaceOp::Add);
    }

    #[test]
    fn apply_boundary_remove_draining_hides_from_visible() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        assert!(auth.is_visible(&sid));

        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                applied_at_turn: turn(2),
            })
            .expect("apply remove");
        assert_eq!(t.transition_name, "ApplyBoundaryRemoveDraining");
        assert!(!auth.is_visible(&sid));
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Removing);
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { .. }
        )));
    }

    #[test]
    fn apply_boundary_remove_noop_when_not_active() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("ghost");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                applied_at_turn: turn(1),
            })
            .expect("apply remove noop");
        assert_eq!(t.transition_name, "ApplyBoundaryRemoveNoop");
        assert!(t.effects.is_empty());
    }

    #[test]
    fn apply_boundary_reload_emits_schedule_and_delta() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageReload {
            surface_id: sid.clone(),
        })
        .expect("stage reload");
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                applied_at_turn: turn(2),
            })
            .expect("apply reload");
        assert_eq!(t.transition_name, "ApplyBoundaryReload");
        assert_eq!(auth.pending_op(&sid), PendingSurfaceOp::Reload);
    }

    #[test]
    fn apply_boundary_no_staged_op_fails() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        let result = auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid,
            applied_at_turn: turn(1),
        });
        assert!(result.is_err());
    }

    // ---- PendingSucceeded / PendingFailed ----

    #[test]
    fn pending_succeeded_add_makes_visible() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("stage add");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(5),
        })
        .expect("apply add");
        assert!(!auth.is_visible(&sid));

        let input = pending_succeeded_input(&auth, "alpha", 5);
        let t = auth.apply(input).expect("pending succeeded");
        assert_eq!(t.transition_name, "PendingSucceededAdd");
        assert!(auth.is_visible(&sid));
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Active);
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::EmitExternalToolDelta { phase, persisted, .. }
                if *phase == SurfaceDeltaPhase::Applied && *persisted
        )));
    }

    #[test]
    fn pending_succeeded_reload_stays_active() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageReload {
            surface_id: sid.clone(),
        })
        .expect("stage reload");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply reload");
        let input = pending_succeeded_input(&auth, "alpha", 2);
        let t = auth.apply(input).expect("reload succeeded");
        assert_eq!(t.transition_name, "PendingSucceededReload");
        assert!(auth.is_visible(&sid));
    }

    #[test]
    fn pending_failed_add_stays_invisible() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("stage add");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(5),
        })
        .expect("apply add");
        let input = pending_failed_input(&auth, "alpha", 5);
        let t = auth.apply(input).expect("pending failed");
        assert_eq!(t.transition_name, "PendingFailedAdd");
        assert!(!auth.is_visible(&sid));
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::EmitExternalToolDelta { phase, .. }
                if *phase == SurfaceDeltaPhase::Failed
        )));
    }

    #[test]
    fn pending_failed_reload_keeps_old_active() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageReload {
            surface_id: sid.clone(),
        })
        .expect("stage reload");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply reload");
        let input = pending_failed_input(&auth, "alpha", 2);
        let t = auth.apply(input).expect("reload failed");
        assert_eq!(t.transition_name, "PendingFailedReload");
        // Surface stays active (reload failure keeps old connection).
        assert!(auth.is_visible(&sid));
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Active);
    }

    #[test]
    fn pending_succeeded_no_pending_fails() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("ghost");
        let result = auth.apply(ExternalToolSurfaceInput::PendingSucceeded {
            surface_id: sid,
            operation: SurfaceDeltaOperation::None,
            pending_task_sequence: 0,
            staged_intent_sequence: 0,
            applied_at_turn: turn(1),
        });
        assert!(result.is_err());
    }

    #[test]
    fn apply_boundary_rejects_while_pending_operation_is_unresolved() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("stage add");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(1),
        })
        .expect("apply add");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");

        let result = auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid,
            applied_at_turn: turn(2),
        });
        assert!(
            result.is_err(),
            "should reject boundary while add is pending"
        );
    }

    // ---- CallStarted / CallFinished ----

    #[test]
    fn call_started_increments_inflight() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call started");
        assert_eq!(auth.inflight_call_count(&sid), 1);
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call started 2");
        assert_eq!(auth.inflight_call_count(&sid), 2);
    }

    #[test]
    fn call_started_rejects_while_removing() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply remove");
        let t = auth
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("call should produce reject effect, not error");
        assert_eq!(t.transition_name, "CallStartedRejectWhileRemoving");
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::RejectSurfaceCall { reason, .. }
                if reason == "surface_draining"
        )));
    }

    #[test]
    fn call_started_rejects_while_unavailable() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("ghost");
        let t = auth
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("call should produce reject effect");
        assert_eq!(t.transition_name, "CallStartedRejectWhileUnavailable");
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::RejectSurfaceCall { reason, .. }
                if reason == "surface_unavailable"
        )));
    }

    #[test]
    fn call_finished_decrements_inflight() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call started");
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call started 2");
        auth.apply(ExternalToolSurfaceInput::CallFinished {
            surface_id: sid.clone(),
        })
        .expect("call finished");
        assert_eq!(auth.inflight_call_count(&sid), 1);
    }

    #[test]
    fn call_finished_zero_inflight_fails() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        let result = auth.apply(ExternalToolSurfaceInput::CallFinished { surface_id: sid });
        assert!(result.is_err(), "should reject with zero inflight");
    }

    #[test]
    fn call_finished_removing_decrements() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        // Start a call, then begin removal.
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call started");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply remove");
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Removing);
        let t = auth
            .apply(ExternalToolSurfaceInput::CallFinished {
                surface_id: sid.clone(),
            })
            .expect("call finished while removing");
        assert_eq!(t.transition_name, "CallFinishedRemoving");
        assert_eq!(auth.inflight_call_count(&sid), 0);
    }

    // ---- FinalizeRemovalClean / FinalizeRemovalForced ----

    #[test]
    fn finalize_removal_clean_transitions_to_removed() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply remove");
        let t = auth
            .apply(ExternalToolSurfaceInput::FinalizeRemovalClean {
                surface_id: sid.clone(),
                applied_at_turn: turn(2),
            })
            .expect("finalize clean");
        assert_eq!(t.transition_name, "FinalizeRemovalClean");
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Removed);
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, ExternalToolSurfaceEffect::CloseSurfaceConnection { .. }))
        );
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::EmitExternalToolDelta { phase, .. }
                if *phase == SurfaceDeltaPhase::Applied
        )));
    }

    #[test]
    fn finalize_removal_clean_requires_zero_inflight() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call started");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply remove");
        let result = auth.apply(ExternalToolSurfaceInput::FinalizeRemovalClean {
            surface_id: sid,
            applied_at_turn: turn(2),
        });
        assert!(result.is_err(), "should reject with inflight calls");
    }

    #[test]
    fn finalize_removal_forced_ignores_inflight() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call started");
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply remove");
        let t = auth
            .apply(ExternalToolSurfaceInput::FinalizeRemovalForced {
                surface_id: sid.clone(),
                applied_at_turn: turn(2),
            })
            .expect("forced finalize");
        assert_eq!(t.transition_name, "FinalizeRemovalForced");
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Removed);
        assert_eq!(auth.inflight_call_count(&sid), 0);
        assert!(t.effects.iter().any(|e| matches!(
            e,
            ExternalToolSurfaceEffect::EmitExternalToolDelta { phase, .. }
                if *phase == SurfaceDeltaPhase::Forced
        )));
    }

    #[test]
    fn finalize_removal_requires_removing_state() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        let sid = SurfaceId::from("alpha");
        let result = auth.apply(ExternalToolSurfaceInput::FinalizeRemovalClean {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        });
        assert!(result.is_err(), "cannot finalize Active surface");

        let result = auth.apply(ExternalToolSurfaceInput::FinalizeRemovalForced {
            surface_id: sid,
            applied_at_turn: turn(2),
        });
        assert!(result.is_err(), "cannot force-finalize Active surface");
    }

    // ---- Shutdown ----

    #[test]
    fn shutdown_clears_all_state() {
        let mut auth = make_authority();
        add_surface_active(&mut auth, "alpha", 1);
        add_surface_active(&mut auth, "beta", 2);
        let t = auth
            .apply(ExternalToolSurfaceInput::Shutdown)
            .expect("shutdown");
        assert_eq!(t.transition_name, "Shutdown");
        assert_eq!(auth.phase(), ExternalToolSurfacePhase::Shutdown);
        assert_eq!(auth.known_surfaces().count(), 0);
        assert_eq!(auth.visible_surfaces().count(), 0);
    }

    #[test]
    fn shutdown_is_idempotent() {
        let mut auth = make_authority();
        auth.apply(ExternalToolSurfaceInput::Shutdown)
            .expect("first shutdown");
        auth.apply(ExternalToolSurfaceInput::Shutdown)
            .expect("second shutdown");
        assert_eq!(auth.phase(), ExternalToolSurfacePhase::Shutdown);
    }

    #[test]
    fn operating_inputs_rejected_after_shutdown() {
        let mut auth = make_authority();
        auth.apply(ExternalToolSurfaceInput::Shutdown)
            .expect("shutdown");
        let result = auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: SurfaceId::from("alpha"),
        });
        assert!(result.is_err());
    }

    // ---- Full lifecycle scenario ----

    #[test]
    fn full_add_call_remove_drain_finalize_lifecycle() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("server-a");

        // 1. Stage add
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("stage add");

        // 2. Apply boundary -> pending
        let t = auth
            .apply(ExternalToolSurfaceInput::ApplyBoundary {
                surface_id: sid.clone(),
                applied_at_turn: turn(1),
            })
            .expect("apply add");
        assert_eq!(t.transition_name, "ApplyBoundaryAdd");

        // 3. Pending succeeds -> active
        let input = pending_succeeded_input(&auth, "server-a", 1);
        auth.apply(input).expect("pending succeeded");
        assert!(auth.is_visible(&sid));
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Active);

        // 4. Start some calls
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call 1");
        auth.apply(ExternalToolSurfaceInput::CallStarted {
            surface_id: sid.clone(),
        })
        .expect("call 2");
        assert_eq!(auth.inflight_call_count(&sid), 2);

        // 5. Stage remove
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");

        // 6. Apply boundary -> draining
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply remove");
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Removing);
        assert!(!auth.is_visible(&sid));

        // 7. New calls rejected
        let t = auth
            .apply(ExternalToolSurfaceInput::CallStarted {
                surface_id: sid.clone(),
            })
            .expect("rejected call");
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, ExternalToolSurfaceEffect::RejectSurfaceCall { .. }))
        );

        // 8. Finish in-flight calls
        auth.apply(ExternalToolSurfaceInput::CallFinished {
            surface_id: sid.clone(),
        })
        .expect("finish call 1");
        auth.apply(ExternalToolSurfaceInput::CallFinished {
            surface_id: sid.clone(),
        })
        .expect("finish call 2");
        assert_eq!(auth.inflight_call_count(&sid), 0);

        // 9. Finalize clean removal
        let t = auth
            .apply(ExternalToolSurfaceInput::FinalizeRemovalClean {
                surface_id: sid.clone(),
                applied_at_turn: turn(2),
            })
            .expect("finalize clean");
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Removed);
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, ExternalToolSurfaceEffect::CloseSurfaceConnection { .. }))
        );
    }

    #[test]
    fn re_add_after_removal() {
        let mut auth = make_authority();
        let sid = SurfaceId::from("alpha");
        add_surface_active(&mut auth, "alpha", 1);

        // Remove the surface.
        auth.apply(ExternalToolSurfaceInput::StageRemove {
            surface_id: sid.clone(),
        })
        .expect("stage remove");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("apply remove");
        auth.apply(ExternalToolSurfaceInput::FinalizeRemovalClean {
            surface_id: sid.clone(),
            applied_at_turn: turn(2),
        })
        .expect("finalize");
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Removed);

        // Re-add should work.
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("re-stage add");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(3),
        })
        .expect("re-apply add");
        let input = pending_succeeded_input(&auth, "alpha", 3);
        auth.apply(input).expect("re-pending succeeded");
        assert!(auth.is_visible(&sid));
        assert_eq!(auth.surface_base(&sid), SurfaceBaseState::Active);
    }

    #[test]
    fn pending_count_tracks_in_flight_async_ops() {
        let mut auth = make_authority();
        assert_eq!(auth.pending_count(), 0);

        let sid = SurfaceId::from("alpha");
        auth.apply(ExternalToolSurfaceInput::StageAdd {
            surface_id: sid.clone(),
        })
        .expect("stage add");
        auth.apply(ExternalToolSurfaceInput::ApplyBoundary {
            surface_id: sid.clone(),
            applied_at_turn: turn(1),
        })
        .expect("apply add");
        assert_eq!(auth.pending_count(), 1);

        let input = pending_succeeded_input(&auth, "alpha", 1);
        auth.apply(input).expect("pending succeeded");
        assert_eq!(auth.pending_count(), 0);
    }
}

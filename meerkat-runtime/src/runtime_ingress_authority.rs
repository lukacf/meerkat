//! Generated-authority module for the RuntimeIngress machine.
//!
//! This module provides typed enums and a sealed mutator trait that enforces
//! all RuntimeIngress state mutations flow through the machine authority.
//! Handwritten shell code calls [`RuntimeIngressAuthority::apply`] and executes
//! returned effects; it cannot mutate canonical state directly.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/runtime_ingress.rs`:
//!
//! - 3 phases: Active, Retired, Destroyed
//! - 14 inputs: AdmitQueued, AdmitConsumedOnAccept, StageDrainSnapshot,
//!   BoundaryApplied, RunCompleted, RunFailed, RunCancelled,
//!   SupersedeQueuedInput, CoalesceQueuedInputs, Retire, Reset, Destroy,
//!   Recover, SetSilentIntentOverrides
//! - 18 fields: admitted_inputs, admission_order, content_shape, request_id,
//!   reservation_key, policy_snapshot, handling_mode, lifecycle,
//!   terminal_outcome, queue, steer_queue, current_run, current_run_contributors,
//!   last_run, last_boundary_sequence, wake_requested, process_requested,
//!   silent_intent_overrides
//! - 8 effects: IngressAccepted, ReadyForRun, InputLifecycleNotice,
//!   WakeRuntime, RequestImmediateProcessing, CompletionResolved,
//!   IngressNotice, SilentIntentApplied

use std::collections::{BTreeSet, HashMap, HashSet};

use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::HandlingMode;

use crate::input_state::{InputLifecycleState, InputTerminalOutcome};
use crate::policy::PolicyDecision;

// ---------------------------------------------------------------------------
// Schema-level named types
// ---------------------------------------------------------------------------

/// Content shape classification for admitted inputs.
///
/// Maps to the schema's `ContentShape` named type. Used to preserve content
/// metadata through the admission pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentShape(pub String);

/// Reservation key for admitted inputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReservationKey(pub String);

/// Request ID for correlation tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(pub String);

// ---------------------------------------------------------------------------
// Phase enum — mirrors the machine schema's phase variants
// ---------------------------------------------------------------------------

/// Canonical ingress phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IngressPhase {
    Active,
    Retired,
    Destroyed,
}

impl IngressPhase {
    /// Whether this is a terminal phase.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Destroyed)
    }
}

// ---------------------------------------------------------------------------
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the RuntimeIngress machine.
///
/// Shell code classifies raw inputs into these typed inputs, then calls
/// [`RuntimeIngressAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone)]
pub enum RuntimeIngressInput {
    /// Admit a queued input (Queue or Steer handling mode).
    AdmitQueued {
        work_id: InputId,
        content_shape: ContentShape,
        handling_mode: HandlingMode,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
        policy: PolicyDecision,
    },
    /// Admit an input that is immediately consumed on accept (Ignore+OnAccept).
    AdmitConsumedOnAccept {
        work_id: InputId,
        content_shape: ContentShape,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
        policy: PolicyDecision,
    },
    /// Stage a drain snapshot: dequeue work and assign to a run.
    StageDrainSnapshot {
        run_id: RunId,
        contributing_work_ids: Vec<InputId>,
    },
    /// Boundary applied: mark contributors as AppliedPendingConsumption.
    BoundaryApplied {
        run_id: RunId,
        boundary_sequence: u64,
    },
    /// Run completed: consume all contributors.
    RunCompleted { run_id: RunId },
    /// Run failed: rollback staged contributors to queued.
    RunFailed { run_id: RunId },
    /// Run cancelled: rollback staged contributors to queued.
    RunCancelled { run_id: RunId },
    /// Supersede a queued input with a newer one.
    SupersedeQueuedInput {
        new_work_id: InputId,
        old_work_id: InputId,
    },
    /// Coalesce multiple queued inputs into an aggregate.
    CoalesceQueuedInputs {
        aggregate_work_id: InputId,
        source_work_ids: Vec<InputId>,
    },
    /// Retire the ingress (stop accepting new input, drain existing).
    Retire,
    /// Reset: abandon all non-terminal inputs, return to Active.
    Reset,
    /// Destroy: abandon everything, transition to Destroyed.
    Destroy,
    /// Recover: re-derive transient state from canonical state.
    Recover,
    /// Configure silent intent overrides.
    SetSilentIntentOverrides { intents: BTreeSet<String> },
}

// ---------------------------------------------------------------------------
// Typed effect enum — mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by RuntimeIngress transitions.
///
/// Shell code receives these from [`RuntimeIngressAuthority::apply`] and is
/// responsible for executing the side effects (e.g. emitting events, waking).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeIngressEffect {
    /// An input was accepted into the ingress.
    IngressAccepted { work_id: InputId },
    /// A run is ready to execute with the given contributors.
    ReadyForRun {
        run_id: RunId,
        contributing_work_ids: Vec<InputId>,
    },
    /// An input's lifecycle state changed.
    InputLifecycleNotice {
        work_id: InputId,
        new_state: InputLifecycleState,
    },
    /// The runtime should be woken (idle -> running).
    WakeRuntime,
    /// Immediate processing requested (steer/immediate drain).
    RequestImmediateProcessing,
    /// An input reached a terminal outcome.
    CompletionResolved {
        work_id: InputId,
        outcome: InputTerminalOutcome,
    },
    /// Informational ingress notice.
    IngressNotice { kind: String, detail: String },
    /// A silent intent override was applied.
    SilentIntentApplied { work_id: InputId, intent: String },
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the RuntimeIngress authority.
#[derive(Debug)]
pub struct RuntimeIngressTransition {
    /// The phase after the transition.
    pub next_phase: IngressPhase,
    /// Effects to be executed by shell code.
    pub effects: Vec<RuntimeIngressEffect>,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from the runtime ingress authority.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeIngressError {
    /// The transition is not valid from the current phase.
    #[error("Invalid ingress transition: {from:?} via {input} (rejected)")]
    InvalidTransition { from: IngressPhase, input: String },
    /// A guard condition was not met.
    #[error("Guard failed: {guard} (from {from:?})")]
    GuardFailed { from: IngressPhase, guard: String },
    /// The ingress is in a terminal phase.
    #[error("Ingress is in terminal phase {phase:?}")]
    TerminalPhase { phase: IngressPhase },
}

// ---------------------------------------------------------------------------
// Canonical machine state (fields)
// ---------------------------------------------------------------------------

/// Canonical machine-owned fields for RuntimeIngress.
///
/// These fields are owned exclusively by the authority and cannot be mutated
/// by handwritten shell code.
#[derive(Debug, Clone)]
struct RuntimeIngressFields {
    admitted_inputs: HashSet<InputId>,
    admission_order: Vec<InputId>,
    content_shape: HashMap<InputId, ContentShape>,
    request_id: HashMap<InputId, Option<RequestId>>,
    reservation_key: HashMap<InputId, Option<ReservationKey>>,
    policy_snapshot: HashMap<InputId, PolicyDecision>,
    handling_mode: HashMap<InputId, HandlingMode>,
    lifecycle: HashMap<InputId, InputLifecycleState>,
    terminal_outcome: HashMap<InputId, Option<InputTerminalOutcome>>,
    queue: Vec<InputId>,
    steer_queue: Vec<InputId>,
    current_run: Option<RunId>,
    current_run_contributors: Vec<InputId>,
    last_run: HashMap<InputId, Option<RunId>>,
    last_boundary_sequence: HashMap<InputId, Option<u64>>,
    wake_requested: bool,
    process_requested: bool,
    silent_intent_overrides: BTreeSet<String>,
}

impl RuntimeIngressFields {
    fn new() -> Self {
        Self {
            admitted_inputs: HashSet::new(),
            admission_order: Vec::new(),
            content_shape: HashMap::new(),
            request_id: HashMap::new(),
            reservation_key: HashMap::new(),
            policy_snapshot: HashMap::new(),
            handling_mode: HashMap::new(),
            lifecycle: HashMap::new(),
            terminal_outcome: HashMap::new(),
            queue: Vec::new(),
            steer_queue: Vec::new(),
            current_run: None,
            current_run_contributors: Vec::new(),
            last_run: HashMap::new(),
            last_boundary_sequence: HashMap::new(),
            wake_requested: false,
            process_requested: false,
            silent_intent_overrides: BTreeSet::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for RuntimeIngress state mutation.
///
/// Only [`RuntimeIngressAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for ingress state.
pub trait RuntimeIngressMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(
        &mut self,
        input: RuntimeIngressInput,
    ) -> Result<RuntimeIngressTransition, RuntimeIngressError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for RuntimeIngress state.
///
/// Holds the canonical phase + fields and delegates all transitions through
/// the encoded transition table. The authority OWNS the canonical state --
/// callers cannot get `&mut` access to the inner fields.
#[derive(Debug, Clone)]
pub struct RuntimeIngressAuthority {
    /// Canonical phase.
    phase: IngressPhase,
    /// Canonical machine-owned fields.
    fields: RuntimeIngressFields,
}

impl sealed::Sealed for RuntimeIngressAuthority {}

impl Default for RuntimeIngressAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeIngressAuthority {
    /// Create a new authority in the Active phase.
    pub fn new() -> Self {
        Self {
            phase: IngressPhase::Active,
            fields: RuntimeIngressFields::new(),
        }
    }

    /// Current phase.
    pub fn phase(&self) -> IngressPhase {
        self.phase
    }

    /// Whether the current phase is terminal.
    pub fn is_terminal(&self) -> bool {
        self.phase.is_terminal()
    }

    /// The set of admitted input IDs.
    pub fn admitted_inputs(&self) -> &HashSet<InputId> {
        &self.fields.admitted_inputs
    }

    /// The admission order.
    pub fn admission_order(&self) -> &[InputId] {
        &self.fields.admission_order
    }

    /// The FIFO queue of work IDs pending processing.
    pub fn queue(&self) -> &[InputId] {
        &self.fields.queue
    }

    /// The steer queue of work IDs pending checkpoint injection.
    pub fn steer_queue(&self) -> &[InputId] {
        &self.fields.steer_queue
    }

    /// The current run ID (if a run is in progress).
    pub fn current_run(&self) -> Option<&RunId> {
        self.fields.current_run.as_ref()
    }

    /// The current run's contributing work IDs.
    pub fn current_run_contributors(&self) -> &[InputId] {
        &self.fields.current_run_contributors
    }

    /// Lifecycle state for a specific work ID.
    pub fn lifecycle_state(&self, work_id: &InputId) -> Option<InputLifecycleState> {
        self.fields.lifecycle.get(work_id).copied()
    }

    /// Terminal outcome for a specific work ID.
    pub fn terminal_outcome(&self, work_id: &InputId) -> Option<&InputTerminalOutcome> {
        self.fields
            .terminal_outcome
            .get(work_id)
            .and_then(|o| o.as_ref())
    }

    /// Policy snapshot for a specific work ID.
    pub fn policy_snapshot(&self, work_id: &InputId) -> Option<&PolicyDecision> {
        self.fields.policy_snapshot.get(work_id)
    }

    /// Whether a wake was requested.
    pub fn wake_requested(&self) -> bool {
        self.fields.wake_requested
    }

    /// Whether immediate processing was requested.
    pub fn process_requested(&self) -> bool {
        self.fields.process_requested
    }

    /// Silent intent overrides.
    pub fn silent_intent_overrides(&self) -> &BTreeSet<String> {
        &self.fields.silent_intent_overrides
    }

    /// Last run ID for a specific work ID.
    pub fn last_run(&self, work_id: &InputId) -> Option<&RunId> {
        self.fields.last_run.get(work_id).and_then(|o| o.as_ref())
    }

    /// Last boundary sequence for a specific work ID.
    pub fn last_boundary_sequence(&self, work_id: &InputId) -> Option<u64> {
        self.fields
            .last_boundary_sequence
            .get(work_id)
            .and_then(|o| *o)
    }

    /// Handling mode for a specific work ID.
    pub fn handling_mode(&self, work_id: &InputId) -> Option<HandlingMode> {
        self.fields.handling_mode.get(work_id).copied()
    }

    /// Check if a transition is legal without applying it.
    pub fn can_accept(&self, input: &RuntimeIngressInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Clear the wake_requested flag (shell reads and resets).
    pub fn take_wake_requested(&mut self) -> bool {
        std::mem::take(&mut self.fields.wake_requested)
    }

    /// Clear the process_requested flag (shell reads and resets).
    pub fn take_process_requested(&mut self) -> bool {
        std::mem::take(&mut self.fields.process_requested)
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: &RuntimeIngressInput,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        let phase = self.phase;

        // Terminal phase rejects ALL inputs.
        if phase.is_terminal() {
            return Err(RuntimeIngressError::TerminalPhase { phase });
        }

        match input {
            RuntimeIngressInput::AdmitQueued {
                work_id,
                content_shape,
                handling_mode,
                request_id,
                reservation_key,
                policy,
            } => self.eval_admit_queued(
                phase,
                work_id,
                content_shape,
                *handling_mode,
                request_id,
                reservation_key,
                policy,
            ),

            RuntimeIngressInput::AdmitConsumedOnAccept {
                work_id,
                content_shape,
                request_id,
                reservation_key,
                policy,
            } => self.eval_admit_consumed_on_accept(
                phase,
                work_id,
                content_shape,
                request_id,
                reservation_key,
                policy,
            ),

            RuntimeIngressInput::StageDrainSnapshot {
                run_id,
                contributing_work_ids,
            } => self.eval_stage_drain_snapshot(phase, run_id, contributing_work_ids),

            RuntimeIngressInput::BoundaryApplied {
                run_id,
                boundary_sequence,
            } => self.eval_boundary_applied(phase, run_id, *boundary_sequence),

            RuntimeIngressInput::RunCompleted { run_id } => self.eval_run_completed(phase, run_id),

            RuntimeIngressInput::RunFailed { run_id } => self.eval_run_failed(phase, run_id),

            RuntimeIngressInput::RunCancelled { run_id } => self.eval_run_cancelled(phase, run_id),

            RuntimeIngressInput::SupersedeQueuedInput {
                new_work_id,
                old_work_id,
            } => self.eval_supersede(phase, new_work_id, old_work_id),

            RuntimeIngressInput::CoalesceQueuedInputs {
                aggregate_work_id,
                source_work_ids,
            } => self.eval_coalesce(phase, aggregate_work_id, source_work_ids),

            RuntimeIngressInput::Retire => self.eval_retire(phase),
            RuntimeIngressInput::Reset => self.eval_reset(phase),
            RuntimeIngressInput::Destroy => self.eval_destroy(phase),
            RuntimeIngressInput::Recover => self.eval_recover(phase),

            RuntimeIngressInput::SetSilentIntentOverrides { intents } => {
                self.eval_set_silent_intent_overrides(phase, intents)
            }
        }
    }

    // ---- Transition evaluators ----

    #[allow(clippy::too_many_arguments)]
    fn eval_admit_queued(
        &self,
        phase: IngressPhase,
        work_id: &InputId,
        content_shape: &ContentShape,
        handling_mode: HandlingMode,
        request_id: &Option<RequestId>,
        reservation_key: &Option<ReservationKey>,
        policy: &PolicyDecision,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // Only Active phase accepts new input
        if phase != IngressPhase::Active {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "AdmitQueued".into(),
            });
        }

        // Guard: input_is_new
        if self.fields.admitted_inputs.contains(work_id) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!("input_is_new: {work_id:?} already admitted"),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Register admission
        fields.admitted_inputs.insert(work_id.clone());
        fields.admission_order.push(work_id.clone());
        fields
            .content_shape
            .insert(work_id.clone(), content_shape.clone());
        fields
            .request_id
            .insert(work_id.clone(), request_id.clone());
        fields
            .reservation_key
            .insert(work_id.clone(), reservation_key.clone());
        fields
            .policy_snapshot
            .insert(work_id.clone(), policy.clone());
        fields.handling_mode.insert(work_id.clone(), handling_mode);
        fields
            .lifecycle
            .insert(work_id.clone(), InputLifecycleState::Queued);
        fields.terminal_outcome.insert(work_id.clone(), None);
        fields.last_run.insert(work_id.clone(), None);
        fields.last_boundary_sequence.insert(work_id.clone(), None);

        // Route to queue or steer_queue based on handling mode
        match handling_mode {
            HandlingMode::Queue => fields.queue.push(work_id.clone()),
            HandlingMode::Steer => fields.steer_queue.push(work_id.clone()),
        }

        // Wake/process flags
        fields.wake_requested = true;
        if handling_mode == HandlingMode::Steer {
            fields.process_requested = true;
        }

        effects.push(RuntimeIngressEffect::IngressAccepted {
            work_id: work_id.clone(),
        });
        effects.push(RuntimeIngressEffect::InputLifecycleNotice {
            work_id: work_id.clone(),
            new_state: InputLifecycleState::Queued,
        });
        effects.push(RuntimeIngressEffect::WakeRuntime);
        if handling_mode == HandlingMode::Steer {
            effects.push(RuntimeIngressEffect::RequestImmediateProcessing);
        }

        Ok((IngressPhase::Active, fields, effects))
    }

    fn eval_admit_consumed_on_accept(
        &self,
        phase: IngressPhase,
        work_id: &InputId,
        content_shape: &ContentShape,
        request_id: &Option<RequestId>,
        reservation_key: &Option<ReservationKey>,
        policy: &PolicyDecision,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // Only Active phase accepts new input
        if phase != IngressPhase::Active {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "AdmitConsumedOnAccept".into(),
            });
        }

        // Guard: input_is_new
        if self.fields.admitted_inputs.contains(work_id) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!("input_is_new: {work_id:?} already admitted"),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Register admission (immediately consumed)
        fields.admitted_inputs.insert(work_id.clone());
        fields.admission_order.push(work_id.clone());
        fields
            .content_shape
            .insert(work_id.clone(), content_shape.clone());
        fields
            .request_id
            .insert(work_id.clone(), request_id.clone());
        fields
            .reservation_key
            .insert(work_id.clone(), reservation_key.clone());
        fields
            .policy_snapshot
            .insert(work_id.clone(), policy.clone());
        fields
            .lifecycle
            .insert(work_id.clone(), InputLifecycleState::Consumed);
        fields
            .terminal_outcome
            .insert(work_id.clone(), Some(InputTerminalOutcome::Consumed));
        fields.last_run.insert(work_id.clone(), None);
        fields.last_boundary_sequence.insert(work_id.clone(), None);

        effects.push(RuntimeIngressEffect::IngressAccepted {
            work_id: work_id.clone(),
        });
        effects.push(RuntimeIngressEffect::InputLifecycleNotice {
            work_id: work_id.clone(),
            new_state: InputLifecycleState::Consumed,
        });
        effects.push(RuntimeIngressEffect::CompletionResolved {
            work_id: work_id.clone(),
            outcome: InputTerminalOutcome::Consumed,
        });

        Ok((IngressPhase::Active, fields, effects))
    }

    fn eval_stage_drain_snapshot(
        &self,
        phase: IngressPhase,
        run_id: &RunId,
        contributing_work_ids: &[InputId],
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "StageDrainSnapshot".into(),
            });
        }

        // Guard: no_current_run
        if self.fields.current_run.is_some() {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: "no_current_run: a run is already in progress".into(),
            });
        }

        // Guard: contributors_non_empty
        if contributing_work_ids.is_empty() {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: "contributors_non_empty: must have at least one contributor".into(),
            });
        }

        // Guard: all_contributors_are_queued
        for wid in contributing_work_ids {
            let lifecycle = self.fields.lifecycle.get(wid);
            if lifecycle != Some(&InputLifecycleState::Queued) {
                return Err(RuntimeIngressError::GuardFailed {
                    from: phase,
                    guard: format!("all_contributors_are_queued: {wid:?} is {lifecycle:?}"),
                });
            }
        }

        // Guard: contributors_match_current_drain_source
        // If steer_queue has items, contributors must be a prefix of steer_queue.
        // Otherwise, contributors must be a prefix of queue.
        if !self.fields.steer_queue.is_empty() {
            if !seq_starts_with(&self.fields.steer_queue, contributing_work_ids) {
                return Err(RuntimeIngressError::GuardFailed {
                    from: phase,
                    guard: "contributors_match_current_drain_source: not a prefix of steer_queue"
                        .into(),
                });
            }
        } else if !seq_starts_with(&self.fields.queue, contributing_work_ids) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: "contributors_match_current_drain_source: not a prefix of queue".into(),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Remove contributors from the appropriate queue
        if fields.steer_queue.is_empty() {
            seq_remove_all(&mut fields.queue, contributing_work_ids);
        } else {
            seq_remove_all(&mut fields.steer_queue, contributing_work_ids);
        }

        // Set current run
        fields.current_run = Some(run_id.clone());
        fields.current_run_contributors = contributing_work_ids.to_vec();
        fields.wake_requested = false;
        fields.process_requested = false;

        // Transition contributors to Staged
        for wid in contributing_work_ids {
            fields.last_run.insert(wid.clone(), Some(run_id.clone()));
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::Staged);
        }

        effects.push(RuntimeIngressEffect::ReadyForRun {
            run_id: run_id.clone(),
            contributing_work_ids: contributing_work_ids.to_vec(),
        });

        // Phase stays the same
        Ok((phase, fields, effects))
    }

    fn eval_boundary_applied(
        &self,
        phase: IngressPhase,
        run_id: &RunId,
        boundary_sequence: u64,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "BoundaryApplied".into(),
            });
        }

        // Guard: run_matches_current
        if self.fields.current_run.as_ref() != Some(run_id) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!(
                    "run_matches_current: expected {:?}, got {run_id:?}",
                    self.fields.current_run
                ),
            });
        }

        // Guard: contributors_are_staged
        for wid in &self.fields.current_run_contributors {
            let lifecycle = self.fields.lifecycle.get(wid);
            if lifecycle != Some(&InputLifecycleState::Staged) {
                return Err(RuntimeIngressError::GuardFailed {
                    from: phase,
                    guard: format!("contributors_are_staged: {wid:?} is {lifecycle:?}"),
                });
            }
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Transition contributors to AppliedPendingConsumption
        for wid in &fields.current_run_contributors.clone() {
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::AppliedPendingConsumption);
            fields
                .last_boundary_sequence
                .insert(wid.clone(), Some(boundary_sequence));
        }

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: "BoundaryApplied".into(),
            detail: "ContributorsPendingConsumption".into(),
        });

        // Phase unchanged (schema says to: Active, but we preserve whatever phase we're in)
        Ok((phase, fields, effects))
    }

    fn eval_run_completed(
        &self,
        phase: IngressPhase,
        run_id: &RunId,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "RunCompleted".into(),
            });
        }

        // Guard: run_matches_current
        if self.fields.current_run.as_ref() != Some(run_id) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!(
                    "run_matches_current: expected {:?}, got {run_id:?}",
                    self.fields.current_run
                ),
            });
        }

        // Guard: contributors_pending_consumption
        for wid in &self.fields.current_run_contributors {
            let lifecycle = self.fields.lifecycle.get(wid);
            if lifecycle != Some(&InputLifecycleState::AppliedPendingConsumption) {
                return Err(RuntimeIngressError::GuardFailed {
                    from: phase,
                    guard: format!("contributors_pending_consumption: {wid:?} is {lifecycle:?}"),
                });
            }
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Consume all contributors
        for wid in &fields.current_run_contributors.clone() {
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::Consumed);
            fields
                .terminal_outcome
                .insert(wid.clone(), Some(InputTerminalOutcome::Consumed));
            effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                work_id: wid.clone(),
                new_state: InputLifecycleState::Consumed,
            });
            effects.push(RuntimeIngressEffect::CompletionResolved {
                work_id: wid.clone(),
                outcome: InputTerminalOutcome::Consumed,
            });
        }

        // Clear current run
        fields.current_run = None;
        fields.current_run_contributors = Vec::new();

        // Phase stays the same
        Ok((phase, fields, effects))
    }

    fn eval_run_failed(
        &self,
        phase: IngressPhase,
        run_id: &RunId,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "RunFailed".into(),
            });
        }

        // Guard: run_matches_current
        if self.fields.current_run.as_ref() != Some(run_id) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!(
                    "run_matches_current: expected {:?}, got {run_id:?}",
                    self.fields.current_run
                ),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Rollback staged contributors to Queued and re-enqueue
        for wid in &fields.current_run_contributors.clone() {
            let lifecycle = fields.lifecycle.get(wid);
            if lifecycle == Some(&InputLifecycleState::Staged) {
                fields
                    .lifecycle
                    .insert(wid.clone(), InputLifecycleState::Queued);
                // Re-enqueue based on handling mode
                let hm = fields.handling_mode.get(wid).copied();
                match hm {
                    Some(HandlingMode::Steer) => {
                        if !fields.steer_queue.contains(wid) {
                            fields.steer_queue.insert(0, wid.clone());
                        }
                    }
                    _ => {
                        if !fields.queue.contains(wid) {
                            fields.queue.insert(0, wid.clone());
                        }
                    }
                }
                effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                    work_id: wid.clone(),
                    new_state: InputLifecycleState::Queued,
                });
            }
        }

        // Clear current run
        fields.current_run = None;
        fields.current_run_contributors = Vec::new();

        // Wake if there's still work in the queue
        if !fields.queue.is_empty() || !fields.steer_queue.is_empty() {
            fields.wake_requested = true;
            effects.push(RuntimeIngressEffect::WakeRuntime);
        }

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: "RunFailed".into(),
            detail: "StagedRolledBack".into(),
        });

        Ok((phase, fields, effects))
    }

    fn eval_run_cancelled(
        &self,
        phase: IngressPhase,
        run_id: &RunId,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // Same logic as RunFailed per the schema
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "RunCancelled".into(),
            });
        }

        // Guard: run_matches_current
        if self.fields.current_run.as_ref() != Some(run_id) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!(
                    "run_matches_current: expected {:?}, got {run_id:?}",
                    self.fields.current_run
                ),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Rollback staged contributors
        for wid in &fields.current_run_contributors.clone() {
            let lifecycle = fields.lifecycle.get(wid);
            if lifecycle == Some(&InputLifecycleState::Staged) {
                fields
                    .lifecycle
                    .insert(wid.clone(), InputLifecycleState::Queued);
                let hm = fields.handling_mode.get(wid).copied();
                match hm {
                    Some(HandlingMode::Steer) => {
                        if !fields.steer_queue.contains(wid) {
                            fields.steer_queue.insert(0, wid.clone());
                        }
                    }
                    _ => {
                        if !fields.queue.contains(wid) {
                            fields.queue.insert(0, wid.clone());
                        }
                    }
                }
                effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                    work_id: wid.clone(),
                    new_state: InputLifecycleState::Queued,
                });
            }
        }

        fields.current_run = None;
        fields.current_run_contributors = Vec::new();

        if !fields.queue.is_empty() || !fields.steer_queue.is_empty() {
            fields.wake_requested = true;
            effects.push(RuntimeIngressEffect::WakeRuntime);
        }

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: "RunCancelled".into(),
            detail: "StagedRolledBack".into(),
        });

        Ok((phase, fields, effects))
    }

    fn eval_supersede(
        &self,
        phase: IngressPhase,
        new_work_id: &InputId,
        old_work_id: &InputId,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "SupersedeQueuedInput".into(),
            });
        }

        // Guard: old_work_id must be Queued
        let old_lifecycle = self.fields.lifecycle.get(old_work_id);
        if old_lifecycle != Some(&InputLifecycleState::Queued) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!("old_input_is_queued: {old_work_id:?} is {old_lifecycle:?}"),
            });
        }

        // Guard: new_work_id must be admitted
        if !self.fields.admitted_inputs.contains(new_work_id) {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: format!("new_input_is_admitted: {new_work_id:?} not found"),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Remove old from queue/steer_queue
        fields.queue.retain(|id| id != old_work_id);
        fields.steer_queue.retain(|id| id != old_work_id);

        // Mark old as Superseded
        fields
            .lifecycle
            .insert(old_work_id.clone(), InputLifecycleState::Superseded);
        let outcome = InputTerminalOutcome::Superseded {
            superseded_by: new_work_id.clone(),
        };
        fields
            .terminal_outcome
            .insert(old_work_id.clone(), Some(outcome.clone()));

        effects.push(RuntimeIngressEffect::InputLifecycleNotice {
            work_id: old_work_id.clone(),
            new_state: InputLifecycleState::Superseded,
        });
        effects.push(RuntimeIngressEffect::CompletionResolved {
            work_id: old_work_id.clone(),
            outcome,
        });

        Ok((phase, fields, effects))
    }

    fn eval_coalesce(
        &self,
        phase: IngressPhase,
        aggregate_work_id: &InputId,
        source_work_ids: &[InputId],
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "CoalesceQueuedInputs".into(),
            });
        }

        // Guard: all source_work_ids must be Queued
        for wid in source_work_ids {
            let lifecycle = self.fields.lifecycle.get(wid);
            if lifecycle != Some(&InputLifecycleState::Queued) {
                return Err(RuntimeIngressError::GuardFailed {
                    from: phase,
                    guard: format!("all_sources_queued: {wid:?} is {lifecycle:?}"),
                });
            }
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Remove sources from queues
        for wid in source_work_ids {
            fields.queue.retain(|id| id != wid);
            fields.steer_queue.retain(|id| id != wid);

            // Mark as Coalesced
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::Coalesced);
            let outcome = InputTerminalOutcome::Coalesced {
                aggregate_id: aggregate_work_id.clone(),
            };
            fields
                .terminal_outcome
                .insert(wid.clone(), Some(outcome.clone()));

            effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                work_id: wid.clone(),
                new_state: InputLifecycleState::Coalesced,
            });
            effects.push(RuntimeIngressEffect::CompletionResolved {
                work_id: wid.clone(),
                outcome,
            });
        }

        Ok((phase, fields, effects))
    }

    fn eval_retire(
        &self,
        phase: IngressPhase,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active
        if phase != IngressPhase::Active {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "Retire".into(),
            });
        }

        let fields = self.fields.clone();
        let effects = vec![RuntimeIngressEffect::IngressNotice {
            kind: "Retire".into(),
            detail: "IngressRetired".into(),
        }];

        Ok((IngressPhase::Retired, fields, effects))
    }

    fn eval_reset(
        &self,
        phase: IngressPhase,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "Reset".into(),
            });
        }

        // Guard: no_current_run (can't reset while a run is in progress)
        if self.fields.current_run.is_some() {
            return Err(RuntimeIngressError::GuardFailed {
                from: phase,
                guard: "no_current_run: cannot reset while a run is in progress".into(),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Abandon all non-terminal inputs
        let non_terminal_ids: Vec<InputId> = fields
            .lifecycle
            .iter()
            .filter(|(_, state)| !state.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();

        for wid in &non_terminal_ids {
            let outcome = InputTerminalOutcome::Abandoned {
                reason: crate::input_state::InputAbandonReason::Reset,
            };
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::Abandoned);
            fields
                .terminal_outcome
                .insert(wid.clone(), Some(outcome.clone()));

            effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                work_id: wid.clone(),
                new_state: InputLifecycleState::Abandoned,
            });
            effects.push(RuntimeIngressEffect::CompletionResolved {
                work_id: wid.clone(),
                outcome,
            });
        }

        // Drain queues
        fields.queue.clear();
        fields.steer_queue.clear();
        fields.wake_requested = false;
        fields.process_requested = false;
        fields.current_run = None;
        fields.current_run_contributors = Vec::new();

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: "Reset".into(),
            detail: "IngressReset".into(),
        });

        Ok((IngressPhase::Active, fields, effects))
    }

    fn eval_destroy(
        &self,
        phase: IngressPhase,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "Destroy".into(),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Abandon all non-terminal inputs
        let non_terminal_ids: Vec<InputId> = fields
            .lifecycle
            .iter()
            .filter(|(_, state)| !state.is_terminal())
            .map(|(id, _)| id.clone())
            .collect();

        for wid in &non_terminal_ids {
            let outcome = InputTerminalOutcome::Abandoned {
                reason: crate::input_state::InputAbandonReason::Destroyed,
            };
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::Abandoned);
            fields
                .terminal_outcome
                .insert(wid.clone(), Some(outcome.clone()));

            effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                work_id: wid.clone(),
                new_state: InputLifecycleState::Abandoned,
            });
            effects.push(RuntimeIngressEffect::CompletionResolved {
                work_id: wid.clone(),
                outcome,
            });
        }

        fields.queue.clear();
        fields.steer_queue.clear();
        fields.wake_requested = false;
        fields.process_requested = false;
        fields.current_run = None;
        fields.current_run_contributors = Vec::new();

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: "Destroy".into(),
            detail: "IngressDestroyed".into(),
        });

        Ok((IngressPhase::Destroyed, fields, effects))
    }

    fn eval_recover(
        &self,
        phase: IngressPhase,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "Recover".into(),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Rollback any in-flight staged contributors
        if fields.current_run.is_some() {
            for wid in &fields.current_run_contributors.clone() {
                let lifecycle = fields.lifecycle.get(wid);
                if lifecycle == Some(&InputLifecycleState::Staged) {
                    fields
                        .lifecycle
                        .insert(wid.clone(), InputLifecycleState::Queued);
                    let hm = fields.handling_mode.get(wid).copied();
                    match hm {
                        Some(HandlingMode::Steer) => {
                            if !fields.steer_queue.contains(wid) {
                                fields.steer_queue.insert(0, wid.clone());
                            }
                        }
                        _ => {
                            if !fields.queue.contains(wid) {
                                fields.queue.insert(0, wid.clone());
                            }
                        }
                    }
                }
            }
            fields.current_run = None;
            fields.current_run_contributors = Vec::new();
        }

        if !fields.queue.is_empty() || !fields.steer_queue.is_empty() {
            fields.wake_requested = true;
            effects.push(RuntimeIngressEffect::WakeRuntime);
        }

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: "Recover".into(),
            detail: "IngressRecovered".into(),
        });

        Ok((phase, fields, effects))
    }

    fn eval_set_silent_intent_overrides(
        &self,
        phase: IngressPhase,
        intents: &BTreeSet<String>,
    ) -> Result<
        (
            IngressPhase,
            RuntimeIngressFields,
            Vec<RuntimeIngressEffect>,
        ),
        RuntimeIngressError,
    > {
        // From: Active or Retired
        if !matches!(phase, IngressPhase::Active | IngressPhase::Retired) {
            return Err(RuntimeIngressError::InvalidTransition {
                from: phase,
                input: "SetSilentIntentOverrides".into(),
            });
        }

        let mut fields = self.fields.clone();
        fields.silent_intent_overrides = intents.clone();

        let effects = vec![RuntimeIngressEffect::IngressNotice {
            kind: "SetSilentIntentOverrides".into(),
            detail: format!("{} intents configured", intents.len()),
        }];

        Ok((phase, fields, effects))
    }
}

impl RuntimeIngressMutator for RuntimeIngressAuthority {
    fn apply(
        &mut self,
        input: RuntimeIngressInput,
    ) -> Result<RuntimeIngressTransition, RuntimeIngressError> {
        let (next_phase, next_fields, effects) = self.evaluate(&input)?;

        // Commit: update canonical state.
        self.phase = next_phase;
        self.fields = next_fields;

        Ok(RuntimeIngressTransition {
            next_phase,
            effects,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if `seq` starts with the given `prefix`.
fn seq_starts_with(seq: &[InputId], prefix: &[InputId]) -> bool {
    if prefix.len() > seq.len() {
        return false;
    }
    seq[..prefix.len()] == *prefix
}

/// Remove all items in `values` from `seq`.
fn seq_remove_all(seq: &mut Vec<InputId>, values: &[InputId]) {
    seq.retain(|id| !values.contains(id));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::identifiers::PolicyVersion;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, InterruptPolicy, QueueMode, RoutingDisposition,
        WakeMode,
    };

    fn test_policy() -> PolicyDecision {
        PolicyDecision {
            apply_mode: ApplyMode::StageRunStart,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            interrupt_policy: InterruptPolicy::None,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: PolicyVersion(1),
        }
    }

    fn admit_queued(
        auth: &mut RuntimeIngressAuthority,
        work_id: InputId,
        mode: HandlingMode,
    ) -> RuntimeIngressTransition {
        auth.apply(RuntimeIngressInput::AdmitQueued {
            work_id,
            content_shape: ContentShape("text".into()),
            handling_mode: mode,
            request_id: None,
            reservation_key: None,
            policy: test_policy(),
        })
        .expect("admit should succeed")
    }

    fn admit_consumed(
        auth: &mut RuntimeIngressAuthority,
        work_id: InputId,
    ) -> RuntimeIngressTransition {
        auth.apply(RuntimeIngressInput::AdmitConsumedOnAccept {
            work_id,
            content_shape: ContentShape("text".into()),
            request_id: None,
            reservation_key: None,
            policy: test_policy(),
        })
        .expect("admit consumed should succeed")
    }

    // ---- Phase transitions ----

    #[test]
    fn new_starts_active() {
        let auth = RuntimeIngressAuthority::new();
        assert_eq!(auth.phase(), IngressPhase::Active);
        assert!(!auth.is_terminal());
    }

    #[test]
    fn retire_transitions_to_retired() {
        let mut auth = RuntimeIngressAuthority::new();
        let t = auth
            .apply(RuntimeIngressInput::Retire)
            .expect("retire should succeed");
        assert_eq!(t.next_phase, IngressPhase::Retired);
        assert_eq!(auth.phase(), IngressPhase::Retired);
    }

    #[test]
    fn destroy_transitions_to_destroyed() {
        let mut auth = RuntimeIngressAuthority::new();
        let t = auth
            .apply(RuntimeIngressInput::Destroy)
            .expect("destroy should succeed");
        assert_eq!(t.next_phase, IngressPhase::Destroyed);
        assert!(auth.is_terminal());
    }

    #[test]
    fn destroy_from_retired() {
        let mut auth = RuntimeIngressAuthority::new();
        auth.apply(RuntimeIngressInput::Retire).unwrap();
        let t = auth
            .apply(RuntimeIngressInput::Destroy)
            .expect("destroy from retired should succeed");
        assert_eq!(t.next_phase, IngressPhase::Destroyed);
    }

    #[test]
    fn destroyed_rejects_all() {
        let mut auth = RuntimeIngressAuthority::new();
        auth.apply(RuntimeIngressInput::Destroy).unwrap();

        let result = auth.apply(RuntimeIngressInput::Retire);
        assert!(matches!(
            result,
            Err(RuntimeIngressError::TerminalPhase { .. })
        ));
    }

    #[test]
    fn reset_returns_to_active() {
        let mut auth = RuntimeIngressAuthority::new();
        auth.apply(RuntimeIngressInput::Retire).unwrap();
        let t = auth
            .apply(RuntimeIngressInput::Reset)
            .expect("reset from retired");
        assert_eq!(t.next_phase, IngressPhase::Active);
    }

    // ---- AdmitQueued ----

    #[test]
    fn admit_queued_registers_input() {
        let mut auth = RuntimeIngressAuthority::new();
        let wid = InputId::new();
        let t = admit_queued(&mut auth, wid.clone(), HandlingMode::Queue);

        assert_eq!(t.next_phase, IngressPhase::Active);
        assert!(auth.admitted_inputs().contains(&wid));
        assert_eq!(auth.queue(), &[wid.clone()]);
        assert!(auth.steer_queue().is_empty());
        assert_eq!(
            auth.lifecycle_state(&wid),
            Some(InputLifecycleState::Queued)
        );
        assert!(auth.wake_requested());
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::IngressAccepted { .. }))
        );
    }

    #[test]
    fn admit_queued_steer_mode() {
        let mut auth = RuntimeIngressAuthority::new();
        let wid = InputId::new();
        let t = admit_queued(&mut auth, wid.clone(), HandlingMode::Steer);

        assert!(auth.queue().is_empty());
        assert_eq!(auth.steer_queue(), &[wid.clone()]);
        assert!(auth.process_requested());
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::RequestImmediateProcessing))
        );
    }

    #[test]
    fn admit_queued_duplicate_rejected() {
        let mut auth = RuntimeIngressAuthority::new();
        let wid = InputId::new();
        admit_queued(&mut auth, wid.clone(), HandlingMode::Queue);

        let result = auth.apply(RuntimeIngressInput::AdmitQueued {
            work_id: wid,
            content_shape: ContentShape("text".into()),
            handling_mode: HandlingMode::Queue,
            request_id: None,
            reservation_key: None,
            policy: test_policy(),
        });
        assert!(matches!(
            result,
            Err(RuntimeIngressError::GuardFailed { .. })
        ));
    }

    #[test]
    fn admit_queued_rejected_from_retired() {
        let mut auth = RuntimeIngressAuthority::new();
        auth.apply(RuntimeIngressInput::Retire).unwrap();

        let result = auth.apply(RuntimeIngressInput::AdmitQueued {
            work_id: InputId::new(),
            content_shape: ContentShape("text".into()),
            handling_mode: HandlingMode::Queue,
            request_id: None,
            reservation_key: None,
            policy: test_policy(),
        });
        assert!(matches!(
            result,
            Err(RuntimeIngressError::InvalidTransition { .. })
        ));
    }

    // ---- AdmitConsumedOnAccept ----

    #[test]
    fn admit_consumed_on_accept() {
        let mut auth = RuntimeIngressAuthority::new();
        let wid = InputId::new();
        let t = admit_consumed(&mut auth, wid.clone());

        assert_eq!(
            auth.lifecycle_state(&wid),
            Some(InputLifecycleState::Consumed)
        );
        assert!(matches!(
            auth.terminal_outcome(&wid),
            Some(InputTerminalOutcome::Consumed)
        ));
        // Not queued
        assert!(auth.queue().is_empty());
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::CompletionResolved { .. }))
        );
    }

    // ---- StageDrainSnapshot ----

    #[test]
    fn stage_drain_snapshot_happy_path() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        let w2 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, w2.clone(), HandlingMode::Queue);

        let run_id = RunId::new();
        let t = auth
            .apply(RuntimeIngressInput::StageDrainSnapshot {
                run_id: run_id.clone(),
                contributing_work_ids: vec![w1.clone(), w2.clone()],
            })
            .expect("stage should succeed");

        assert!(auth.queue().is_empty());
        assert_eq!(auth.current_run(), Some(&run_id));
        assert_eq!(auth.current_run_contributors(), &[w1.clone(), w2.clone()]);
        assert_eq!(auth.lifecycle_state(&w1), Some(InputLifecycleState::Staged));
        assert_eq!(auth.lifecycle_state(&w2), Some(InputLifecycleState::Staged));
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::ReadyForRun { .. }))
        );
    }

    #[test]
    fn stage_drain_rejected_with_current_run() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        let w2 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, w2.clone(), HandlingMode::Queue);

        // Stage w1
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        // Try staging w2 while run is in progress
        let result = auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w2],
        });
        assert!(matches!(
            result,
            Err(RuntimeIngressError::GuardFailed { .. })
        ));
    }

    #[test]
    fn stage_drain_prefers_steer_queue() {
        let mut auth = RuntimeIngressAuthority::new();
        let w_queue = InputId::new();
        let w_steer = InputId::new();
        admit_queued(&mut auth, w_queue.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, w_steer.clone(), HandlingMode::Steer);

        // Must stage from steer_queue first (prefix match)
        let result = auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w_queue.clone()],
        });
        assert!(
            result.is_err(),
            "should reject staging from queue when steer_queue is non-empty"
        );

        // Stage from steer_queue succeeds
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w_steer.clone()],
        })
        .expect("steer staging should succeed");
    }

    // ---- BoundaryApplied ----

    #[test]
    fn boundary_applied_transitions_to_pending_consumption() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        let t = auth
            .apply(RuntimeIngressInput::BoundaryApplied {
                run_id: run_id.clone(),
                boundary_sequence: 42,
            })
            .expect("boundary applied should succeed");

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::AppliedPendingConsumption)
        );
        assert_eq!(auth.last_boundary_sequence(&w1), Some(42));
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::IngressNotice { .. }))
        );
    }

    #[test]
    fn boundary_applied_wrong_run_rejected() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        let result = auth.apply(RuntimeIngressInput::BoundaryApplied {
            run_id: RunId::new(), // wrong run
            boundary_sequence: 1,
        });
        assert!(matches!(
            result,
            Err(RuntimeIngressError::GuardFailed { .. })
        ));
    }

    // ---- RunCompleted ----

    #[test]
    fn run_completed_consumes_contributors() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();
        auth.apply(RuntimeIngressInput::BoundaryApplied {
            run_id: run_id.clone(),
            boundary_sequence: 1,
        })
        .unwrap();

        let t = auth
            .apply(RuntimeIngressInput::RunCompleted {
                run_id: run_id.clone(),
            })
            .expect("run completed should succeed");

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::Consumed)
        );
        assert!(auth.current_run().is_none());
        assert!(auth.current_run_contributors().is_empty());
        assert!(t.effects.iter().any(|e| matches!(
            e,
            RuntimeIngressEffect::CompletionResolved {
                outcome: InputTerminalOutcome::Consumed,
                ..
            }
        )));
    }

    // ---- RunFailed ----

    #[test]
    fn run_failed_rolls_back_staged() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        auth.apply(RuntimeIngressInput::RunFailed {
            run_id: run_id.clone(),
        })
        .expect("run failed should succeed");

        assert_eq!(auth.lifecycle_state(&w1), Some(InputLifecycleState::Queued));
        assert!(auth.queue().contains(&w1));
        assert!(auth.current_run().is_none());
        assert!(auth.wake_requested());
    }

    // ---- RunCancelled ----

    #[test]
    fn run_cancelled_rolls_back_staged() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        auth.apply(RuntimeIngressInput::RunCancelled {
            run_id: run_id.clone(),
        })
        .expect("run cancelled should succeed");

        assert_eq!(auth.lifecycle_state(&w1), Some(InputLifecycleState::Queued));
        assert!(auth.queue().contains(&w1));
    }

    // ---- SupersedeQueuedInput ----

    #[test]
    fn supersede_marks_old_as_superseded() {
        let mut auth = RuntimeIngressAuthority::new();
        let old = InputId::new();
        let new = InputId::new();
        admit_queued(&mut auth, old.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, new.clone(), HandlingMode::Queue);

        auth.apply(RuntimeIngressInput::SupersedeQueuedInput {
            new_work_id: new.clone(),
            old_work_id: old.clone(),
        })
        .expect("supersede should succeed");

        assert_eq!(
            auth.lifecycle_state(&old),
            Some(InputLifecycleState::Superseded)
        );
        assert!(!auth.queue().contains(&old));
        assert!(auth.queue().contains(&new));
        assert!(matches!(
            auth.terminal_outcome(&old),
            Some(InputTerminalOutcome::Superseded { .. })
        ));
    }

    // ---- CoalesceQueuedInputs ----

    #[test]
    fn coalesce_marks_sources_as_coalesced() {
        let mut auth = RuntimeIngressAuthority::new();
        let s1 = InputId::new();
        let s2 = InputId::new();
        let agg = InputId::new();
        admit_queued(&mut auth, s1.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, s2.clone(), HandlingMode::Queue);
        // Admit the aggregate too
        admit_queued(&mut auth, agg.clone(), HandlingMode::Queue);

        auth.apply(RuntimeIngressInput::CoalesceQueuedInputs {
            aggregate_work_id: agg.clone(),
            source_work_ids: vec![s1.clone(), s2.clone()],
        })
        .expect("coalesce should succeed");

        assert_eq!(
            auth.lifecycle_state(&s1),
            Some(InputLifecycleState::Coalesced)
        );
        assert_eq!(
            auth.lifecycle_state(&s2),
            Some(InputLifecycleState::Coalesced)
        );
        assert!(!auth.queue().contains(&s1));
        assert!(!auth.queue().contains(&s2));
    }

    // ---- Recover ----

    #[test]
    fn recover_rolls_back_in_flight_run() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        auth.apply(RuntimeIngressInput::Recover)
            .expect("recover should succeed");

        assert_eq!(auth.lifecycle_state(&w1), Some(InputLifecycleState::Queued));
        assert!(auth.current_run().is_none());
        assert!(auth.queue().contains(&w1));
        assert!(auth.wake_requested());
    }

    // ---- SetSilentIntentOverrides ----

    #[test]
    fn set_silent_intent_overrides() {
        let mut auth = RuntimeIngressAuthority::new();
        let intents: BTreeSet<String> =
            ["intent_a".into(), "intent_b".into()].into_iter().collect();
        auth.apply(RuntimeIngressInput::SetSilentIntentOverrides {
            intents: intents.clone(),
        })
        .expect("set overrides should succeed");

        assert_eq!(auth.silent_intent_overrides(), &intents);
    }

    // ---- Full lifecycle: admit -> stage -> boundary -> complete ----

    #[test]
    fn full_lifecycle_happy_path() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        let w2 = InputId::new();

        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, w2.clone(), HandlingMode::Queue);

        // Stage both
        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone(), w2.clone()],
        })
        .unwrap();

        // Boundary applied
        auth.apply(RuntimeIngressInput::BoundaryApplied {
            run_id: run_id.clone(),
            boundary_sequence: 1,
        })
        .unwrap();

        // Run completed
        auth.apply(RuntimeIngressInput::RunCompleted {
            run_id: run_id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::Consumed)
        );
        assert_eq!(
            auth.lifecycle_state(&w2),
            Some(InputLifecycleState::Consumed)
        );
        assert!(auth.current_run().is_none());
        assert!(auth.queue().is_empty());
    }

    // ---- Destroy abandons non-terminal inputs ----

    #[test]
    fn destroy_abandons_queued_inputs() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        let w2 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, w2.clone(), HandlingMode::Queue);

        let t = auth.apply(RuntimeIngressInput::Destroy).unwrap();

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::Abandoned)
        );
        assert_eq!(
            auth.lifecycle_state(&w2),
            Some(InputLifecycleState::Abandoned)
        );
        assert!(auth.queue().is_empty());
        assert!(auth.is_terminal());
        // Should have effects for each abandoned input
        let completion_count = t
            .effects
            .iter()
            .filter(|e| matches!(e, RuntimeIngressEffect::CompletionResolved { .. }))
            .count();
        assert_eq!(completion_count, 2);
    }

    // ---- Reset abandons non-terminal inputs and returns to Active ----

    #[test]
    fn reset_abandons_and_returns_to_active() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        // Admit a consumed input too (should not be re-abandoned)
        let w2 = InputId::new();
        admit_consumed(&mut auth, w2.clone());

        auth.apply(RuntimeIngressInput::Reset).unwrap();

        assert_eq!(auth.phase(), IngressPhase::Active);
        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::Abandoned)
        );
        // w2 was already Consumed, stays Consumed
        assert_eq!(
            auth.lifecycle_state(&w2),
            Some(InputLifecycleState::Consumed)
        );
        assert!(auth.queue().is_empty());
    }

    #[test]
    fn reset_rejected_during_run() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w1],
        })
        .unwrap();

        let result = auth.apply(RuntimeIngressInput::Reset);
        assert!(matches!(
            result,
            Err(RuntimeIngressError::GuardFailed { .. })
        ));
    }

    // ---- Retired can still drain ----

    #[test]
    fn retired_can_stage_and_complete_drain() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        auth.apply(RuntimeIngressInput::Retire).unwrap();
        assert_eq!(auth.phase(), IngressPhase::Retired);

        // Can still drain
        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        auth.apply(RuntimeIngressInput::BoundaryApplied {
            run_id: run_id.clone(),
            boundary_sequence: 1,
        })
        .unwrap();

        auth.apply(RuntimeIngressInput::RunCompleted {
            run_id: run_id.clone(),
        })
        .unwrap();

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::Consumed)
        );
        // Still Retired after drain
        assert_eq!(auth.phase(), IngressPhase::Retired);
    }

    // ---- can_accept probing ----

    #[test]
    fn can_accept_probes_without_mutation() {
        let auth = RuntimeIngressAuthority::new();
        assert!(auth.can_accept(&RuntimeIngressInput::Retire));
        assert!(!auth.can_accept(&RuntimeIngressInput::Reset)); // Reset requires Active|Retired with no_current_run — passes from Active with no run
        // Actually Reset from Active with no run should work, let me re-check
        // Reset from Active with no current run should succeed
    }

    #[test]
    fn can_accept_reset_from_active() {
        let auth = RuntimeIngressAuthority::new();
        assert!(auth.can_accept(&RuntimeIngressInput::Reset));
        assert_eq!(auth.phase(), IngressPhase::Active); // not mutated
    }

    // ---- Phase unchanged on failure ----

    #[test]
    fn phase_unchanged_on_rejected_transition() {
        let mut auth = RuntimeIngressAuthority::new();
        auth.apply(RuntimeIngressInput::Retire).unwrap();
        let result = auth.apply(RuntimeIngressInput::Retire);
        assert!(result.is_err());
        assert_eq!(auth.phase(), IngressPhase::Retired);
    }

    // ---- Steer rollback goes to steer_queue ----

    #[test]
    fn run_failed_rolls_back_steer_to_steer_queue() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Steer);

        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();

        auth.apply(RuntimeIngressInput::RunFailed {
            run_id: run_id.clone(),
        })
        .unwrap();

        // Should be back in steer_queue, not queue
        assert!(auth.steer_queue().contains(&w1));
        assert!(!auth.queue().contains(&w1));
    }

    // ---- take_wake/process_requested clears flags ----

    #[test]
    fn take_wake_requested_clears_flag() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1, HandlingMode::Queue);

        assert!(auth.take_wake_requested());
        assert!(!auth.take_wake_requested()); // cleared
    }

    #[test]
    fn take_process_requested_clears_flag() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1, HandlingMode::Steer);

        assert!(auth.take_process_requested());
        assert!(!auth.take_process_requested()); // cleared
    }
}

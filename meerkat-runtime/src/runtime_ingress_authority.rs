//! Queue/ledger helper for runtime ingress bookkeeping.
//!
//! This helper still owns the admitted-input ledger, queue routing, and
//! mechanical queue/lifecycle updates for run contributors, but it no longer
//! owns a separate coarse ingress lifecycle or contributor-set legality.
//! Meerkat phase plus the checked-in runtime machine now own coarse lifecycle
//! truth and run-batch legality; this helper exists only for lower-level
//! ledger and queue mechanics.

use std::collections::HashMap;

use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::HandlingMode;

use crate::accept::{AdmissionQueueAction, ExistingQueuedAdmissionAction};
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
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the RuntimeIngress helper.
///
/// Shell code classifies raw inputs into these typed inputs, then calls
/// [`RuntimeIngressAuthority::apply`]. The checked-in `MeerkatMachine` now owns
/// coarse lifecycle legality plus admission/replay classification; this helper
/// applies the already-decided queue/ledger mutations and emits the matching
/// mechanical effects.
#[derive(Debug, Clone)]
pub enum RuntimeIngressInput {
    /// Admit a queued input (Queue or Steer handling mode).
    AdmitQueued {
        work_id: InputId,
        content_shape: ContentShape,
        handling_mode: HandlingMode,
        is_prompt: bool,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
        policy: PolicyDecision,
        /// Whether the machine decided this admission should materialize a
        /// queued ledger/queue entry rather than remain a no-routing accept.
        persist_and_queue: bool,
        /// Machine-owned routing plan for this admitted input.
        queue_action: AdmissionQueueAction,
        /// Machine-owned action against an existing queued input.
        existing_action: Option<ExistingQueuedAdmissionAction>,
    },
    /// Admit an input that is immediately consumed on accept (Ignore+OnAccept).
    AdmitConsumedOnAccept {
        work_id: InputId,
        content_shape: ContentShape,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
        policy: PolicyDecision,
    },
    /// Admit a deduplicated input. The shell performed the ledger lookup
    /// (mechanical IO) and found an existing input with the same idempotency
    /// key. The authority records the dedup and emits the appropriate effect.
    AdmitDeduplicated {
        work_id: InputId,
        existing_id: InputId,
    },
    /// Stage a drain snapshot: dequeue work and assign to a run.
    StageDrainSnapshot {
        run_id: RunId,
        contributing_work_ids: Vec<InputId>,
    },
    /// Boundary applied: mark contributors as AppliedPendingConsumption.
    BoundaryApplied {
        contributing_work_ids: Vec<InputId>,
        boundary_sequence: u64,
    },
    /// Run completed: consume all contributors.
    RunCompleted { contributing_work_ids: Vec<InputId> },
    /// Replay already-classified staged contributors back to their queue lanes.
    ///
    /// The checked-in Meerkat machine owns replay classification and wake
    /// semantics; the ingress helper only applies the already-decided queue and
    /// lifecycle mutations.
    ReplayQueuedContributors {
        queue_work_ids: Vec<InputId>,
        steer_work_ids: Vec<InputId>,
        wake_runtime: bool,
        notice_kind: String,
    },
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
    /// Admit a recovered input from persistent store (crash recovery).
    /// Restores the input into ingress tracking at its persisted lifecycle state
    /// without re-running the admission policy.
    AdmitRecovered {
        work_id: InputId,
        content_shape: ContentShape,
        handling_mode: HandlingMode,
        lifecycle_state: InputLifecycleState,
        policy: PolicyDecision,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
    },
    /// Recover: re-derive transient state from canonical state.
    /// Reconcile inputs that became terminal outside the ingress authority's
    /// local retry bookkeeping.
    ReconcileTerminalInputs {
        terminal_inputs: Vec<(InputId, InputTerminalOutcome)>,
    },
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
    /// An input was deduplicated against an existing input.
    Deduplicated {
        work_id: InputId,
        existing_id: InputId,
    },
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
    /// Interrupt cooperative yielding points within the running turn.
    InterruptYielding,
    /// Immediate processing requested (steer/immediate drain).
    RequestImmediateProcessing,
    /// An input reached a terminal outcome.
    CompletionResolved {
        work_id: InputId,
        outcome: InputTerminalOutcome,
    },
    /// Informational ingress notice.
    IngressNotice { kind: String, detail: String },
    // --- Accept-phase shell directives ---
    // These effects tell the shell exactly what to do with the Input payload
    // and ledger after admission, removing all policy branching from shell code.
    /// Persist the input payload on the ledger entry and transition lifecycle to Queued.
    PersistAndQueue { work_id: InputId },
    /// Route input to the back of the correct queue.
    EnqueueTo {
        work_id: InputId,
        target: HandlingMode,
    },
    /// Route input to the front of the correct queue (Priority mode).
    EnqueueFront {
        work_id: InputId,
        target: HandlingMode,
    },
    /// Remove an existing input from queues (for coalesce/supersede).
    RemoveFromQueues { work_id: InputId },
    /// Apply coalescing to an existing input.
    CoalesceExisting {
        new_id: InputId,
        existing_id: InputId,
    },
    /// Apply supersession to an existing input.
    SupersedeExisting {
        new_id: InputId,
        existing_id: InputId,
    },
    /// Consume input on accept (Ignore+OnAccept path).
    ConsumeOnAccept { work_id: InputId },
    /// Emit a Queued lifecycle event.
    EmitQueuedEvent { work_id: InputId },

    // --- Stage-phase shell directive ---
    // Emitted by StageDrainSnapshot so the per-input lifecycle transition is
    // clearly derived from the ingress authority's staging decision rather
    // than being an independent shell write.
    /// Stage an input for a run — shell calls the per-input lifecycle
    /// authority and emits the Staged event in response.
    StageInput { work_id: InputId, run_id: RunId },
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the RuntimeIngress authority.
#[derive(Debug)]
pub struct RuntimeIngressTransition {
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
    #[error("Invalid ingress transition via {input} (rejected)")]
    InvalidTransition { input: String },
    /// A guard condition was not met.
    #[error("Guard failed: {guard}")]
    GuardFailed { guard: String },
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
    admission_order: Vec<InputId>,
    content_shape: HashMap<InputId, ContentShape>,
    request_id: HashMap<InputId, Option<RequestId>>,
    reservation_key: HashMap<InputId, Option<ReservationKey>>,
    policy_snapshot: HashMap<InputId, PolicyDecision>,
    handling_mode: HashMap<InputId, HandlingMode>,
    is_prompt: HashMap<InputId, bool>,
    lifecycle: HashMap<InputId, InputLifecycleState>,
    queue: Vec<InputId>,
    steer_queue: Vec<InputId>,
}

impl RuntimeIngressFields {
    fn new() -> Self {
        Self {
            admission_order: Vec::new(),
            content_shape: HashMap::new(),
            request_id: HashMap::new(),
            reservation_key: HashMap::new(),
            policy_snapshot: HashMap::new(),
            handling_mode: HashMap::new(),
            is_prompt: HashMap::new(),
            lifecycle: HashMap::new(),
            queue: Vec::new(),
            steer_queue: Vec::new(),
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

/// The canonical helper for ingress ledger/queue state.
///
/// Coarse lifecycle truth is owned by `MeerkatMachine`; this helper owns only
/// the admitted-input ledger, queue routing, and per-run contributor fields.
#[derive(Debug, Clone)]
pub struct RuntimeIngressAuthority {
    fields: RuntimeIngressFields,
}

impl sealed::Sealed for RuntimeIngressAuthority {}

impl Default for RuntimeIngressAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeIngressAuthority {
    /// Create a new ingress helper.
    pub fn new() -> Self {
        Self {
            fields: RuntimeIngressFields::new(),
        }
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

    /// Lifecycle state for a specific work ID.
    pub fn lifecycle_state(&self, work_id: &InputId) -> Option<InputLifecycleState> {
        self.fields.lifecycle.get(work_id).copied()
    }

    /// Policy snapshot for a specific work ID.
    pub fn policy_snapshot(&self, work_id: &InputId) -> Option<&PolicyDecision> {
        self.fields.policy_snapshot.get(work_id)
    }

    /// Content shape for a specific work ID.
    pub fn content_shape(&self, work_id: &InputId) -> Option<ContentShape> {
        self.fields.content_shape.get(work_id).cloned()
    }

    /// Request ID for a specific work ID.
    pub fn request_id(&self, work_id: &InputId) -> Option<RequestId> {
        self.fields.request_id.get(work_id).cloned().flatten()
    }

    /// Reservation key for a specific work ID.
    pub fn reservation_key(&self, work_id: &InputId) -> Option<ReservationKey> {
        self.fields.reservation_key.get(work_id).cloned().flatten()
    }

    /// Handling mode for a specific work ID.
    pub fn handling_mode(&self, work_id: &InputId) -> Option<HandlingMode> {
        self.fields.handling_mode.get(work_id).copied()
    }

    /// Whether the input was classified as a prompt at admission.
    pub fn is_prompt(&self, work_id: &InputId) -> bool {
        self.fields.is_prompt.get(work_id).copied().unwrap_or(false)
    }

    /// Check if a transition is legal without applying it.
    pub fn can_accept(&self, input: &RuntimeIngressInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: &RuntimeIngressInput,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        match input {
            RuntimeIngressInput::AdmitQueued {
                work_id,
                content_shape,
                handling_mode,
                is_prompt,
                request_id,
                reservation_key,
                policy,
                persist_and_queue,
                queue_action,
                existing_action,
            } => self.eval_admit_queued(
                work_id,
                content_shape,
                *handling_mode,
                *is_prompt,
                request_id,
                reservation_key,
                policy,
                *persist_and_queue,
                queue_action,
                existing_action,
            ),

            RuntimeIngressInput::AdmitConsumedOnAccept {
                work_id,
                content_shape,
                request_id,
                reservation_key,
                policy,
            } => self.eval_admit_consumed_on_accept(
                work_id,
                content_shape,
                request_id,
                reservation_key,
                policy,
            ),

            RuntimeIngressInput::AdmitDeduplicated {
                work_id,
                existing_id,
            } => self.eval_admit_deduplicated(work_id, existing_id),

            RuntimeIngressInput::StageDrainSnapshot {
                run_id,
                contributing_work_ids,
            } => self.eval_stage_drain_snapshot(run_id, contributing_work_ids),

            RuntimeIngressInput::BoundaryApplied {
                contributing_work_ids,
                boundary_sequence,
            } => self.eval_boundary_applied(contributing_work_ids, *boundary_sequence),

            RuntimeIngressInput::RunCompleted {
                contributing_work_ids,
            } => self.eval_run_completed(contributing_work_ids),
            RuntimeIngressInput::ReplayQueuedContributors {
                queue_work_ids,
                steer_work_ids,
                wake_runtime,
                notice_kind,
            } => self.eval_replay_queued_contributors(
                queue_work_ids,
                steer_work_ids,
                *wake_runtime,
                notice_kind,
            ),

            RuntimeIngressInput::SupersedeQueuedInput {
                new_work_id,
                old_work_id,
            } => self.eval_supersede(new_work_id, old_work_id),

            RuntimeIngressInput::CoalesceQueuedInputs {
                aggregate_work_id,
                source_work_ids,
            } => self.eval_coalesce(aggregate_work_id, source_work_ids),

            RuntimeIngressInput::AdmitRecovered {
                work_id,
                content_shape,
                handling_mode,
                lifecycle_state,
                policy,
                request_id,
                reservation_key,
            } => self.eval_admit_recovered(
                work_id.clone(),
                content_shape.clone(),
                *handling_mode,
                *lifecycle_state,
                policy.clone(),
                request_id.clone(),
                reservation_key.clone(),
            ),

            RuntimeIngressInput::ReconcileTerminalInputs { terminal_inputs } => {
                self.eval_reconcile_terminal_inputs(terminal_inputs)
            }
        }
    }

    // ---- Transition evaluators ----

    #[allow(clippy::too_many_arguments)]
    fn eval_admit_queued(
        &self,
        work_id: &InputId,
        content_shape: &ContentShape,
        handling_mode: HandlingMode,
        is_prompt: bool,
        request_id: &Option<RequestId>,
        reservation_key: &Option<ReservationKey>,
        policy: &PolicyDecision,
        persist_and_queue: bool,
        queue_action: &AdmissionQueueAction,
        existing_action: &Option<ExistingQueuedAdmissionAction>,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        // Guard: input_is_new
        if self.fields.lifecycle.contains_key(work_id) {
            return Err(RuntimeIngressError::GuardFailed {
                guard: format!("input_is_new: {work_id:?} already admitted"),
            });
        }
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Register admission
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
        fields.is_prompt.insert(work_id.clone(), is_prompt);
        fields
            .lifecycle
            .insert(work_id.clone(), InputLifecycleState::Queued);

        // Route to queue or steer_queue based on handling mode
        match handling_mode {
            HandlingMode::Queue => fields.queue.push(work_id.clone()),
            HandlingMode::Steer => fields.steer_queue.push(work_id.clone()),
        }

        effects.push(RuntimeIngressEffect::IngressAccepted {
            work_id: work_id.clone(),
        });
        effects.push(RuntimeIngressEffect::InputLifecycleNotice {
            work_id: work_id.clone(),
            new_state: InputLifecycleState::Queued,
        });

        // --- Shell-directive effects ---
        // Routing/coalescing/supersession decisions are machine-owned; this
        // helper only realizes the already-decided mutations.
        if persist_and_queue {
            effects.push(RuntimeIngressEffect::PersistAndQueue {
                work_id: work_id.clone(),
            });

            if let Some(existing_action) = existing_action {
                match existing_action {
                    ExistingQueuedAdmissionAction::Coalesce { existing_id } => {
                        effects.push(RuntimeIngressEffect::RemoveFromQueues {
                            work_id: existing_id.clone(),
                        });
                        effects.push(RuntimeIngressEffect::CoalesceExisting {
                            new_id: work_id.clone(),
                            existing_id: existing_id.clone(),
                        });
                    }
                    ExistingQueuedAdmissionAction::Supersede { existing_id } => {
                        effects.push(RuntimeIngressEffect::RemoveFromQueues {
                            work_id: existing_id.clone(),
                        });
                        effects.push(RuntimeIngressEffect::SupersedeExisting {
                            new_id: work_id.clone(),
                            existing_id: existing_id.clone(),
                        });
                    }
                }
            }

            match queue_action {
                AdmissionQueueAction::None => {}
                AdmissionQueueAction::EnqueueTo { target } => {
                    effects.push(RuntimeIngressEffect::EnqueueTo {
                        work_id: work_id.clone(),
                        target: *target,
                    });
                }
                AdmissionQueueAction::EnqueueFront { target } => {
                    effects.push(RuntimeIngressEffect::EnqueueFront {
                        work_id: work_id.clone(),
                        target: *target,
                    });
                }
            }

            effects.push(RuntimeIngressEffect::EmitQueuedEvent {
                work_id: work_id.clone(),
            });
        }

        Ok((fields, effects))
    }

    fn eval_admit_deduplicated(
        &self,
        work_id: &InputId,
        existing_id: &InputId,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        // No state mutation — the duplicate input is not admitted.
        // Emit Deduplicated effect so the shell can emit the lifecycle event.
        let fields = self.fields.clone();
        let effects = vec![RuntimeIngressEffect::Deduplicated {
            work_id: work_id.clone(),
            existing_id: existing_id.clone(),
        }];
        Ok((fields, effects))
    }

    fn eval_admit_consumed_on_accept(
        &self,
        work_id: &InputId,
        content_shape: &ContentShape,
        request_id: &Option<RequestId>,
        reservation_key: &Option<ReservationKey>,
        policy: &PolicyDecision,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        // Guard: input_is_new
        if self.fields.lifecycle.contains_key(work_id) {
            return Err(RuntimeIngressError::GuardFailed {
                guard: format!("input_is_new: {work_id:?} already admitted"),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // Register admission (immediately consumed)
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

        // Shell directive: consume this input on accept.
        effects.push(RuntimeIngressEffect::ConsumeOnAccept {
            work_id: work_id.clone(),
        });

        Ok((fields, effects))
    }

    fn eval_stage_drain_snapshot(
        &self,
        run_id: &RunId,
        contributing_work_ids: &[InputId],
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // The checked-in Meerkat machine already owns contributor-set legality
        // for StageDrainSnapshot. This helper only applies the already-decided
        // queue/lifecycle updates.
        if fields.steer_queue.is_empty() {
            seq_remove_all(&mut fields.queue, contributing_work_ids);
        } else {
            seq_remove_all(&mut fields.steer_queue, contributing_work_ids);
        }

        // Transition contributors to Staged and emit per-input StageInput effects
        // so the shell derives per-input lifecycle transitions from this authority.
        for wid in contributing_work_ids {
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::Staged);
            effects.push(RuntimeIngressEffect::StageInput {
                work_id: wid.clone(),
                run_id: run_id.clone(),
            });
        }

        effects.push(RuntimeIngressEffect::ReadyForRun {
            run_id: run_id.clone(),
            contributing_work_ids: contributing_work_ids.to_vec(),
        });

        // Phase stays the same
        Ok((fields, effects))
    }

    fn eval_boundary_applied(
        &self,
        contributing_work_ids: &[InputId],
        _boundary_sequence: u64,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // The checked-in Meerkat machine owns boundary-applied contributor
        // legality. This helper only applies the already-decided lifecycle
        // updates.
        for wid in contributing_work_ids {
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::AppliedPendingConsumption);
        }

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: "BoundaryApplied".into(),
            detail: "ContributorsPendingConsumption".into(),
        });

        Ok((fields, effects))
    }

    fn eval_run_completed(
        &self,
        contributing_work_ids: &[InputId],
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        // The checked-in Meerkat machine owns run-completion contributor
        // legality. This helper only applies the already-decided lifecycle
        // updates.
        for wid in contributing_work_ids {
            fields
                .lifecycle
                .insert(wid.clone(), InputLifecycleState::Consumed);
            effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                work_id: wid.clone(),
                new_state: InputLifecycleState::Consumed,
            });
            effects.push(RuntimeIngressEffect::CompletionResolved {
                work_id: wid.clone(),
                outcome: InputTerminalOutcome::Consumed,
            });
        }

        Ok((fields, effects))
    }

    fn eval_replay_queued_contributors(
        &self,
        queue_work_ids: &[InputId],
        steer_work_ids: &[InputId],
        wake_runtime: bool,
        notice_kind: &str,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        if queue_work_ids.is_empty() && steer_work_ids.is_empty() {
            return Err(RuntimeIngressError::GuardFailed {
                guard: "current_run_present: no contributors are staged".into(),
            });
        }

        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        for wid in queue_work_ids {
            if fields.lifecycle.get(wid) == Some(&InputLifecycleState::Staged) {
                fields
                    .lifecycle
                    .insert(wid.clone(), InputLifecycleState::Queued);
                if !fields.queue.contains(wid) {
                    fields.queue.insert(0, wid.clone());
                }
                effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                    work_id: wid.clone(),
                    new_state: InputLifecycleState::Queued,
                });
            }
        }

        for wid in steer_work_ids {
            if fields.lifecycle.get(wid) == Some(&InputLifecycleState::Staged) {
                fields
                    .lifecycle
                    .insert(wid.clone(), InputLifecycleState::Queued);
                if !fields.steer_queue.contains(wid) {
                    fields.steer_queue.insert(0, wid.clone());
                }
                effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                    work_id: wid.clone(),
                    new_state: InputLifecycleState::Queued,
                });
            }
        }

        if wake_runtime {
            effects.push(RuntimeIngressEffect::WakeRuntime);
        }

        effects.push(RuntimeIngressEffect::IngressNotice {
            kind: notice_kind.into(),
            detail: "StagedRolledBack".into(),
        });

        Ok((fields, effects))
    }

    fn eval_supersede(
        &self,
        new_work_id: &InputId,
        old_work_id: &InputId,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        // Guard: old_work_id must be Queued
        let old_lifecycle = self.fields.lifecycle.get(old_work_id);
        if old_lifecycle != Some(&InputLifecycleState::Queued) {
            return Err(RuntimeIngressError::GuardFailed {
                guard: format!("old_input_is_queued: {old_work_id:?} is {old_lifecycle:?}"),
            });
        }

        // Guard: new_work_id must be admitted
        if !self.fields.lifecycle.contains_key(new_work_id) {
            return Err(RuntimeIngressError::GuardFailed {
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

        effects.push(RuntimeIngressEffect::InputLifecycleNotice {
            work_id: old_work_id.clone(),
            new_state: InputLifecycleState::Superseded,
        });
        effects.push(RuntimeIngressEffect::CompletionResolved {
            work_id: old_work_id.clone(),
            outcome,
        });

        Ok((fields, effects))
    }

    fn eval_coalesce(
        &self,
        aggregate_work_id: &InputId,
        source_work_ids: &[InputId],
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        // Guard: all source_work_ids must be Queued
        for wid in source_work_ids {
            let lifecycle = self.fields.lifecycle.get(wid);
            if lifecycle != Some(&InputLifecycleState::Queued) {
                return Err(RuntimeIngressError::GuardFailed {
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

            effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                work_id: wid.clone(),
                new_state: InputLifecycleState::Coalesced,
            });
            effects.push(RuntimeIngressEffect::CompletionResolved {
                work_id: wid.clone(),
                outcome,
            });
        }

        Ok((fields, effects))
    }

    /// Restore a store-recovered input into the ingress authority's tracking.
    /// This does NOT re-run the admission pipeline — the input was already admitted
    /// before the crash. We just need the authority to know about it so that
    /// `Recover` can emit proper per-input recovery effects.
    #[allow(clippy::too_many_arguments)]
    fn eval_admit_recovered(
        &self,
        work_id: InputId,
        content_shape: ContentShape,
        handling_mode: HandlingMode,
        lifecycle_state: InputLifecycleState,
        policy: PolicyDecision,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        let mut fields = self.fields.clone();

        // Restore the input into the authority's canonical tracking at its persisted state.
        fields.content_shape.insert(work_id.clone(), content_shape);
        fields.handling_mode.insert(work_id.clone(), handling_mode);
        fields.lifecycle.insert(work_id.clone(), lifecycle_state);
        fields.policy_snapshot.insert(work_id.clone(), policy);
        fields.request_id.insert(work_id.clone(), request_id);
        fields
            .reservation_key
            .insert(work_id.clone(), reservation_key);

        // Re-enqueue if the lifecycle state is Queued (so Recover finds it in the queue).
        if lifecycle_state == InputLifecycleState::Queued {
            match handling_mode {
                HandlingMode::Steer => {
                    if !fields.steer_queue.contains(&work_id) {
                        fields.steer_queue.push(work_id);
                    }
                }
                HandlingMode::Queue => {
                    if !fields.queue.contains(&work_id) {
                        fields.queue.push(work_id);
                    }
                }
            }
        }

        Ok((fields, vec![]))
    }

    fn eval_reconcile_terminal_inputs(
        &self,
        terminal_inputs: &[(InputId, InputTerminalOutcome)],
    ) -> Result<(RuntimeIngressFields, Vec<RuntimeIngressEffect>), RuntimeIngressError> {
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        for (work_id, outcome) in terminal_inputs {
            if !fields.lifecycle.contains_key(work_id) {
                continue;
            }

            fields.queue.retain(|id| id != work_id);
            fields.steer_queue.retain(|id| id != work_id);

            let terminal_state = match outcome {
                InputTerminalOutcome::Consumed => InputLifecycleState::Consumed,
                InputTerminalOutcome::Superseded { .. } => InputLifecycleState::Superseded,
                InputTerminalOutcome::Coalesced { .. } => InputLifecycleState::Coalesced,
                InputTerminalOutcome::Abandoned { .. } => InputLifecycleState::Abandoned,
            };

            fields.lifecycle.insert(work_id.clone(), terminal_state);

            effects.push(RuntimeIngressEffect::InputLifecycleNotice {
                work_id: work_id.clone(),
                new_state: terminal_state,
            });
            effects.push(RuntimeIngressEffect::CompletionResolved {
                work_id: work_id.clone(),
                outcome: outcome.clone(),
            });
        }

        Ok((fields, effects))
    }
}

impl RuntimeIngressMutator for RuntimeIngressAuthority {
    fn apply(
        &mut self,
        input: RuntimeIngressInput,
    ) -> Result<RuntimeIngressTransition, RuntimeIngressError> {
        let (next_fields, effects) = self.evaluate(&input)?;

        self.fields = next_fields;

        Ok(RuntimeIngressTransition { effects })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Remove all items in `values` from `seq`.
fn seq_remove_all(seq: &mut Vec<InputId>, values: &[InputId]) {
    seq.retain(|id| !values.contains(id));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::redundant_clone,
    clippy::cloned_ref_to_slice_refs
)]
mod tests {
    use super::*;
    use crate::identifiers::PolicyVersion;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
    };

    fn test_policy() -> PolicyDecision {
        PolicyDecision {
            apply_mode: ApplyMode::StageRunStart,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
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
        admit_queued_with_options(auth, work_id, mode, false)
    }

    fn admit_queued_with_options(
        auth: &mut RuntimeIngressAuthority,
        work_id: InputId,
        mode: HandlingMode,
        is_prompt: bool,
    ) -> RuntimeIngressTransition {
        auth.apply(RuntimeIngressInput::AdmitQueued {
            work_id,
            content_shape: ContentShape("text".into()),
            handling_mode: mode,
            is_prompt,
            request_id: None,
            reservation_key: None,
            policy: test_policy(),
            persist_and_queue: true,
            queue_action: AdmissionQueueAction::EnqueueTo { target: mode },
            existing_action: None,
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

    fn current_run_contributors_for_test(auth: &RuntimeIngressAuthority) -> Vec<InputId> {
        auth.admission_order()
            .iter()
            .filter(|work_id| {
                matches!(
                    auth.lifecycle_state(work_id),
                    Some(
                        InputLifecycleState::Staged
                            | InputLifecycleState::Applied
                            | InputLifecycleState::AppliedPendingConsumption
                    )
                )
            })
            .cloned()
            .collect()
    }

    fn has_current_run_for_test(auth: &RuntimeIngressAuthority) -> bool {
        !current_run_contributors_for_test(auth).is_empty()
    }

    // ---- Queue/ledger helper behavior ----

    #[test]
    fn new_starts_with_empty_queues() {
        let auth = RuntimeIngressAuthority::new();
        assert!(auth.queue().is_empty());
        assert!(auth.steer_queue().is_empty());
        assert!(!has_current_run_for_test(&auth));
    }

    // ---- AdmitQueued ----

    #[test]
    fn admit_queued_registers_input() {
        let mut auth = RuntimeIngressAuthority::new();
        let wid = InputId::new();
        let t = admit_queued(&mut auth, wid.clone(), HandlingMode::Queue);

        assert!(auth.lifecycle_state(&wid).is_some());
        assert_eq!(auth.queue(), &[wid.clone()]);
        assert!(auth.steer_queue().is_empty());
        assert_eq!(
            auth.lifecycle_state(&wid),
            Some(InputLifecycleState::Queued)
        );
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::IngressAccepted { .. }))
        );
        assert!(
            !t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::WakeRuntime))
        );
    }

    #[test]
    fn admit_queued_steer_mode() {
        let mut auth = RuntimeIngressAuthority::new();
        let wid = InputId::new();
        let t = admit_queued(&mut auth, wid.clone(), HandlingMode::Steer);

        assert!(auth.queue().is_empty());
        assert_eq!(auth.steer_queue(), &[wid.clone()]);
        assert!(
            !t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::RequestImmediateProcessing))
        );
    }

    #[test]
    fn admit_queued_steer_lane_can_remain_passive() {
        let mut auth = RuntimeIngressAuthority::new();
        let wid = InputId::new();
        let t = auth
            .apply(RuntimeIngressInput::AdmitQueued {
                work_id: wid.clone(),
                content_shape: ContentShape("text".into()),
                handling_mode: HandlingMode::Steer,
                is_prompt: false,
                request_id: None,
                reservation_key: None,
                policy: PolicyDecision {
                    apply_mode: ApplyMode::StageRunBoundary,
                    wake_mode: WakeMode::None,
                    queue_mode: QueueMode::Coalesce,
                    consume_point: ConsumePoint::OnRunComplete,
                    drain_policy: DrainPolicy::SteerBatch,
                    routing_disposition: RoutingDisposition::Steer,
                    record_transcript: true,
                    emit_operator_content: true,
                    policy_version: PolicyVersion(1),
                },
                persist_and_queue: true,
                queue_action: AdmissionQueueAction::EnqueueTo {
                    target: HandlingMode::Steer,
                },
                existing_action: None,
            })
            .expect("admit should succeed");

        assert!(auth.queue().is_empty());
        assert_eq!(auth.steer_queue(), &[wid]);
        assert!(
            !t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::RequestImmediateProcessing))
        );
        assert!(
            !t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::WakeRuntime))
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
            is_prompt: false,
            request_id: None,
            reservation_key: None,
            policy: test_policy(),
            persist_and_queue: true,
            queue_action: AdmissionQueueAction::EnqueueTo {
                target: HandlingMode::Queue,
            },
            existing_action: None,
        });
        assert!(matches!(
            result,
            Err(RuntimeIngressError::GuardFailed { .. })
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
        assert!(has_current_run_for_test(&auth));
        assert_eq!(
            current_run_contributors_for_test(&auth),
            vec![w1.clone(), w2.clone()]
        );
        assert_eq!(auth.lifecycle_state(&w1), Some(InputLifecycleState::Staged));
        assert_eq!(auth.lifecycle_state(&w2), Some(InputLifecycleState::Staged));
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::ReadyForRun { .. }))
        );
    }

    #[test]
    fn stage_drain_helper_no_longer_owns_active_run_exclusion() {
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

        // The helper no longer owns the "one active run at a time" rule; the
        // top-level Meerkat machine owns it. A second standalone stage call is
        // therefore legal here as pure queue bookkeeping.
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w2],
        })
        .expect("standalone helper stage should succeed without control-phase context");
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
                contributing_work_ids: vec![w1.clone()],
                boundary_sequence: 42,
            })
            .expect("boundary applied should succeed");

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::AppliedPendingConsumption)
        );
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, RuntimeIngressEffect::IngressNotice { .. }))
        );
    }

    #[test]
    fn boundary_applied_without_staged_contributors_rejected() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        let result = auth.apply(RuntimeIngressInput::BoundaryApplied {
            contributing_work_ids: vec![w1.clone()],
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
            contributing_work_ids: vec![w1.clone()],
            boundary_sequence: 1,
        })
        .unwrap();

        let t = auth
            .apply(RuntimeIngressInput::RunCompleted {
                contributing_work_ids: vec![w1.clone()],
            })
            .expect("run completed should succeed");

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::Consumed)
        );
        assert!(!has_current_run_for_test(&auth));
        assert!(current_run_contributors_for_test(&auth).is_empty());
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

        let transition = auth
            .apply(RuntimeIngressInput::ReplayQueuedContributors {
                queue_work_ids: vec![w1.clone()],
                steer_work_ids: Vec::new(),
                wake_runtime: true,
                notice_kind: "RunFailed".into(),
            })
            .expect("run failed should succeed");

        assert_eq!(auth.lifecycle_state(&w1), Some(InputLifecycleState::Queued));
        assert!(auth.queue().contains(&w1));
        assert!(!has_current_run_for_test(&auth));
        assert!(
            transition
                .effects
                .iter()
                .any(|effect| matches!(effect, RuntimeIngressEffect::WakeRuntime))
        );
    }

    #[test]
    fn reconcile_terminal_inputs_prunes_requeued_terminal_work() {
        let mut auth = RuntimeIngressAuthority::new();
        let w1 = InputId::new();
        admit_queued(&mut auth, w1.clone(), HandlingMode::Queue);

        let run_id = RunId::new();
        auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: run_id.clone(),
            contributing_work_ids: vec![w1.clone()],
        })
        .unwrap();
        auth.apply(RuntimeIngressInput::ReplayQueuedContributors {
            queue_work_ids: vec![w1.clone()],
            steer_work_ids: Vec::new(),
            wake_runtime: true,
            notice_kind: "RunFailed".into(),
        })
        .unwrap();

        let transition = auth
            .apply(RuntimeIngressInput::ReconcileTerminalInputs {
                terminal_inputs: vec![(
                    w1.clone(),
                    InputTerminalOutcome::Abandoned {
                        reason: crate::input_state::InputAbandonReason::MaxAttemptsExhausted {
                            attempts: 3,
                        },
                    },
                )],
            })
            .expect("reconcile terminal inputs should succeed");

        assert_eq!(
            auth.lifecycle_state(&w1),
            Some(InputLifecycleState::Abandoned)
        );
        assert!(!auth.queue().contains(&w1));
        assert!(
            !transition
                .effects
                .iter()
                .any(|effect| matches!(effect, RuntimeIngressEffect::WakeRuntime))
        );
        assert!(transition.effects.iter().any(|effect| matches!(
            effect,
            RuntimeIngressEffect::CompletionResolved {
                work_id,
                outcome: InputTerminalOutcome::Abandoned { .. },
            } if work_id == &w1
        )));
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

        auth.apply(RuntimeIngressInput::ReplayQueuedContributors {
            queue_work_ids: vec![w1.clone()],
            steer_work_ids: Vec::new(),
            wake_runtime: true,
            notice_kind: "RunCancelled".into(),
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
            contributing_work_ids: vec![w1.clone(), w2.clone()],
            boundary_sequence: 1,
        })
        .unwrap();

        // Run completed
        auth.apply(RuntimeIngressInput::RunCompleted {
            contributing_work_ids: vec![w1.clone(), w2.clone()],
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
        assert!(!has_current_run_for_test(&auth));
        assert!(auth.queue().is_empty());
    }

    // ---- can_accept probing ----

    #[test]
    fn can_accept_probes_without_mutation() {
        let auth = RuntimeIngressAuthority::new();
        assert!(
            auth.can_accept(&RuntimeIngressInput::AdmitConsumedOnAccept {
                work_id: InputId::new(),
                content_shape: ContentShape("text".into()),
                request_id: None,
                reservation_key: None,
                policy: test_policy(),
            })
        );
        assert!(!has_current_run_for_test(&auth));
    }

    #[test]
    fn can_accept_stage_rejects_without_matching_queue_prefix() {
        let auth = RuntimeIngressAuthority::new();
        assert!(!auth.can_accept(&RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![InputId::new()],
        }));
    }

    // ---- State unchanged on failure ----

    #[test]
    fn helper_state_unchanged_on_rejected_prefix_mismatch() {
        let mut auth = RuntimeIngressAuthority::new();
        let w_queue = InputId::new();
        let w_steer = InputId::new();
        admit_queued(&mut auth, w_queue.clone(), HandlingMode::Queue);
        admit_queued(&mut auth, w_steer.clone(), HandlingMode::Steer);

        let result = auth.apply(RuntimeIngressInput::StageDrainSnapshot {
            run_id: RunId::new(),
            contributing_work_ids: vec![w_queue.clone()],
        });
        assert!(result.is_err());
        assert_eq!(auth.queue(), &[w_queue]);
        assert_eq!(auth.steer_queue(), &[w_steer]);
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

        auth.apply(RuntimeIngressInput::ReplayQueuedContributors {
            queue_work_ids: Vec::new(),
            steer_work_ids: vec![w1.clone()],
            wake_runtime: true,
            notice_kind: "RunFailed".into(),
        })
        .unwrap();

        // Should be back in steer_queue, not queue
        assert!(auth.steer_queue().contains(&w1));
        assert!(!auth.queue().contains(&w1));
    }
}

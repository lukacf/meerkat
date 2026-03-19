//! Machine authority for the TurnExecution state machine.
//!
//! This module provides the canonical authority for turn-execution state,
//! following the RMAT pattern. All state mutations flow through
//! [`TurnExecutionAuthority::apply`]; handwritten shell code in the agent loop
//! classifies events into typed inputs, calls `apply`, and executes returned
//! effects.
//!
//! The transition table encoded here is the single source of truth, matching
//! the machine schema in `meerkat-machine-schema/src/catalog/turn_execution.rs`:
//!
//! - 11 states: Ready, ApplyingPrimitive, CallingLlm, WaitingForOps,
//!   DrainingBoundary, Extracting, ErrorRecovery, Cancelling, Completed, Failed, Cancelled
//! - 25 inputs: StartConversationRun, StartImmediateAppend, StartImmediateContext,
//!   PrimitiveApplied, LlmReturnedToolCalls, LlmReturnedTerminal,
//!   ToolCallsResolved, BoundaryContinue, BoundaryComplete, EnterExtraction,
//!   ExtractionValidationPassed, ExtractionRetry, ExtractionExhausted,
//!   RecoverableFailure, FatalFailure, RetryRequested, CancelNow,
//!   CancelAfterBoundary, CancellationObserved, AcknowledgeTerminal,
//!   TurnLimitReached, BudgetExhausted, ForceCancelNoRun, RunCompleted,
//!   RunFailed, RunCancelled
//! - 11 fields: active_run, primitive_kind, admitted_content_shape,
//!   vision_enabled, image_tool_results_enabled, tool_calls_pending,
//!   boundary_count, cancel_after_boundary, terminal_outcome,
//!   extraction_attempts, max_extraction_retries
//! - 5 effects: RunStarted, BoundaryApplied, RunCompleted, RunFailed,
//!   RunCancelled

use crate::error::AgentError;
use crate::lifecycle::RunId;

// ---------------------------------------------------------------------------
// Phase enum — mirrors the machine schema's state variants
// ---------------------------------------------------------------------------

/// Canonical phases for the TurnExecution machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPhase {
    Ready,
    ApplyingPrimitive,
    CallingLlm,
    WaitingForOps,
    DrainingBoundary,
    Extracting,
    ErrorRecovery,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

impl TurnPhase {
    /// Whether this phase is terminal (no further transitions except
    /// AcknowledgeTerminal back to Ready).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Whether this phase is the extraction validation phase.
    pub fn is_extracting(self) -> bool {
        matches!(self, Self::Extracting)
    }
}

impl std::fmt::Display for TurnPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Ready => "Ready",
            Self::ApplyingPrimitive => "ApplyingPrimitive",
            Self::CallingLlm => "CallingLlm",
            Self::WaitingForOps => "WaitingForOps",
            Self::DrainingBoundary => "DrainingBoundary",
            Self::Extracting => "Extracting",
            Self::ErrorRecovery => "ErrorRecovery",
            Self::Cancelling => "Cancelling",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        };
        f.write_str(s)
    }
}

// ---------------------------------------------------------------------------
// Primitive kind — typed enum for the primitive_kind field
// ---------------------------------------------------------------------------

/// What kind of run primitive is in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPrimitiveKind {
    /// No active primitive (Ready state).
    None,
    /// Standard conversation turn with LLM calls.
    ConversationTurn,
    /// Immediate append — skip LLM, apply content directly.
    ImmediateAppend,
    /// Immediate context append — skip LLM, append system context.
    ImmediateContextAppend,
}

// ---------------------------------------------------------------------------
// Terminal outcome — typed enum for the terminal_outcome field
// ---------------------------------------------------------------------------

/// Terminal outcome of a turn execution run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnTerminalOutcome {
    /// No terminal outcome yet.
    None,
    Completed,
    Failed,
    Cancelled,
    BudgetExhausted,
}

// ---------------------------------------------------------------------------
// Content shape — opaque type for admitted_content_shape
// ---------------------------------------------------------------------------

/// Content shape admitted by the primitive. Opaque to the authority;
/// shell code interprets its meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentShape(pub String);

// ---------------------------------------------------------------------------
// Typed input enum — mirrors the machine schema's input variants
// ---------------------------------------------------------------------------

/// Typed inputs for the TurnExecution machine.
///
/// Shell code classifies raw events into these typed inputs, then calls
/// [`TurnExecutionAuthority::apply`]. The authority decides transition legality.
#[derive(Debug, Clone)]
pub enum TurnExecutionInput {
    StartConversationRun {
        run_id: RunId,
    },
    StartImmediateAppend {
        run_id: RunId,
    },
    StartImmediateContext {
        run_id: RunId,
    },
    PrimitiveApplied {
        run_id: RunId,
        admitted_content_shape: ContentShape,
        vision_enabled: bool,
        image_tool_results_enabled: bool,
    },
    LlmReturnedToolCalls {
        run_id: RunId,
        tool_count: u32,
    },
    LlmReturnedTerminal {
        run_id: RunId,
    },
    ToolCallsResolved {
        run_id: RunId,
    },
    BoundaryContinue {
        run_id: RunId,
    },
    BoundaryComplete {
        run_id: RunId,
    },
    RecoverableFailure {
        run_id: RunId,
    },
    FatalFailure {
        run_id: RunId,
    },
    RetryRequested {
        run_id: RunId,
    },
    CancelNow {
        run_id: RunId,
    },
    CancelAfterBoundary {
        run_id: RunId,
    },
    CancellationObserved {
        run_id: RunId,
    },
    AcknowledgeTerminal {
        run_id: RunId,
    },
    TurnLimitReached {
        run_id: RunId,
    },
    BudgetExhausted {
        run_id: RunId,
    },
    /// Enter extraction phase after the conversation turn completes.
    ///
    /// Fired by the shell when BoundaryComplete detects that extraction is
    /// needed (output_schema is set and extraction_mode is not yet active).
    /// Transitions DrainingBoundary -> Extracting and initializes extraction
    /// tracking fields.
    EnterExtraction {
        run_id: RunId,
        max_retries: u32,
    },
    /// Extraction validation passed — extraction is complete.
    ExtractionValidationPassed {
        run_id: RunId,
    },
    /// Extraction validation failed but retries remain — retry the extraction.
    ExtractionRetry {
        run_id: RunId,
    },
    /// Extraction retries exhausted — complete with failure.
    ExtractionExhausted {
        run_id: RunId,
    },
    /// Force-cancel when no active run exists.
    ///
    /// Used by `cancel()` when the machine is in a non-terminal phase but has
    /// no active run (e.g. Ready, or an edge case where the run was already
    /// acknowledged). Transitions directly to Cancelled without requiring a
    /// run_id guard.
    ForceCancelNoRun,
}

// ---------------------------------------------------------------------------
// Typed effect enum — mirrors the machine schema's effect variants
// ---------------------------------------------------------------------------

/// Effects emitted by TurnExecution transitions.
///
/// Shell code receives these from [`TurnExecutionAuthority::apply`] and is
/// responsible for executing the side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnExecutionEffect {
    RunStarted {
        run_id: RunId,
    },
    BoundaryApplied {
        run_id: RunId,
        boundary_sequence: u64,
    },
    RunCompleted {
        run_id: RunId,
    },
    RunFailed {
        run_id: RunId,
    },
    RunCancelled {
        run_id: RunId,
    },
    /// Drain the comms inbox before the next LLM call.
    ///
    /// Emitted on transitions that re-enter CallingLlm (BoundaryContinue,
    /// RetryRequested, ExtractionRetry). Shell executes the drain instead
    /// of checking `state == CallingLlm`.
    DrainCommsInbox,
    /// Check whether compaction should run before the next LLM call.
    ///
    /// Emitted on the same transitions as DrainCommsInbox.
    CheckCompaction,
}

// ---------------------------------------------------------------------------
// Transition result
// ---------------------------------------------------------------------------

/// Successful transition outcome from the TurnExecution authority.
#[derive(Debug)]
pub struct TurnExecutionTransition {
    /// The phase before the transition.
    pub prev_phase: TurnPhase,
    /// The phase after the transition.
    pub next_phase: TurnPhase,
    /// Effects to be executed by shell code.
    pub effects: Vec<TurnExecutionEffect>,
}

// ---------------------------------------------------------------------------
// Canonical machine state (fields)
// ---------------------------------------------------------------------------

/// Canonical machine-owned fields for TurnExecution.
#[derive(Debug, Clone)]
struct TurnExecutionFields {
    active_run: Option<RunId>,
    primitive_kind: TurnPrimitiveKind,
    admitted_content_shape: Option<ContentShape>,
    vision_enabled: bool,
    image_tool_results_enabled: bool,
    tool_calls_pending: u32,
    boundary_count: u32,
    cancel_after_boundary: bool,
    terminal_outcome: TurnTerminalOutcome,
    extraction_attempts: u32,
    max_extraction_retries: u32,
}

impl TurnExecutionFields {
    fn init() -> Self {
        Self {
            active_run: None,
            primitive_kind: TurnPrimitiveKind::None,
            admitted_content_shape: None,
            vision_enabled: false,
            image_tool_results_enabled: false,
            tool_calls_pending: 0,
            boundary_count: 0,
            cancel_after_boundary: false,
            terminal_outcome: TurnTerminalOutcome::None,
            extraction_attempts: 0,
            max_extraction_retries: 0,
        }
    }

    /// Reset all fields to initial values (used by AcknowledgeTerminal).
    fn reset(&mut self) {
        *self = Self::init();
    }
}

// ---------------------------------------------------------------------------
// Sealed mutator trait — only the authority implements this
// ---------------------------------------------------------------------------

mod sealed {
    pub trait Sealed {}
}

/// Sealed trait for TurnExecution state mutation.
///
/// Only [`TurnExecutionAuthority`] implements this. Handwritten code cannot
/// create alternative implementations, ensuring single-source-of-truth
/// semantics for turn-execution state.
pub trait TurnExecutionMutator: sealed::Sealed {
    /// Apply a typed input to the current machine state.
    ///
    /// Returns the transition result including next state and effects,
    /// or an error if the transition is not legal from the current state.
    fn apply(&mut self, input: TurnExecutionInput) -> Result<TurnExecutionTransition, AgentError>;
}

// ---------------------------------------------------------------------------
// Authority implementation
// ---------------------------------------------------------------------------

/// The canonical authority for TurnExecution state.
///
/// Holds the canonical phase + fields and delegates all transitions through
/// the encoded transition table.
#[derive(Debug, Clone)]
pub struct TurnExecutionAuthority {
    /// Canonical phase.
    phase: TurnPhase,
    /// Canonical machine-owned fields.
    fields: TurnExecutionFields,
}

impl Default for TurnExecutionAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl sealed::Sealed for TurnExecutionAuthority {}

impl TurnExecutionAuthority {
    /// Create a new authority in the Ready state with default fields.
    pub fn new() -> Self {
        Self {
            phase: TurnPhase::Ready,
            fields: TurnExecutionFields::init(),
        }
    }

    /// Current phase (read from canonical state).
    pub fn phase(&self) -> TurnPhase {
        self.phase
    }

    /// Current active run ID.
    pub fn active_run(&self) -> Option<&RunId> {
        self.fields.active_run.as_ref()
    }

    /// Current primitive kind.
    pub fn primitive_kind(&self) -> TurnPrimitiveKind {
        self.fields.primitive_kind
    }

    /// Current boundary count.
    pub fn boundary_count(&self) -> u32 {
        self.fields.boundary_count
    }

    /// Whether cancel-after-boundary is requested.
    pub fn cancel_after_boundary(&self) -> bool {
        self.fields.cancel_after_boundary
    }

    /// Current terminal outcome.
    pub fn terminal_outcome(&self) -> TurnTerminalOutcome {
        self.fields.terminal_outcome
    }

    /// Current tool calls pending count.
    pub fn tool_calls_pending(&self) -> u32 {
        self.fields.tool_calls_pending
    }

    /// Whether vision is enabled for the current run.
    pub fn vision_enabled(&self) -> bool {
        self.fields.vision_enabled
    }

    /// Whether image tool results are enabled for the current run.
    pub fn image_tool_results_enabled(&self) -> bool {
        self.fields.image_tool_results_enabled
    }

    /// Current admitted content shape.
    pub fn admitted_content_shape(&self) -> Option<&ContentShape> {
        self.fields.admitted_content_shape.as_ref()
    }

    /// Current extraction attempt count.
    pub fn extraction_attempts(&self) -> u32 {
        self.fields.extraction_attempts
    }

    /// Maximum extraction retries configured for the current run.
    pub fn max_extraction_retries(&self) -> u32 {
        self.fields.max_extraction_retries
    }

    /// Check if a transition is legal without applying it.
    pub fn can_accept(&self, input: &TurnExecutionInput) -> bool {
        self.evaluate(input).is_ok()
    }

    /// Helper: build an InvalidStateTransition error.
    fn invalid(from: TurnPhase, input: &TurnExecutionInput) -> AgentError {
        AgentError::InvalidStateTransition {
            from: from.to_string(),
            to: format!("{input:?}"),
        }
    }

    /// Helper: check run_matches_active guard.
    fn guard_run_matches(&self, run_id: &RunId) -> bool {
        self.fields.active_run.as_ref() == Some(run_id)
    }

    /// Evaluate a transition without committing it.
    fn evaluate(
        &self,
        input: &TurnExecutionInput,
    ) -> Result<(TurnPhase, TurnExecutionFields, Vec<TurnExecutionEffect>), AgentError> {
        use TurnExecutionInput::{
            AcknowledgeTerminal, BoundaryComplete, BoundaryContinue, BudgetExhausted,
            CancelAfterBoundary, CancelNow, CancellationObserved, EnterExtraction,
            ExtractionExhausted, ExtractionRetry, ExtractionValidationPassed, FatalFailure,
            ForceCancelNoRun, LlmReturnedTerminal, LlmReturnedToolCalls, PrimitiveApplied,
            RecoverableFailure, RetryRequested, StartConversationRun, StartImmediateAppend,
            StartImmediateContext, ToolCallsResolved, TurnLimitReached,
        };
        use TurnPhase::{
            ApplyingPrimitive, CallingLlm, Cancelled, Cancelling, Completed, DrainingBoundary,
            ErrorRecovery, Extracting, Failed, Ready, WaitingForOps,
        };

        let phase = self.phase;
        let mut fields = self.fields.clone();
        let mut effects = Vec::new();

        let next_phase = match (phase, input) {
            // ---------------------------------------------------------------
            // Start* transitions: Ready -> ApplyingPrimitive
            // ---------------------------------------------------------------
            (Ready, StartConversationRun { run_id }) => {
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ConversationTurn;
                fields.tool_calls_pending = 0;
                fields.admitted_content_shape = None;
                fields.vision_enabled = false;
                fields.image_tool_results_enabled = false;
                fields.boundary_count = 0;
                fields.cancel_after_boundary = false;
                fields.terminal_outcome = TurnTerminalOutcome::None;
                effects.push(TurnExecutionEffect::RunStarted {
                    run_id: run_id.clone(),
                });
                ApplyingPrimitive
            }

            (Ready, StartImmediateAppend { run_id }) => {
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ImmediateAppend;
                fields.tool_calls_pending = 0;
                fields.admitted_content_shape = None;
                fields.vision_enabled = false;
                fields.image_tool_results_enabled = false;
                fields.boundary_count = 0;
                fields.cancel_after_boundary = false;
                fields.terminal_outcome = TurnTerminalOutcome::None;
                effects.push(TurnExecutionEffect::RunStarted {
                    run_id: run_id.clone(),
                });
                ApplyingPrimitive
            }

            (Ready, StartImmediateContext { run_id }) => {
                fields.active_run = Some(run_id.clone());
                fields.primitive_kind = TurnPrimitiveKind::ImmediateContextAppend;
                fields.tool_calls_pending = 0;
                fields.admitted_content_shape = None;
                fields.vision_enabled = false;
                fields.image_tool_results_enabled = false;
                fields.boundary_count = 0;
                fields.cancel_after_boundary = false;
                fields.terminal_outcome = TurnTerminalOutcome::None;
                effects.push(TurnExecutionEffect::RunStarted {
                    run_id: run_id.clone(),
                });
                ApplyingPrimitive
            }

            // ---------------------------------------------------------------
            // PrimitiveApplied: ApplyingPrimitive -> CallingLlm | Completed | Cancelled
            // ---------------------------------------------------------------
            (
                ApplyingPrimitive,
                PrimitiveApplied {
                    run_id,
                    admitted_content_shape,
                    vision_enabled,
                    image_tool_results_enabled,
                },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }

                // Apply common field updates
                fields.admitted_content_shape = Some(admitted_content_shape.clone());
                fields.vision_enabled = *vision_enabled;
                fields.image_tool_results_enabled = *image_tool_results_enabled;

                match fields.primitive_kind {
                    TurnPrimitiveKind::ConversationTurn => {
                        // ConversationTurn -> CallingLlm (no boundary/cancel checks)
                        effects.push(TurnExecutionEffect::DrainCommsInbox);
                        effects.push(TurnExecutionEffect::CheckCompaction);
                        CallingLlm
                    }
                    TurnPrimitiveKind::ImmediateAppend => {
                        fields.boundary_count += 1;
                        let boundary_seq = u64::from(fields.boundary_count);
                        effects.push(TurnExecutionEffect::BoundaryApplied {
                            run_id: run_id.clone(),
                            boundary_sequence: boundary_seq,
                        });

                        if fields.cancel_after_boundary {
                            fields.cancel_after_boundary = false;
                            fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                            effects.push(TurnExecutionEffect::RunCancelled {
                                run_id: run_id.clone(),
                            });
                            Cancelled
                        } else {
                            fields.terminal_outcome = TurnTerminalOutcome::Completed;
                            effects.push(TurnExecutionEffect::RunCompleted {
                                run_id: run_id.clone(),
                            });
                            Completed
                        }
                    }
                    TurnPrimitiveKind::ImmediateContextAppend => {
                        fields.boundary_count += 1;
                        let boundary_seq = u64::from(fields.boundary_count);
                        effects.push(TurnExecutionEffect::BoundaryApplied {
                            run_id: run_id.clone(),
                            boundary_sequence: boundary_seq,
                        });

                        if fields.cancel_after_boundary {
                            fields.cancel_after_boundary = false;
                            fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                            effects.push(TurnExecutionEffect::RunCancelled {
                                run_id: run_id.clone(),
                            });
                            Cancelled
                        } else {
                            fields.terminal_outcome = TurnTerminalOutcome::Completed;
                            effects.push(TurnExecutionEffect::RunCompleted {
                                run_id: run_id.clone(),
                            });
                            Completed
                        }
                    }
                    TurnPrimitiveKind::None => {
                        return Err(Self::invalid(phase, input));
                    }
                }
            }

            // ---------------------------------------------------------------
            // LlmReturnedToolCalls: CallingLlm -> WaitingForOps
            // ---------------------------------------------------------------
            (CallingLlm, LlmReturnedToolCalls { run_id, tool_count }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if *tool_count == 0 {
                    return Err(Self::invalid(phase, input));
                }
                fields.tool_calls_pending = *tool_count;
                WaitingForOps
            }

            // ---------------------------------------------------------------
            // LlmReturnedTerminal: CallingLlm -> DrainingBoundary
            // ---------------------------------------------------------------
            (CallingLlm, LlmReturnedTerminal { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.boundary_count += 1;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                DrainingBoundary
            }

            // ---------------------------------------------------------------
            // ToolCallsResolved: WaitingForOps -> DrainingBoundary
            // ---------------------------------------------------------------
            (WaitingForOps, ToolCallsResolved { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if fields.tool_calls_pending == 0 {
                    return Err(Self::invalid(phase, input));
                }
                fields.tool_calls_pending = 0;
                fields.boundary_count += 1;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                DrainingBoundary
            }

            // ---------------------------------------------------------------
            // BoundaryContinue: DrainingBoundary -> CallingLlm | Cancelled
            // ---------------------------------------------------------------
            (DrainingBoundary, BoundaryContinue { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                // Only ConversationTurn can continue
                if fields.primitive_kind != TurnPrimitiveKind::ConversationTurn {
                    return Err(Self::invalid(phase, input));
                }
                if fields.cancel_after_boundary {
                    fields.cancel_after_boundary = false;
                    fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                    effects.push(TurnExecutionEffect::RunCancelled {
                        run_id: run_id.clone(),
                    });
                    Cancelled
                } else {
                    effects.push(TurnExecutionEffect::DrainCommsInbox);
                    effects.push(TurnExecutionEffect::CheckCompaction);
                    CallingLlm
                }
            }

            // ---------------------------------------------------------------
            // BoundaryComplete: DrainingBoundary -> Completed | Cancelled
            // ---------------------------------------------------------------
            (DrainingBoundary, BoundaryComplete { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                if fields.cancel_after_boundary {
                    fields.cancel_after_boundary = false;
                    fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                    effects.push(TurnExecutionEffect::RunCancelled {
                        run_id: run_id.clone(),
                    });
                    Cancelled
                } else {
                    fields.terminal_outcome = TurnTerminalOutcome::Completed;
                    effects.push(TurnExecutionEffect::RunCompleted {
                        run_id: run_id.clone(),
                    });
                    Completed
                }
            }

            // ---------------------------------------------------------------
            // EnterExtraction: DrainingBoundary -> Extracting
            //
            // Called each time the shell reaches DrainingBoundary during an
            // extraction run. On the first call extraction_attempts is 0
            // (from run start); ExtractionRetry increments it on each retry.
            // ---------------------------------------------------------------
            (
                DrainingBoundary,
                EnterExtraction {
                    run_id,
                    max_retries,
                },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.max_extraction_retries = *max_retries;
                Extracting
            }

            // ---------------------------------------------------------------
            // ExtractionValidationPassed: Extracting -> Completed
            // ---------------------------------------------------------------
            (Extracting, ExtractionValidationPassed { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Completed;
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }

            // ---------------------------------------------------------------
            // ExtractionRetry: Extracting -> CallingLlm
            // ---------------------------------------------------------------
            (Extracting, ExtractionRetry { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.extraction_attempts += 1;
                effects.push(TurnExecutionEffect::DrainCommsInbox);
                effects.push(TurnExecutionEffect::CheckCompaction);
                CallingLlm
            }

            // ---------------------------------------------------------------
            // ExtractionExhausted: Extracting -> Completed
            // ---------------------------------------------------------------
            (Extracting, ExtractionExhausted { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Completed;
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }

            // ---------------------------------------------------------------
            // RecoverableFailure: CallingLlm|WaitingForOps|DrainingBoundary -> ErrorRecovery
            // ---------------------------------------------------------------
            (CallingLlm | WaitingForOps | DrainingBoundary, RecoverableFailure { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                ErrorRecovery
            }

            // ---------------------------------------------------------------
            // RetryRequested: ErrorRecovery -> CallingLlm
            // ---------------------------------------------------------------
            (ErrorRecovery, RetryRequested { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                effects.push(TurnExecutionEffect::DrainCommsInbox);
                effects.push(TurnExecutionEffect::CheckCompaction);
                CallingLlm
            }

            // ---------------------------------------------------------------
            // FatalFailure: ApplyingPrimitive|CallingLlm|WaitingForOps|
            //               DrainingBoundary|Extracting|ErrorRecovery -> Failed
            // ---------------------------------------------------------------
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                FatalFailure { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Failed;
                effects.push(TurnExecutionEffect::RunFailed {
                    run_id: run_id.clone(),
                });
                Failed
            }

            // ---------------------------------------------------------------
            // CancelNow: ApplyingPrimitive|CallingLlm|WaitingForOps|
            //            DrainingBoundary|Extracting|ErrorRecovery -> Cancelling
            // ---------------------------------------------------------------
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                CancelNow { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                Cancelling
            }

            // ---------------------------------------------------------------
            // CancelAfterBoundary: ApplyingPrimitive|CallingLlm|WaitingForOps|
            //                      DrainingBoundary|Extracting|ErrorRecovery -> same phase
            // ---------------------------------------------------------------
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                CancelAfterBoundary { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.cancel_after_boundary = true;
                // Stay in current phase
                phase
            }

            // ---------------------------------------------------------------
            // CancellationObserved: Cancelling -> Cancelled
            // ---------------------------------------------------------------
            (Cancelling, CancellationObserved { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                fields.cancel_after_boundary = false;
                effects.push(TurnExecutionEffect::RunCancelled {
                    run_id: run_id.clone(),
                });
                Cancelled
            }

            // ---------------------------------------------------------------
            // TurnLimitReached: any active phase -> Completed
            // ---------------------------------------------------------------
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                TurnLimitReached { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::Completed;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }

            // ---------------------------------------------------------------
            // BudgetExhausted: any active phase -> Completed
            // ---------------------------------------------------------------
            (
                ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary | Extracting
                | ErrorRecovery,
                BudgetExhausted { run_id },
            ) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.boundary_count += 1;
                fields.terminal_outcome = TurnTerminalOutcome::BudgetExhausted;
                effects.push(TurnExecutionEffect::BoundaryApplied {
                    run_id: run_id.clone(),
                    boundary_sequence: u64::from(fields.boundary_count),
                });
                effects.push(TurnExecutionEffect::RunCompleted {
                    run_id: run_id.clone(),
                });
                Completed
            }

            // ---------------------------------------------------------------
            // ForceCancelNoRun: any non-terminal phase -> Cancelled
            // (no run_id guard — used when cancel() has no active run)
            // ---------------------------------------------------------------
            (
                Ready | ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary
                | Extracting | ErrorRecovery | Cancelling,
                ForceCancelNoRun,
            ) => {
                fields.terminal_outcome = TurnTerminalOutcome::Cancelled;
                if let Some(ref run_id) = fields.active_run {
                    effects.push(TurnExecutionEffect::RunCancelled {
                        run_id: run_id.clone(),
                    });
                }
                Cancelled
            }

            // ---------------------------------------------------------------
            // AcknowledgeTerminal: Completed|Failed|Cancelled -> Ready
            // ---------------------------------------------------------------
            (Completed | Failed | Cancelled, AcknowledgeTerminal { run_id }) => {
                if !self.guard_run_matches(run_id) {
                    return Err(Self::invalid(phase, input));
                }
                fields.reset();
                Ready
            }

            // ---------------------------------------------------------------
            // All other combinations are illegal
            // ---------------------------------------------------------------
            _ => {
                return Err(Self::invalid(phase, input));
            }
        };

        Ok((next_phase, fields, effects))
    }
}

impl TurnExecutionMutator for TurnExecutionAuthority {
    fn apply(&mut self, input: TurnExecutionInput) -> Result<TurnExecutionTransition, AgentError> {
        let prev_phase = self.phase;
        let (next_phase, next_fields, effects) = self.evaluate(&input)?;

        // Commit: update canonical state.
        self.phase = next_phase;
        self.fields = next_fields;

        Ok(TurnExecutionTransition {
            prev_phase,
            next_phase,
            effects,
        })
    }
}

// ---------------------------------------------------------------------------
// LoopState bridge — convert between authority phase and observable LoopState
// ---------------------------------------------------------------------------

impl TurnPhase {
    /// Convert authority phase to the public observable `LoopState`.
    ///
    /// Some schema phases (Ready, ApplyingPrimitive) don't exist in the
    /// original LoopState; they map to the closest observable equivalent.
    pub fn to_loop_state(self) -> crate::state::LoopState {
        use crate::state::LoopState;
        match self {
            Self::Ready | Self::ApplyingPrimitive | Self::CallingLlm => LoopState::CallingLlm,
            Self::WaitingForOps => LoopState::WaitingForOps,
            Self::DrainingBoundary => LoopState::DrainingEvents,
            Self::Extracting => LoopState::DrainingEvents,
            Self::ErrorRecovery => LoopState::ErrorRecovery,
            Self::Cancelling => LoopState::Cancelling,
            Self::Completed | Self::Failed | Self::Cancelled => LoopState::Completed,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_run_id() -> RunId {
        RunId(Uuid::from_u128(1))
    }

    fn other_run_id() -> RunId {
        RunId(Uuid::from_u128(2))
    }

    fn make_authority() -> TurnExecutionAuthority {
        TurnExecutionAuthority::new()
    }

    /// Helper: start a conversation run and apply primitive to reach CallingLlm.
    fn authority_at_calling_llm() -> TurnExecutionAuthority {
        let mut auth = make_authority();
        auth.apply(TurnExecutionInput::StartConversationRun {
            run_id: test_run_id(),
        })
        .expect("start");
        auth.apply(TurnExecutionInput::PrimitiveApplied {
            run_id: test_run_id(),
            admitted_content_shape: ContentShape("text".into()),
            vision_enabled: false,
            image_tool_results_enabled: false,
        })
        .expect("primitive applied");
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);
        auth
    }

    /// Helper: reach WaitingForOps.
    fn authority_at_waiting_for_ops() -> TurnExecutionAuthority {
        let mut auth = authority_at_calling_llm();
        auth.apply(TurnExecutionInput::LlmReturnedToolCalls {
            run_id: test_run_id(),
            tool_count: 3,
        })
        .expect("tool calls");
        assert_eq!(auth.phase(), TurnPhase::WaitingForOps);
        auth
    }

    /// Helper: reach DrainingBoundary via tool calls resolved.
    fn authority_at_draining_boundary() -> TurnExecutionAuthority {
        let mut auth = authority_at_waiting_for_ops();
        auth.apply(TurnExecutionInput::ToolCallsResolved {
            run_id: test_run_id(),
        })
        .expect("tool calls resolved");
        assert_eq!(auth.phase(), TurnPhase::DrainingBoundary);
        auth
    }

    // === Start transitions ===

    #[test]
    fn start_conversation_run_from_ready() {
        let mut auth = make_authority();
        let t = auth
            .apply(TurnExecutionInput::StartConversationRun {
                run_id: test_run_id(),
            })
            .expect("start conversation run");
        assert_eq!(t.prev_phase, TurnPhase::Ready);
        assert_eq!(t.next_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(auth.phase(), TurnPhase::ApplyingPrimitive);
        assert_eq!(auth.active_run(), Some(&test_run_id()));
        assert_eq!(auth.primitive_kind(), TurnPrimitiveKind::ConversationTurn);
        assert_eq!(t.effects.len(), 1);
        assert!(matches!(
            t.effects[0],
            TurnExecutionEffect::RunStarted { .. }
        ));
    }

    #[test]
    fn start_immediate_append_from_ready() {
        let mut auth = make_authority();
        let t = auth
            .apply(TurnExecutionInput::StartImmediateAppend {
                run_id: test_run_id(),
            })
            .expect("start immediate append");
        assert_eq!(t.next_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(auth.primitive_kind(), TurnPrimitiveKind::ImmediateAppend);
    }

    #[test]
    fn start_immediate_context_from_ready() {
        let mut auth = make_authority();
        let t = auth
            .apply(TurnExecutionInput::StartImmediateContext {
                run_id: test_run_id(),
            })
            .expect("start immediate context");
        assert_eq!(t.next_phase, TurnPhase::ApplyingPrimitive);
        assert_eq!(
            auth.primitive_kind(),
            TurnPrimitiveKind::ImmediateContextAppend
        );
    }

    #[test]
    fn start_from_non_ready_is_rejected() {
        let mut auth = authority_at_calling_llm();
        assert!(
            auth.apply(TurnExecutionInput::StartConversationRun {
                run_id: test_run_id(),
            })
            .is_err()
        );
    }

    // === PrimitiveApplied ===

    #[test]
    fn primitive_applied_conversation_turn_goes_to_calling_llm() {
        let mut auth = make_authority();
        auth.apply(TurnExecutionInput::StartConversationRun {
            run_id: test_run_id(),
        })
        .expect("start");
        let t = auth
            .apply(TurnExecutionInput::PrimitiveApplied {
                run_id: test_run_id(),
                admitted_content_shape: ContentShape("text".into()),
                vision_enabled: true,
                image_tool_results_enabled: true,
            })
            .expect("primitive applied");
        assert_eq!(t.next_phase, TurnPhase::CallingLlm);
        assert!(auth.vision_enabled());
        assert!(auth.image_tool_results_enabled());
        assert_eq!(t.effects.len(), 2);
        assert!(matches!(t.effects[0], TurnExecutionEffect::DrainCommsInbox));
        assert!(matches!(t.effects[1], TurnExecutionEffect::CheckCompaction));
    }

    #[test]
    fn primitive_applied_immediate_append_completes() {
        let mut auth = make_authority();
        auth.apply(TurnExecutionInput::StartImmediateAppend {
            run_id: test_run_id(),
        })
        .expect("start");
        let t = auth
            .apply(TurnExecutionInput::PrimitiveApplied {
                run_id: test_run_id(),
                admitted_content_shape: ContentShape("text".into()),
                vision_enabled: false,
                image_tool_results_enabled: false,
            })
            .expect("primitive applied");
        assert_eq!(t.next_phase, TurnPhase::Completed);
        assert_eq!(auth.boundary_count(), 1);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Completed);
        assert_eq!(t.effects.len(), 2);
        assert!(matches!(
            t.effects[0],
            TurnExecutionEffect::BoundaryApplied { .. }
        ));
        assert!(matches!(
            t.effects[1],
            TurnExecutionEffect::RunCompleted { .. }
        ));
    }

    #[test]
    fn primitive_applied_immediate_append_with_cancel_after_boundary() {
        let mut auth = make_authority();
        auth.apply(TurnExecutionInput::StartImmediateAppend {
            run_id: test_run_id(),
        })
        .expect("start");
        auth.apply(TurnExecutionInput::CancelAfterBoundary {
            run_id: test_run_id(),
        })
        .expect("cancel after boundary");
        let t = auth
            .apply(TurnExecutionInput::PrimitiveApplied {
                run_id: test_run_id(),
                admitted_content_shape: ContentShape("text".into()),
                vision_enabled: false,
                image_tool_results_enabled: false,
            })
            .expect("primitive applied");
        assert_eq!(t.next_phase, TurnPhase::Cancelled);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Cancelled);
        assert!(!auth.cancel_after_boundary());
    }

    #[test]
    fn primitive_applied_wrong_run_id_rejected() {
        let mut auth = make_authority();
        auth.apply(TurnExecutionInput::StartConversationRun {
            run_id: test_run_id(),
        })
        .expect("start");
        assert!(
            auth.apply(TurnExecutionInput::PrimitiveApplied {
                run_id: other_run_id(),
                admitted_content_shape: ContentShape("text".into()),
                vision_enabled: false,
                image_tool_results_enabled: false,
            })
            .is_err()
        );
    }

    // === LLM results ===

    #[test]
    fn llm_returned_tool_calls() {
        let mut auth = authority_at_calling_llm();
        let t = auth
            .apply(TurnExecutionInput::LlmReturnedToolCalls {
                run_id: test_run_id(),
                tool_count: 5,
            })
            .expect("tool calls");
        assert_eq!(t.next_phase, TurnPhase::WaitingForOps);
        assert_eq!(auth.tool_calls_pending(), 5);
    }

    #[test]
    fn llm_returned_tool_calls_zero_count_rejected() {
        let mut auth = authority_at_calling_llm();
        assert!(
            auth.apply(TurnExecutionInput::LlmReturnedToolCalls {
                run_id: test_run_id(),
                tool_count: 0,
            })
            .is_err()
        );
    }

    #[test]
    fn llm_returned_terminal() {
        let mut auth = authority_at_calling_llm();
        let t = auth
            .apply(TurnExecutionInput::LlmReturnedTerminal {
                run_id: test_run_id(),
            })
            .expect("terminal");
        assert_eq!(t.next_phase, TurnPhase::DrainingBoundary);
        assert_eq!(auth.boundary_count(), 1);
        assert_eq!(t.effects.len(), 1);
        assert!(matches!(
            t.effects[0],
            TurnExecutionEffect::BoundaryApplied { .. }
        ));
    }

    // === Tool resolution and boundary ===

    #[test]
    fn tool_calls_resolved() {
        let mut auth = authority_at_waiting_for_ops();
        let t = auth
            .apply(TurnExecutionInput::ToolCallsResolved {
                run_id: test_run_id(),
            })
            .expect("resolved");
        assert_eq!(t.next_phase, TurnPhase::DrainingBoundary);
        assert_eq!(auth.tool_calls_pending(), 0);
        assert_eq!(auth.boundary_count(), 1);
    }

    #[test]
    fn tool_calls_resolved_from_wrong_phase_rejected() {
        // After resolving, we're at DrainingBoundary — resolving again should fail.
        let mut auth = authority_at_waiting_for_ops();
        auth.apply(TurnExecutionInput::ToolCallsResolved {
            run_id: test_run_id(),
        })
        .expect("first resolve");
        assert!(
            auth.apply(TurnExecutionInput::ToolCallsResolved {
                run_id: test_run_id(),
            })
            .is_err()
        );
    }

    #[test]
    fn boundary_continue_goes_to_calling_llm() {
        let mut auth = authority_at_draining_boundary();
        let t = auth
            .apply(TurnExecutionInput::BoundaryContinue {
                run_id: test_run_id(),
            })
            .expect("boundary continue");
        assert_eq!(t.next_phase, TurnPhase::CallingLlm);
    }

    #[test]
    fn boundary_continue_with_cancel_after_boundary_goes_to_cancelled() {
        let mut auth = authority_at_draining_boundary();
        auth.apply(TurnExecutionInput::CancelAfterBoundary {
            run_id: test_run_id(),
        })
        .expect("cancel after boundary");
        let t = auth
            .apply(TurnExecutionInput::BoundaryContinue {
                run_id: test_run_id(),
            })
            .expect("boundary continue");
        assert_eq!(t.next_phase, TurnPhase::Cancelled);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Cancelled);
    }

    #[test]
    fn boundary_complete_goes_to_completed() {
        let mut auth = authority_at_draining_boundary();
        let t = auth
            .apply(TurnExecutionInput::BoundaryComplete {
                run_id: test_run_id(),
            })
            .expect("boundary complete");
        assert_eq!(t.next_phase, TurnPhase::Completed);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Completed);
        assert_eq!(t.effects.len(), 1);
        assert!(matches!(
            t.effects[0],
            TurnExecutionEffect::RunCompleted { .. }
        ));
    }

    #[test]
    fn boundary_complete_with_cancel_after_boundary_goes_to_cancelled() {
        let mut auth = authority_at_draining_boundary();
        auth.apply(TurnExecutionInput::CancelAfterBoundary {
            run_id: test_run_id(),
        })
        .expect("cancel after boundary");
        let t = auth
            .apply(TurnExecutionInput::BoundaryComplete {
                run_id: test_run_id(),
            })
            .expect("boundary complete");
        assert_eq!(t.next_phase, TurnPhase::Cancelled);
    }

    // === Error recovery ===

    #[test]
    fn recoverable_failure_from_calling_llm() {
        let mut auth = authority_at_calling_llm();
        let t = auth
            .apply(TurnExecutionInput::RecoverableFailure {
                run_id: test_run_id(),
            })
            .expect("recoverable");
        assert_eq!(t.next_phase, TurnPhase::ErrorRecovery);
    }

    #[test]
    fn recoverable_failure_from_waiting_for_ops() {
        let mut auth = authority_at_waiting_for_ops();
        let t = auth
            .apply(TurnExecutionInput::RecoverableFailure {
                run_id: test_run_id(),
            })
            .expect("recoverable");
        assert_eq!(t.next_phase, TurnPhase::ErrorRecovery);
    }

    #[test]
    fn recoverable_failure_from_draining_boundary() {
        let mut auth = authority_at_draining_boundary();
        let t = auth
            .apply(TurnExecutionInput::RecoverableFailure {
                run_id: test_run_id(),
            })
            .expect("recoverable");
        assert_eq!(t.next_phase, TurnPhase::ErrorRecovery);
    }

    #[test]
    fn retry_requested_from_error_recovery() {
        let mut auth = authority_at_calling_llm();
        auth.apply(TurnExecutionInput::RecoverableFailure {
            run_id: test_run_id(),
        })
        .expect("recoverable");
        let t = auth
            .apply(TurnExecutionInput::RetryRequested {
                run_id: test_run_id(),
            })
            .expect("retry");
        assert_eq!(t.next_phase, TurnPhase::CallingLlm);
    }

    // === Fatal failure ===

    #[test]
    fn fatal_failure_from_various_phases() {
        for phase_fn in [
            authority_at_calling_llm as fn() -> TurnExecutionAuthority,
            authority_at_waiting_for_ops,
            authority_at_draining_boundary,
        ] {
            let mut auth = phase_fn();
            let t = auth
                .apply(TurnExecutionInput::FatalFailure {
                    run_id: test_run_id(),
                })
                .expect("fatal failure should work");
            assert_eq!(t.next_phase, TurnPhase::Failed);
            assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Failed);
            assert!(
                t.effects
                    .iter()
                    .any(|e| matches!(e, TurnExecutionEffect::RunFailed { .. }))
            );
        }
    }

    #[test]
    fn fatal_failure_from_error_recovery() {
        let mut auth = authority_at_calling_llm();
        auth.apply(TurnExecutionInput::RecoverableFailure {
            run_id: test_run_id(),
        })
        .expect("recoverable");
        let t = auth
            .apply(TurnExecutionInput::FatalFailure {
                run_id: test_run_id(),
            })
            .expect("fatal");
        assert_eq!(t.next_phase, TurnPhase::Failed);
    }

    #[test]
    fn fatal_failure_from_applying_primitive() {
        let mut auth = make_authority();
        auth.apply(TurnExecutionInput::StartConversationRun {
            run_id: test_run_id(),
        })
        .expect("start");
        let t = auth
            .apply(TurnExecutionInput::FatalFailure {
                run_id: test_run_id(),
            })
            .expect("fatal");
        assert_eq!(t.next_phase, TurnPhase::Failed);
    }

    // === Cancellation ===

    #[test]
    fn cancel_now_from_various_phases() {
        for phase_fn in [
            authority_at_calling_llm as fn() -> TurnExecutionAuthority,
            authority_at_waiting_for_ops,
            authority_at_draining_boundary,
        ] {
            let mut auth = phase_fn();
            let t = auth
                .apply(TurnExecutionInput::CancelNow {
                    run_id: test_run_id(),
                })
                .expect("cancel now");
            assert_eq!(t.next_phase, TurnPhase::Cancelling);
        }
    }

    #[test]
    fn cancel_now_from_applying_primitive() {
        let mut auth = make_authority();
        auth.apply(TurnExecutionInput::StartConversationRun {
            run_id: test_run_id(),
        })
        .expect("start");
        let t = auth
            .apply(TurnExecutionInput::CancelNow {
                run_id: test_run_id(),
            })
            .expect("cancel now");
        assert_eq!(t.next_phase, TurnPhase::Cancelling);
    }

    #[test]
    fn cancellation_observed() {
        let mut auth = authority_at_calling_llm();
        auth.apply(TurnExecutionInput::CancelNow {
            run_id: test_run_id(),
        })
        .expect("cancel now");
        let t = auth
            .apply(TurnExecutionInput::CancellationObserved {
                run_id: test_run_id(),
            })
            .expect("cancellation observed");
        assert_eq!(t.next_phase, TurnPhase::Cancelled);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Cancelled);
        assert!(!auth.cancel_after_boundary());
    }

    // === CancelAfterBoundary ===

    #[test]
    fn cancel_after_boundary_stays_in_same_phase() {
        let mut auth = authority_at_calling_llm();
        let t = auth
            .apply(TurnExecutionInput::CancelAfterBoundary {
                run_id: test_run_id(),
            })
            .expect("cancel after boundary");
        assert_eq!(t.next_phase, TurnPhase::CallingLlm);
        assert!(auth.cancel_after_boundary());
    }

    #[test]
    fn cancel_after_boundary_from_waiting_for_ops() {
        let mut auth = authority_at_waiting_for_ops();
        let t = auth
            .apply(TurnExecutionInput::CancelAfterBoundary {
                run_id: test_run_id(),
            })
            .expect("cancel after boundary");
        assert_eq!(t.next_phase, TurnPhase::WaitingForOps);
        assert!(auth.cancel_after_boundary());
    }

    // === AcknowledgeTerminal ===

    #[test]
    fn acknowledge_terminal_from_completed_resets_to_ready() {
        let mut auth = authority_at_draining_boundary();
        auth.apply(TurnExecutionInput::BoundaryComplete {
            run_id: test_run_id(),
        })
        .expect("complete");
        assert_eq!(auth.phase(), TurnPhase::Completed);

        let t = auth
            .apply(TurnExecutionInput::AcknowledgeTerminal {
                run_id: test_run_id(),
            })
            .expect("acknowledge");
        assert_eq!(t.next_phase, TurnPhase::Ready);
        assert_eq!(auth.active_run(), None);
        assert_eq!(auth.primitive_kind(), TurnPrimitiveKind::None);
        assert_eq!(auth.boundary_count(), 0);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::None);
    }

    #[test]
    fn acknowledge_terminal_from_failed_resets_to_ready() {
        let mut auth = authority_at_calling_llm();
        auth.apply(TurnExecutionInput::FatalFailure {
            run_id: test_run_id(),
        })
        .expect("fatal");
        let t = auth
            .apply(TurnExecutionInput::AcknowledgeTerminal {
                run_id: test_run_id(),
            })
            .expect("acknowledge");
        assert_eq!(t.next_phase, TurnPhase::Ready);
    }

    #[test]
    fn acknowledge_terminal_from_cancelled_resets_to_ready() {
        let mut auth = authority_at_calling_llm();
        auth.apply(TurnExecutionInput::CancelNow {
            run_id: test_run_id(),
        })
        .expect("cancel");
        auth.apply(TurnExecutionInput::CancellationObserved {
            run_id: test_run_id(),
        })
        .expect("observed");
        let t = auth
            .apply(TurnExecutionInput::AcknowledgeTerminal {
                run_id: test_run_id(),
            })
            .expect("acknowledge");
        assert_eq!(t.next_phase, TurnPhase::Ready);
    }

    #[test]
    fn acknowledge_terminal_wrong_run_id_rejected() {
        let mut auth = authority_at_draining_boundary();
        auth.apply(TurnExecutionInput::BoundaryComplete {
            run_id: test_run_id(),
        })
        .expect("complete");
        assert!(
            auth.apply(TurnExecutionInput::AcknowledgeTerminal {
                run_id: other_run_id(),
            })
            .is_err()
        );
    }

    // === Run ID guard ===

    #[test]
    fn wrong_run_id_rejected_everywhere() {
        let mut auth = authority_at_calling_llm();
        assert!(
            auth.apply(TurnExecutionInput::LlmReturnedToolCalls {
                run_id: other_run_id(),
                tool_count: 1,
            })
            .is_err()
        );
        // Phase unchanged on rejected transition
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);
    }

    // === Full happy paths ===

    #[test]
    fn full_conversation_turn_happy_path() {
        let mut auth = make_authority();
        let rid = test_run_id();

        // Start
        auth.apply(TurnExecutionInput::StartConversationRun {
            run_id: rid.clone(),
        })
        .expect("start");

        // Primitive applied
        auth.apply(TurnExecutionInput::PrimitiveApplied {
            run_id: rid.clone(),
            admitted_content_shape: ContentShape("text".into()),
            vision_enabled: false,
            image_tool_results_enabled: false,
        })
        .expect("primitive");
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);

        // LLM returns tool calls
        auth.apply(TurnExecutionInput::LlmReturnedToolCalls {
            run_id: rid.clone(),
            tool_count: 2,
        })
        .expect("tool calls");
        assert_eq!(auth.phase(), TurnPhase::WaitingForOps);

        // Tools resolved
        auth.apply(TurnExecutionInput::ToolCallsResolved {
            run_id: rid.clone(),
        })
        .expect("resolved");
        assert_eq!(auth.phase(), TurnPhase::DrainingBoundary);
        assert_eq!(auth.boundary_count(), 1);

        // Boundary continue -> back to CallingLlm
        auth.apply(TurnExecutionInput::BoundaryContinue {
            run_id: rid.clone(),
        })
        .expect("continue");
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);

        // LLM returns terminal
        auth.apply(TurnExecutionInput::LlmReturnedTerminal {
            run_id: rid.clone(),
        })
        .expect("terminal");
        assert_eq!(auth.phase(), TurnPhase::DrainingBoundary);
        assert_eq!(auth.boundary_count(), 2);

        // Boundary complete
        let t = auth
            .apply(TurnExecutionInput::BoundaryComplete {
                run_id: rid.clone(),
            })
            .expect("complete");
        assert_eq!(t.next_phase, TurnPhase::Completed);
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, TurnExecutionEffect::RunCompleted { .. }))
        );

        // Acknowledge
        auth.apply(TurnExecutionInput::AcknowledgeTerminal { run_id: rid })
            .expect("ack");
        assert_eq!(auth.phase(), TurnPhase::Ready);
        assert_eq!(auth.active_run(), None);
    }

    #[test]
    fn full_error_recovery_path() {
        let mut auth = authority_at_calling_llm();
        let rid = test_run_id();

        // Recoverable failure
        auth.apply(TurnExecutionInput::RecoverableFailure {
            run_id: rid.clone(),
        })
        .expect("recoverable");
        assert_eq!(auth.phase(), TurnPhase::ErrorRecovery);

        // Retry
        auth.apply(TurnExecutionInput::RetryRequested {
            run_id: rid.clone(),
        })
        .expect("retry");
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);

        // Now fatal
        auth.apply(TurnExecutionInput::FatalFailure { run_id: rid })
            .expect("fatal");
        assert_eq!(auth.phase(), TurnPhase::Failed);
    }

    #[test]
    fn full_cancel_after_boundary_path() {
        let mut auth = authority_at_calling_llm();
        let rid = test_run_id();

        // Request cancel after boundary
        auth.apply(TurnExecutionInput::CancelAfterBoundary {
            run_id: rid.clone(),
        })
        .expect("cancel after boundary");
        assert!(auth.cancel_after_boundary());
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);

        // LLM returns tool calls
        auth.apply(TurnExecutionInput::LlmReturnedToolCalls {
            run_id: rid.clone(),
            tool_count: 1,
        })
        .expect("tool calls");

        // Tools resolved
        auth.apply(TurnExecutionInput::ToolCallsResolved {
            run_id: rid.clone(),
        })
        .expect("resolved");
        assert_eq!(auth.phase(), TurnPhase::DrainingBoundary);

        // Boundary continue -> should go to Cancelled (cancel_after_boundary is set)
        let t = auth
            .apply(TurnExecutionInput::BoundaryContinue { run_id: rid })
            .expect("boundary continue");
        assert_eq!(t.next_phase, TurnPhase::Cancelled);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Cancelled);
    }

    #[test]
    fn can_accept_probes_without_mutation() {
        let auth = make_authority();
        assert!(auth.can_accept(&TurnExecutionInput::StartConversationRun {
            run_id: test_run_id(),
        }));
        assert!(!auth.can_accept(&TurnExecutionInput::LlmReturnedTerminal {
            run_id: test_run_id(),
        }));
        // Phase unchanged
        assert_eq!(auth.phase(), TurnPhase::Ready);
    }

    // === LoopState bridge ===

    #[test]
    fn turn_phase_to_loop_state_mapping() {
        use crate::state::LoopState;
        assert_eq!(TurnPhase::Ready.to_loop_state(), LoopState::CallingLlm);
        assert_eq!(
            TurnPhase::ApplyingPrimitive.to_loop_state(),
            LoopState::CallingLlm
        );
        assert_eq!(TurnPhase::CallingLlm.to_loop_state(), LoopState::CallingLlm);
        assert_eq!(
            TurnPhase::WaitingForOps.to_loop_state(),
            LoopState::WaitingForOps
        );
        assert_eq!(
            TurnPhase::DrainingBoundary.to_loop_state(),
            LoopState::DrainingEvents
        );
        assert_eq!(
            TurnPhase::Extracting.to_loop_state(),
            LoopState::DrainingEvents
        );
        assert_eq!(
            TurnPhase::ErrorRecovery.to_loop_state(),
            LoopState::ErrorRecovery
        );
        assert_eq!(TurnPhase::Cancelling.to_loop_state(), LoopState::Cancelling);
        assert_eq!(TurnPhase::Completed.to_loop_state(), LoopState::Completed);
        assert_eq!(TurnPhase::Failed.to_loop_state(), LoopState::Completed);
        assert_eq!(TurnPhase::Cancelled.to_loop_state(), LoopState::Completed);
    }

    // === Extraction lifecycle ===

    /// Helper: reach DrainingBoundary via LlmReturnedTerminal (terminal response path).
    fn authority_at_draining_boundary_terminal() -> TurnExecutionAuthority {
        let mut auth = authority_at_calling_llm();
        auth.apply(TurnExecutionInput::LlmReturnedTerminal {
            run_id: test_run_id(),
        })
        .expect("llm returned terminal");
        assert_eq!(auth.phase(), TurnPhase::DrainingBoundary);
        auth
    }

    #[test]
    fn enter_extraction_from_draining_boundary() {
        let mut auth = authority_at_draining_boundary_terminal();
        let t = auth
            .apply(TurnExecutionInput::EnterExtraction {
                run_id: test_run_id(),
                max_retries: 3,
            })
            .expect("enter extraction");
        assert_eq!(t.next_phase, TurnPhase::Extracting);
        assert_eq!(auth.max_extraction_retries(), 3);
        assert_eq!(auth.extraction_attempts(), 0);
    }

    #[test]
    fn extraction_retry_increments_attempts_and_goes_to_calling_llm() {
        let mut auth = authority_at_draining_boundary_terminal();
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: test_run_id(),
            max_retries: 3,
        })
        .expect("enter extraction");

        let t = auth
            .apply(TurnExecutionInput::ExtractionRetry {
                run_id: test_run_id(),
            })
            .expect("extraction retry");
        assert_eq!(t.next_phase, TurnPhase::CallingLlm);
        assert_eq!(auth.extraction_attempts(), 1);
    }

    #[test]
    fn extraction_validation_passed_completes() {
        let mut auth = authority_at_draining_boundary_terminal();
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: test_run_id(),
            max_retries: 3,
        })
        .expect("enter extraction");

        let t = auth
            .apply(TurnExecutionInput::ExtractionValidationPassed {
                run_id: test_run_id(),
            })
            .expect("validation passed");
        assert_eq!(t.next_phase, TurnPhase::Completed);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Completed);
        assert!(
            t.effects
                .iter()
                .any(|e| matches!(e, TurnExecutionEffect::RunCompleted { .. }))
        );
    }

    #[test]
    fn extraction_exhausted_completes() {
        let mut auth = authority_at_draining_boundary_terminal();
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: test_run_id(),
            max_retries: 1,
        })
        .expect("enter extraction");

        let t = auth
            .apply(TurnExecutionInput::ExtractionExhausted {
                run_id: test_run_id(),
            })
            .expect("exhausted");
        assert_eq!(t.next_phase, TurnPhase::Completed);
        assert_eq!(auth.terminal_outcome(), TurnTerminalOutcome::Completed);
    }

    #[test]
    fn full_extraction_retry_cycle() {
        let mut auth = authority_at_draining_boundary_terminal();
        let rid = test_run_id();

        // Enter extraction
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: rid.clone(),
            max_retries: 2,
        })
        .expect("enter");
        assert_eq!(auth.phase(), TurnPhase::Extracting);

        // First retry -> CallingLlm
        auth.apply(TurnExecutionInput::ExtractionRetry {
            run_id: rid.clone(),
        })
        .expect("retry 1");
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);
        assert_eq!(auth.extraction_attempts(), 1);

        // LLM returns terminal -> DrainingBoundary
        auth.apply(TurnExecutionInput::LlmReturnedTerminal {
            run_id: rid.clone(),
        })
        .expect("llm terminal");
        assert_eq!(auth.phase(), TurnPhase::DrainingBoundary);

        // Re-enter extraction for validation
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: rid.clone(),
            max_retries: 2,
        })
        .expect("re-enter");
        assert_eq!(auth.phase(), TurnPhase::Extracting);
        // Attempts preserved (not reset)
        assert_eq!(auth.extraction_attempts(), 1);

        // Second retry -> CallingLlm
        auth.apply(TurnExecutionInput::ExtractionRetry {
            run_id: rid.clone(),
        })
        .expect("retry 2");
        assert_eq!(auth.phase(), TurnPhase::CallingLlm);
        assert_eq!(auth.extraction_attempts(), 2);

        // LLM returns terminal -> DrainingBoundary
        auth.apply(TurnExecutionInput::LlmReturnedTerminal {
            run_id: rid.clone(),
        })
        .expect("llm terminal 2");

        // Re-enter extraction
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: rid.clone(),
            max_retries: 2,
        })
        .expect("re-enter 2");

        // Validation passed
        let t = auth
            .apply(TurnExecutionInput::ExtractionValidationPassed { run_id: rid })
            .expect("passed");
        assert_eq!(t.next_phase, TurnPhase::Completed);
    }

    #[test]
    fn extraction_wrong_run_id_rejected() {
        let mut auth = authority_at_draining_boundary_terminal();
        assert!(
            auth.apply(TurnExecutionInput::EnterExtraction {
                run_id: other_run_id(),
                max_retries: 3,
            })
            .is_err()
        );
    }

    #[test]
    fn fatal_failure_from_extracting() {
        let mut auth = authority_at_draining_boundary_terminal();
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: test_run_id(),
            max_retries: 3,
        })
        .expect("enter extraction");

        let t = auth
            .apply(TurnExecutionInput::FatalFailure {
                run_id: test_run_id(),
            })
            .expect("fatal");
        assert_eq!(t.next_phase, TurnPhase::Failed);
    }

    #[test]
    fn cancel_now_from_extracting() {
        let mut auth = authority_at_draining_boundary_terminal();
        auth.apply(TurnExecutionInput::EnterExtraction {
            run_id: test_run_id(),
            max_retries: 3,
        })
        .expect("enter extraction");

        let t = auth
            .apply(TurnExecutionInput::CancelNow {
                run_id: test_run_id(),
            })
            .expect("cancel");
        assert_eq!(t.next_phase, TurnPhase::Cancelling);
    }
}

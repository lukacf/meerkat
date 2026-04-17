//! Shared typed turn-execution shapes.
//!
//! This module intentionally does not define or export a handwritten
//! authority/state-machine surface. It holds the stable enums and transition
//! payloads shared by the standalone core fallback and runtime-backed handles.

use crate::lifecycle::RunId;
use crate::ops::{AsyncOpRef, OperationId};

/// Canonical phases for turn execution.
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
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub fn is_extracting(self) -> bool {
        matches!(self, Self::Extracting)
    }

    /// Convert turn phase to the observable `LoopState`.
    pub fn to_loop_state(self) -> crate::state::LoopState {
        use crate::state::LoopState;
        match self {
            Self::Ready | Self::ApplyingPrimitive | Self::CallingLlm => LoopState::CallingLlm,
            Self::WaitingForOps => LoopState::WaitingForOps,
            Self::DrainingBoundary | Self::Extracting => LoopState::DrainingEvents,
            Self::ErrorRecovery => LoopState::ErrorRecovery,
            Self::Cancelling => LoopState::Cancelling,
            Self::Completed | Self::Failed | Self::Cancelled => LoopState::Completed,
        }
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

/// What kind of primitive is currently in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPrimitiveKind {
    None,
    ConversationTurn,
    ImmediateAppend,
    ImmediateContextAppend,
}

/// Terminal outcome of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnTerminalOutcome {
    None,
    Completed,
    Failed,
    Cancelled,
    BudgetExhausted,
    TimeBudgetExceeded,
    StructuredOutputValidationFailed,
}

/// Content shape admitted by the primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentShape(pub String);

/// Typed inputs describing turn-execution events.
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
    RegisterPendingOps {
        run_id: RunId,
        op_refs: Vec<AsyncOpRef>,
        barrier_operation_ids: Vec<OperationId>,
        has_barrier_ops: bool,
    },
    ToolCallsResolved {
        run_id: RunId,
    },
    OpsBarrierSatisfied {
        run_id: RunId,
        operation_ids: Vec<OperationId>,
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
    TimeBudgetExceeded {
        run_id: RunId,
    },
    EnterExtraction {
        run_id: RunId,
        max_retries: u32,
    },
    ExtractionValidationPassed {
        run_id: RunId,
    },
    ExtractionValidationFailed {
        run_id: RunId,
        error: String,
    },
    ExtractionStart {
        run_id: RunId,
    },
    ForceCancelNoRun,
}

/// Side effects emitted by turn transitions.
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
    CheckCompaction,
}

/// Successful transition outcome for turn execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnExecutionTransition {
    pub prev_phase: TurnPhase,
    pub next_phase: TurnPhase,
    pub effects: Vec<TurnExecutionEffect>,
}

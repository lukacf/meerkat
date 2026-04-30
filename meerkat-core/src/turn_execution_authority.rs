//! Shared typed turn-execution shapes.
//!
//! This module intentionally does not define or export a handwritten
//! authority/state-machine surface. It holds the stable enums and transition
//! payloads shared by the standalone core fallback and runtime-backed handles.

use crate::budget::{BudgetDimension, BudgetExceeded};
use crate::error::AgentError;
use crate::event::AgentErrorClass;
use crate::lifecycle::RunId;
use crate::ops::{AsyncOpRef, OperationId};
use crate::retry::LlmRetrySchedule;

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

/// Typed reason for a turn failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFailureReason {
    pub class: AgentErrorClass,
    pub message: String,
}

impl TurnFailureReason {
    pub fn new(class: AgentErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    pub fn from_agent_error(error: &AgentError) -> Self {
        Self::new(AgentErrorClass::from(error), error.to_string())
    }

    pub fn budget_exceeded(exceeded: BudgetExceeded) -> Self {
        let class = AgentErrorClass::Budget;
        let message = match exceeded.dimension {
            BudgetDimension::Tokens => {
                format!(
                    "token budget exceeded: {} > {}",
                    exceeded.used, exceeded.limit
                )
            }
            BudgetDimension::Time => {
                format!(
                    "time budget exceeded: {} > {}",
                    exceeded.used, exceeded.limit
                )
            }
            BudgetDimension::ToolCalls => {
                format!(
                    "tool call budget exceeded: {} > {}",
                    exceeded.used, exceeded.limit
                )
            }
        };
        Self::new(class, message)
    }

    pub fn terminal_outcome(outcome: TurnTerminalOutcome) -> Self {
        match outcome {
            TurnTerminalOutcome::BudgetExhausted => {
                Self::new(AgentErrorClass::Budget, "budget exhausted")
            }
            TurnTerminalOutcome::TimeBudgetExceeded => {
                Self::new(AgentErrorClass::Budget, "time budget exceeded")
            }
            TurnTerminalOutcome::StructuredOutputValidationFailed => Self::new(
                AgentErrorClass::StructuredOutput,
                "structured output validation failed",
            ),
            TurnTerminalOutcome::Cancelled => Self::new(AgentErrorClass::Cancelled, "cancelled"),
            TurnTerminalOutcome::Completed | TurnTerminalOutcome::None => {
                Self::new(AgentErrorClass::Terminal, "terminal outcome")
            }
            TurnTerminalOutcome::Failed => Self::new(AgentErrorClass::Terminal, "turn failed"),
        }
    }

    pub fn to_dsl_error(&self) -> String {
        format!("{:?}: {}", self.class, self.message)
    }
}

/// Content shape admitted by the turn primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContentShape {
    Conversation,
    ConversationAndContext,
    Context,
    Empty,
    ImmediateAppend,
    ImmediateContext,
}

impl ContentShape {
    pub const SCHEMA_TYPE_NAME: &'static str = "ContentShape";

    pub const ALL: [Self; 6] = [
        Self::Conversation,
        Self::ConversationAndContext,
        Self::Context,
        Self::Empty,
        Self::ImmediateAppend,
        Self::ImmediateContext,
    ];

    pub const SCHEMA_VARIANTS: [&'static str; 6] = [
        Self::Conversation.schema_variant(),
        Self::ConversationAndContext.schema_variant(),
        Self::Context.schema_variant(),
        Self::Empty.schema_variant(),
        Self::ImmediateAppend.schema_variant(),
        Self::ImmediateContext.schema_variant(),
    ];

    pub const fn from_staged_presence(has_conversation: bool, has_context: bool) -> Self {
        match (has_conversation, has_context) {
            (true, true) => Self::ConversationAndContext,
            (true, false) => Self::Conversation,
            (false, true) => Self::Context,
            (false, false) => Self::Empty,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::ConversationAndContext => "conversation+context",
            Self::Context => "context",
            Self::Empty => "empty",
            Self::ImmediateAppend => "immediate_append",
            Self::ImmediateContext => "immediate_context",
        }
    }

    pub const fn schema_variant(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::ConversationAndContext => "ConversationAndContext",
            Self::Context => "Context",
            Self::Empty => "Empty",
            Self::ImmediateAppend => "ImmediateAppend",
            Self::ImmediateContext => "ImmediateContext",
        }
    }

    pub fn from_schema_variant(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|shape| shape.schema_variant() == value)
    }
}

impl std::fmt::Display for ContentShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

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
        retry: LlmRetrySchedule,
    },
    FatalFailure {
        run_id: RunId,
        reason: TurnFailureReason,
    },
    RetryRequested {
        run_id: RunId,
        retry_attempt: u32,
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
    BudgetLimitExceeded {
        run_id: RunId,
        exceeded: BudgetExceeded,
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
        reason: TurnFailureReason,
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

pub fn terminal_outcome_for_budget_exceeded(exceeded: BudgetExceeded) -> TurnTerminalOutcome {
    match exceeded.dimension {
        BudgetDimension::Time => TurnTerminalOutcome::TimeBudgetExceeded,
        BudgetDimension::Tokens | BudgetDimension::ToolCalls => {
            TurnTerminalOutcome::BudgetExhausted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exceeded(dimension: BudgetDimension) -> BudgetExceeded {
        BudgetExceeded {
            dimension,
            used: 1,
            limit: 1,
        }
    }

    #[test]
    fn budget_terminal_classification_is_turn_authority_owned() {
        assert_eq!(
            terminal_outcome_for_budget_exceeded(exceeded(BudgetDimension::Tokens)),
            TurnTerminalOutcome::BudgetExhausted
        );
        assert_eq!(
            terminal_outcome_for_budget_exceeded(exceeded(BudgetDimension::ToolCalls)),
            TurnTerminalOutcome::BudgetExhausted
        );
        assert_eq!(
            terminal_outcome_for_budget_exceeded(exceeded(BudgetDimension::Time)),
            TurnTerminalOutcome::TimeBudgetExceeded
        );
    }

    #[test]
    fn content_shape_is_closed_contract_with_stable_wire_labels() {
        let shapes = [
            (ContentShape::Conversation, "conversation"),
            (ContentShape::ConversationAndContext, "conversation+context"),
            (ContentShape::Context, "context"),
            (ContentShape::Empty, "empty"),
            (ContentShape::ImmediateAppend, "immediate_append"),
            (ContentShape::ImmediateContext, "immediate_context"),
        ];

        for (shape, label) in shapes {
            assert_eq!(shape.as_str(), label);
            assert_eq!(shape.to_string(), label);
            assert_eq!(
                ContentShape::from_schema_variant(shape.schema_variant()),
                Some(shape)
            );
        }

        assert_eq!(
            ContentShape::SCHEMA_VARIANTS,
            [
                "Conversation",
                "ConversationAndContext",
                "Context",
                "Empty",
                "ImmediateAppend",
                "ImmediateContext"
            ]
        );
    }
}

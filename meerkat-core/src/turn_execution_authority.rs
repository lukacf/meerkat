//! Shared typed turn-execution shapes.
//!
//! This module intentionally does not define or export a handwritten
//! authority/state-machine surface. It holds the stable enums and transition
//! payloads shared by core unit-test adapters and runtime-backed handles.

use crate::budget::{BudgetDimension, BudgetExceeded};
use crate::error::AgentError;
use crate::event::AgentErrorClass;
use crate::lifecycle::RunId;
use crate::ops::{AsyncOpRef, OperationId};
use crate::retry::{LlmRetryFailureKind, LlmRetrySchedule};
use serde::{Deserialize, Serialize};

/// P0 Dogma Invariant 1: machine-owned LLM-failure recovery verdict, mirrored
/// across the turn-execution boundary. The agent loop drives
/// `TurnExecutionInput::ClassifyLlmFailureRecovery` and mirrors this verdict
/// emitted by MeerkatMachine's `ClassifyLlmFailureRecovery` classifier: the
/// machine — not the shell `RetryPolicy` — owns whether an LLM failure may
/// `Recover`, is `Exhausted`, or is `Fatal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmFailureRecoveryKind {
    Recover,
    Exhausted,
    Fatal,
}

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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnTerminalOutcome {
    None,
    Completed,
    Failed,
    Cancelled,
    BudgetExhausted,
    TimeBudgetExceeded,
    StructuredOutputValidationFailed,
}

/// Closed machine-owned classifier for why a turn reached a terminal failure.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnTerminalCauseKind {
    Unknown,
    HookDenied,
    HookFailure,
    LlmFailure,
    ToolFailure,
    StructuredOutputValidationFailed,
    BudgetExhausted,
    TimeBudgetExceeded,
    RetryExhausted,
    TurnLimitReached,
    RuntimeApplyFailure,
    FatalFailure,
}

impl TurnTerminalCauseKind {
    pub fn agent_error_class(self) -> AgentErrorClass {
        match self {
            Self::HookDenied | Self::HookFailure => AgentErrorClass::Hook,
            Self::LlmFailure => AgentErrorClass::Llm,
            Self::ToolFailure => AgentErrorClass::Tool,
            Self::StructuredOutputValidationFailed => AgentErrorClass::StructuredOutput,
            Self::BudgetExhausted | Self::TimeBudgetExceeded => AgentErrorClass::Budget,
            Self::RetryExhausted => AgentErrorClass::Llm,
            Self::TurnLimitReached => AgentErrorClass::MaxTurns,
            Self::RuntimeApplyFailure | Self::Unknown => AgentErrorClass::Internal,
            Self::FatalFailure => AgentErrorClass::Terminal,
        }
    }

    pub fn is_specific_failure_cause(self) -> bool {
        !matches!(self, Self::Unknown)
    }

    pub fn default_message(self, _outcome: TurnTerminalOutcome) -> &'static str {
        match self {
            Self::HookDenied => "hook denied terminal turn",
            Self::HookFailure => "hook failure terminal turn",
            Self::LlmFailure => "LLM failure terminal turn",
            Self::ToolFailure => "tool failure terminal turn",
            Self::StructuredOutputValidationFailed => "structured output validation failed",
            Self::BudgetExhausted => "budget exhausted",
            Self::TimeBudgetExceeded => "time budget exceeded",
            Self::RetryExhausted => "retry exhausted",
            Self::TurnLimitReached => "turn limit reached",
            Self::RuntimeApplyFailure => "runtime apply failure",
            Self::FatalFailure => "fatal turn failure",
            Self::Unknown => "unknown terminal cause",
        }
    }
}

/// Raw failure source fact reported to generated turn authority.
///
/// This is not a terminal cause or grouped terminal class. The variant names
/// mirror the failure source that observed the error; MeerkatMachine owns the
/// mapping from these source facts to terminal outcome/cause and emits the
/// authorized [`TurnTerminalCauseKind`] on `TurnRunFailed`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnFailureSourceKind {
    Unknown,
    Llm,
    StoreError,
    ToolError,
    McpError,
    SessionNotFound,
    TokenBudgetExceeded,
    TimeBudgetExceeded,
    ToolCallBudgetExceeded,
    MaxTokensReached,
    ContentFiltered,
    MaxTurnsReached,
    Cancelled,
    InvalidStateTransition,
    OperationNotFound,
    DepthLimitExceeded,
    ConcurrencyLimitExceeded,
    ConfigError,
    InvalidToolAccess,
    InternalError,
    BuildError,
    AuthReauthRequired,
    CallbackPending,
    StructuredOutputValidationFailed,
    InvalidOutputSchema,
    HookDenied,
    HookTimeout,
    HookExecutionFailed,
    HookConfigInvalid,
    TerminalFailure,
    NoPendingBoundary,
    LlmRetryExhausted,
}

impl TurnFailureSourceKind {
    pub fn from_agent_error(error: &AgentError) -> Self {
        match error {
            AgentError::Llm { .. } => Self::Llm,
            AgentError::StoreError(_) => Self::StoreError,
            AgentError::ToolError(_) => Self::ToolError,
            AgentError::Tool { .. } => Self::ToolError,
            AgentError::McpError(_) => Self::McpError,
            AgentError::SessionNotFound(_) => Self::SessionNotFound,
            AgentError::TokenBudgetExceeded { .. } => Self::TokenBudgetExceeded,
            AgentError::TimeBudgetExceeded { .. } => Self::TimeBudgetExceeded,
            AgentError::ToolCallBudgetExceeded { .. } => Self::ToolCallBudgetExceeded,
            AgentError::MaxTokensReached { .. } => Self::MaxTokensReached,
            AgentError::ContentFiltered { .. } => Self::ContentFiltered,
            AgentError::MaxTurnsReached { .. } => Self::MaxTurnsReached,
            AgentError::Cancelled => Self::Cancelled,
            AgentError::InvalidStateTransition { .. } => Self::InvalidStateTransition,
            AgentError::OperationNotFound(_) => Self::OperationNotFound,
            AgentError::DepthLimitExceeded { .. } => Self::DepthLimitExceeded,
            AgentError::ConcurrencyLimitExceeded => Self::ConcurrencyLimitExceeded,
            AgentError::ConfigError(_) => Self::ConfigError,
            AgentError::InvalidToolAccess { .. } => Self::InvalidToolAccess,
            AgentError::InternalError(_) => Self::InternalError,
            AgentError::BuildError(_) => Self::BuildError,
            AgentError::AuthReauthRequired { .. } => Self::AuthReauthRequired,
            AgentError::CallbackPending { .. } => Self::CallbackPending,
            AgentError::StructuredOutputValidationFailed { .. } => {
                Self::StructuredOutputValidationFailed
            }
            AgentError::InvalidOutputSchema(_) => Self::InvalidOutputSchema,
            AgentError::HookDenied { .. } => Self::HookDenied,
            AgentError::HookTimeout { .. } => Self::HookTimeout,
            AgentError::HookExecutionFailed { .. } => Self::HookExecutionFailed,
            AgentError::HookConfigInvalid { .. } => Self::HookConfigInvalid,
            AgentError::TerminalFailure { .. } => Self::TerminalFailure,
            AgentError::NoPendingBoundary => Self::NoPendingBoundary,
            // Capability-unsupported sync; classified like the config-class it
            // was previously represented as (a `ConfigError`).
            AgentError::DurableSnapshotSyncUnsupported => Self::ConfigError,
        }
    }

    pub fn is_known(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// Failure source accepted by the generated turn authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFailureSource {
    pub source_kind: TurnFailureSourceKind,
    pub message: String,
}

impl TurnFailureSource {
    pub fn new(source_kind: TurnFailureSourceKind, message: impl Into<String>) -> Self {
        Self {
            source_kind,
            message: message.into(),
        }
    }

    pub fn from_agent_error(error: &AgentError) -> Self {
        Self::new(
            TurnFailureSourceKind::from_agent_error(error),
            error.to_string(),
        )
    }

    pub fn llm_retry_exhausted(error: &AgentError) -> Self {
        Self::new(TurnFailureSourceKind::LlmRetryExhausted, error.to_string())
    }
}

/// Typed reason for a turn failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFailureReason {
    pub class: AgentErrorClass,
    pub cause_kind: TurnTerminalCauseKind,
    pub message: String,
}

impl TurnFailureReason {
    pub fn with_cause(
        cause_kind: TurnTerminalCauseKind,
        class: AgentErrorClass,
        message: impl Into<String>,
    ) -> Self {
        Self {
            class,
            cause_kind,
            message: message.into(),
        }
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
        primitive_kind: TurnPrimitiveKind,
        admitted_content_shape: ContentShape,
        vision_enabled: bool,
        image_tool_results_enabled: bool,
        max_extraction_retries: u64,
    },
    StartImmediateAppend {
        run_id: RunId,
    },
    StartImmediateContext {
        run_id: RunId,
    },
    PrimitiveApplied {
        run_id: RunId,
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
        failure: TurnFailureSource,
    },
    RetryRequested {
        run_id: RunId,
        retry_attempt: u32,
    },
    /// P0 Dogma Invariant 1: drive MeerkatMachine's LLM-failure recovery
    /// classifier. The agent loop extracts the typed `failure_kind` (absent
    /// when the `AgentError` yields no recoverable kind) and the one-based
    /// `retry_attempt` / `max_retries`, then mirrors the emitted
    /// `LlmFailureRecoveryClassified` verdict. Pure classification — no turn
    /// state mutation.
    ClassifyLlmFailureRecovery {
        failure_kind: Option<LlmRetryFailureKind>,
        retry_attempt: u32,
        max_retries: u32,
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
    ExtractionFailed {
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
    /// P0 Dogma Invariant 1: machine-owned LLM-failure recovery verdict. The
    /// agent loop mirrors this verdict instead of deciding fatal/exhaustion
    /// itself.
    LlmFailureRecoveryClassified {
        recovery: LlmFailureRecoveryKind,
    },
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
    fn budget_success_classification_requires_matching_machine_cause() {
        use crate::generated::terminal_surface_mapping::{SurfaceResultClass, classify_terminal};

        assert_eq!(
            classify_terminal(
                &TurnTerminalOutcome::BudgetExhausted,
                Some(TurnTerminalCauseKind::BudgetExhausted),
            ),
            SurfaceResultClass::Success
        );
        assert_eq!(
            classify_terminal(&TurnTerminalOutcome::BudgetExhausted, None),
            SurfaceResultClass::HardFailure
        );
        assert_eq!(
            classify_terminal(
                &TurnTerminalOutcome::BudgetExhausted,
                Some(TurnTerminalCauseKind::Unknown),
            ),
            SurfaceResultClass::HardFailure
        );
        assert_eq!(
            classify_terminal(
                &TurnTerminalOutcome::BudgetExhausted,
                Some(TurnTerminalCauseKind::RetryExhausted),
            ),
            SurfaceResultClass::HardFailure
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

    #[test]
    fn terminal_failure_source_ignores_display_message() {
        let first = TurnFailureSource::new(TurnFailureSourceKind::HookDenied, "display one");
        let second = TurnFailureSource::new(TurnFailureSourceKind::HookDenied, "display two");

        assert_eq!(first.source_kind, TurnFailureSourceKind::HookDenied);
        assert_eq!(second.source_kind, TurnFailureSourceKind::HookDenied);
        assert_ne!(first.message, second.message);
    }

    #[test]
    fn retry_exhaustion_source_is_not_llm_display() {
        let error = AgentError::llm(
            "mock",
            crate::error::LlmFailureReason::RateLimited { retry_after: None },
            "display text changed",
        );

        let failure = TurnFailureSource::llm_retry_exhausted(&error);

        assert_eq!(
            failure.source_kind,
            TurnFailureSourceKind::LlmRetryExhausted
        );
        assert_ne!(failure.source_kind, TurnFailureSourceKind::Llm);
        assert_eq!(failure.message, error.to_string());
    }
}

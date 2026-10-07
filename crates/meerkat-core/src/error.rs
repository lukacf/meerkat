//! Core error types for Meerkat

use crate::hooks::{HookId, HookPoint, HookReasonCode};
use crate::tool_catalog::ToolUnavailableReason;
use crate::types::SessionId;
use serde::{Deserialize, Serialize};

/// One externally routed callback tool call inside a suspended assistant
/// tool-use batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PendingCallbackToolCall {
    pub tool_use_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settlement_failures: Vec<crate::ops::ToolDispatchSettlementFailure>,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum LlmFailureReason {
    RateLimited {
        retry_after: Option<std::time::Duration>,
    },
    ContextExceeded {
        max: u32,
        requested: u32,
    },
    AuthError,
    InvalidModel(String),
    ProviderError(LlmProviderError),
    /// Provider/client-native network timeout (owned by client layer)
    NetworkTimeout {
        duration_ms: u64,
    },
    /// Agent-loop hard call timeout (owned by agent loop policy)
    CallTimeout {
        duration_ms: u64,
    },
    /// Agent-loop stream-inactivity watchdog fired: the provider stream
    /// produced no events for the configured window (owned by agent loop
    /// policy). Distinct from [`Self::CallTimeout`], which bounds the whole
    /// call regardless of stream liveness.
    StreamStalled {
        inactivity_ms: u64,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProviderErrorKind {
    InvalidRequest,
    /// The local authorization owner refused this concrete operation before
    /// dispatch. This is agent feedback, not a provider retry or run failure.
    OperationRefused,
    /// Protected observation infrastructure failed before physical entry.
    /// This is never permission feedback, a retry, or a fallback trigger.
    OperationObservationUnavailable,
    /// Current operation authority could not be obtained.
    OperationAuthorizationUnavailable,
    /// A dynamic authorization refresh changed the concrete provider route
    /// before dispatch. The caller must rebuild provider projections before
    /// retrying.
    AuthorizationRouteChanged,
    /// The provider rejected (or preflight proved) a serialized request body
    /// that exceeds its request-size cap.
    RequestTooLarge,
    /// The provider account or project has no quota left (exhausted credits,
    /// a spend cap or a hard billing limit). Terminal for the key until a
    /// human tops up the account: no retry clears it and it is not a
    /// per-window rate limit, which stays `RateLimited` with its
    /// `Retry-After` hint.
    QuotaExhausted,
    ContentFiltered,
    /// The provider stopped the conversation for operator review. Never
    /// automatically retry, refresh auth, fall back, or replace the conversation.
    PolicyStop,
    ServerError,
    ServerOverloaded,
    ConnectionReset,
    Unknown,
    StreamParseError,
    IncompleteResponse,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProviderErrorRetryability {
    Retryable,
    NonRetryable,
}

impl LlmProviderErrorRetryability {
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Retryable)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmProviderError {
    pub kind: LlmProviderErrorKind,
    pub retryability: LlmProviderErrorRetryability,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

impl LlmProviderError {
    pub fn new(
        kind: LlmProviderErrorKind,
        retryability: LlmProviderErrorRetryability,
        details: serde_json::Value,
    ) -> Self {
        Self {
            kind,
            retryability,
            details,
        }
    }

    pub fn retryable(kind: LlmProviderErrorKind, details: serde_json::Value) -> Self {
        Self::new(kind, LlmProviderErrorRetryability::Retryable, details)
    }

    pub fn non_retryable(kind: LlmProviderErrorKind, details: serde_json::Value) -> Self {
        Self::new(kind, LlmProviderErrorRetryability::NonRetryable, details)
    }

    pub fn is_retryable(&self) -> bool {
        !matches!(
            self.kind,
            LlmProviderErrorKind::OperationObservationUnavailable
                | LlmProviderErrorKind::OperationAuthorizationUnavailable
        ) && self.retryability.is_retryable()
    }
}

/// Errors that can occur during tool validation
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum ToolValidationError {
    /// The requested tool was not found
    #[error("Tool not found: {name}")]
    NotFound { name: String },
    /// The tool arguments failed validation
    #[error("Invalid arguments for tool '{name}': {reason}")]
    InvalidArguments { name: String, reason: String },
}

impl ToolValidationError {
    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound { name: name.into() }
    }
    pub fn invalid_arguments(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidArguments {
            name: name.into(),
            reason: reason.into(),
        }
    }
}

/// Error returned by tool dispatch operations.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToolError {
    /// The requested tool was not found
    #[error("Tool not found: {name}")]
    NotFound { name: String },

    /// The tool exists but is currently unavailable
    #[error("Tool '{name}' is currently unavailable: {reason}")]
    Unavailable {
        name: String,
        reason: ToolUnavailableReason,
    },

    /// The tool arguments failed validation
    #[error("Invalid arguments for tool '{name}': {reason}")]
    InvalidArguments { name: String, reason: String },

    /// The tool execution failed
    #[error("Tool execution failed: {message}")]
    ExecutionFailed { message: String },

    /// The tool execution failed with structured error data for protocol surfaces.
    #[error("Tool execution failed: {message}")]
    ExecutionFailedWithData {
        message: String,
        data: serde_json::Value,
    },

    /// The tool execution timed out
    #[error("Tool '{name}' timed out after {timeout_ms}ms")]
    Timeout { name: String, timeout_ms: u64 },

    /// A declared streaming tool stopped producing accepted progress.
    #[error("Streaming tool '{name}' stalled after {inactivity_ms}ms of inactivity")]
    InactivityTimeout { name: String, inactivity_ms: u64 },

    /// Tool access was denied by policy
    #[error("Tool '{name}' is not allowed by policy")]
    AccessDenied { name: String },

    /// The affected operation was refused by its governed authorization owner.
    /// This is an ordinary tool result, never a run or session disposition.
    #[error("{refusal}")]
    AuthorizationRefused {
        refusal: crate::authorization::OperationRefused,
    },

    /// Protected observation infrastructure failed before this tool entered.
    #[error("operation observation unavailable")]
    OperationObservationUnavailable,
    #[error("operation authorization unavailable")]
    OperationAuthorizationUnavailable,

    /// Application consequence policy denied an otherwise statically admitted call.
    #[error("Tool call denied by application policy: {denial:?}")]
    PolicyDenied {
        denial: crate::ToolConsequenceDenial,
    },

    /// Application consequence policy could not produce an authoritative verdict.
    #[error("Tool consequence policy is indeterminate: {failure}")]
    PolicyIndeterminate {
        failure: crate::ToolConsequenceFailure,
    },

    /// A generic tool error with a message
    #[error("{0}")]
    Other(String),

    /// Tool call must be routed externally (callback pending)
    ///
    /// This variant signals that a tool call cannot be handled internally
    /// and must be routed to an external handler. The payload contains
    /// serialized information about the pending tool call.
    #[error("Callback pending for tool '{tool_name}'")]
    CallbackPending {
        tool_name: String,
        args: serde_json::Value,
    },
    /// Settlement failed after the primary dispatch result was already known.
    /// Classification and presentation continue to use the original error.
    #[error("{error}")]
    WithSettlementFailures {
        error: Box<ToolError>,
        failures: Vec<crate::ops::ToolDispatchSettlementFailure>,
    },
    /// Mechanical launch requirements refused this tool.
    /// The exact domain cause remains available to tool-feedback owners.
    #[error("{refusal}")]
    ConfinementRefused {
        refusal: crate::confinement::ConfinementRefusal,
    },
    /// An entered hook refused this attempted operation by explicit policy.
    #[error("{denial}")]
    HookDenied {
        /// Keep the full denial without enlarging unrelated result values.
        denial: Box<crate::hooks::HookDenial>,
    },
}

impl From<crate::OperationAuthorizationError> for ToolError {
    fn from(error: crate::OperationAuthorizationError) -> Self {
        match error {
            crate::OperationAuthorizationError::Refused(refusal) => {
                Self::AuthorizationRefused { refusal }
            }
            crate::OperationAuthorizationError::Unavailable => {
                Self::OperationAuthorizationUnavailable
            }
            crate::OperationAuthorizationError::ObservationUnavailable(_) => {
                Self::OperationObservationUnavailable
            }
        }
    }
}

impl From<crate::authorization::OperationObservationError> for ToolError {
    fn from(_: crate::authorization::OperationObservationError) -> Self {
        Self::OperationObservationUnavailable
    }
}

impl ToolError {
    /// The original tool result, independent of admission settlement.
    #[must_use]
    pub fn primary_error(&self) -> &Self {
        let mut current = self;
        while let Self::WithSettlementFailures { error, .. } = current {
            current = error;
        }
        current
    }

    pub fn settlement_failures(
        &self,
    ) -> impl Iterator<Item = &crate::ops::ToolDispatchSettlementFailure> {
        std::iter::successors(Some(self), |error| match error {
            Self::WithSettlementFailures { error, .. } => Some(error.as_ref()),
            _ => None,
        })
        .flat_map(|error| match error {
            Self::WithSettlementFailures { failures, .. } => failures.as_slice(),
            _ => &[],
        })
    }

    pub fn into_primary_and_settlement_failures(
        self,
    ) -> (Self, Vec<crate::ops::ToolDispatchSettlementFailure>) {
        let mut current = self;
        let mut retained = Vec::new();
        while let Self::WithSettlementFailures { error, failures } = current {
            retained.extend(failures);
            current = *error;
        }
        (current, retained)
    }

    #[must_use]
    pub fn with_settlement_failures(
        self,
        failures: Vec<crate::ops::ToolDispatchSettlementFailure>,
    ) -> Self {
        if failures.is_empty() {
            return self;
        }
        let (error, mut retained) = self.into_primary_and_settlement_failures();
        retained.extend(failures);
        Self::WithSettlementFailures {
            error: Box::new(error),
            failures: retained,
        }
    }

    pub fn error_code(&self) -> &'static str {
        match self {
            Self::WithSettlementFailures { .. } => self.primary_error().error_code(),
            Self::NotFound { .. } => "tool_not_found",
            Self::Unavailable { .. } => "tool_unavailable",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::ExecutionFailed { .. } | Self::ExecutionFailedWithData { .. } => {
                "execution_failed"
            }
            Self::Timeout { .. } => "timeout",
            Self::InactivityTimeout { .. } => "inactivity_timeout",
            Self::AccessDenied { .. } => "access_denied",
            Self::AuthorizationRefused { .. } => "operation_refused",
            Self::ConfinementRefused { .. } => "confinement_refused",
            Self::HookDenied { .. } => "hook_denied",
            Self::OperationObservationUnavailable => "operation_observation_unavailable",
            Self::OperationAuthorizationUnavailable => "operation_authorization_unavailable",
            Self::PolicyDenied { .. } => "policy_denied",
            Self::PolicyIndeterminate { .. } => "policy_indeterminate",
            Self::Other(_) => "tool_error",
            Self::CallbackPending { .. } => "callback_pending",
        }
    }

    pub fn to_error_payload(&self) -> serde_json::Value {
        // Policy infrastructure details belong to its owner. The public/model
        // projection preserves the typed class without disclosing those facts.
        let indeterminate = matches!(self.primary_error(), Self::PolicyIndeterminate { .. });
        let mut payload = serde_json::json!({
            "error": self.error_code(),
            "message": if indeterminate { "operation policy unavailable".to_owned() } else { self.to_string() },
        });
        if !indeterminate && let Some(data) = self.structured_data() {
            payload["data"] = data;
        }
        let failures: Vec<_> = self.settlement_failures().collect();
        if !failures.is_empty() {
            payload["settlement_failures"] = serde_json::json!(failures);
        }
        payload
    }

    /// Render the canonical model-facing transcript text for this typed error.
    ///
    /// This is the single render boundary over the typed error: it projects
    /// [`Self::to_error_payload`] into the JSON string that appears in the
    /// conversation transcript. It is infallible — `serde_json::Value` always
    /// renders via its `Display` impl — so there is no fallback string that
    /// could mask a serialization fault.
    #[must_use]
    pub fn to_transcript_content(&self) -> String {
        self.to_error_payload().to_string()
    }

    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound { name: name.into() }
    }
    pub fn unavailable(name: impl Into<String>, reason: ToolUnavailableReason) -> Self {
        Self::Unavailable {
            name: name.into(),
            reason,
        }
    }
    pub fn invalid_arguments(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidArguments {
            name: name.into(),
            reason: reason.into(),
        }
    }
    pub fn execution_failed(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }
    pub fn execution_failed_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self::ExecutionFailedWithData {
            message: message.into(),
            data,
        }
    }
    pub fn structured_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::WithSettlementFailures { .. } => self.primary_error().structured_data(),
            Self::ExecutionFailedWithData { data, .. } => Some(data.clone()),
            Self::ConfinementRefused { refusal } => Some(serde_json::json!({"refusal": refusal})),
            Self::HookDenied { denial } => {
                let mut data = serde_json::json!({
                    "hook_id": denial.hook_id,
                    "point": denial.point,
                    "reason_code": denial.reason_code,
                });
                if let Some(payload) = &denial.payload {
                    data["payload"] = payload.clone();
                }
                Some(data)
            }
            Self::PolicyDenied { denial } => serde_json::to_value(denial).ok(),
            Self::PolicyIndeterminate { failure } => serde_json::to_value(failure).ok(),
            _ => None,
        }
    }
    pub fn timeout(name: impl Into<String>, timeout_ms: u64) -> Self {
        Self::Timeout {
            name: name.into(),
            timeout_ms,
        }
    }
    pub fn inactivity_timeout(name: impl Into<String>, inactivity_ms: u64) -> Self {
        Self::InactivityTimeout {
            name: name.into(),
            inactivity_ms,
        }
    }
    pub fn access_denied(name: impl Into<String>) -> Self {
        Self::AccessDenied { name: name.into() }
    }
    pub fn policy_denied(denial: crate::ToolConsequenceDenial) -> Self {
        Self::PolicyDenied { denial }
    }
    pub fn policy_indeterminate(failure: crate::ToolConsequenceFailure) -> Self {
        Self::PolicyIndeterminate { failure }
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// Create a callback pending error for external tool routing
    pub fn callback_pending(tool_name: impl Into<String>, args: serde_json::Value) -> Self {
        Self::CallbackPending {
            tool_name: tool_name.into(),
            args,
        }
    }

    /// Check if this is a callback pending error
    pub fn is_callback_pending(&self) -> bool {
        matches!(self.primary_error(), Self::CallbackPending { .. })
    }

    /// Extract callback pending info if this is a CallbackPending error
    pub fn as_callback_pending(&self) -> Option<(&str, &serde_json::Value)> {
        match self.primary_error() {
            Self::CallbackPending { tool_name, args } => Some((tool_name, args)),
            _ => None,
        }
    }
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}
impl From<&str> for ToolError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}

/// A declared tool restriction denies a tool that the build neither composed
/// nor finds in any tool vocabulary (a stale or mistyped name).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{declared_by} denies tool '{tool}', which is in none of its tool vocabularies \
     ({}; enabled families: {})",
    vocabulary.join(", "),
    enabled_families.join(", ")
)]
pub struct DeclaredToolUnknown {
    /// Who declared the restriction (for example the mob profile).
    pub declared_by: String,
    /// The denied name no vocabulary knows.
    pub tool: String,
    /// The tool families the declaring configuration enabled.
    pub enabled_families: Vec<String>,
    /// The vocabulary sources checked, by display name.
    pub vocabulary: Vec<String>,
}

/// Errors that can occur during agent execution
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AgentError {
    #[error("LLM error ({provider}): {message}")]
    Llm {
        provider: &'static str,
        reason: LlmFailureReason,
        message: String,
    },
    #[error("Storage error: {0}")]
    StoreError(String),
    /// A tool failure carrying the typed [`ToolError`] cause.
    ///
    /// The typed cause is preserved so the `access_denied` vs `not_found`
    /// vs `invalid_arguments` distinction survives to
    /// [`ToolError::error_code`] and the wire surface.
    #[error("Tool error: {error}")]
    Tool { error: ToolError },
    /// An operation-local authorization refusal. The agent loop returns safe
    /// feedback through its permitted controller instead of failing the run.
    #[error("{refusal}")]
    OperationRefused {
        refusal: crate::authorization::OperationRefused,
    },
    #[error("Tool consequence policy is indeterminate: {failure}")]
    PolicyIndeterminate {
        failure: Box<crate::ToolConsequenceFailure>,
        /// Admission settlement diagnostics accompanying the original policy failure.
        settlement_failures: Vec<crate::ops::ToolDispatchSettlementFailure>,
    },
    #[error("MCP error: {0}")]
    McpError(String),
    #[error("Session not found: {0}")]
    SessionNotFound(SessionId),
    #[error("Token budget exceeded: used {used}, limit {limit}")]
    TokenBudgetExceeded { used: u64, limit: u64 },
    #[error("Time budget exceeded: {elapsed_secs}s > {limit_secs}s")]
    TimeBudgetExceeded { elapsed_secs: u64, limit_secs: u64 },
    #[error("Tool call budget exceeded: {count} calls > {limit} limit")]
    ToolCallBudgetExceeded { count: usize, limit: usize },
    #[error("Max tokens reached on turn {turn}, partial output: {partial}")]
    MaxTokensReached { turn: u32, partial: String },
    #[error("Content filtered on turn {turn}")]
    ContentFiltered { turn: u32 },
    #[error("Max turns reached: {turns}")]
    MaxTurnsReached { turns: u32 },
    #[error("Run was cancelled")]
    Cancelled,
    #[error("Invalid state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },
    #[error("Operation not found: {0}")]
    OperationNotFound(String),
    #[error("Depth limit exceeded: {depth} > {max}")]
    DepthLimitExceeded { depth: u32, max: u32 },
    #[error("Concurrency limit exceeded")]
    ConcurrencyLimitExceeded,
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Invalid tool in access policy: {tool}")]
    InvalidToolAccess { tool: String },
    #[error("Skill resolution failed for {skill_key:?}: {reason}")]
    SkillResolutionFailed {
        skill_key: Option<crate::skills::SkillKey>,
        reason: Box<crate::event::SkillResolutionFailureReason>,
    },
    #[error("Internal error: {0}")]
    InternalError(String),

    /// A supervised sticky-fallback durability saga could not prove whether
    /// runtime session authority and generated routing authority converged.
    /// The live executor must be handed to canonical teardown; ordinary
    /// failed-batch retry could continue against split model identity.
    #[error("Sticky model fallback authority outcome is unknown: {message}")]
    StickyModelFallbackAuthorityUnknown { message: String },
    #[error("fallback-origin session requires explicit model reconfiguration: {target:?}")]
    ModelFallbackResumeHeld {
        target: Box<crate::model_fallback::ModelFallbackSkippedTarget>,
    },

    /// One live session projection advanced but its paired durable/session
    /// authority did not provably converge. Reusing the actor could duplicate
    /// or omit an already-observed fact, so the runtime must tear it down and
    /// reload from durable authority instead of entering failed-batch retry.
    #[error("Session durable projection authority outcome is unknown: {message}")]
    SessionDurableProjectionAuthorityUnknown { message: String },

    /// Agent construction failed (e.g. missing API key, unknown provider).
    #[error("Build error: {0}")]
    BuildError(String),

    /// Agent construction attempted to claim a session identity that is already live.
    #[error("Session identity already active: {0}")]
    SessionIdentityInUse(SessionId),

    /// A configuration's declared tool restriction denies a tool that the
    /// build neither composed nor finds in any tool vocabulary (a stale or
    /// mistyped name). Boxed to keep `AgentError` (and every `Result` carrying
    /// it) small.
    #[error(transparent)]
    DeclaredToolUnknown(Box<DeclaredToolUnknown>),

    /// MeerkatMachine DSL observed an auth lease in `reauth_required`
    /// state at a CallingLlm boundary; the lease cannot proceed
    /// until the user re-authenticates (`rkat auth login`). This is a
    /// machine-owned terminal class (Phase 1.5-rev), distinct from
    /// [`AgentError::InternalError`] which is for genuinely
    /// unexpected failures.
    #[error("Connection `{binding_key}` requires re-authentication: {message}")]
    AuthReauthRequired {
        binding_key: String,
        message: String,
    },

    /// A tool call must be routed externally (callback pending)
    #[error("Callback pending for tool '{tool_name}'")]
    CallbackPending {
        tool_use_id: String,
        tool_name: String,
        args: serde_json::Value,
    },

    /// Multiple callback tools from one assistant batch are pending together.
    ///
    /// The complete typed set is surfaced so callers can supply one exact
    /// result set; no callback is silently selected or dropped.
    #[error("Callback batch pending for {} tools", pending_tool_calls.len())]
    CallbackBatchPending {
        pending_tool_calls: Vec<PendingCallbackToolCall>,
    },

    /// Structured output validation failed after retries
    #[error("Structured output validation failed after {attempts} attempts: {reason}")]
    StructuredOutputValidationFailed {
        attempts: u32,
        reason: String,
        last_output: String,
    },

    /// Invalid output schema provided
    #[error("Invalid output schema: {0}")]
    InvalidOutputSchema(String),

    #[error("Hook '{hook_id}' denied at {point:?}: {reason_code:?} - {message}")]
    HookDenied {
        hook_id: HookId,
        point: HookPoint,
        reason_code: HookReasonCode,
        message: String,
        payload: Option<serde_json::Value>,
    },

    #[error("Hook '{hook_id}' timed out after {timeout_ms}ms")]
    HookTimeout { hook_id: HookId, timeout_ms: u64 },

    #[error("Hook execution failed for '{hook_id}': {reason}")]
    HookExecutionFailed { hook_id: HookId, reason: String },

    #[error("Hook configuration invalid: {reason}")]
    HookConfigInvalid { reason: String },

    /// Turn execution reached a terminal outcome classified as HardFailure.
    #[error("Terminal failure: {outcome:?} ({cause_kind:?}): {message}")]
    TerminalFailure {
        outcome: crate::turn_execution_authority::TurnTerminalOutcome,
        cause_kind: crate::turn_execution_authority::TurnTerminalCauseKind,
        message: String,
    },

    /// Generated pending-continuation authority found no resumable boundary.
    ///
    /// The caller should treat this as a successful no-op (no turn ran, no
    /// output produced).
    #[error("no pending boundary for resume")]
    NoPendingBoundary,

    /// An external control append (not the running turn's own) was refused
    /// because a callback tool batch awaits its results: appending now would
    /// detach the batch from the assistant tool-use tail it resolves. The
    /// session surface reports it as a retryable busy.
    #[error("control append refused while a callback tool batch awaits its results")]
    ControlAppendBlockedByCallbackBatch,

    /// The session agent does not support durable-snapshot synchronization
    /// (the default `SessionAgent::sync_session_from_durable_snapshot`
    /// capability). Consumers treat this as "skip live sync", distinct from a
    /// genuine [`AgentError::ConfigError`].
    #[error("durable session snapshot synchronization is not supported by this session agent")]
    DurableSnapshotSyncUnsupported,

    /// A hook prerequisite failed before target entry. Its calling operation
    /// owner decides local feedback; the cause is not an authorization denial.
    #[error("Hook launch refused for '{hook_id}': {reason}")]
    HookLaunchRefused {
        hook_id: HookId,
        reason: crate::hooks::HookFailureReason,
    },
}

impl From<crate::OperationAuthorizationError> for AgentError {
    fn from(error: crate::OperationAuthorizationError) -> Self {
        match error {
            crate::OperationAuthorizationError::Refused(refusal) => {
                Self::OperationRefused { refusal }
            }
            crate::OperationAuthorizationError::Unavailable => Self::authorization_unavailable(),
            crate::OperationAuthorizationError::ObservationUnavailable(_) => {
                Self::operation_observation_unavailable()
            }
        }
    }
}

impl From<crate::authorization::OperationObservationError> for AgentError {
    fn from(_: crate::authorization::OperationObservationError) -> Self {
        Self::operation_observation_unavailable()
    }
}

impl AgentError {
    pub fn authorization_unavailable() -> Self {
        Self::llm(
            "authorization",
            LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                LlmProviderErrorKind::OperationAuthorizationUnavailable,
                serde_json::Value::Null,
            )),
            "operation authorization unavailable",
        )
    }

    /// Typed local authority failure, never inferred from a provider message.
    pub fn operation_authorization_unavailable(&self) -> bool {
        matches!(self, Self::Llm { reason: LlmFailureReason::ProviderError(error), .. }
            if error.kind == LlmProviderErrorKind::OperationAuthorizationUnavailable)
    }

    pub fn operation_observation_unavailable() -> Self {
        Self::llm(
            "authorization",
            LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                LlmProviderErrorKind::OperationObservationUnavailable,
                serde_json::Value::Null,
            )),
            "operation observation unavailable",
        )
    }

    /// Recover a local refusal across the compatibility provider-error shape.
    /// Provider retryability and diagnostic text cannot turn it into a terminal
    /// or retryable provider failure. Missing typed details remain a refusal.
    pub fn operation_refusal(&self) -> Option<crate::OperationRefused> {
        match self {
            Self::OperationRefused { refusal } => Some(*refusal),
            Self::Llm {
                reason: LlmFailureReason::ProviderError(error),
                ..
            } if error.kind == LlmProviderErrorKind::OperationRefused => {
                let kind = error
                    .details
                    .get("kind")
                    .cloned()
                    .and_then(|kind| serde_json::from_value(kind).ok())
                    .unwrap_or(crate::OperationRefusalKind::MalformedFacts);
                Some(crate::OperationRefused::new(kind))
            }
            _ => None,
        }
    }

    /// Preserve the existing single-callback shape unless a typed settlement
    /// companion needs the complete pending-item carrier.
    pub fn callback_pending_with_settlement(call: PendingCallbackToolCall) -> Self {
        if call.settlement_failures.is_empty() {
            Self::CallbackPending {
                tool_use_id: call.tool_use_id,
                tool_name: call.tool_name,
                args: call.args,
            }
        } else {
            Self::CallbackBatchPending {
                pending_tool_calls: vec![call],
            }
        }
    }

    /// Wrap a typed [`ToolError`] as a terminal agent failure, preserving the
    /// typed cause (and thus its [`ToolError::error_code`]).
    pub fn tool(error: ToolError) -> Self {
        Self::Tool { error }
    }

    /// Stable error code for the typed tool cause, when this is the
    /// [`AgentError::Tool`] variant. Returns `None` for every other variant.
    ///
    /// This is the seam that lets `access_denied` / `not_found` /
    /// `invalid_arguments` survive to the wire instead of being flattened
    /// into an opaque message.
    pub fn tool_error_code(&self) -> Option<&'static str> {
        match self {
            Self::Tool { error } => Some(error.error_code()),
            _ => None,
        }
    }

    pub fn llm(
        provider: &'static str,
        reason: LlmFailureReason,
        message: impl Into<String>,
    ) -> Self {
        Self::Llm {
            provider,
            reason,
            message: message.into(),
        }
    }

    pub fn llm_empty_response(provider: &'static str) -> Self {
        Self::llm(
            provider,
            LlmFailureReason::ProviderError(LlmProviderError::retryable(
                LlmProviderErrorKind::IncompleteResponse,
                serde_json::json!({
                    "reason": "provider completed without user-visible text, images, or tool calls"
                }),
            )),
            "LLM completed without user-visible text, images, or tool calls",
        )
    }

    pub fn is_graceful(&self) -> bool {
        matches!(
            self,
            Self::TokenBudgetExceeded { .. }
                | Self::TimeBudgetExceeded { .. }
                | Self::ToolCallBudgetExceeded { .. }
                | Self::MaxTurnsReached { .. }
        )
    }
    pub fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            Self::Llm {
                reason: LlmFailureReason::RateLimited { .. },
                ..
            }
        )
    }

    pub fn retry_after_hint(&self) -> Option<std::time::Duration> {
        match self {
            Self::Llm {
                reason: LlmFailureReason::RateLimited { retry_after },
                ..
            } => *retry_after,
            _ => None,
        }
    }

    pub fn is_recoverable(&self) -> bool {
        match self {
            Self::Llm { reason, .. } => match reason {
                LlmFailureReason::RateLimited { .. } => true,
                LlmFailureReason::NetworkTimeout { .. } => true,
                LlmFailureReason::CallTimeout { .. } => true,
                LlmFailureReason::StreamStalled { .. } => true,
                LlmFailureReason::ProviderError(provider_error) => provider_error.is_retryable(),
                _ => false,
            },
            _ => false,
        }
    }

    /// Whether this error must survive surface/cleanup wrappers so the runtime
    /// can hand the live executor to canonical teardown.
    pub fn requires_session_teardown(&self) -> bool {
        matches!(
            self,
            Self::StickyModelFallbackAuthorityUnknown { .. }
                | Self::SessionDurableProjectionAuthorityUnknown { .. }
        )
    }

    /// Mint the general teardown-required failure for a session projection
    /// whose paired authority can no longer be proved coherent.
    pub fn session_durable_projection_authority_unknown(message: impl Into<String>) -> Self {
        Self::SessionDurableProjectionAuthorityUnknown {
            message: message.into(),
        }
    }

    /// Attach an ancillary cleanup failure without erasing a teardown-required
    /// primary cause. Non-teardown errors retain the historical combined
    /// internal-error shape.
    pub fn with_ancillary_failure(self, context: &str, failure: impl std::fmt::Display) -> Self {
        match self {
            Self::StickyModelFallbackAuthorityUnknown { message } => {
                Self::StickyModelFallbackAuthorityUnknown {
                    message: format!("{message}; additionally {context}: {failure}"),
                }
            }
            Self::SessionDurableProjectionAuthorityUnknown { message } => {
                Self::SessionDurableProjectionAuthorityUnknown {
                    message: format!("{message}; additionally {context}: {failure}"),
                }
            }
            other => Self::InternalError(format!("{other}; additionally {context}: {failure}")),
        }
    }
}

pub fn store_error(err: impl std::fmt::Display) -> AgentError {
    AgentError::StoreError(store_error_message(err))
}
pub fn invalid_session_id(err: impl std::fmt::Display) -> AgentError {
    AgentError::StoreError(invalid_session_id_message(err))
}
pub fn store_error_message(err: impl std::fmt::Display) -> String {
    err.to_string()
}
pub fn invalid_session_id_message(err: impl std::fmt::Display) -> String {
    format!("Invalid session ID: {err}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_network_timeout_is_recoverable() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::NetworkTimeout { duration_ms: 30000 },
            "network timeout after 30s",
        );
        assert!(err.is_recoverable());
    }

    #[test]
    fn ancillary_failure_preserves_teardown_required_variant() {
        let combined = AgentError::StickyModelFallbackAuthorityUnknown {
            message: "fallback CAS outcome unknown".to_string(),
        }
        .with_ancillary_failure("failed to clear overlay", "synthetic cleanup fault");

        assert!(combined.requires_session_teardown());
        assert!(matches!(
            combined,
            AgentError::StickyModelFallbackAuthorityUnknown { ref message }
                if message.contains("fallback CAS outcome unknown")
                    && message.contains("synthetic cleanup fault")
        ));
    }

    #[test]
    fn durable_projection_authority_unknown_requires_teardown_and_survives_cleanup() {
        let combined = AgentError::session_durable_projection_authority_unknown(
            "durable projection advanced before store commit",
        )
        .with_ancillary_failure("failed to clear overlay", "synthetic cleanup fault");

        assert!(combined.requires_session_teardown());
        assert!(matches!(
            combined,
            AgentError::SessionDurableProjectionAuthorityUnknown { ref message }
                if message.contains("projection advanced before store commit")
                    && message.contains("synthetic cleanup fault")
        ));
    }

    #[test]
    fn test_call_timeout_is_recoverable() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::CallTimeout { duration_ms: 45000 },
            "call timeout after 45s",
        );
        assert!(err.is_recoverable());
    }

    #[test]
    fn test_network_timeout_typed_mapping() {
        let reason = LlmFailureReason::NetworkTimeout { duration_ms: 5000 };
        match reason {
            LlmFailureReason::NetworkTimeout { duration_ms } => {
                assert_eq!(duration_ms, 5000);
            }
            _ => panic!("expected NetworkTimeout"),
        }
    }

    #[test]
    fn test_call_timeout_typed_mapping() {
        let reason = LlmFailureReason::CallTimeout { duration_ms: 60000 };
        match reason {
            LlmFailureReason::CallTimeout { duration_ms } => {
                assert_eq!(duration_ms, 60000);
            }
            _ => panic!("expected CallTimeout"),
        }
    }

    #[test]
    fn test_timeout_variants_are_distinct() {
        let net = LlmFailureReason::NetworkTimeout { duration_ms: 1000 };
        let call = LlmFailureReason::CallTimeout { duration_ms: 1000 };
        assert_ne!(net, call);
    }

    #[test]
    fn test_stream_stalled_is_recoverable() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::StreamStalled {
                inactivity_ms: 300_000,
            },
            "stream stalled after 300s of inactivity",
        );
        assert!(err.is_recoverable());
        assert!(!err.is_graceful());
    }

    #[test]
    fn test_stream_stalled_distinct_from_call_timeout() {
        let stalled = LlmFailureReason::StreamStalled {
            inactivity_ms: 1000,
        };
        let call = LlmFailureReason::CallTimeout { duration_ms: 1000 };
        assert_ne!(stalled, call);
    }

    #[test]
    fn test_auth_error_not_recoverable() {
        let err = AgentError::llm("anthropic", LlmFailureReason::AuthError, "bad key");
        assert!(!err.is_recoverable());
    }

    #[test]
    fn provider_error_uses_typed_retryability_for_recovery() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::ProviderError(LlmProviderError::retryable(
                LlmProviderErrorKind::ServerOverloaded,
                serde_json::json!({
                    "message": "provider overloaded"
                }),
            )),
            "provider overloaded",
        );

        assert!(err.is_recoverable());
    }

    #[test]
    fn provider_error_fails_closed_when_json_claims_retryable() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::ProviderError(LlmProviderError::non_retryable(
                LlmProviderErrorKind::InvalidRequest,
                serde_json::json!({
                    "kind": "server_overloaded",
                    "retryable": true,
                    "message": "json payload must not control retryability"
                }),
            )),
            "invalid request",
        );

        assert!(!err.is_recoverable());
    }

    // -- Rate-limit helper tests (PR #156 port) --

    #[test]
    fn test_is_rate_limited_true_for_rate_limit_error() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(30)),
            },
            "rate limited",
        );
        assert!(err.is_rate_limited());
    }

    #[test]
    fn test_is_rate_limited_false_for_other_errors() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::NetworkTimeout { duration_ms: 5000 },
            "timeout",
        );
        assert!(!err.is_rate_limited());

        let err = AgentError::llm("anthropic", LlmFailureReason::AuthError, "bad key");
        assert!(!err.is_rate_limited());
    }

    #[test]
    fn test_retry_after_hint_returns_duration_for_rate_limit() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(60)),
            },
            "rate limited",
        );
        assert_eq!(
            err.retry_after_hint(),
            Some(std::time::Duration::from_secs(60))
        );
    }

    #[test]
    fn test_retry_after_hint_returns_none_for_non_rate_limit() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::NetworkTimeout { duration_ms: 5000 },
            "timeout",
        );
        assert_eq!(err.retry_after_hint(), None);
    }

    #[test]
    fn test_timeout_variants_not_graceful() {
        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::NetworkTimeout { duration_ms: 1000 },
            "timeout",
        );
        assert!(!err.is_graceful());

        let err = AgentError::llm(
            "anthropic",
            LlmFailureReason::CallTimeout { duration_ms: 1000 },
            "timeout",
        );
        assert!(!err.is_graceful());
    }

    // -- P2-6: Typed BuildError variant --

    #[test]
    fn test_build_error_variant_exists_and_carries_message() {
        let err = AgentError::BuildError("Missing API key for provider 'anthropic'".to_string());
        match &err {
            AgentError::BuildError(msg) => {
                assert!(
                    msg.contains("API key"),
                    "message should contain source text"
                );
            }
            other => panic!("expected BuildError, got: {other}"),
        }
    }

    #[test]
    fn test_build_error_is_not_recoverable() {
        let err = AgentError::BuildError("Unknown provider for model 'llama-3'".to_string());
        assert!(!err.is_recoverable(), "build errors are not recoverable");
    }

    #[test]
    fn test_build_error_is_not_graceful() {
        let err = AgentError::BuildError("Missing API key".to_string());
        assert!(!err.is_graceful(), "build errors are not graceful");
    }

    #[test]
    fn test_build_error_display() {
        let err = AgentError::BuildError("Missing API key for provider 'anthropic'".to_string());
        let display = err.to_string();
        assert!(
            display.contains("Build error")
                || display.contains("build error")
                || display.contains("Missing API key"),
            "display should mention the build error: {display}"
        );
    }

    // -- P2-7: Typed TerminalFailure outcome --

    #[test]
    fn test_terminal_failure_carries_typed_outcome() {
        use crate::turn_execution_authority::{TurnTerminalCauseKind, TurnTerminalOutcome};

        // TerminalFailure must carry typed enums, not Debug-formatted strings.
        let err = AgentError::TerminalFailure {
            outcome: TurnTerminalOutcome::Failed,
            cause_kind: TurnTerminalCauseKind::LlmFailure,
            message: "llm failed".to_string(),
        };
        match &err {
            AgentError::TerminalFailure {
                outcome,
                cause_kind,
                ..
            } => {
                // If this compiles, outcome/cause_kind are typed enums, not Strings.
                assert_eq!(*outcome, TurnTerminalOutcome::Failed);
                assert_eq!(*cause_kind, TurnTerminalCauseKind::LlmFailure);
            }
            other => panic!("expected TerminalFailure, got: {other}"),
        }
    }

    #[test]
    fn test_terminal_failure_display_includes_outcome() {
        use crate::turn_execution_authority::{TurnTerminalCauseKind, TurnTerminalOutcome};

        let err = AgentError::TerminalFailure {
            outcome: TurnTerminalOutcome::TimeBudgetExceeded,
            cause_kind: TurnTerminalCauseKind::TimeBudgetExceeded,
            message: "deadline reached".to_string(),
        };
        let display = err.to_string();
        assert!(
            display.contains("TimeBudgetExceeded"),
            "display should include the outcome variant name: {display}"
        );
        assert!(
            display.contains("TimeBudgetExceeded") && display.contains("deadline reached"),
            "display should include cause and display message: {display}"
        );
    }

    // -- Rows 12 / 57: typed ToolError cause survives on AgentError --

    #[test]
    fn tool_variant_preserves_access_denied_error_code() {
        // Row 12: a hidden-tool denial must terminalize carrying the typed
        // ToolError so `access_denied` survives to error_code() (not flattened
        // into an opaque string).
        let err = AgentError::tool(ToolError::access_denied("secret_tool"));
        match &err {
            AgentError::Tool { error } => {
                assert_eq!(error.error_code(), "access_denied");
            }
            other => panic!("expected AgentError::Tool, got: {other}"),
        }
        assert_eq!(err.tool_error_code(), Some("access_denied"));
    }

    #[test]
    fn tool_variant_preserves_not_found_error_code() {
        // Row 12 sibling: not_found must remain distinct from access_denied.
        let err = AgentError::tool(ToolError::not_found("missing_tool"));
        assert_eq!(err.tool_error_code(), Some("tool_not_found"));
        assert_ne!(
            err.tool_error_code(),
            AgentError::tool(ToolError::access_denied("missing_tool")).tool_error_code(),
            "not_found must stay distinct from access_denied"
        );
    }

    #[test]
    fn tool_variant_preserves_invalid_arguments_error_code() {
        // Row 57: an invalid tool-call args projection failure must terminalize
        // carrying the typed invalid_arguments ToolError, not a flattened string.
        let err = AgentError::tool(ToolError::invalid_arguments(
            "search",
            "tool call arguments projection failed: bad json",
        ));
        match &err {
            AgentError::Tool { error } => {
                assert_eq!(error.error_code(), "invalid_arguments");
            }
            other => panic!("expected AgentError::Tool, got: {other}"),
        }
        assert_eq!(err.tool_error_code(), Some("invalid_arguments"));
    }

    #[test]
    fn test_terminal_failure_all_hard_failure_outcomes() {
        use crate::turn_execution_authority::{TurnTerminalCauseKind, TurnTerminalOutcome};

        // Both hard-failure outcomes should be representable.
        for outcome in [
            TurnTerminalOutcome::Failed,
            TurnTerminalOutcome::TimeBudgetExceeded,
        ] {
            let err = AgentError::TerminalFailure {
                outcome,
                cause_kind: TurnTerminalCauseKind::FatalFailure,
                message: "terminal".to_string(),
            };
            assert!(
                !err.is_graceful(),
                "TerminalFailure({outcome:?}) should not be graceful"
            );
        }
    }

    #[test]
    fn settlement_companion_keeps_primary_classification_and_callback_payload() {
        let marker = crate::ToolDispatchSettlementFailure {
            admission_source: crate::ToolDispatchAdmissionSource::ConfiguredGate,
            effect_kind: crate::LiveBridgeEffectKind::ToolDispatch,
            physical_outcome: crate::LiveBridgeEffectOutcome::Unknown,
            failure_kind: crate::ToolDispatchTerminalErrorKind::ExecutionFailed,
        };
        for primary in [
            ToolError::access_denied("tool"),
            ToolError::inactivity_timeout("tool", 10),
            ToolError::policy_indeterminate(crate::ToolConsequenceFailure::InvalidProvenance {
                reason: "primary-policy-failure".into(),
            }),
            ToolError::callback_pending("tool", serde_json::json!({"question":"answer"})),
        ] {
            let error = primary
                .clone()
                .with_settlement_failures(vec![marker.clone()]);
            assert_eq!(error.primary_error(), &primary);
            assert_eq!(error.error_code(), primary.error_code());
            assert_eq!(error.structured_data(), primary.structured_data());
            assert_eq!(error.is_callback_pending(), primary.is_callback_pending());
            assert_eq!(error.as_callback_pending(), primary.as_callback_pending());
            assert_eq!(
                crate::ToolDispatchTerminalErrorKind::from(&error),
                crate::ToolDispatchTerminalErrorKind::from(&primary)
            );
            let nested = ToolError::WithSettlementFailures {
                error: Box::new(error),
                failures: vec![marker.clone()],
            };
            assert_eq!(nested.settlement_failures().count(), 2);
            let (retained, failures) = nested.into_primary_and_settlement_failures();
            assert_eq!(retained, primary);
            assert_eq!(failures, vec![marker.clone(), marker.clone()]);
        }
        let call = PendingCallbackToolCall {
            tool_use_id: "callback".into(),
            tool_name: "tool".into(),
            args: serde_json::json!({}),
            settlement_failures: Vec::new(),
        };
        assert!(matches!(
            AgentError::callback_pending_with_settlement(call),
            AgentError::CallbackPending { .. }
        ));
    }

    #[test]
    fn policy_indeterminate_payload_is_safe_without_erasing_owner_details() {
        let error =
            ToolError::policy_indeterminate(crate::ToolConsequenceFailure::EvaluationFailed {
                reason: "private evaluator canary".into(),
            });
        let payload = error.to_error_payload();
        assert_eq!(payload["error"], "policy_indeterminate");
        assert_eq!(payload["message"], "operation policy unavailable");
        assert!(payload.get("data").is_none());
        assert!(!payload.to_string().contains("private evaluator canary"));
        assert!(
            !error
                .to_transcript_content()
                .contains("private evaluator canary")
        );
        assert!(
            error
                .structured_data()
                .expect("owner data retained")
                .to_string()
                .contains("private evaluator canary")
        );
        assert!(error.to_string().contains("private evaluator canary"));
        let ordinary = ToolError::execution_failed_with_data(
            "ordinary failure",
            serde_json::json!({"detail": "retained"}),
        );
        assert_eq!(
            ordinary.to_error_payload()["message"],
            "Tool execution failed: ordinary failure"
        );
        assert_eq!(
            ordinary.to_error_payload()["data"],
            serde_json::json!({"detail": "retained"})
        );
    }

    #[test]
    fn wrapped_policy_indeterminate_keeps_ordered_settlement_without_private_payload() {
        let first = crate::ToolDispatchSettlementFailure {
            admission_source: crate::ToolDispatchAdmissionSource::ConfiguredGate,
            effect_kind: crate::LiveBridgeEffectKind::ToolDispatch,
            physical_outcome: crate::LiveBridgeEffectOutcome::Unknown,
            failure_kind: crate::ToolDispatchTerminalErrorKind::Unavailable,
        };
        let second = crate::ToolDispatchSettlementFailure {
            admission_source: crate::ToolDispatchAdmissionSource::ContextGate,
            ..first
        };
        let primary =
            ToolError::policy_indeterminate(crate::ToolConsequenceFailure::InvalidProvenance {
                reason: "private wrapped evaluator canary".into(),
            });
        let inner = ToolError::WithSettlementFailures {
            error: Box::new(primary),
            failures: vec![first.clone()],
        };
        let error = ToolError::WithSettlementFailures {
            error: Box::new(inner),
            failures: vec![second.clone()],
        };
        let expected = vec![second, first];
        let payload = error.to_error_payload();
        assert_eq!(payload["error"], "policy_indeterminate");
        assert_eq!(payload["message"], "operation policy unavailable");
        assert!(payload.get("data").is_none());
        assert_eq!(
            payload["settlement_failures"],
            serde_json::to_value(&expected).unwrap()
        );
        assert!(
            !error
                .to_transcript_content()
                .contains("private wrapped evaluator canary")
        );
        let (retained, failures) = error.into_primary_and_settlement_failures();
        assert_eq!(failures, expected);
        assert!(matches!(retained, ToolError::PolicyIndeterminate {
            failure: crate::ToolConsequenceFailure::InvalidProvenance { reason }
        } if reason == "private wrapped evaluator canary"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod operation_unavailable_tests {
    use super::*;

    #[test]
    fn unavailable_is_distinct_safe_and_never_retryable() {
        let tool = ToolError::from(crate::OperationAuthorizationError::Unavailable);
        assert_eq!(
            tool.to_error_payload(),
            serde_json::json!({
                "error": "operation_authorization_unavailable",
                "message": "operation authorization unavailable",
            })
        );
        let cause = crate::ToolDispatchTerminalErrorKind::from(&tool);
        assert_eq!(
            cause,
            crate::ToolDispatchTerminalErrorKind::OperationAuthorizationUnavailable
        );
        assert_eq!(
            serde_json::from_value::<crate::ToolDispatchTerminalErrorKind>(
                serde_json::to_value(cause).unwrap()
            )
            .unwrap(),
            cause
        );
        let agent = AgentError::from(crate::OperationAuthorizationError::Unavailable);
        assert!(agent.operation_refusal().is_none());
        assert!(agent.operation_authorization_unavailable());
        for retryability in [
            LlmProviderErrorRetryability::Retryable,
            LlmProviderErrorRetryability::NonRetryable,
        ] {
            let provider: LlmProviderError = serde_json::from_value(serde_json::json!({
                "kind": "operation_authorization_unavailable", "retryability": retryability,
            }))
            .unwrap();
            assert!(!provider.is_retryable());
            let error = AgentError::llm(
                "fixture",
                LlmFailureReason::ProviderError(provider),
                "private canary",
            );
            assert!(error.operation_authorization_unavailable());
            assert!(crate::retry::LlmRetryFailure::from_agent_error(&error).is_none());
            assert!(crate::model_fallback::model_fallback_trigger(&error).is_none());
            let metadata = crate::TurnErrorMetadata::from_agent_error(&error).unwrap();
            assert_eq!(metadata.retryable, Some(false));
        }
        assert!(
            LlmProviderError::retryable(LlmProviderErrorKind::ServerError, serde_json::Value::Null)
                .is_retryable()
        );
    }
}

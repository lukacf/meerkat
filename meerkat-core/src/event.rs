//! Agent events for streaming output
//!
//! These events form the streaming API for consumers.

use crate::error::{
    AgentError, LlmFailureReason, LlmProviderErrorKind, LlmProviderErrorRetryability,
};
use crate::hooks::{HookId, HookPoint, HookReasonCode};
use crate::interaction::InteractionId;
use crate::ops_lifecycle::{OperationStatus, OperationTerminalOutcome};
use crate::retry::LlmRetrySchedule;
use crate::skills::{CapabilityId, SkillError, SkillKey};
use crate::time_compat::SystemTime;
use crate::turn_execution_authority::{TurnTerminalCauseKind, TurnTerminalOutcome};
use crate::types::{ContentBlock, ContentInput, SessionId, StopReason, Usage};
use serde::de::{self, DeserializeOwned};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use std::cmp::Ordering;
use std::fmt;

/// Canonical typed source identity for streamed agent events.
///
/// `source_id` on [`EventEnvelope`] is a compatibility/display projection.
/// Callers that need source semantics must read this typed identity instead of
/// parsing legacy strings such as `session:{uuid}`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventSourceIdentity {
    Session { session_id: SessionId },
    Runtime { runtime_id: String },
    Interaction { interaction_id: InteractionId },
    Callback,
    External { source_id: String },
}

impl EventSourceIdentity {
    #[must_use]
    pub fn session(session_id: SessionId) -> Self {
        Self::Session { session_id }
    }

    #[must_use]
    pub fn runtime(runtime_id: impl Into<String>) -> Self {
        Self::Runtime {
            runtime_id: runtime_id.into(),
        }
    }

    #[must_use]
    pub fn interaction(interaction_id: InteractionId) -> Self {
        Self::Interaction { interaction_id }
    }

    #[must_use]
    pub fn callback() -> Self {
        Self::Callback
    }

    #[must_use]
    pub fn external(source_id: impl Into<String>) -> Self {
        Self::External {
            source_id: source_id.into(),
        }
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&SessionId> {
        match self {
            Self::Session { session_id } => Some(session_id),
            Self::Runtime { .. }
            | Self::Interaction { .. }
            | Self::Callback
            | Self::External { .. } => None,
        }
    }

    #[must_use]
    pub fn legacy_source_id(&self) -> String {
        match self {
            Self::Session { session_id } => format!("session:{session_id}"),
            Self::Runtime { runtime_id } => format!("runtime:{runtime_id}"),
            Self::Interaction { interaction_id } => format!("interaction:{interaction_id}"),
            Self::Callback => "callback".to_string(),
            Self::External { source_id } => source_id.clone(),
        }
    }

    fn canonical_sort_key(&self) -> String {
        match self {
            Self::Session { session_id } => format!("session:{session_id}"),
            Self::Runtime { runtime_id } => format!("runtime:{runtime_id}"),
            Self::Interaction { interaction_id } => format!("interaction:{interaction_id}"),
            Self::Callback => "callback".to_string(),
            Self::External { source_id } => format!("external:{source_id}"),
        }
    }
}

/// Canonical event envelope for stream transport and ordering.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub event_id: uuid::Uuid,
    pub source: EventSourceIdentity,
    /// Legacy display/compat projection. Do not parse for source semantics.
    pub source_id: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<String>,
    pub timestamp_ms: u64,
    pub payload: T,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorClass {
    Llm,
    Store,
    Tool,
    Mcp,
    SessionNotFound,
    Budget,
    MaxTokens,
    ContentFiltered,
    MaxTurns,
    Cancelled,
    InvalidState,
    OperationNotFound,
    DepthLimit,
    ConcurrencyLimit,
    Config,
    Internal,
    Build,
    Auth,
    CallbackPending,
    StructuredOutput,
    InvalidOutputSchema,
    Hook,
    Terminal,
    NoPendingBoundary,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundJobTerminalStatus {
    Completed,
    Failed,
    Aborted,
    Cancelled,
    Retired,
    Terminated,
}

impl BackgroundJobTerminalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Cancelled => "cancelled",
            Self::Retired => "retired",
            Self::Terminated => "terminated",
        }
    }

    pub fn from_terminal_outcome(outcome: &OperationTerminalOutcome) -> Self {
        match outcome {
            OperationTerminalOutcome::Completed(_) => Self::Completed,
            OperationTerminalOutcome::Failed { .. } => Self::Failed,
            OperationTerminalOutcome::Aborted { .. } => Self::Aborted,
            OperationTerminalOutcome::Cancelled { .. } => Self::Cancelled,
            OperationTerminalOutcome::Retired => Self::Retired,
            OperationTerminalOutcome::Terminated { .. } => Self::Terminated,
        }
    }

    pub fn from_operation_status(status: OperationStatus) -> Option<Self> {
        match status {
            OperationStatus::Completed => Some(Self::Completed),
            OperationStatus::Failed => Some(Self::Failed),
            OperationStatus::Aborted => Some(Self::Aborted),
            OperationStatus::Cancelled => Some(Self::Cancelled),
            OperationStatus::Retired => Some(Self::Retired),
            OperationStatus::Terminated => Some(Self::Terminated),
            OperationStatus::Absent
            | OperationStatus::Provisioning
            | OperationStatus::Running
            | OperationStatus::Retiring => None,
        }
    }
}

fn deserialize_legacy_background_job_status<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| value.as_str().map(str::to_owned)))
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ToolCallArguments(
    #[cfg_attr(
        feature = "schema",
        schemars(with = "std::collections::BTreeMap<String, serde_json::Value>")
    )]
    Value,
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallArgumentsError {
    message: String,
}

impl ToolCallArgumentsError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ToolCallArgumentsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for ToolCallArgumentsError {}

impl ToolCallArguments {
    pub fn from_value(value: Value) -> Result<Self, ToolCallArgumentsError> {
        if value.is_object() {
            Ok(Self(value))
        } else {
            Err(ToolCallArgumentsError::new(format!(
                "tool call arguments must be a JSON object, got {}",
                value_kind(&value)
            )))
        }
    }

    pub fn from_raw_json(raw: &RawValue) -> Result<Self, ToolCallArgumentsError> {
        let value = serde_json::from_str(raw.get()).map_err(|error| {
            ToolCallArgumentsError::new(format!("tool call arguments must be valid JSON: {error}"))
        })?;
        Self::from_value(value)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

impl<'de> Deserialize<'de> for ToolCallArguments {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_value(Value::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

impl TryFrom<Value> for ToolCallArguments {
    type Error = ToolCallArgumentsError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::from_value(value)
    }
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason_type", rename_all = "snake_case")]
pub enum AgentErrorReason {
    LlmRateLimited {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_ms: Option<u64>,
    },
    LlmContextExceeded {
        max: u32,
        requested: u32,
    },
    LlmAuthError,
    LlmInvalidModel {
        model: String,
    },
    LlmProviderError {
        provider_error_kind: LlmProviderErrorKind,
        provider_error_retryability: LlmProviderErrorRetryability,
        provider_error: Value,
    },
    LlmNetworkTimeout {
        duration_ms: u64,
    },
    LlmCallTimeout {
        duration_ms: u64,
    },
    HookDenied {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hook_id: Option<HookId>,
        point: HookPoint,
        reason_code: HookReasonCode,
    },
    HookTimeout {
        hook_id: HookId,
        timeout_ms: u64,
    },
    HookExecutionFailed {
        hook_id: HookId,
        reason: String,
    },
    HookConfigInvalid {
        reason: String,
    },
    StructuredOutputValidationFailed {
        attempts: u32,
        reason: String,
    },
    InvalidOutputSchema {
        reason: String,
    },
    AuthReauthRequired {
        binding_key: String,
        message: String,
    },
    CallbackPending {
        tool_name: String,
        args: Value,
    },
    TurnTerminalCause {
        outcome: TurnTerminalOutcome,
        cause_kind: TurnTerminalCauseKind,
    },
}

impl AgentErrorReason {
    fn from_llm_reason(reason: &LlmFailureReason) -> Self {
        match reason {
            LlmFailureReason::RateLimited { retry_after } => Self::LlmRateLimited {
                retry_after_ms: retry_after
                    .as_ref()
                    .map(|duration| duration.as_millis() as u64),
            },
            LlmFailureReason::ContextExceeded { max, requested } => Self::LlmContextExceeded {
                max: *max,
                requested: *requested,
            },
            LlmFailureReason::AuthError => Self::LlmAuthError,
            LlmFailureReason::InvalidModel(model) => Self::LlmInvalidModel {
                model: model.clone(),
            },
            LlmFailureReason::ProviderError(provider_error) => Self::LlmProviderError {
                provider_error_kind: provider_error.kind,
                provider_error_retryability: provider_error.retryability,
                provider_error: provider_error.details.clone(),
            },
            LlmFailureReason::NetworkTimeout { duration_ms } => Self::LlmNetworkTimeout {
                duration_ms: *duration_ms,
            },
            LlmFailureReason::CallTimeout { duration_ms } => Self::LlmCallTimeout {
                duration_ms: *duration_ms,
            },
        }
    }

    pub fn from_agent_error(error: &AgentError) -> Option<Self> {
        match error {
            AgentError::Llm { reason, .. } => Some(Self::from_llm_reason(reason)),
            AgentError::HookDenied {
                hook_id,
                point,
                reason_code,
                ..
            } => Some(Self::HookDenied {
                hook_id: Some(hook_id.clone()),
                point: *point,
                reason_code: *reason_code,
            }),
            AgentError::HookTimeout {
                hook_id,
                timeout_ms,
            } => Some(Self::HookTimeout {
                hook_id: hook_id.clone(),
                timeout_ms: *timeout_ms,
            }),
            AgentError::HookExecutionFailed { hook_id, reason } => {
                Some(Self::HookExecutionFailed {
                    hook_id: hook_id.clone(),
                    reason: reason.clone(),
                })
            }
            AgentError::HookConfigInvalid { reason } => Some(Self::HookConfigInvalid {
                reason: reason.clone(),
            }),
            AgentError::StructuredOutputValidationFailed {
                attempts, reason, ..
            } => Some(Self::StructuredOutputValidationFailed {
                attempts: *attempts,
                reason: reason.clone(),
            }),
            AgentError::InvalidOutputSchema(reason) => Some(Self::InvalidOutputSchema {
                reason: reason.clone(),
            }),
            AgentError::AuthReauthRequired {
                binding_key,
                message,
            } => Some(Self::AuthReauthRequired {
                binding_key: binding_key.clone(),
                message: message.clone(),
            }),
            AgentError::CallbackPending { tool_name, args } => Some(Self::CallbackPending {
                tool_name: tool_name.clone(),
                args: args.clone(),
            }),
            AgentError::TerminalFailure {
                outcome,
                cause_kind,
                ..
            } => cause_kind
                .is_specific_failure_cause()
                .then_some(Self::TurnTerminalCause {
                    outcome: *outcome,
                    cause_kind: *cause_kind,
                }),
            _ => None,
        }
    }
}

impl From<&AgentError> for AgentErrorClass {
    fn from(error: &AgentError) -> Self {
        match error {
            AgentError::Llm { .. } => Self::Llm,
            AgentError::StoreError(_) => Self::Store,
            AgentError::ToolError(_) => Self::Tool,
            AgentError::McpError(_) => Self::Mcp,
            AgentError::SessionNotFound(_) => Self::SessionNotFound,
            AgentError::TokenBudgetExceeded { .. }
            | AgentError::TimeBudgetExceeded { .. }
            | AgentError::ToolCallBudgetExceeded { .. } => Self::Budget,
            AgentError::MaxTokensReached { .. } => Self::MaxTokens,
            AgentError::ContentFiltered { .. } => Self::ContentFiltered,
            AgentError::MaxTurnsReached { .. } => Self::MaxTurns,
            AgentError::Cancelled => Self::Cancelled,
            AgentError::InvalidStateTransition { .. } => Self::InvalidState,
            AgentError::OperationNotFound(_) => Self::OperationNotFound,
            AgentError::DepthLimitExceeded { .. } => Self::DepthLimit,
            AgentError::ConcurrencyLimitExceeded => Self::ConcurrencyLimit,
            AgentError::ConfigError(_) => Self::Config,
            AgentError::InvalidToolAccess { .. } => Self::Tool,
            AgentError::InternalError(_) => Self::Internal,
            AgentError::BuildError(_) => Self::Build,
            AgentError::AuthReauthRequired { .. } => Self::Auth,
            AgentError::CallbackPending { .. } => Self::CallbackPending,
            AgentError::StructuredOutputValidationFailed { .. } => Self::StructuredOutput,
            AgentError::InvalidOutputSchema(_) => Self::InvalidOutputSchema,
            AgentError::HookDenied { .. }
            | AgentError::HookTimeout { .. }
            | AgentError::HookExecutionFailed { .. }
            | AgentError::HookConfigInvalid { .. } => Self::Hook,
            AgentError::TerminalFailure { cause_kind, .. } => {
                if cause_kind.is_specific_failure_cause() {
                    cause_kind.agent_error_class()
                } else {
                    Self::Internal
                }
            }
            AgentError::NoPendingBoundary => Self::NoPendingBoundary,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentErrorReport {
    pub class: AgentErrorClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<AgentErrorReason>,
    pub message: String,
}

impl AgentErrorReport {
    pub fn from_agent_error(error: &AgentError) -> Self {
        Self {
            class: AgentErrorClass::from(error),
            reason: AgentErrorReason::from_agent_error(error),
            message: error.to_string(),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason_type", rename_all = "snake_case")]
pub enum SkillResolutionFailureReason {
    NotFound {
        key: SkillKey,
    },
    CapabilityUnavailable {
        key: SkillKey,
        capability: CapabilityId,
    },
    Load {
        message: String,
    },
    Parse {
        message: String,
    },
    SourceUuidCollision {
        source_uuid: String,
        existing_fingerprint: String,
        new_fingerprint: String,
    },
    SourceUuidMutationWithoutLineage {
        fingerprint: String,
        existing_source_uuid: String,
        mutated_source_uuid: String,
    },
    MissingSkillRemaps {
        event_id: String,
        event_kind: String,
    },
    RemapWithoutLineage {
        from_source_uuid: String,
        from_skill_name: String,
        to_source_uuid: String,
        to_skill_name: String,
    },
    UnknownSkillAlias {
        alias: String,
    },
    RemapCycle {
        source_uuid: String,
        skill_name: String,
    },
    Unknown {
        message: String,
    },
}

impl Default for SkillResolutionFailureReason {
    fn default() -> Self {
        Self::Unknown {
            message: String::new(),
        }
    }
}

fn deserialize_skill_resolution_field<T, E>(value: &Value, field: &'static str) -> Result<T, E>
where
    T: DeserializeOwned,
    E: de::Error,
{
    let field_value = value
        .get(field)
        .cloned()
        .ok_or_else(|| E::missing_field(field))?;
    serde_json::from_value(field_value).map_err(E::custom)
}

impl<'de> Deserialize<'de> for SkillResolutionFailureReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let reason_type = value
            .get("reason_type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        match reason_type {
            "not_found" => Ok(Self::NotFound {
                key: deserialize_skill_resolution_field(&value, "key")?,
            }),
            "capability_unavailable" => Ok(Self::CapabilityUnavailable {
                key: deserialize_skill_resolution_field(&value, "key")?,
                capability: deserialize_skill_resolution_field(&value, "capability")?,
            }),
            "load" => Ok(Self::Load {
                message: deserialize_skill_resolution_field(&value, "message")?,
            }),
            "parse" => Ok(Self::Parse {
                message: deserialize_skill_resolution_field(&value, "message")?,
            }),
            "source_uuid_collision" => Ok(Self::SourceUuidCollision {
                source_uuid: deserialize_skill_resolution_field(&value, "source_uuid")?,
                existing_fingerprint: deserialize_skill_resolution_field(
                    &value,
                    "existing_fingerprint",
                )?,
                new_fingerprint: deserialize_skill_resolution_field(&value, "new_fingerprint")?,
            }),
            "source_uuid_mutation_without_lineage" => Ok(Self::SourceUuidMutationWithoutLineage {
                fingerprint: deserialize_skill_resolution_field(&value, "fingerprint")?,
                existing_source_uuid: deserialize_skill_resolution_field(
                    &value,
                    "existing_source_uuid",
                )?,
                mutated_source_uuid: deserialize_skill_resolution_field(
                    &value,
                    "mutated_source_uuid",
                )?,
            }),
            "missing_skill_remaps" => Ok(Self::MissingSkillRemaps {
                event_id: deserialize_skill_resolution_field(&value, "event_id")?,
                event_kind: deserialize_skill_resolution_field(&value, "event_kind")?,
            }),
            "remap_without_lineage" => Ok(Self::RemapWithoutLineage {
                from_source_uuid: deserialize_skill_resolution_field(&value, "from_source_uuid")?,
                from_skill_name: deserialize_skill_resolution_field(&value, "from_skill_name")?,
                to_source_uuid: deserialize_skill_resolution_field(&value, "to_source_uuid")?,
                to_skill_name: deserialize_skill_resolution_field(&value, "to_skill_name")?,
            }),
            "unknown_skill_alias" => Ok(Self::UnknownSkillAlias {
                alias: deserialize_skill_resolution_field(&value, "alias")?,
            }),
            "remap_cycle" => Ok(Self::RemapCycle {
                source_uuid: deserialize_skill_resolution_field(&value, "source_uuid")?,
                skill_name: deserialize_skill_resolution_field(&value, "skill_name")?,
            }),
            "unknown" => Ok(Self::Unknown {
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            _ => Ok(Self::Unknown {
                message: value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
        }
    }
}

impl SkillResolutionFailureReason {
    pub fn from_skill_error(error: &SkillError) -> Self {
        match error {
            SkillError::NotFound { key } => Self::NotFound { key: key.clone() },
            SkillError::CapabilityUnavailable { key, capability } => Self::CapabilityUnavailable {
                key: key.clone(),
                capability: capability.clone(),
            },
            SkillError::Load(message) => Self::Load {
                message: message.to_string(),
            },
            SkillError::Parse(message) => Self::Parse {
                message: message.to_string(),
            },
            SkillError::SourceUuidCollision {
                source_uuid,
                existing_fingerprint,
                new_fingerprint,
            } => Self::SourceUuidCollision {
                source_uuid: source_uuid.clone(),
                existing_fingerprint: existing_fingerprint.clone(),
                new_fingerprint: new_fingerprint.clone(),
            },
            SkillError::SourceUuidMutationWithoutLineage {
                fingerprint,
                existing_source_uuid,
                mutated_source_uuid,
            } => Self::SourceUuidMutationWithoutLineage {
                fingerprint: fingerprint.clone(),
                existing_source_uuid: existing_source_uuid.clone(),
                mutated_source_uuid: mutated_source_uuid.clone(),
            },
            SkillError::MissingSkillRemaps {
                event_id,
                event_kind,
            } => Self::MissingSkillRemaps {
                event_id: event_id.clone(),
                event_kind: (*event_kind).to_string(),
            },
            SkillError::RemapWithoutLineage {
                from_source_uuid,
                from_skill_name,
                to_source_uuid,
                to_skill_name,
            } => Self::RemapWithoutLineage {
                from_source_uuid: from_source_uuid.clone(),
                from_skill_name: from_skill_name.clone(),
                to_source_uuid: to_source_uuid.clone(),
                to_skill_name: to_skill_name.clone(),
            },
            SkillError::UnknownSkillAlias { alias } => Self::UnknownSkillAlias {
                alias: alias.clone(),
            },
            SkillError::RemapCycle {
                source_uuid,
                skill_name,
            } => Self::RemapCycle {
                source_uuid: source_uuid.clone(),
                skill_name: skill_name.clone(),
            },
        }
    }
}

impl std::fmt::Display for SkillResolutionFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { key } => write!(f, "skill not found: {key}"),
            Self::CapabilityUnavailable { key, capability } => {
                write!(
                    f,
                    "skill '{key}' requires unavailable capability: {capability}"
                )
            }
            Self::Load { message } => write!(f, "skill loading failed: {message}"),
            Self::Parse { message } => write!(f, "skill parse failed: {message}"),
            Self::SourceUuidCollision {
                source_uuid,
                existing_fingerprint,
                new_fingerprint,
            } => write!(
                f,
                "source UUID collision for {source_uuid}: existing fingerprint '{existing_fingerprint}' conflicts with '{new_fingerprint}'"
            ),
            Self::SourceUuidMutationWithoutLineage {
                fingerprint,
                existing_source_uuid,
                mutated_source_uuid,
            } => write!(
                f,
                "source UUID mutation rejected for fingerprint '{fingerprint}': {existing_source_uuid} -> {mutated_source_uuid} without lineage"
            ),
            Self::MissingSkillRemaps {
                event_id,
                event_kind,
            } => write!(
                f,
                "lineage event '{event_id}' ({event_kind}) requires explicit per-skill remap entries"
            ),
            Self::RemapWithoutLineage {
                from_source_uuid,
                from_skill_name,
                to_source_uuid,
                to_skill_name,
            } => write!(
                f,
                "skill remap from {from_source_uuid}/{from_skill_name} to {to_source_uuid}/{to_skill_name} is not allowed by lineage"
            ),
            Self::UnknownSkillAlias { alias } => write!(f, "unknown skill alias '{alias}'"),
            Self::RemapCycle {
                source_uuid,
                skill_name,
            } => write!(
                f,
                "skill remap cycle detected for {source_uuid}/{skill_name}"
            ),
            Self::Unknown { message } if message.is_empty() => {
                f.write_str("unknown skill resolution failure")
            }
            Self::Unknown { message } => f.write_str(message),
        }
    }
}

impl From<&SkillError> for SkillResolutionFailureReason {
    fn from(error: &SkillError) -> Self {
        Self::from_skill_error(error)
    }
}

impl<T> EventEnvelope<T> {
    /// Create a new envelope with a UUIDv7 id and current wall-clock timestamp.
    pub fn new(source_id: impl Into<String>, seq: u64, mob_id: Option<String>, payload: T) -> Self {
        Self::new_with_source(
            EventSourceIdentity::external(source_id),
            seq,
            mob_id,
            payload,
        )
    }

    /// Create a new envelope from a typed source identity.
    pub fn new_with_source(
        source: EventSourceIdentity,
        seq: u64,
        mob_id: Option<String>,
        payload: T,
    ) -> Self {
        let timestamp_ms = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(duration) => duration.as_millis() as u64,
            Err(_) => u64::MAX,
        };
        let source_id = source.legacy_source_id();
        Self {
            event_id: crate::time_compat::new_uuid_v7(),
            source,
            source_id,
            seq,
            mob_id,
            timestamp_ms,
            payload,
        }
    }

    /// Create a new session-scoped envelope.
    pub fn new_session(
        session_id: SessionId,
        seq: u64,
        mob_id: Option<String>,
        payload: T,
    ) -> Self {
        Self::new_with_source(
            EventSourceIdentity::session(session_id),
            seq,
            mob_id,
            payload,
        )
    }

    /// Typed session source, when this event is session-scoped.
    #[must_use]
    pub fn source_session_id(&self) -> Option<&SessionId> {
        self.source.session_id()
    }
}

/// Canonical serialized event kind for SSE/RPC discriminators.
///
/// Intentionally exhaustive: when a new `AgentEvent` variant is added, this
/// match should fail to compile until that variant gets an explicit wire name.
pub fn agent_event_type(event: &AgentEvent) -> &'static str {
    match event {
        AgentEvent::RunStarted { .. } => "run_started",
        AgentEvent::RunCompleted { .. } => "run_completed",
        AgentEvent::RunFailed { .. } => "run_failed",
        AgentEvent::HookStarted { .. } => "hook_started",
        AgentEvent::HookCompleted { .. } => "hook_completed",
        AgentEvent::HookFailed { .. } => "hook_failed",
        AgentEvent::HookDenied { .. } => "hook_denied",
        AgentEvent::TurnStarted { .. } => "turn_started",
        AgentEvent::ReasoningDelta { .. } => "reasoning_delta",
        AgentEvent::ReasoningComplete { .. } => "reasoning_complete",
        AgentEvent::TextDelta { .. } => "text_delta",
        AgentEvent::TextComplete { .. } => "text_complete",
        AgentEvent::ToolCallRequested { .. } => "tool_call_requested",
        AgentEvent::ToolResultReceived { .. } => "tool_result_received",
        AgentEvent::TurnCompleted { .. } => "turn_completed",
        AgentEvent::ToolExecutionStarted { .. } => "tool_execution_started",
        AgentEvent::ToolExecutionCompleted { .. } => "tool_execution_completed",
        AgentEvent::ToolExecutionTimedOut { .. } => "tool_execution_timed_out",
        AgentEvent::CompactionStarted { .. } => "compaction_started",
        AgentEvent::CompactionCompleted { .. } => "compaction_completed",
        AgentEvent::CompactionFailed { .. } => "compaction_failed",
        AgentEvent::BudgetWarning { .. } => "budget_warning",
        AgentEvent::Retrying { .. } => "retrying",
        AgentEvent::SkillsResolved { .. } => "skills_resolved",
        AgentEvent::SkillResolutionFailed { .. } => "skill_resolution_failed",
        AgentEvent::InteractionComplete { .. } => "interaction_complete",
        AgentEvent::InteractionCallbackPending { .. } => "interaction_callback_pending",
        AgentEvent::InteractionFailed { .. } => "interaction_failed",
        AgentEvent::StreamTruncated { .. } => "stream_truncated",
        AgentEvent::ToolConfigChanged { .. } => "tool_config_changed",
        AgentEvent::BackgroundJobCompleted { .. } => "background_job_completed",
    }
}

/// Deterministic total ordering comparator for event envelopes.
pub fn compare_event_envelopes<T>(a: &EventEnvelope<T>, b: &EventEnvelope<T>) -> Ordering {
    a.timestamp_ms
        .cmp(&b.timestamp_ms)
        .then_with(|| {
            a.source
                .canonical_sort_key()
                .cmp(&b.source.canonical_sort_key())
        })
        .then_with(|| a.seq.cmp(&b.seq))
        .then_with(|| a.event_id.cmp(&b.event_id))
}

/// Payload for tool configuration change notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolConfigChangedPayload {
    pub operation: ToolConfigChangeOperation,
    pub target: String,
    status_info: ToolConfigChangeStatus,
    pub persisted: bool,
    pub applied_at_turn: Option<u32>,
    pub domain: Option<ToolConfigChangeDomain>,
    pub deferred_catalog_delta: Option<DeferredCatalogDelta>,
}

impl ToolConfigChangedPayload {
    #[must_use]
    pub fn new(
        operation: ToolConfigChangeOperation,
        target: impl Into<String>,
        status_info: ToolConfigChangeStatus,
        persisted: bool,
    ) -> Self {
        Self {
            operation,
            target: target.into(),
            status_info,
            persisted,
            applied_at_turn: None,
            domain: None,
            deferred_catalog_delta: None,
        }
    }

    #[must_use]
    pub fn status_info(&self) -> &ToolConfigChangeStatus {
        &self.status_info
    }

    #[must_use]
    pub fn status_text(&self) -> String {
        self.status_info.status_text()
    }

    #[must_use]
    pub fn with_applied_at_turn(mut self, applied_at_turn: Option<u32>) -> Self {
        self.applied_at_turn = applied_at_turn;
        self
    }

    #[must_use]
    pub fn with_domain(mut self, domain: Option<ToolConfigChangeDomain>) -> Self {
        self.domain = domain;
        self
    }

    #[must_use]
    pub fn with_deferred_catalog_delta(
        mut self,
        deferred_catalog_delta: Option<DeferredCatalogDelta>,
    ) -> Self {
        self.deferred_catalog_delta = deferred_catalog_delta;
        self
    }
}

impl Serialize for ToolConfigChangedPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("ToolConfigChangedPayload", 8)?;
        state.serialize_field("operation", &self.operation)?;
        state.serialize_field("target", &self.target)?;
        state.serialize_field("status", &self.status_text())?;
        state.serialize_field("status_info", &self.status_info)?;
        state.serialize_field("persisted", &self.persisted)?;
        if let Some(applied_at_turn) = self.applied_at_turn {
            state.serialize_field("applied_at_turn", &applied_at_turn)?;
        }
        if let Some(domain) = &self.domain {
            state.serialize_field("domain", domain)?;
        }
        if let Some(delta) = &self.deferred_catalog_delta {
            state.serialize_field("deferred_catalog_delta", delta)?;
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for ToolConfigChangedPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WirePayload {
            operation: ToolConfigChangeOperation,
            target: String,
            #[serde(default)]
            status: Option<String>,
            #[serde(default)]
            status_info: Option<ToolConfigChangeStatus>,
            persisted: bool,
            #[serde(default)]
            applied_at_turn: Option<u32>,
            #[serde(default)]
            domain: Option<ToolConfigChangeDomain>,
            #[serde(default)]
            deferred_catalog_delta: Option<DeferredCatalogDelta>,
        }

        let wire = WirePayload::deserialize(deserializer)?;
        let status_info = wire
            .status_info
            .or_else(|| wire.status.map(ToolConfigChangeStatus::legacy_status))
            .ok_or_else(|| de::Error::missing_field("status_info"))?;

        Ok(Self {
            operation: wire.operation,
            target: wire.target,
            status_info,
            persisted: wire.persisted,
            applied_at_turn: wire.applied_at_turn,
            domain: wire.domain,
            deferred_catalog_delta: wire.deferred_catalog_delta,
        })
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for ToolConfigChangedPayload {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ToolConfigChangedPayload".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        /// Payload for tool configuration change notifications.
        #[allow(dead_code)]
        #[derive(schemars::JsonSchema)]
        struct ToolConfigChangedPayloadSchema {
            operation: ToolConfigChangeOperation,
            target: String,
            status: String,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            status_info: Option<ToolConfigChangeStatus>,
            persisted: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            applied_at_turn: Option<u32>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            domain: Option<ToolConfigChangeDomain>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            deferred_catalog_delta: Option<DeferredCatalogDelta>,
        }

        ToolConfigChangedPayloadSchema::json_schema(generator)
    }
}

/// Optional typed domain for tool-configuration change payloads.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolConfigChangeDomain {
    ToolScope,
    DeferredCatalog,
}

/// Additive hidden-catalog delta metadata for runtime notices.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeferredCatalogDelta {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_hidden_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_hidden_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_sources: Vec<String>,
}

/// Operation kind for live tool configuration changes.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolConfigChangeOperation {
    Add,
    Remove,
    Reload,
}

/// Canonical lifecycle phase for external-tool boundary deltas.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ExternalToolDeltaPhase {
    Pending,
    Applied,
    Draining,
    Forced,
    Failed,
}

impl ExternalToolDeltaPhase {
    #[must_use]
    pub fn as_status(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Draining => "draining",
            Self::Forced => "forced",
            Self::Failed => "failed",
        }
    }
}

/// Structured status data for live tool configuration change notifications.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolConfigChangeStatus {
    BoundaryApplied {
        base_changed: bool,
        visible_changed: bool,
        revision: u64,
    },
    DeferredCatalogDelta {
        added_hidden_count: usize,
        removed_hidden_count: usize,
        pending_source_count: usize,
    },
    WarningFailedClosed {
        error: String,
    },
    ExternalToolDelta {
        phase: ExternalToolDeltaPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    LegacyStatus {
        status: String,
    },
}

impl ToolConfigChangeStatus {
    #[must_use]
    pub fn boundary_applied(base_changed: bool, visible_changed: bool, revision: u64) -> Self {
        Self::BoundaryApplied {
            base_changed,
            visible_changed,
            revision,
        }
    }

    #[must_use]
    pub fn deferred_catalog_delta(
        added_hidden_count: usize,
        removed_hidden_count: usize,
        pending_source_count: usize,
    ) -> Self {
        Self::DeferredCatalogDelta {
            added_hidden_count,
            removed_hidden_count,
            pending_source_count,
        }
    }

    #[must_use]
    pub fn warning_failed_closed(error: impl Into<String>) -> Self {
        Self::WarningFailedClosed {
            error: error.into(),
        }
    }

    #[must_use]
    pub fn external_tool_delta(phase: ExternalToolDeltaPhase, detail: Option<String>) -> Self {
        Self::ExternalToolDelta { phase, detail }
    }

    #[must_use]
    pub fn legacy_status(status: impl Into<String>) -> Self {
        Self::LegacyStatus {
            status: status.into(),
        }
    }

    #[must_use]
    pub fn status_text(&self) -> String {
        match self {
            Self::BoundaryApplied {
                base_changed,
                visible_changed,
                revision,
            } => format!(
                "boundary_applied(base_changed={base_changed},visible_changed={visible_changed},revision={revision})"
            ),
            Self::DeferredCatalogDelta {
                added_hidden_count,
                removed_hidden_count,
                pending_source_count,
            } => format!(
                "deferred_catalog_delta(added_hidden={added_hidden_count},removed_hidden={removed_hidden_count},pending_sources={pending_source_count})"
            ),
            Self::WarningFailedClosed { error } => {
                format!("warning_failed_closed({error})")
            }
            Self::ExternalToolDelta { phase, detail } => {
                let mut status = phase.as_status().to_string();
                if *phase == ExternalToolDeltaPhase::Failed
                    && let Some(detail) = detail
                {
                    status = format!("{status}: {detail}");
                }
                status
            }
            Self::LegacyStatus { status } => status.clone(),
        }
    }
}

/// Canonical outward lifecycle delta for external-tool surface changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalToolDelta {
    pub target: String,
    pub operation: ToolConfigChangeOperation,
    pub phase: ExternalToolDeltaPhase,
    pub persisted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_at_turn: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ExternalToolDelta {
    #[must_use]
    pub fn new(
        target: impl Into<String>,
        operation: ToolConfigChangeOperation,
        phase: ExternalToolDeltaPhase,
    ) -> Self {
        Self {
            target: target.into(),
            operation,
            phase,
            persisted: !matches!(
                phase,
                ExternalToolDeltaPhase::Pending | ExternalToolDeltaPhase::Draining
            ),
            applied_at_turn: None,
            tool_count: None,
            detail: None,
        }
    }

    #[must_use]
    pub fn with_tool_count(mut self, tool_count: Option<usize>) -> Self {
        self.tool_count = tool_count;
        self
    }

    #[must_use]
    pub fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }

    #[must_use]
    pub fn status_text(&self) -> String {
        ToolConfigChangeStatus::external_tool_delta(self.phase, self.detail.clone()).status_text()
    }

    #[must_use]
    pub fn to_tool_config_changed_payload(&self) -> ToolConfigChangedPayload {
        let status_info =
            ToolConfigChangeStatus::external_tool_delta(self.phase, self.detail.clone());
        ToolConfigChangedPayload::new(
            self.operation.clone(),
            self.target.clone(),
            status_info,
            self.persisted,
        )
        .with_applied_at_turn(self.applied_at_turn)
    }
}

/// Events emitted during agent execution
///
/// These events form the streaming API for consumers.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AgentEvent {
    // === Session Lifecycle ===
    /// Agent run started
    RunStarted {
        session_id: SessionId,
        prompt: ContentInput,
    },

    /// Agent run completed successfully
    RunCompleted {
        session_id: SessionId,
        result: String,
        /// Structured output from the completed run, when schema extraction
        /// produced a typed value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured_output: Option<Value>,
        usage: Usage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_cause_kind: Option<TurnTerminalCauseKind>,
    },

    /// Agent run failed
    RunFailed {
        session_id: SessionId,
        error_class: AgentErrorClass,
        /// Display projection of `error_report.message`.
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal_cause_kind: Option<TurnTerminalCauseKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_report: Option<AgentErrorReport>,
    },

    // === Hook Lifecycle ===
    /// Hook invocation started.
    HookStarted { hook_id: HookId, point: HookPoint },

    /// Hook invocation completed.
    HookCompleted {
        hook_id: HookId,
        point: HookPoint,
        duration_ms: u64,
    },

    /// Hook invocation failed.
    HookFailed {
        hook_id: HookId,
        point: HookPoint,
        error: String,
    },

    /// Hook denied an action.
    HookDenied {
        hook_id: HookId,
        point: HookPoint,
        reason_code: HookReasonCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },

    // === LLM Interaction ===
    /// New turn started (calling LLM)
    TurnStarted { turn_number: u32 },

    /// Streaming reasoning/thinking from the model
    ReasoningDelta { delta: String },

    /// Reasoning/thinking complete for this block
    ReasoningComplete { content: String },

    /// Streaming text from the model
    TextDelta { delta: String },

    /// Text generation complete for this turn
    TextComplete { content: String },

    /// Model requested a tool call
    ToolCallRequested {
        id: String,
        name: String,
        args: ToolCallArguments,
    },

    /// Tool result received (injected into conversation)
    ToolResultReceived {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
        is_error: bool,
    },

    /// Turn completed
    TurnCompleted {
        stop_reason: StopReason,
        usage: Usage,
    },

    // === Tool Execution ===
    /// Starting tool execution
    ToolExecutionStarted { id: String, name: String },

    /// Tool execution completed
    ToolExecutionCompleted {
        id: String,
        name: String,
        /// Legacy text projection retained for existing event consumers.
        result: String,
        /// Canonical typed tool-result content.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
        is_error: bool,
        duration_ms: u64,
    },

    /// Tool execution timed out
    ToolExecutionTimedOut {
        id: String,
        name: String,
        timeout_ms: u64,
    },

    // === Compaction ===
    /// Context compaction started.
    CompactionStarted {
        /// Input tokens from the last LLM call that triggered compaction.
        input_tokens: u64,
        /// Estimated total history tokens before compaction.
        estimated_history_tokens: u64,
        /// Number of messages before compaction.
        message_count: usize,
    },

    /// Context compaction completed successfully.
    CompactionCompleted {
        /// Tokens consumed by the summary.
        summary_tokens: u64,
        /// Messages before compaction.
        messages_before: usize,
        /// Messages after compaction.
        messages_after: usize,
    },

    /// Context compaction failed (non-fatal — agent continues with uncompacted history).
    CompactionFailed { error: String },

    // === Budget ===
    /// Budget warning (approaching limits)
    BudgetWarning {
        budget_type: BudgetType,
        used: u64,
        limit: u64,
        percent: f32,
    },

    // === Retry Events ===
    /// Retrying after error
    Retrying {
        attempt: u32,
        max_attempts: u32,
        error: String,
        delay_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry: Option<LlmRetrySchedule>,
    },

    // === Skill Events ===
    /// Skills resolved for this turn.
    SkillsResolved {
        skills: Vec<SkillKey>,
        injection_bytes: usize,
    },

    /// A skill reference could not be resolved.
    SkillResolutionFailed {
        /// Canonical structured skill identity/reference, when resolution had one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skill_key: Option<SkillKey>,
        /// Structured reason for the failure. Legacy payloads deserialize as `unknown`.
        #[serde(default)]
        reason: SkillResolutionFailureReason,
        /// Legacy display mirror for consumers still reading string references.
        #[serde(default)]
        reference: String,
        /// Legacy display mirror for consumers still reading string errors.
        #[serde(default)]
        error: String,
    },

    // === Interaction-Scoped Streaming ===
    /// An interaction completed successfully (terminal event for tap subscribers).
    InteractionComplete {
        interaction_id: crate::interaction::InteractionId,
        result: String,
        /// Structured output from the completed interaction, when schema
        /// extraction produced a typed value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured_output: Option<Value>,
    },

    /// An interaction reached an external callback boundary and is waiting for
    /// tool results before the session can continue.
    InteractionCallbackPending {
        interaction_id: crate::interaction::InteractionId,
        tool_name: String,
        args: Value,
    },

    /// An interaction failed (terminal event for tap subscribers).
    InteractionFailed {
        interaction_id: crate::interaction::InteractionId,
        error: String,
    },

    /// Some streaming events were dropped due to channel backpressure.
    /// Best-effort marker — the terminal event is authoritative.
    StreamTruncated { reason: String },

    /// Live tool configuration changed for this session.
    ToolConfigChanged { payload: ToolConfigChangedPayload },

    /// A background shell job completed (or failed/cancelled/timed out).
    BackgroundJobCompleted {
        job_id: String,
        display_name: String,
        /// Legacy display mirror for consumers still rendering string status.
        #[serde(rename = "status")]
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_legacy_background_job_status"
        )]
        legacy_status: Option<String>,
        terminal_status: BackgroundJobTerminalStatus,
        detail: String,
    },
}

impl AgentEvent {
    pub fn background_job_completed(
        job_id: impl Into<String>,
        display_name: impl Into<String>,
        terminal_status: BackgroundJobTerminalStatus,
        detail: impl Into<String>,
    ) -> Self {
        Self::BackgroundJobCompleted {
            job_id: job_id.into(),
            display_name: display_name.into(),
            legacy_status: Some(terminal_status.as_str().to_string()),
            terminal_status,
            detail: detail.into(),
        }
    }
}

/// Scope attribution frame for multi-agent streaming.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "scope", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StreamScopeFrame {
    /// Top-level primary session scope.
    Primary { session_id: String },
    /// Mob member scope for flow dispatch turns.
    MobMember {
        flow_run_id: String,
        agent_identity: String,
        #[cfg_attr(feature = "schema", schemars(skip))]
        #[serde(default, skip_serializing)]
        agent_runtime_id: Option<String>,
        #[cfg_attr(feature = "schema", schemars(skip))]
        #[serde(default, skip_serializing)]
        fence_token: Option<u64>,
        #[cfg_attr(feature = "schema", schemars(skip))]
        #[serde(default, skip_serializing)]
        generation: Option<u64>,
    },
}

/// Attributed stream event wrapper for multi-agent streaming.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedAgentEvent {
    pub scope_id: String,
    pub scope_path: Vec<StreamScopeFrame>,
    pub event: AgentEvent,
}

impl ScopedAgentEvent {
    /// Build a scoped event from a scope path and payload event.
    pub fn new(scope_path: Vec<StreamScopeFrame>, event: AgentEvent) -> Self {
        let scope_id = Self::scope_id_from_path(&scope_path);
        Self {
            scope_id,
            scope_path,
            event,
        }
    }

    /// Build a primary-scoped event for a top-level session event.
    pub fn primary(session_id: impl Into<String>, event: AgentEvent) -> Self {
        Self::new(
            vec![StreamScopeFrame::Primary {
                session_id: session_id.into(),
            }],
            event,
        )
    }

    /// Convenience alias for converting a legacy event into primary scope.
    pub fn from_agent_event_primary(session_id: impl Into<String>, event: AgentEvent) -> Self {
        Self::primary(session_id, event)
    }

    /// Append one scope frame and recompute scope_id deterministically.
    pub fn append_scope(mut self, frame: StreamScopeFrame) -> Self {
        self.scope_path.push(frame);
        self.scope_id = Self::scope_id_from_path(&self.scope_path);
        self
    }

    /// Deterministic canonical selector from scope path.
    ///
    /// Formats:
    /// - `primary`
    /// - `mob:<agent_identity>`
    pub fn scope_id_from_path(path: &[StreamScopeFrame]) -> String {
        if path.is_empty() {
            return "primary".to_string();
        }
        let mut segments: Vec<String> = Vec::with_capacity(path.len());
        for frame in path {
            match frame {
                StreamScopeFrame::Primary { .. } => segments.push("primary".to_string()),
                StreamScopeFrame::MobMember { agent_identity, .. } => {
                    segments.push(format!("mob:{agent_identity}"));
                }
            }
        }
        segments.join("/")
    }
}

/// Type of budget being tracked
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetType {
    Tokens,
    Time,
    ToolCalls,
}

/// Configuration for formatting verbose event output.
#[derive(Debug, Clone, Copy)]
pub struct VerboseEventConfig {
    pub max_tool_args_bytes: usize,
    pub max_tool_result_bytes: usize,
    pub max_text_bytes: usize,
}

impl Default for VerboseEventConfig {
    fn default() -> Self {
        Self {
            max_tool_args_bytes: 100,
            max_tool_result_bytes: 200,
            max_text_bytes: 500,
        }
    }
}

/// Format an agent event using default verbose formatting rules.
pub fn format_verbose_event(event: &AgentEvent) -> Option<String> {
    format_verbose_event_with_config(event, &VerboseEventConfig::default())
}

/// Format an agent event using custom verbose formatting rules.
pub fn format_verbose_event_with_config(
    event: &AgentEvent,
    config: &VerboseEventConfig,
) -> Option<String> {
    match event {
        AgentEvent::TurnStarted { turn_number } => {
            Some(format!("\n━━━ Turn {} ━━━", turn_number + 1))
        }
        AgentEvent::ToolCallRequested { name, args, .. } => {
            let args_str = serde_json::to_string(args).unwrap_or_default();
            let args_preview = truncate_preview(&args_str, config.max_tool_args_bytes);
            Some(format!("  → Calling tool: {name} {args_preview}"))
        }
        AgentEvent::ToolExecutionCompleted {
            name,
            result,
            is_error,
            duration_ms,
            ..
        } => {
            let status = if *is_error { "✗" } else { "✓" };
            let result_preview = truncate_preview(result, config.max_tool_result_bytes);
            Some(format!(
                "  {status} {name} ({duration_ms}ms): {result_preview}"
            ))
        }
        AgentEvent::TurnCompleted { stop_reason, usage } => Some(format!(
            "  ── Turn complete: {:?} ({} in / {} out tokens)",
            stop_reason, usage.input_tokens, usage.output_tokens
        )),
        AgentEvent::TextComplete { content } => {
            if content.is_empty() {
                None
            } else {
                let preview = truncate_preview(content, config.max_text_bytes);
                Some(format!("  💬 Response: {preview}"))
            }
        }
        AgentEvent::ReasoningComplete { content } => {
            if content.is_empty() {
                None
            } else {
                let preview = truncate_preview(content, config.max_text_bytes);
                Some(format!("  💭 Thinking: {preview}"))
            }
        }
        AgentEvent::Retrying {
            attempt,
            max_attempts,
            error,
            delay_ms,
            ..
        } => Some(format!(
            "  ⟳ Retry {attempt}/{max_attempts}: {error} (waiting {delay_ms}ms)"
        )),
        AgentEvent::BudgetWarning {
            budget_type,
            used,
            limit,
            percent,
        } => Some(format!(
            "  ⚠ Budget warning: {:?} at {:.0}% ({}/{})",
            budget_type,
            percent * 100.0,
            used,
            limit
        )),
        AgentEvent::CompactionStarted {
            input_tokens,
            estimated_history_tokens,
            message_count,
        } => Some(format!(
            "  ⟳ Compaction started: {input_tokens} input tokens, ~{estimated_history_tokens} history tokens, {message_count} messages"
        )),
        AgentEvent::CompactionCompleted {
            summary_tokens,
            messages_before,
            messages_after,
        } => Some(format!(
            "  ✓ Compaction complete: {messages_before} → {messages_after} messages, {summary_tokens} summary tokens"
        )),
        AgentEvent::CompactionFailed { error } => {
            Some(format!("  ✗ Compaction failed (continuing): {error}"))
        }
        AgentEvent::BackgroundJobCompleted {
            job_id,
            display_name,
            terminal_status,
            detail,
            ..
        } => {
            let status = terminal_status.as_str();
            Some(format!(
                "  BG job {job_id} ({display_name}) {status}: {detail}"
            ))
        }
        AgentEvent::InteractionCallbackPending {
            tool_name, args, ..
        } => Some(format!(
            "  ⧖ Callback pending: {tool_name} {}",
            truncate_preview(&args.to_string(), config.max_tool_args_bytes)
        )),
        _ => None,
    }
}

fn truncate_preview(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    format!("{}...", truncate_str(input, max_bytes))
}

fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let truncate_at = s
        .char_indices()
        .take_while(|(i, _)| *i < max_bytes)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    &s[..truncate_at]
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::retry::{LlmRetryFailure, LlmRetryFailureKind, LlmRetryPlan, LlmRetrySchedule};
    use crate::skills::SkillName;
    use crate::types::ContentBlock;

    fn text_block(text: &str) -> ContentBlock {
        ContentBlock::Text {
            text: text.to_string(),
        }
    }

    fn image_block(media_type: &str, data: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: media_type.to_string(),
            data: data.into(),
        }
    }

    fn tool_args(value: Value) -> ToolCallArguments {
        ToolCallArguments::from_value(value).expect("test tool args must be an object")
    }

    #[test]
    fn tool_call_arguments_reject_string_projection() {
        let err = ToolCallArguments::from_value(serde_json::json!("{\"path\":"))
            .expect_err("provider argument strings must not become semantic tool-call args");

        assert!(
            err.to_string().contains("JSON object, got string"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn tool_call_requested_rejects_string_args_on_deserialize() {
        let value = serde_json::json!({
            "type": "tool_call_requested",
            "id": "tc_1",
            "name": "search",
            "args": "{\"query\":"
        });

        let err = serde_json::from_value::<AgentEvent>(value)
            .expect_err("event surface must reject string-success tool args");
        assert!(
            err.to_string().contains("JSON object, got string"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn tool_config_change_status_mirrors_legacy_status_text() {
        assert_eq!(
            ToolConfigChangeStatus::boundary_applied(true, false, 7).status_text(),
            "boundary_applied(base_changed=true,visible_changed=false,revision=7)"
        );
        assert_eq!(
            ToolConfigChangeStatus::deferred_catalog_delta(2, 1, 3).status_text(),
            "deferred_catalog_delta(added_hidden=2,removed_hidden=1,pending_sources=3)"
        );
        assert_eq!(
            ToolConfigChangeStatus::warning_failed_closed("injected failure").status_text(),
            "warning_failed_closed(injected failure)"
        );
        assert_eq!(
            ToolConfigChangeStatus::external_tool_delta(
                ExternalToolDeltaPhase::Failed,
                Some("exit 1".to_string()),
            )
            .status_text(),
            "failed: exit 1"
        );
    }

    #[test]
    fn tool_result_events_carry_text_only_content_blocks() {
        let content = vec![text_block("plain output")];
        let completed = AgentEvent::ToolExecutionCompleted {
            id: "tc_text".to_string(),
            name: "text_tool".to_string(),
            result: "plain output".to_string(),
            content: content.clone(),
            is_error: false,
            duration_ms: 12,
        };
        let received = AgentEvent::ToolResultReceived {
            id: "tc_text".to_string(),
            name: "text_tool".to_string(),
            content,
            is_error: false,
        };

        let completed_json = serde_json::to_value(&completed).expect("serialize completed event");
        assert_eq!(
            completed_json["content"],
            serde_json::json!([{"type": "text", "text": "plain output"}])
        );
        assert!(
            completed_json.get("has_images").is_none(),
            "typed content blocks should replace image side flags on event surfaces"
        );

        let received_json = serde_json::to_value(&received).expect("serialize received event");
        assert_eq!(
            received_json["content"],
            serde_json::json!([{"type": "text", "text": "plain output"}])
        );
    }

    #[test]
    fn tool_result_events_carry_image_only_content_blocks() {
        let content = vec![image_block("image/png", "AAAA")];
        let completed = AgentEvent::ToolExecutionCompleted {
            id: "tc_image".to_string(),
            name: "view_image".to_string(),
            result: "[image: image/png]".to_string(),
            content: content.clone(),
            is_error: false,
            duration_ms: 12,
        };
        let received = AgentEvent::ToolResultReceived {
            id: "tc_image".to_string(),
            name: "view_image".to_string(),
            content,
            is_error: false,
        };

        let completed_json = serde_json::to_value(&completed).expect("serialize completed event");
        assert_eq!(
            completed_json["content"],
            serde_json::json!([{
                "type": "image",
                "media_type": "image/png",
                "source": "inline",
                "data": "AAAA"
            }])
        );
        assert!(
            completed_json.get("has_images").is_none(),
            "typed content blocks should replace image side flags on event surfaces"
        );

        let received_json = serde_json::to_value(&received).expect("serialize received event");
        assert_eq!(received_json["content"], completed_json["content"]);
    }

    #[test]
    fn tool_result_events_carry_mixed_content_blocks_in_order() {
        let content = vec![
            text_block("before"),
            image_block("image/png", "AAAA"),
            text_block("after"),
        ];
        let completed = AgentEvent::ToolExecutionCompleted {
            id: "tc_mixed".to_string(),
            name: "mixed_tool".to_string(),
            result: "before\n[image: image/png]\nafter".to_string(),
            content: content.clone(),
            is_error: false,
            duration_ms: 12,
        };
        let received = AgentEvent::ToolResultReceived {
            id: "tc_mixed".to_string(),
            name: "mixed_tool".to_string(),
            content: content.clone(),
            is_error: false,
        };

        let completed_json = serde_json::to_value(&completed).expect("serialize completed event");
        assert_eq!(
            completed_json["content"],
            serde_json::json!([
                {"type": "text", "text": "before"},
                {
                    "type": "image",
                    "media_type": "image/png",
                    "source": "inline",
                    "data": "AAAA"
                },
                {"type": "text", "text": "after"}
            ])
        );
        assert!(
            completed_json.get("has_images").is_none(),
            "typed content blocks should replace image side flags on event surfaces"
        );

        let roundtrip: AgentEvent = serde_json::from_value(completed_json).unwrap();
        match roundtrip {
            AgentEvent::ToolExecutionCompleted {
                content: roundtrip_content,
                ..
            } => assert_eq!(roundtrip_content, content),
            other => unreachable!("unexpected event: {other:?}"),
        }

        let received_json = serde_json::to_value(&received).expect("serialize received event");
        assert_eq!(received_json["content"][0]["text"], "before");
        assert_eq!(received_json["content"][1]["media_type"], "image/png");
        assert_eq!(received_json["content"][2]["text"], "after");
    }

    #[test]
    fn legacy_tool_result_event_payloads_deserialize_without_typed_content() {
        let completed: AgentEvent = serde_json::from_value(serde_json::json!({
            "type": "tool_execution_completed",
            "id": "tc_legacy",
            "name": "legacy_tool",
            "result": "legacy output",
            "is_error": false,
            "duration_ms": 3,
            "has_images": true
        }))
        .expect("legacy tool_execution_completed payload should deserialize");
        match completed {
            AgentEvent::ToolExecutionCompleted {
                result,
                content,
                is_error,
                ..
            } => {
                assert_eq!(result, "legacy output");
                assert!(content.is_empty());
                assert!(!is_error);
            }
            other => unreachable!("unexpected event: {other:?}"),
        }

        let received: AgentEvent = serde_json::from_value(serde_json::json!({
            "type": "tool_result_received",
            "id": "tc_legacy",
            "name": "legacy_tool",
            "is_error": false
        }))
        .expect("legacy tool_result_received payload should deserialize");
        match received {
            AgentEvent::ToolResultReceived {
                content, is_error, ..
            } => {
                assert!(content.is_empty());
                assert!(!is_error);
            }
            other => unreachable!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn tool_config_changed_payload_carries_structured_status_with_legacy_mirror() {
        let status_info = ToolConfigChangeStatus::boundary_applied(true, true, 42);
        let event = AgentEvent::ToolConfigChanged {
            payload: ToolConfigChangedPayload::new(
                ToolConfigChangeOperation::Reload,
                "tool_scope",
                status_info,
                false,
            )
            .with_applied_at_turn(Some(3))
            .with_domain(Some(ToolConfigChangeDomain::ToolScope)),
        };

        let json = serde_json::to_value(event).unwrap();
        assert_eq!(
            json["payload"]["status"],
            "boundary_applied(base_changed=true,visible_changed=true,revision=42)"
        );
        assert_eq!(json["payload"]["status_info"]["kind"], "boundary_applied");
        assert_eq!(json["payload"]["status_info"]["base_changed"], true);
        assert_eq!(json["payload"]["status_info"]["visible_changed"], true);
        assert_eq!(json["payload"]["status_info"]["revision"], 42);
    }

    #[test]
    fn tool_config_changed_payload_derives_legacy_status_from_typed_status() {
        let status = ToolConfigChangeStatus::boundary_applied(true, false, 9);
        let event = AgentEvent::ToolConfigChanged {
            payload: ToolConfigChangedPayload::new(
                ToolConfigChangeOperation::Reload,
                "tool_scope",
                status.clone(),
                false,
            )
            .with_applied_at_turn(Some(4))
            .with_domain(Some(ToolConfigChangeDomain::ToolScope)),
        };

        let json = serde_json::to_value(event).unwrap();
        assert_eq!(
            json["payload"]["status"],
            "boundary_applied(base_changed=true,visible_changed=false,revision=9)"
        );
        assert_eq!(json["payload"]["status_info"]["kind"], "boundary_applied");

        let event: AgentEvent = serde_json::from_value(json).unwrap();
        if let AgentEvent::ToolConfigChanged { payload } = event {
            assert_eq!(payload.status_info(), &status);
            assert_eq!(
                payload.status_text(),
                "boundary_applied(base_changed=true,visible_changed=false,revision=9)"
            );
        } else {
            panic!("expected tool_config_changed event");
        }
    }

    #[test]
    fn tool_config_changed_payload_deserializes_legacy_status_without_typed_data() {
        let event: AgentEvent = serde_json::from_value(serde_json::json!({
            "type": "tool_config_changed",
            "payload": {
                "operation": "reload",
                "target": "tool_scope",
                "status": "boundary_applied(base_changed=true,visible_changed=true,revision=42)",
                "persisted": false,
                "applied_at_turn": 3,
                "domain": "tool_scope"
            }
        }))
        .unwrap();

        assert!(
            matches!(event, AgentEvent::ToolConfigChanged { .. }),
            "expected tool_config_changed, got {event:?}"
        );
        if let AgentEvent::ToolConfigChanged { payload } = event {
            assert_eq!(
                payload.status_text(),
                "boundary_applied(base_changed=true,visible_changed=true,revision=42)"
            );
            assert_eq!(
                payload.status_info(),
                &ToolConfigChangeStatus::legacy_status(
                    "boundary_applied(base_changed=true,visible_changed=true,revision=42)"
                )
            );
        }
    }

    #[test]
    fn tool_config_changed_payload_prefers_typed_status_over_legacy_mirror() {
        let event: AgentEvent = serde_json::from_value(serde_json::json!({
            "type": "tool_config_changed",
            "payload": {
                "operation": "reload",
                "target": "tool_scope",
                "status": "legacy stale status",
                "status_info": {
                    "kind": "boundary_applied",
                    "base_changed": true,
                    "visible_changed": false,
                    "revision": 9
                },
                "persisted": false,
                "domain": "tool_scope"
            }
        }))
        .unwrap();

        if let AgentEvent::ToolConfigChanged { payload } = event {
            assert_eq!(
                payload.status_info(),
                &ToolConfigChangeStatus::boundary_applied(true, false, 9)
            );
            assert_eq!(
                payload.status_text(),
                "boundary_applied(base_changed=true,visible_changed=false,revision=9)"
            );
        } else {
            panic!("expected tool_config_changed event");
        }
    }

    #[cfg(feature = "schema")]
    #[test]
    fn tool_config_changed_payload_schema_allows_legacy_status_only_replays() {
        let schema = serde_json::to_value(schemars::schema_for!(ToolConfigChangedPayload)).unwrap();
        let required = schema["required"].as_array().expect("required array");

        assert!(
            required.iter().any(|field| field == "status"),
            "legacy status mirror remains required while it is emitted publicly"
        );
        assert!(
            !required.iter().any(|field| field == "status_info"),
            "legacy status-only event replays must remain schema-compatible"
        );
        assert!(
            schema["properties"]["status_info"].is_object(),
            "typed status_info remains part of the schema when present"
        );
    }

    #[test]
    fn test_agent_event_json_schema() {
        // Test all event variants serialize correctly
        let events = vec![
            AgentEvent::RunStarted {
                session_id: SessionId::new(),
                prompt: ContentInput::Text("Hello".to_string()),
            },
            AgentEvent::TextDelta {
                delta: "chunk".to_string(),
            },
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::TurnCompleted {
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
            AgentEvent::ToolCallRequested {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                args: tool_args(serde_json::json!({"path": "/tmp/test"})),
            },
            AgentEvent::ToolResultReceived {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                content: ContentBlock::text_vec("ok".to_string()),
                is_error: false,
            },
            AgentEvent::BudgetWarning {
                budget_type: BudgetType::Tokens,
                used: 8000,
                limit: 10000,
                percent: 0.8,
            },
            AgentEvent::Retrying {
                attempt: 1,
                max_attempts: 3,
                error: "Rate limited".to_string(),
                delay_ms: 1000,
                retry: None,
            },
            AgentEvent::RunCompleted {
                session_id: SessionId::new(),
                result: "Done".to_string(),
                structured_output: None,
                usage: Usage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
                terminal_cause_kind: None,
            },
            AgentEvent::RunFailed {
                session_id: SessionId::new(),
                error_class: AgentErrorClass::Budget,
                error: "Budget exceeded".to_string(),
                terminal_cause_kind: None,
                error_report: Some(AgentErrorReport {
                    class: AgentErrorClass::Budget,
                    reason: None,
                    message: "Budget exceeded".to_string(),
                }),
            },
            AgentEvent::CompactionStarted {
                input_tokens: 120_000,
                estimated_history_tokens: 150_000,
                message_count: 42,
            },
            AgentEvent::CompactionCompleted {
                summary_tokens: 2048,
                messages_before: 42,
                messages_after: 8,
            },
            AgentEvent::CompactionFailed {
                error: "LLM request failed".to_string(),
            },
            AgentEvent::InteractionComplete {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                result: "agent response".to_string(),
                structured_output: None,
            },
            AgentEvent::InteractionCallbackPending {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                tool_name: "external_mock".to_string(),
                args: serde_json::json!({"value": "browser"}),
            },
            AgentEvent::InteractionFailed {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                error: "LLM failure".to_string(),
            },
            AgentEvent::StreamTruncated {
                reason: "channel full".to_string(),
            },
            AgentEvent::ToolConfigChanged {
                payload: ToolConfigChangedPayload::new(
                    ToolConfigChangeOperation::Remove,
                    "filesystem",
                    ToolConfigChangeStatus::legacy_status("staged"),
                    false,
                )
                .with_applied_at_turn(Some(12)),
            },
            AgentEvent::background_job_completed(
                "j_123",
                "sleep 2",
                BackgroundJobTerminalStatus::Completed,
                "exit_code: 0",
            ),
        ];

        for event in events {
            let json = serde_json::to_value(&event).unwrap();

            // All events should have a "type" field
            assert!(
                json.get("type").is_some(),
                "Event missing type field: {event:?}"
            );

            // Should roundtrip
            let roundtrip: AgentEvent = serde_json::from_value(json.clone()).unwrap();
            let json2 = serde_json::to_value(&roundtrip).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn background_job_completed_carries_typed_terminal_status() {
        let event = AgentEvent::background_job_completed(
            "j_123",
            "sleep 2",
            BackgroundJobTerminalStatus::Failed,
            "exit_code: 1",
        );

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "background_job_completed");
        assert_eq!(json["status"], "failed");
        assert_eq!(json["terminal_status"], "failed");

        let roundtrip: AgentEvent = serde_json::from_value(json).unwrap();
        match roundtrip {
            AgentEvent::BackgroundJobCompleted {
                legacy_status,
                terminal_status,
                ..
            } => {
                assert_eq!(legacy_status.as_deref(), Some("failed"));
                assert_eq!(terminal_status, BackgroundJobTerminalStatus::Failed);
            }
            other => unreachable!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn background_job_completed_requires_typed_terminal_status() {
        let string_only_json = serde_json::json!({
            "type": "background_job_completed",
            "job_id": "j_123",
            "display_name": "sleep 2",
            "status": "completed",
            "detail": "exit_code: 1"
        });
        assert!(
            serde_json::from_value::<AgentEvent>(string_only_json).is_err(),
            "legacy status-only payload must not decode as completed"
        );

        let malformed_status_only_json = serde_json::json!({
            "type": "background_job_completed",
            "job_id": "j_123",
            "display_name": "sleep 2",
            "status": "success",
            "detail": "exit_code: 0"
        });
        assert!(
            serde_json::from_value::<AgentEvent>(malformed_status_only_json).is_err(),
            "unknown legacy status string must not become success"
        );

        let unknown_typed_json = serde_json::json!({
            "type": "background_job_completed",
            "job_id": "j_123",
            "display_name": "sleep 2",
            "status": "completed",
            "terminal_status": "success",
            "detail": "exit_code: 0"
        });
        assert!(
            serde_json::from_value::<AgentEvent>(unknown_typed_json).is_err(),
            "unknown typed terminal status must fail closed"
        );

        let typed_without_legacy_json = serde_json::json!({
            "type": "background_job_completed",
            "job_id": "j_123",
            "display_name": "sleep 2",
            "terminal_status": "failed",
            "detail": "exit_code: 1"
        });
        let event: AgentEvent = serde_json::from_value(typed_without_legacy_json).unwrap();
        match event {
            AgentEvent::BackgroundJobCompleted {
                job_id,
                display_name,
                legacy_status,
                terminal_status,
                detail,
            } => {
                assert_eq!(job_id, "j_123");
                assert_eq!(display_name, "sleep 2");
                assert_eq!(legacy_status, None);
                assert_eq!(terminal_status, BackgroundJobTerminalStatus::Failed);
                assert_eq!(detail, "exit_code: 1");
            }
            other => unreachable!("unexpected event: {other:?}"),
        }

        let stale_legacy_json = serde_json::json!({
            "type": "background_job_completed",
            "job_id": "j_123",
            "display_name": "sleep 2",
            "status": "completed",
            "terminal_status": "failed",
            "detail": "exit_code: 1"
        });
        let event: AgentEvent = serde_json::from_value(stale_legacy_json).unwrap();
        match event {
            AgentEvent::BackgroundJobCompleted {
                job_id,
                display_name,
                legacy_status,
                terminal_status,
                detail,
            } => {
                assert_eq!(job_id, "j_123");
                assert_eq!(display_name, "sleep 2");
                assert_eq!(legacy_status.as_deref(), Some("completed"));
                assert_eq!(terminal_status, BackgroundJobTerminalStatus::Failed);
                assert_eq!(detail, "exit_code: 1");
            }
            other => unreachable!("unexpected event: {other:?}"),
        }

        let malformed_legacy_json = serde_json::json!({
            "type": "background_job_completed",
            "job_id": "j_123",
            "display_name": "sleep 2",
            "status": 0,
            "terminal_status": "failed",
            "detail": "exit_code: 1"
        });
        let event: AgentEvent = serde_json::from_value(malformed_legacy_json).unwrap();
        match event {
            AgentEvent::BackgroundJobCompleted {
                legacy_status,
                terminal_status,
                detail,
                ..
            } => {
                assert_eq!(legacy_status, None);
                assert_eq!(terminal_status, BackgroundJobTerminalStatus::Failed);
                assert_eq!(detail, "exit_code: 1");
            }
            other => unreachable!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn background_job_terminal_status_maps_operation_truth() {
        use crate::ops::{OperationId, OperationResult};
        use crate::ops_lifecycle::{OperationStatus, OperationTerminalOutcome};

        let result = OperationResult {
            id: OperationId(uuid::Uuid::new_v4()),
            content: String::new(),
            is_error: false,
            duration_ms: 0,
            tokens_used: 0,
        };

        assert_eq!(
            BackgroundJobTerminalStatus::from_terminal_outcome(
                &OperationTerminalOutcome::Completed(result)
            ),
            BackgroundJobTerminalStatus::Completed
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_terminal_outcome(&OperationTerminalOutcome::Failed {
                error: "boom".to_string(),
            }),
            BackgroundJobTerminalStatus::Failed
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_terminal_outcome(
                &OperationTerminalOutcome::Aborted { reason: None }
            ),
            BackgroundJobTerminalStatus::Aborted
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_terminal_outcome(
                &OperationTerminalOutcome::Cancelled {
                    reason: Some("user".to_string()),
                }
            ),
            BackgroundJobTerminalStatus::Cancelled
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_terminal_outcome(&OperationTerminalOutcome::Retired),
            BackgroundJobTerminalStatus::Retired
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_terminal_outcome(
                &OperationTerminalOutcome::Terminated {
                    reason: "channel closed".to_string(),
                }
            ),
            BackgroundJobTerminalStatus::Terminated
        );

        assert_eq!(
            BackgroundJobTerminalStatus::from_operation_status(OperationStatus::Completed),
            Some(BackgroundJobTerminalStatus::Completed)
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_operation_status(OperationStatus::Failed),
            Some(BackgroundJobTerminalStatus::Failed)
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_operation_status(OperationStatus::Aborted),
            Some(BackgroundJobTerminalStatus::Aborted)
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_operation_status(OperationStatus::Cancelled),
            Some(BackgroundJobTerminalStatus::Cancelled)
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_operation_status(OperationStatus::Retired),
            Some(BackgroundJobTerminalStatus::Retired)
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_operation_status(OperationStatus::Terminated),
            Some(BackgroundJobTerminalStatus::Terminated)
        );
        assert_eq!(
            BackgroundJobTerminalStatus::from_operation_status(OperationStatus::Running),
            None
        );
    }

    #[test]
    fn retry_event_carries_typed_schedule() {
        let schedule = LlmRetrySchedule {
            failure: LlmRetryFailure {
                provider: "test".to_string(),
                kind: LlmRetryFailureKind::RateLimited,
                retry_after_ms: Some(30_000),
                duration_ms: None,
                message: "rate limited".to_string(),
            },
            plan: LlmRetryPlan {
                attempt: 1,
                max_retries: 3,
                computed_delay_ms: 500,
                selected_delay_ms: 30_000,
                retry_after_hint_ms: Some(30_000),
                rate_limit_floor_applied: true,
                budget_capped: false,
            },
        };
        let event = AgentEvent::Retrying {
            attempt: schedule.plan.attempt,
            max_attempts: schedule.plan.max_retries,
            error: schedule.failure.message.clone(),
            delay_ms: schedule.plan.selected_delay_ms,
            retry: Some(schedule),
        };

        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["retry"]["failure"]["kind"], "rate_limited");
        assert_eq!(value["retry"]["plan"]["attempt"], 1);
        assert_eq!(value["retry"]["plan"]["selected_delay_ms"], 30_000);
    }

    #[test]
    fn skill_resolution_failed_carries_typed_key_and_reason_with_legacy_mirrors() {
        let key = SkillKey::builtin(SkillName::parse("test-skill").unwrap());
        let error = SkillError::NotFound { key: key.clone() };
        let reason = SkillResolutionFailureReason::from_skill_error(&error);
        let event = AgentEvent::SkillResolutionFailed {
            skill_key: Some(key.clone()),
            reason,
            reference: key.to_string(),
            error: error.to_string(),
        };

        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value["skill_key"]["source_uuid"],
            key.source_uuid.to_string()
        );
        assert_eq!(value["skill_key"]["skill_name"], key.skill_name.as_str());
        assert_eq!(value["reason"]["reason_type"], "not_found");
        assert_eq!(
            value["reason"]["key"]["source_uuid"],
            key.source_uuid.to_string()
        );
        assert_eq!(
            value["reason"]["key"]["skill_name"],
            key.skill_name.as_str()
        );
        assert_eq!(value["reference"], key.to_string());
        assert_eq!(value["error"], error.to_string());

        let roundtrip: AgentEvent = serde_json::from_value(value).unwrap();
        match roundtrip {
            AgentEvent::SkillResolutionFailed {
                skill_key,
                reason,
                reference,
                error: error_message,
            } => {
                assert_eq!(skill_key, Some(key.clone()));
                assert_eq!(
                    reason,
                    SkillResolutionFailureReason::NotFound { key: key.clone() }
                );
                assert_eq!(reference, key.to_string());
                assert_eq!(error_message, error.to_string());
            }
            other => unreachable!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn legacy_skill_resolution_failed_payload_deserializes() {
        let value = serde_json::json!({
            "type": "skill_resolution_failed",
            "reference": "legacy/ref",
            "error": "missing",
        });

        let event: AgentEvent = serde_json::from_value(value).unwrap();
        match event {
            AgentEvent::SkillResolutionFailed {
                skill_key,
                reason,
                reference,
                error,
            } => {
                assert_eq!(skill_key, None);
                assert_eq!(
                    reason,
                    SkillResolutionFailureReason::Unknown {
                        message: String::new()
                    }
                );
                assert_eq!(reference, "legacy/ref");
                assert_eq!(error, "missing");
            }
            other => unreachable!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn unknown_skill_resolution_failed_reason_type_deserializes_as_unknown() {
        let value = serde_json::json!({
            "type": "skill_resolution_failed",
            "reason": {
                "reason_type": "future_reason",
                "message": "future reason details"
            },
        });

        let event: AgentEvent = serde_json::from_value(value).unwrap();
        match event {
            AgentEvent::SkillResolutionFailed { reason, .. } => {
                assert_eq!(
                    reason,
                    SkillResolutionFailureReason::Unknown {
                        message: "future reason details".to_string()
                    }
                );
                assert_eq!(reason.to_string(), "future reason details");
            }
            other => unreachable!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn agent_error_report_carries_typed_hook_reason() {
        let hook_id = HookId::new("guard-pre-tool");
        let error = crate::error::AgentError::HookDenied {
            hook_id: hook_id.clone(),
            point: HookPoint::RunStarted,
            reason_code: HookReasonCode::PolicyViolation,
            message: "blocked".to_string(),
            payload: None,
        };
        let report = AgentErrorReport::from_agent_error(&error);
        assert_eq!(report.class, AgentErrorClass::Hook);
        assert_eq!(
            report.reason,
            Some(AgentErrorReason::HookDenied {
                hook_id: Some(hook_id),
                point: HookPoint::RunStarted,
                reason_code: HookReasonCode::PolicyViolation,
            })
        );
        assert_eq!(report.message, error.to_string());
    }

    #[test]
    fn agent_error_report_carries_typed_provider_error_reason() {
        let error = crate::error::AgentError::llm(
            "anthropic",
            LlmFailureReason::ProviderError(crate::error::LlmProviderError::retryable(
                LlmProviderErrorKind::ServerOverloaded,
                serde_json::json!({
                    "message": "provider overloaded"
                }),
            )),
            "provider overloaded",
        );

        let report = AgentErrorReport::from_agent_error(&error);

        assert_eq!(report.class, AgentErrorClass::Llm);
        assert_eq!(
            report.reason,
            Some(AgentErrorReason::LlmProviderError {
                provider_error_kind: LlmProviderErrorKind::ServerOverloaded,
                provider_error_retryability: LlmProviderErrorRetryability::Retryable,
                provider_error: serde_json::json!({
                    "message": "provider overloaded"
                }),
            })
        );
    }

    #[test]
    fn agent_error_report_fails_closed_for_unknown_terminal_cause() {
        let error = crate::error::AgentError::TerminalFailure {
            outcome: TurnTerminalOutcome::Failed,
            cause_kind: TurnTerminalCauseKind::Unknown,
            message: "display text must not publish terminal cause".to_string(),
        };

        let report = AgentErrorReport::from_agent_error(&error);

        assert_eq!(report.class, AgentErrorClass::Internal);
        assert_eq!(report.reason, None);
        assert_eq!(report.message, error.to_string());
    }

    #[test]
    fn test_agent_event_type_mapping_is_total_for_all_variants() {
        let events = vec![
            AgentEvent::RunStarted {
                session_id: SessionId::new(),
                prompt: ContentInput::Text("Hello".to_string()),
            },
            AgentEvent::RunCompleted {
                session_id: SessionId::new(),
                result: "Done".to_string(),
                structured_output: None,
                usage: Usage::default(),
                terminal_cause_kind: None,
            },
            AgentEvent::RunFailed {
                session_id: SessionId::new(),
                error_class: AgentErrorClass::Internal,
                error: "failed".to_string(),
                terminal_cause_kind: None,
                error_report: Some(AgentErrorReport {
                    class: AgentErrorClass::Internal,
                    reason: None,
                    message: "failed".to_string(),
                }),
            },
            AgentEvent::HookStarted {
                hook_id: HookId::new("hook-1"),
                point: HookPoint::RunStarted,
            },
            AgentEvent::HookCompleted {
                hook_id: HookId::new("hook-1"),
                point: HookPoint::RunStarted,
                duration_ms: 1,
            },
            AgentEvent::HookFailed {
                hook_id: HookId::new("hook-1"),
                point: HookPoint::RunStarted,
                error: "failed".to_string(),
            },
            AgentEvent::HookDenied {
                hook_id: HookId::new("hook-1"),
                point: HookPoint::RunStarted,
                reason_code: HookReasonCode::PolicyViolation,
                message: "nope".to_string(),
                payload: None,
            },
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::ReasoningDelta {
                delta: "think".to_string(),
            },
            AgentEvent::ReasoningComplete {
                content: "done".to_string(),
            },
            AgentEvent::TextDelta {
                delta: "chunk".to_string(),
            },
            AgentEvent::TextComplete {
                content: "done".to_string(),
            },
            AgentEvent::ToolCallRequested {
                id: "tool-1".to_string(),
                name: "search".to_string(),
                args: tool_args(serde_json::json!({})),
            },
            AgentEvent::ToolResultReceived {
                id: "tool-1".to_string(),
                name: "search".to_string(),
                content: ContentBlock::text_vec("ok".to_string()),
                is_error: false,
            },
            AgentEvent::TurnCompleted {
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
            AgentEvent::ToolExecutionStarted {
                id: "tool-1".to_string(),
                name: "search".to_string(),
            },
            AgentEvent::ToolExecutionCompleted {
                id: "tool-1".to_string(),
                name: "search".to_string(),
                result: "ok".to_string(),
                content: ContentBlock::text_vec("ok".to_string()),
                is_error: false,
                duration_ms: 1,
            },
            AgentEvent::ToolExecutionTimedOut {
                id: "tool-1".to_string(),
                name: "search".to_string(),
                timeout_ms: 1000,
            },
            AgentEvent::CompactionStarted {
                input_tokens: 1,
                estimated_history_tokens: 2,
                message_count: 3,
            },
            AgentEvent::CompactionCompleted {
                summary_tokens: 1,
                messages_before: 3,
                messages_after: 1,
            },
            AgentEvent::CompactionFailed {
                error: "failed".to_string(),
            },
            AgentEvent::BudgetWarning {
                budget_type: BudgetType::Time,
                used: 1,
                limit: 2,
                percent: 50.0,
            },
            AgentEvent::Retrying {
                attempt: 1,
                max_attempts: 2,
                error: "retry".to_string(),
                delay_ms: 100,
                retry: None,
            },
            AgentEvent::SkillsResolved {
                skills: vec![],
                injection_bytes: 0,
            },
            AgentEvent::SkillResolutionFailed {
                skill_key: None,
                reason: SkillResolutionFailureReason::Unknown {
                    message: "missing".to_string(),
                },
                reference: "skill".to_string(),
                error: "missing".to_string(),
            },
            AgentEvent::InteractionComplete {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                result: "ok".to_string(),
                structured_output: None,
            },
            AgentEvent::InteractionCallbackPending {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                tool_name: "external_mock".to_string(),
                args: serde_json::json!({"value": "browser"}),
            },
            AgentEvent::InteractionFailed {
                interaction_id: crate::interaction::InteractionId(uuid::Uuid::new_v4()),
                error: "failed".to_string(),
            },
            AgentEvent::StreamTruncated {
                reason: "lag".to_string(),
            },
            AgentEvent::ToolConfigChanged {
                payload: ToolConfigChangedPayload::new(
                    ToolConfigChangeOperation::Reload,
                    "external",
                    ToolConfigChangeStatus::external_tool_delta(
                        ExternalToolDeltaPhase::Applied,
                        None,
                    ),
                    true,
                )
                .with_applied_at_turn(Some(1)),
            },
            AgentEvent::background_job_completed(
                "j_123",
                "sleep 2",
                BackgroundJobTerminalStatus::Completed,
                "exit_code: 0",
            ),
        ];

        let expected_event_count = events.len();
        let mut kinds = std::collections::BTreeSet::new();
        for event in events {
            let kind = agent_event_type(&event);
            assert!(
                !kind.is_empty(),
                "event type mapping returned empty discriminator"
            );
            kinds.insert(kind);
        }
        assert_eq!(
            kinds.len(),
            expected_event_count,
            "expected one distinct discriminator per covered event variant"
        );
    }

    #[test]
    fn test_budget_type_serialization() {
        assert_eq!(serde_json::to_value(BudgetType::Tokens).unwrap(), "tokens");
        assert_eq!(serde_json::to_value(BudgetType::Time).unwrap(), "time");
        assert_eq!(
            serde_json::to_value(BudgetType::ToolCalls).unwrap(),
            "tool_calls"
        );
    }

    #[test]
    fn test_scoped_agent_event_roundtrip() {
        let event = ScopedAgentEvent::new(
            vec![StreamScopeFrame::MobMember {
                flow_run_id: "run_123".to_string(),
                agent_identity: "writer".to_string(),
                agent_runtime_id: Some("writer:0".to_string()),
                fence_token: Some(1),
                generation: Some(0),
            }],
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );

        assert_eq!(event.scope_id, "mob:writer");

        let json = serde_json::to_value(&event).unwrap();
        let frame = &json["scope_path"][0];
        assert_eq!(frame["flow_run_id"], "run_123");
        assert_eq!(frame["agent_identity"], "writer");
        assert!(
            frame.get("agent_runtime_id").is_none(),
            "scoped stream frames must not serialize runtime incarnation ids"
        );
        assert!(
            frame.get("fence_token").is_none(),
            "scoped stream frames must not serialize fence tokens"
        );
        assert!(
            frame.get("generation").is_none(),
            "scoped stream frames must not serialize runtime generations"
        );
        let roundtrip: ScopedAgentEvent = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip.scope_id, "mob:writer");
        assert!(matches!(
            roundtrip.event,
            AgentEvent::TextDelta { ref delta } if delta == "hello"
        ));
    }

    #[test]
    fn test_scope_id_from_path_formats() {
        let primary = vec![StreamScopeFrame::Primary {
            session_id: "sid_x".to_string(),
        }];
        assert_eq!(ScopedAgentEvent::scope_id_from_path(&primary), "primary");

        let mob = vec![StreamScopeFrame::MobMember {
            flow_run_id: "run_1".to_string(),
            agent_identity: "planner".to_string(),
            agent_runtime_id: Some("planner:2".to_string()),
            fence_token: Some(3),
            generation: Some(2),
        }];
        assert_eq!(ScopedAgentEvent::scope_id_from_path(&mob), "mob:planner");
    }

    #[test]
    fn test_event_envelope_roundtrip() {
        let session_id = SessionId::new();
        let envelope = EventEnvelope::new_session(
            session_id.clone(),
            7,
            Some("mob_1".to_string()),
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );
        let value = serde_json::to_value(&envelope).expect("serialize envelope");
        let parsed: EventEnvelope<AgentEvent> =
            serde_json::from_value(value).expect("deserialize envelope");
        assert_eq!(parsed.source_session_id(), Some(&session_id));
        assert_eq!(parsed.source_id, format!("session:{session_id}"));
        assert_eq!(parsed.seq, 7);
        assert_eq!(parsed.mob_id.as_deref(), Some("mob_1"));
        assert!(parsed.timestamp_ms > 0);
        assert!(matches!(
            parsed.payload,
            AgentEvent::TextDelta { delta } if delta == "hello"
        ));
    }

    #[test]
    fn event_envelope_requires_typed_source_identity() {
        let value = serde_json::json!({
            "event_id": uuid::Uuid::now_v7(),
            "source_id": "session:00000000-0000-4000-8000-000000000001",
            "seq": 7,
            "timestamp_ms": 1,
            "payload": {
                "type": "text_delta",
                "delta": "hello",
            },
        });

        let result = serde_json::from_value::<EventEnvelope<AgentEvent>>(value);

        assert!(
            result.is_err(),
            "source_id alone must not deserialize as canonical source identity"
        );
    }

    #[test]
    fn malformed_legacy_source_id_does_not_override_typed_source() {
        let session_id = SessionId::new();
        let value = serde_json::json!({
            "event_id": uuid::Uuid::now_v7(),
            "source": {
                "type": "session",
                "session_id": session_id,
            },
            "source_id": "session:not-a-uuid",
            "seq": 7,
            "timestamp_ms": 1,
            "payload": {
                "type": "text_delta",
                "delta": "hello",
            },
        });

        let parsed: EventEnvelope<AgentEvent> =
            serde_json::from_value(value).expect("typed source should deserialize");

        assert_eq!(parsed.source_session_id(), Some(&session_id));
        assert_eq!(parsed.source_id, "session:not-a-uuid");
    }

    #[test]
    fn legacy_session_source_id_string_does_not_classify_envelope() {
        let session_id = SessionId::new();
        let envelope = EventEnvelope::new(
            format!("session:{session_id}"),
            1,
            None,
            AgentEvent::TurnStarted { turn_number: 1 },
        );

        assert_eq!(envelope.source_session_id(), None);
        assert_eq!(envelope.source_id, format!("session:{session_id}"));
    }

    #[test]
    fn test_compare_event_envelopes_total_order() {
        let mut a = EventEnvelope::new("a", 1, None, AgentEvent::TurnStarted { turn_number: 1 });
        let mut b = EventEnvelope::new("a", 2, None, AgentEvent::TurnStarted { turn_number: 2 });
        a.timestamp_ms = 10;
        b.timestamp_ms = 10;
        assert_eq!(compare_event_envelopes(&a, &b), Ordering::Less);
        assert_eq!(compare_event_envelopes(&b, &a), Ordering::Greater);
    }
}

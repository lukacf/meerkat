//! Hook contracts and engine interfaces.

use crate::types::{SessionId, StopReason, Usage};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable identifier for a configured hook.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct HookId(pub String);

impl HookId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for HookId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for HookId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for HookId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Hook points available in V1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    RunStarted,
    RunCompleted,
    RunFailed,
    PreLlmRequest,
    PostLlmResponse,
    PreToolExecution,
    PostToolExecution,
    TurnBoundary,
}

impl HookPoint {
    pub fn is_pre(self) -> bool {
        matches!(
            self,
            Self::RunStarted | Self::PreLlmRequest | Self::PreToolExecution | Self::TurnBoundary
        )
    }

    pub fn is_post(self) -> bool {
        matches!(
            self,
            Self::PostLlmResponse | Self::PostToolExecution | Self::RunCompleted | Self::RunFailed
        )
    }
}

/// Foreground hooks block loop progression; background hooks publish async patches.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookExecutionMode {
    Foreground,
    Background,
}

/// Declared capability determines default failure behavior and constraints.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookCapability {
    Observe,
    Guardrail,
    Rewrite,
}

/// Failure policy can be explicitly configured per hook.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookFailurePolicy {
    FailOpen,
    FailClosed,
}

/// Typed reason codes for guardrail denials.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum HookReasonCode {
    PolicyViolation,
    SafetyViolation,
    SchemaViolation,
    Timeout,
    RuntimeError,
}

/// Final decision produced by merged hook outcomes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum HookDecision {
    Allow,
    Deny {
        hook_id: HookId,
        reason_code: HookReasonCode,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },
}

impl HookDecision {
    pub fn deny(
        hook_id: HookId,
        reason_code: HookReasonCode,
        message: impl Into<String>,
        payload: Option<Value>,
    ) -> Self {
        Self::Deny {
            hook_id,
            reason_code,
            message: message.into(),
            payload,
        }
    }
}

/// Typed patch intents used by rewrite hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "patch_type", rename_all = "snake_case")]
pub enum HookPatch {
    /// Mutate effective LLM request parameters.
    LlmRequest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_tokens: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        temperature: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_params: Option<Value>,
    },
    /// Replace assistant text content in the latest model response.
    AssistantText { text: String },
    /// Replace serialized tool arguments.
    ToolArgs { args: Value },
    /// Mutate tool result payload before it is persisted.
    ToolResult {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// Mutate final run output text.
    RunResult { text: String },
}

/// Monotonic patch revision metadata.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct HookRevision(pub u64);

/// Stable envelope emitted for async patch publication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookPatchEnvelope {
    pub revision: HookRevision,
    pub hook_id: HookId,
    pub point: HookPoint,
    pub patch: HookPatch,
    pub published_at: DateTime<Utc>,
}

/// LLM request view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookLlmRequest {
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
    pub message_count: usize,
}

/// LLM response view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookLlmResponse {
    pub assistant_text: String,
    #[serde(default)]
    pub tool_call_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Tool call view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolCall {
    pub tool_use_id: String,
    pub name: String,
    pub args: Value,
}

/// Tool result view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolResult {
    pub tool_use_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

/// Full invocation payload passed into the hook engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookInvocation {
    pub point: HookPoint,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_request: Option<HookLlmRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_response: Option<HookLlmResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<HookToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<HookToolResult>,
}

/// Outcome emitted by one executed hook entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookOutcome {
    pub hook_id: HookId,
    pub point: HookPoint,
    pub priority: i32,
    pub registration_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    #[serde(default)]
    pub patches: Vec<HookPatch>,
    #[serde(default)]
    pub published_patches: Vec<HookPatchEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Aggregate result used by core loop to apply decisions and rewrites.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub struct HookExecutionReport {
    #[serde(default)]
    pub outcomes: Vec<HookOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
    #[serde(default)]
    pub patches: Vec<HookPatch>,
    #[serde(default)]
    pub published_patches: Vec<HookPatchEnvelope>,
}

impl HookExecutionReport {
    pub fn empty() -> Self {
        Self::default()
    }
}

pub fn default_failure_policy(capability: HookCapability) -> HookFailurePolicy {
    match capability {
        HookCapability::Observe => HookFailurePolicy::FailOpen,
        HookCapability::Guardrail | HookCapability::Rewrite => HookFailurePolicy::FailClosed,
    }
}

/// Engine-level failures that prevented hook execution.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HookEngineError {
    #[error("Hook configuration invalid: {0}")]
    InvalidConfiguration(String),
    #[error("Hook runtime execution failed for '{hook_id}': {reason}")]
    ExecutionFailed { hook_id: HookId, reason: String },
    #[error("Hook '{hook_id}' timed out after {timeout_ms}ms")]
    Timeout { hook_id: HookId, timeout_ms: u64 },
}

/// Runtime-independent engine interface.
#[async_trait]
pub trait HookEngine: Send + Sync {
    fn matching_hooks(
        &self,
        _invocation: &HookInvocation,
        _overrides: Option<&crate::config::HookRunOverrides>,
    ) -> Result<Vec<HookId>, HookEngineError> {
        Ok(Vec::new())
    }

    async fn execute(
        &self,
        invocation: HookInvocation,
        overrides: Option<&crate::config::HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError>;
}

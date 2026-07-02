//! Hook contracts and engine interfaces.

use crate::error::AgentError;
use crate::event::{AgentErrorClass, AgentErrorReport, ToolCallArguments};
use crate::types::{
    ContentBlock, RunInput, ServerToolKind, SessionId, StopReason, ToolProvenance, ToolResult,
    Usage,
};
use async_trait::async_trait;
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

/// Foreground hooks block loop progression; background hooks run asynchronously.
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

/// Typed reason a hook execution failed (engine-level fault, not a guardrail
/// denial).
///
/// Mirrors the [`HookReasonCode`] precedent: the variant is the typed owner of
/// the failure cause; the human-readable string is a [`Display`] derivation,
/// never a separately-stored field.
///
/// [`Display`]: std::fmt::Display
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason_code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum HookFailureReason {
    /// The hook runtime did not complete within its configured timeout.
    Timeout { timeout_ms: u64 },
    /// The hook runtime executed but failed.
    ExecutionFailed {
        /// Display projection of the underlying execution error.
        message: String,
    },
    /// The hook configuration was rejected.
    ConfigInvalid {
        /// Display projection of the configuration error.
        message: String,
    },
    /// A `pre_*` background hook attempted a non-observe action (patch or deny),
    /// which is not permitted for observe-only background hooks.
    ObserveOnlyViolation,
}

impl std::fmt::Display for HookFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { timeout_ms } => write!(f, "hook timed out after {timeout_ms}ms"),
            Self::ExecutionFailed { message } => write!(f, "{message}"),
            Self::ConfigInvalid { message } => write!(f, "{message}"),
            Self::ObserveOnlyViolation => {
                write!(f, "pre_* background hooks are observe-only")
            }
        }
    }
}

impl HookFailureReason {
    /// Typed execution failure carrying the display message.
    pub fn execution_failed(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }

    /// Project a typed [`HookEngineError`] into its failure reason.
    #[must_use]
    pub fn from_engine_error(error: &HookEngineError) -> Self {
        match error {
            HookEngineError::InvalidConfiguration(reason) => Self::ConfigInvalid {
                message: reason.clone(),
            },
            HookEngineError::ExecutionFailed { reason, .. } => Self::ExecutionFailed {
                message: reason.clone(),
            },
            HookEngineError::Timeout { timeout_ms, .. } => Self::Timeout {
                timeout_ms: *timeout_ms,
            },
        }
    }
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

/// LLM request view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookLlmRequest {
    pub max_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Typed effective provider parameter overrides for this LLM call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
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
    /// Typed kinds of provider-executed server-tool evidence present in this
    /// response (`AssistantBlock::ServerToolContent` blocks), in block order.
    /// Projection of the typed block owner: a foreground `PostLlmResponse`
    /// hook classifies provider-native content (e.g. web search) synchronously
    /// from this field instead of racing the lossy observe stream.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_tool_content: Vec<ServerToolKind>,
}

/// Tool call view exposed to hooks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolCall {
    pub tool_use_id: String,
    pub name: String,
    pub args: ToolCallArguments,
    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string): dispatch-time policy
    /// hooks steer on this typed field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}

/// Tool result view exposed to hooks.
///
/// `content_blocks` is the canonical typed tool-result content that post-tool
/// hooks deny/terminalize on. The text projection is presentation-only and is
/// derived from the blocks via [`HookToolResult::text_projection`]; it is never
/// a separately-stored field that policy code can read.
///
/// The wire envelope additionally serializes a `content` string for external
/// (command/HTTP) hook consumers that read the text result. That field is a
/// pure serialize-only derivation of `content_blocks` (the text projection); it
/// is never a stored field, is never deserialized back as authority, and policy
/// code must steer on `content_blocks`. Restoring it (remediation row #331)
/// keeps external hook consumers — which previously read `content` — working
/// without re-introducing a lossy mutable mirror.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolResult {
    pub tool_use_id: String,
    pub name: String,
    /// Canonical typed tool-result content exposed to hooks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_blocks: Vec<ContentBlock>,
    pub is_error: bool,
    /// Typed provenance of the dispatched tool definition, when the active
    /// tool catalog carries one. Projection of the `ToolDef.provenance` owner
    /// (never re-derived from the tool name string).
    #[serde(default)]
    pub provenance: Option<ToolProvenance>,
}

impl Serialize for HookToolResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        // `content` is a serialize-only text projection derived from the
        // canonical `content_blocks` for external hook consumers (row #331).
        // The field count is `tool_use_id`, `name`, `content`, `is_error`,
        // plus `content_blocks` and `provenance` when present.
        let mut len = 4;
        if !self.content_blocks.is_empty() {
            len += 1;
        }
        if self.provenance.is_some() {
            len += 1;
        }
        let mut state = serializer.serialize_struct("HookToolResult", len)?;
        state.serialize_field("tool_use_id", &self.tool_use_id)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("content", &self.text_projection())?;
        if !self.content_blocks.is_empty() {
            state.serialize_field("content_blocks", &self.content_blocks)?;
        }
        state.serialize_field("is_error", &self.is_error)?;
        if let Some(provenance) = &self.provenance {
            state.serialize_field("provenance", provenance)?;
        }
        state.end()
    }
}

impl HookToolResult {
    pub fn from_tool_result(name: impl Into<String>, result: &ToolResult) -> Self {
        Self::from_tool_result_with_id(result.tool_use_id.clone(), name, result)
    }

    pub fn from_tool_result_with_id(
        tool_use_id: impl Into<String>,
        name: impl Into<String>,
        result: &ToolResult,
    ) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            name: name.into(),
            content_blocks: result.content.clone(),
            is_error: result.is_error,
            provenance: None,
        }
    }

    /// Attach the typed provenance of the dispatched tool definition
    /// (chainable builder, mirroring [`crate::types::ToolDef::with_provenance`]).
    #[must_use]
    pub fn with_provenance(mut self, provenance: Option<ToolProvenance>) -> Self {
        self.provenance = provenance;
        self
    }

    /// Presentation-only text projection derived from the canonical typed
    /// `content_blocks`.
    ///
    /// This is for rendering/diagnostics only and MUST NOT feed hook policy
    /// (allow/deny/terminalize) decisions — those steer on `content_blocks`.
    #[must_use]
    pub fn text_projection(&self) -> String {
        crate::types::text_content(&self.content_blocks)
    }
}

/// Full invocation payload passed into the hook engine.
///
/// `prompt_input` and `error_report` are the typed owners of the prompt and
/// failure facts. The wire envelope additionally serializes `prompt` and
/// `error` strings for external (command/HTTP) hook consumers; those fields
/// are pure serialize-only derivations of the typed owners (precedent:
/// [`HookToolResult`]'s `content`, row #331). They are never stored fields,
/// never deserialize back as authority, and in-process policy code must steer
/// on the typed owners.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookInvocation {
    pub point: HookPoint,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_input: Option<RunInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_report: Option<AgentErrorReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<AgentErrorClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_request: Option<HookLlmRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_response: Option<HookLlmResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<HookToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<HookToolResult>,
}

impl Serialize for HookInvocation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        // `prompt` and `error` are serialize-only text projections derived
        // from the typed owners at serialization time for external hook
        // consumers; they are never stored and never deserialized back.
        let prompt = self.prompt_input.as_ref().and_then(RunInput::prompt_text);
        let error = self
            .error_report
            .as_ref()
            .map(|report| report.message.clone());
        let len = 2
            + usize::from(self.turn_number.is_some())
            + usize::from(self.prompt_input.is_some())
            + usize::from(prompt.is_some())
            + usize::from(self.error_report.is_some())
            + usize::from(self.error_class.is_some())
            + usize::from(error.is_some())
            + usize::from(self.llm_request.is_some())
            + usize::from(self.llm_response.is_some())
            + usize::from(self.tool_call.is_some())
            + usize::from(self.tool_result.is_some());
        let mut state = serializer.serialize_struct("HookInvocation", len)?;
        state.serialize_field("point", &self.point)?;
        state.serialize_field("session_id", &self.session_id)?;
        if let Some(turn_number) = &self.turn_number {
            state.serialize_field("turn_number", turn_number)?;
        }
        if let Some(prompt_input) = &self.prompt_input {
            state.serialize_field("prompt_input", prompt_input)?;
        }
        if let Some(prompt) = &prompt {
            state.serialize_field("prompt", prompt)?;
        }
        if let Some(error_report) = &self.error_report {
            state.serialize_field("error_report", error_report)?;
        }
        if let Some(error_class) = &self.error_class {
            state.serialize_field("error_class", error_class)?;
        }
        if let Some(error) = &error {
            state.serialize_field("error", error)?;
        }
        if let Some(llm_request) = &self.llm_request {
            state.serialize_field("llm_request", llm_request)?;
        }
        if let Some(llm_response) = &self.llm_response {
            state.serialize_field("llm_response", llm_response)?;
        }
        if let Some(tool_call) = &self.tool_call {
            state.serialize_field("tool_call", tool_call)?;
        }
        if let Some(tool_result) = &self.tool_result {
            state.serialize_field("tool_result", tool_result)?;
        }
        state.end()
    }
}

impl HookInvocation {
    pub fn new(point: HookPoint, session_id: SessionId) -> Self {
        Self {
            point,
            session_id,
            turn_number: None,
            prompt_input: None,
            error_report: None,
            error_class: None,
            llm_request: None,
            llm_response: None,
            tool_call: None,
            tool_result: None,
        }
    }

    pub fn run_started(session_id: SessionId, prompt_input: RunInput) -> Self {
        Self {
            prompt_input: Some(prompt_input),
            ..Self::new(HookPoint::RunStarted, session_id)
        }
    }

    pub fn run_completed(session_id: SessionId, turn_number: u32) -> Self {
        Self {
            turn_number: Some(turn_number),
            ..Self::new(HookPoint::RunCompleted, session_id)
        }
    }

    pub fn run_failed(session_id: SessionId, error: &AgentError) -> Self {
        let error_report = AgentErrorReport::from_agent_error(error);
        Self {
            error_class: Some(error_report.class),
            error_report: Some(error_report),
            ..Self::new(HookPoint::RunFailed, session_id)
        }
    }
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
    /// Typed failure cause for this hook outcome, when the hook did not succeed.
    /// The string form is derived via [`HookOutcome::failure_message`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<HookFailureReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl HookOutcome {
    /// Display projection of the typed [`HookOutcome::failure_reason`], when
    /// present. This is presentation-only — policy code must branch on the
    /// typed `failure_reason` variant.
    #[must_use]
    pub fn failure_message(&self) -> Option<String> {
        self.failure_reason.as_ref().map(ToString::to_string)
    }
}

/// Aggregate result used by the core loop to apply hook decisions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub struct HookExecutionReport {
    /// Hook ids the engine actually began executing — foreground entries it ran
    /// and background entries it acquired a permit for and spawned. This is the
    /// authoritative basis for `HookStarted` events: a hook only appears here
    /// once execution began, never merely because it matched the invocation
    /// point (a foreground deny short-circuit and a saturated background queue
    /// both leave later/skipped hooks absent).
    #[serde(default)]
    pub started: Vec<HookId>,
    #[serde(default)]
    pub outcomes: Vec<HookOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<HookDecision>,
}

impl HookExecutionReport {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Project an authoritative hook denial into the typed agent error shape.
    ///
    /// Runtime policy owns whether the returned error terminalizes the run.
    /// This projection only preserves the denial facts emitted by the hook
    /// engine without reclassifying them through string matching.
    pub fn denial_error(&self, point: HookPoint) -> Option<AgentError> {
        match self.decision.as_ref()? {
            HookDecision::Deny {
                hook_id,
                reason_code,
                message,
                payload,
            } => Some(AgentError::HookDenied {
                hook_id: hook_id.clone(),
                point,
                reason_code: *reason_code,
                message: message.clone(),
                payload: payload.clone(),
            }),
            HookDecision::Allow => None,
        }
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

impl HookEngineError {
    pub fn hook_id(&self) -> Option<&HookId> {
        match self {
            Self::InvalidConfiguration(_) => None,
            Self::ExecutionFailed { hook_id, .. } | Self::Timeout { hook_id, .. } => Some(hook_id),
        }
    }

    pub fn into_agent_error(self) -> AgentError {
        match self {
            Self::InvalidConfiguration(reason) => AgentError::HookConfigInvalid { reason },
            Self::Timeout {
                hook_id,
                timeout_ms,
            } => AgentError::HookTimeout {
                hook_id,
                timeout_ms,
            },
            Self::ExecutionFailed { hook_id, reason } => {
                AgentError::HookExecutionFailed { hook_id, reason }
            }
        }
    }
}

/// Runtime-independent engine interface.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, ToolResult};

    fn text_block(s: &str) -> ContentBlock {
        ContentBlock::Text {
            text: s.to_string(),
        }
    }

    fn image_block(media_type: &str, data: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: media_type.to_string(),
            data: data.into(),
        }
    }

    #[test]
    fn hook_tool_call_rejects_string_args_on_deserialize() {
        let value = serde_json::json!({
            "tool_use_id": "tc_1",
            "name": "search",
            "args": "{\"query\":"
        });

        let err = serde_json::from_value::<HookToolCall>(value)
            .expect_err("hook surface must reject string-success tool args");
        assert!(
            err.to_string().contains("JSON object, got string"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hook_result_from_multimodal_uses_text_projection() {
        let tr = ToolResult::with_blocks(
            "tc_1".into(),
            vec![text_block("hello"), image_block("image/png", "AAAA")],
            false,
        );
        let hook_result = HookToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            name: "test_tool".into(),
            content_blocks: tr.content.clone(),
            is_error: tr.is_error,
            provenance: None,
        };
        // text projection concatenates text from blocks; image blocks produce "[image: image/png]"
        assert_eq!(hook_result.text_projection(), "hello\n[image: image/png]");
    }

    #[test]
    fn hook_result_text_only_uses_text_projection() {
        let tr = ToolResult::new("tc_1".into(), "just text".into(), false);
        let hook_result = HookToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            name: "test_tool".into(),
            content_blocks: tr.content.clone(),
            is_error: tr.is_error,
            provenance: None,
        };
        assert_eq!(hook_result.text_projection(), "just text");
        assert_eq!(hook_result.content_blocks, vec![text_block("just text")]);
    }

    #[test]
    fn hook_result_text_only_serializes_typed_content_blocks() {
        let tr = ToolResult::new("tc_1".into(), "just text".into(), false);
        let hook_result = HookToolResult::from_tool_result("test_tool", &tr);

        assert_eq!(hook_result.text_projection(), "just text");
        assert_eq!(hook_result.content_blocks, vec![text_block("just text")]);

        let json = serde_json::to_value(&hook_result).expect("serialize hook tool result");
        assert_eq!(
            json["content_blocks"],
            serde_json::json!([{"type": "text", "text": "just text"}])
        );
        // The wire envelope serializes a derived `content` text projection for
        // external hook consumers (row #331), derived purely from the blocks.
        assert_eq!(
            json["content"],
            serde_json::json!("just text"),
            "wire envelope must carry the derived `content` text projection"
        );
    }

    #[test]
    fn hook_result_image_only_serializes_typed_content_blocks() {
        let tr =
            ToolResult::with_blocks("tc_1".into(), vec![image_block("image/png", "AAAA")], false);
        let hook_result = HookToolResult::from_tool_result("view_image", &tr);

        assert_eq!(hook_result.text_projection(), "[image: image/png]");
        assert_eq!(
            hook_result.content_blocks,
            vec![image_block("image/png", "AAAA")]
        );

        let json = serde_json::to_value(&hook_result).expect("serialize hook tool result");
        assert_eq!(
            json["content_blocks"],
            serde_json::json!([{
                "type": "image",
                "media_type": "image/png",
                "source": "inline",
                "data": "AAAA"
            }])
        );
    }

    #[test]
    fn hook_result_mixed_content_preserves_block_order() {
        let tr = ToolResult::with_blocks(
            "tc_1".into(),
            vec![
                text_block("before"),
                image_block("image/png", "AAAA"),
                text_block("after"),
            ],
            false,
        );
        let hook_result = HookToolResult::from_tool_result("mixed_tool", &tr);

        assert_eq!(
            hook_result.text_projection(),
            "before\n[image: image/png]\nafter"
        );
        assert_eq!(hook_result.content_blocks, tr.content);
    }

    #[test]
    fn hook_result_can_use_authoritative_tool_call_id() {
        let tr = ToolResult::new("stale_tool_id".into(), "ok".into(), false);
        let hook_result =
            HookToolResult::from_tool_result_with_id("active_tool_id", "test_tool", &tr);

        assert_eq!(hook_result.tool_use_id, "active_tool_id");
        assert_eq!(hook_result.content_blocks, vec![text_block("ok")]);
    }

    #[test]
    fn hook_tool_result_text_projection_is_derived_only_from_typed_blocks() {
        // The text projection must be derived purely from the canonical typed
        // content_blocks — there is no separately-stored `content` mirror that
        // could diverge from the blocks or feed policy.
        let result = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "tool".into(),
            content_blocks: vec![text_block("alpha"), image_block("image/png", "AAAA")],
            is_error: false,
            provenance: None,
        };
        assert_eq!(
            result.text_projection(),
            crate::types::text_content(&result.content_blocks),
            "text projection must equal the rendering of the typed blocks"
        );

        // Mutating the typed blocks immediately changes the projection (no stale
        // stored mirror).
        let mut mutated = result;
        mutated.content_blocks = vec![text_block("beta")];
        assert_eq!(mutated.text_projection(), "beta");
    }

    #[test]
    fn hook_tool_result_ignores_incoming_content_string_as_authority() {
        // The incoming `content` string is not ingested as authority; the
        // canonical content comes from `content_blocks`.
        let decoded: HookToolResult = serde_json::from_value(serde_json::json!({
            "tool_use_id": "tc_1",
            "name": "tool",
            "content": "ignored-incoming-string",
            "content_blocks": [{"type": "text", "text": "text"}],
            "is_error": false
        }))
        .expect("should deserialize");
        assert_eq!(decoded.content_blocks, vec![text_block("text")]);
        assert_eq!(decoded.text_projection(), "text");
    }

    #[test]
    fn hook_tool_result_wire_content_round_trips_from_blocks() {
        // The wire envelope serializes `content` (derived) + `content_blocks`
        // (canonical); deserializing the envelope reconstructs the canonical
        // blocks and the derived `content` matches the text projection.
        let original = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "tool".into(),
            content_blocks: vec![text_block("alpha"), image_block("image/png", "AAAA")],
            is_error: false,
            provenance: None,
        };
        let json = serde_json::to_value(&original).expect("serialize");
        assert_eq!(
            json["content"],
            serde_json::json!(original.text_projection())
        );
        let decoded: HookToolResult = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.content_blocks, original.content_blocks);
        assert_eq!(decoded.text_projection(), original.text_projection());
    }

    #[test]
    fn hook_invocation_prompt_and_error_are_serialize_only_projections() {
        let mut invocation = HookInvocation::run_started(
            SessionId::new(),
            RunInput::Content {
                content: crate::types::ContentInput::Text("typed prompt".to_string()),
            },
        );
        invocation.error_report = Some(AgentErrorReport {
            class: AgentErrorClass::Llm,
            reason: None,
            message: "typed failure".to_string(),
        });

        let json = serde_json::to_value(&invocation).expect("serialize");
        // The wire envelope derives `prompt`/`error` from the typed owners at
        // serialization time for external hook consumers.
        assert_eq!(json["prompt"], serde_json::json!("typed prompt"));
        assert_eq!(json["error"], serde_json::json!("typed failure"));

        // Deserializing a payload with divergent string mirrors must NOT
        // resurrect them as authority — only the typed owners survive.
        let mut forged = json;
        forged["prompt"] = serde_json::json!("forged prompt");
        forged["error"] = serde_json::json!("forged failure");
        let decoded: HookInvocation = serde_json::from_value(forged).expect("deserialize");
        assert_eq!(decoded, invocation);
        assert_eq!(
            serde_json::to_value(&decoded).expect("re-serialize")["prompt"],
            serde_json::json!("typed prompt")
        );
    }

    /// Ask 5 wire discipline: `provenance` is additive on the external hook
    /// envelope — omitted when absent (old payloads stay byte-stable), a
    /// typed projection of `ToolDef.provenance` when present, and round-trips
    /// through the custom `HookToolResult` serializer.
    #[test]
    fn hook_tool_payload_provenance_is_additive_and_round_trips() {
        use crate::types::{ToolProvenance, ToolSourceId, ToolSourceKind};

        let call = HookToolCall {
            tool_use_id: "tc_1".into(),
            name: "lookup".into(),
            args: ToolCallArguments::empty(),
            provenance: None,
        };
        let json = serde_json::to_value(&call).expect("serialize");
        assert!(
            json.get("provenance").is_none(),
            "absent provenance must be omitted from the hook wire envelope"
        );

        let provenance = ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: ToolSourceId::new("test-server"),
        };
        let call_with = HookToolCall {
            provenance: Some(provenance.clone()),
            ..call
        };
        let json = serde_json::to_value(&call_with).expect("serialize");
        assert_eq!(json["provenance"]["kind"], serde_json::json!("mcp"));
        let decoded: HookToolCall = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.provenance, Some(provenance.clone()));

        let result = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "lookup".into(),
            content_blocks: vec![text_block("ok")],
            is_error: false,
            provenance: None,
        };
        let json = serde_json::to_value(&result).expect("serialize");
        assert!(
            json.get("provenance").is_none(),
            "absent provenance must be omitted by the custom serializer"
        );
        let result_with = result.with_provenance(Some(provenance.clone()));
        let json = serde_json::to_value(&result_with).expect("serialize");
        assert_eq!(
            json["provenance"]["source_id"],
            serde_json::json!("test-server")
        );
        let decoded: HookToolResult = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.provenance, Some(provenance));
    }

    /// Ask 5 wire discipline: `server_tool_content` is a projection of the
    /// response's `AssistantBlock::ServerToolContent` kinds — an empty list is
    /// omitted from the envelope (old payloads stay byte-stable) and typed
    /// kinds round-trip.
    #[test]
    fn hook_llm_response_server_tool_content_is_additive_and_typed() {
        use crate::types::ServerToolKind;

        let response = HookLlmResponse {
            assistant_text: "done".into(),
            tool_call_names: Vec::new(),
            stop_reason: Some(StopReason::EndTurn),
            usage: None,
            server_tool_content: Vec::new(),
        };
        let json = serde_json::to_value(&response).expect("serialize");
        assert!(
            json.get("server_tool_content").is_none(),
            "empty server tool content must be omitted from the hook wire envelope"
        );

        let response = HookLlmResponse {
            server_tool_content: vec![
                ServerToolKind::WebSearch,
                ServerToolKind::ProviderNative {
                    name: "code_exec".to_string(),
                },
            ],
            ..response
        };
        let json = serde_json::to_value(&response).expect("serialize");
        assert_eq!(
            json["server_tool_content"][0]["kind"],
            serde_json::json!("web_search")
        );
        let decoded: HookLlmResponse = serde_json::from_value(json).expect("deserialize");
        assert_eq!(decoded.server_tool_content, response.server_tool_content);
    }

    /// K3 invariant: a pending-tool-results run carries the typed
    /// `RunInput::PendingToolResults` variant — no fabricated empty-string
    /// `prompt` projection appears on the hook wire envelope.
    #[test]
    fn hook_invocation_pending_tail_run_has_typed_variant_and_no_prompt_mirror() {
        let invocation =
            HookInvocation::run_started(SessionId::new(), RunInput::PendingToolResults);
        assert_eq!(
            invocation.prompt_input,
            Some(RunInput::PendingToolResults),
            "pending-tail run must carry the typed variant"
        );

        let json = serde_json::to_value(&invocation).expect("serialize");
        assert_eq!(
            json["prompt_input"],
            serde_json::json!({ "kind": "pending_tool_results" })
        );
        assert!(
            json.get("prompt").is_none(),
            "no empty-string prompt mirror may be fabricated for pending-tail runs: {json}"
        );
    }
}

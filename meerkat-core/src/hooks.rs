//! Hook contracts and engine interfaces.

use crate::error::AgentError;
use crate::event::{AgentErrorClass, AgentErrorReport, ToolCallArguments};
use crate::types::{ContentBlock, ContentInput, SessionId, StopReason, ToolResult, Usage};
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
    /// Legacy fail-closed capability label. Semantic patch authority has been
    /// removed from hooks; retained entries may observe and deny only.
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

/// Retired hook patch surface.
///
/// Hook patches previously allowed hooks to rewrite provider parameters,
/// assistant text, tool arguments/results, and final run text. Those semantic
/// mutations are no longer hook-authorized; this enum intentionally has no
/// variants, so legacy patch payloads fail deserialization instead of being
/// silently ignored or applied.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "patch_type", rename_all = "snake_case")]
pub enum HookPatch {}

/// Monotonic patch revision metadata.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct HookRevision(pub u64);

/// Retired envelope for legacy async patch publication.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookPatchEnvelope {
    pub revision: HookRevision,
    pub hook_id: HookId,
    pub point: HookPoint,
    pub patch: HookPatch,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
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
    pub args: ToolCallArguments,
}

/// Tool result view exposed to hooks.
///
/// `content_blocks` is the canonical typed tool-result content that post-tool
/// hooks deny/terminalize on. The text projection is presentation-only and is
/// derived from the blocks via [`HookToolResult::text_projection`]; it is never
/// a separately-stored field that policy code can read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolResult {
    pub tool_use_id: String,
    pub name: String,
    /// Canonical typed tool-result content exposed to hooks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_blocks: Vec<ContentBlock>,
    pub is_error: bool,
    /// Legacy side flag retained for Rust compatibility. New hook payloads
    /// carry `content_blocks` instead of serializing this projection hint.
    #[serde(default, skip_serializing)]
    pub has_images: bool,
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
            has_images: result.has_images(),
        }
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookInvocation {
    pub point: HookPoint,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_number: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_input: Option<ContentInput>,
    /// Text-only projection of `prompt_input` for legacy hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_report: Option<AgentErrorReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<AgentErrorClass>,
    /// Display projection of `error_report.message` for legacy hooks.
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

impl HookInvocation {
    pub fn new(point: HookPoint, session_id: SessionId) -> Self {
        Self {
            point,
            session_id,
            turn_number: None,
            prompt_input: None,
            prompt: None,
            error_report: None,
            error_class: None,
            error: None,
            llm_request: None,
            llm_response: None,
            tool_call: None,
            tool_result: None,
        }
    }

    pub fn run_started(session_id: SessionId, prompt_input: ContentInput) -> Self {
        let prompt = prompt_input.text_content();
        Self {
            prompt_input: Some(prompt_input),
            prompt: Some(prompt),
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
            error: Some(error_report.message.clone()),
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
    #[serde(default)]
    pub patches: Vec<HookPatch>,
    #[serde(default)]
    pub published_patches: Vec<HookPatchEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Aggregate result used by the core loop to apply hook decisions.
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

    /// Drain retired background patch publications for one session.
    async fn drain_published_patches(
        &self,
        _session_id: &SessionId,
    ) -> Result<Vec<HookPatchEnvelope>, HookEngineError> {
        Ok(Vec::new())
    }
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
            has_images: tr.has_images(),
        };
        // text projection concatenates text from blocks; image blocks produce "[image: image/png]"
        assert_eq!(hook_result.text_projection(), "hello\n[image: image/png]");
        assert!(hook_result.has_images);
    }

    #[test]
    fn hook_result_text_only_has_images_false() {
        let tr = ToolResult::new("tc_1".into(), "just text".into(), false);
        let hook_result = HookToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            name: "test_tool".into(),
            content_blocks: tr.content.clone(),
            is_error: tr.is_error,
            has_images: tr.has_images(),
        };
        assert_eq!(hook_result.text_projection(), "just text");
        assert_eq!(hook_result.content_blocks, vec![text_block("just text")]);
        assert!(!hook_result.has_images);
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
        assert!(
            json.get("content").is_none(),
            "the legacy text mirror must not be serialized on the hook surface"
        );
        assert!(
            json.get("has_images").is_none(),
            "typed content blocks should replace the image side flag on the hook surface"
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
        assert!(
            json.get("has_images").is_none(),
            "typed content blocks should replace the image side flag on the hook surface"
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
            has_images: true,
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
    fn hook_tool_result_has_images_serde_default() {
        // Verify has_images defaults to false when deserializing JSON without it.
        // This ensures backwards compatibility with existing hook payloads.
        let json = r#"{
            "tool_use_id": "tc_1",
            "name": "test",
            "content": "hello",
            "is_error": false
        }"#;
        let result: HookToolResult =
            serde_json::from_str(json).expect("should deserialize without has_images");
        assert!(!result.has_images);
    }

    #[test]
    fn hook_tool_result_has_images_is_deserialize_only_legacy_flag() {
        let result = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "tool".into(),
            content_blocks: vec![text_block("text")],
            is_error: false,
            has_images: true,
        };
        let json = serde_json::to_value(&result).expect("should serialize");
        assert!(
            json.get("has_images").is_none(),
            "new hook payloads carry content_blocks instead of has_images"
        );
        assert!(
            json.get("content").is_none(),
            "the legacy text mirror is no longer a stored field on the hook surface"
        );

        let decoded: HookToolResult = serde_json::from_value(serde_json::json!({
            "tool_use_id": "tc_1",
            "name": "tool",
            "content": "text",
            "content_blocks": [{"type": "text", "text": "text"}],
            "is_error": false,
            "has_images": true
        }))
        .expect("should deserialize");
        assert!(decoded.has_images);
    }

    #[test]
    fn legacy_semantic_hook_patches_are_rejected_on_deserialize() {
        let legacy_payloads = [
            serde_json::json!({
                "patch_type": "llm_request",
                "max_tokens": 1,
                "temperature": 0.1,
                "provider_params": {"reasoning_effort": "low"}
            }),
            serde_json::json!({
                "patch_type": "assistant_text",
                "text": "patched"
            }),
            serde_json::json!({
                "patch_type": "tool_args",
                "args": {"value": "patched"}
            }),
            serde_json::json!({
                "patch_type": "tool_result",
                "content": "patched",
                "is_error": false
            }),
            serde_json::json!({
                "patch_type": "run_result",
                "text": "patched"
            }),
        ];

        for value in legacy_payloads {
            let result = serde_json::from_value::<HookPatch>(value.clone());
            assert!(
                result.is_err(),
                "legacy semantic hook patch payload must be rejected: {value}"
            );
        }
    }
}

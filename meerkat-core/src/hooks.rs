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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct HookRevision(pub u64);

/// Stable envelope emitted for async patch publication.
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
    pub args: Value,
}

/// Tool result view exposed to hooks.
///
/// The `content` field is always the text projection of the tool result.
/// When `ToolResult.content` migrates to `Vec<ContentBlock>`, this will
/// contain the concatenated text and `has_images` will signal the presence
/// of non-text blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct HookToolResult {
    pub tool_use_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    /// Whether the original tool result contains image blocks.
    /// Hooks always see the text projection; this flag signals that
    /// non-text content exists so hook authors can make informed decisions.
    #[serde(default)]
    pub has_images: bool,
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

/// Apply a `HookPatch::ToolResult` to a `ToolResult`, preserving image blocks.
///
/// Deterministic rebuild rule:
/// 1. Strip all `ContentBlock::Text` blocks from the original vec.
/// 2. Prepend a single `ContentBlock::Text { text: patched_text }` at position 0.
/// 3. Append all image blocks in their original relative order.
pub fn apply_tool_result_patch(
    tool_result: &mut crate::types::ToolResult,
    patched_text: String,
    is_error: Option<bool>,
) {
    use crate::types::ContentBlock;

    let image_blocks: Vec<ContentBlock> = tool_result
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::Image { .. }))
        .cloned()
        .collect();
    let mut new_content = vec![ContentBlock::Text { text: patched_text }];
    new_content.extend(image_blocks);
    tool_result.content = new_content;
    if let Some(value) = is_error {
        tool_result.is_error = value;
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
    fn hook_result_from_multimodal_uses_text_projection() {
        let tr = ToolResult::with_blocks(
            "tc_1".into(),
            vec![text_block("hello"), image_block("image/png", "AAAA")],
            false,
        );
        let hook_result = HookToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            name: "test_tool".into(),
            content: tr.text_content(),
            is_error: tr.is_error,
            has_images: tr.has_images(),
        };
        // text_content concatenates text projections; image blocks produce "[image: image/png]"
        assert_eq!(hook_result.content, "hello\n[image: image/png]");
        assert!(hook_result.has_images);
    }

    #[test]
    fn hook_result_text_only_has_images_false() {
        let tr = ToolResult::new("tc_1".into(), "just text".into(), false);
        let hook_result = HookToolResult {
            tool_use_id: tr.tool_use_id.clone(),
            name: "test_tool".into(),
            content: tr.text_content(),
            is_error: tr.is_error,
            has_images: tr.has_images(),
        };
        assert_eq!(hook_result.content, "just text");
        assert!(!hook_result.has_images);
    }

    #[test]
    fn hook_patch_replaces_text_preserves_images() {
        let mut tr = ToolResult::with_blocks(
            "tc_1".into(),
            vec![
                text_block("original text"),
                image_block("image/png", "AAAA"),
                image_block("image/jpeg", "BBBB"),
            ],
            false,
        );
        apply_tool_result_patch(&mut tr, "patched text".into(), None);
        assert_eq!(tr.content.len(), 3);
        assert_eq!(
            tr.content[0],
            ContentBlock::Text {
                text: "patched text".into()
            }
        );
        assert!(
            matches!(&tr.content[1], ContentBlock::Image { media_type, data, .. }
            if media_type == "image/png"
                && matches!(data, crate::types::ImageData::Inline { data } if data == "AAAA"))
        );
        assert!(
            matches!(&tr.content[2], ContentBlock::Image { media_type, data, .. }
            if media_type == "image/jpeg"
                && matches!(data, crate::types::ImageData::Inline { data } if data == "BBBB"))
        );
    }

    #[test]
    fn hook_patch_text_only_unchanged() {
        let mut tr = ToolResult::new("tc_1".into(), "original".into(), false);
        apply_tool_result_patch(&mut tr, "patched".into(), None);
        assert_eq!(tr.content.len(), 1);
        assert_eq!(tr.text_content(), "patched");
        assert!(!tr.is_error);
    }

    #[test]
    fn hook_patch_image_only_result_prepends_text() {
        let mut tr =
            ToolResult::with_blocks("tc_1".into(), vec![image_block("image/png", "AAAA")], false);
        apply_tool_result_patch(&mut tr, "added text".into(), None);
        assert_eq!(tr.content.len(), 2);
        assert_eq!(
            tr.content[0],
            ContentBlock::Text {
                text: "added text".into()
            }
        );
        assert!(matches!(&tr.content[1], ContentBlock::Image { .. }));
    }

    #[test]
    fn hook_patch_interleaved_reorders_text_before_images() {
        // [Text("a"), Image(X), Text("b"), Image(Y)] + patch "c"
        // -> [Text("c"), Image(X), Image(Y)]
        let mut tr = ToolResult::with_blocks(
            "tc_1".into(),
            vec![
                text_block("a"),
                image_block("image/png", "X"),
                text_block("b"),
                image_block("image/jpeg", "Y"),
            ],
            false,
        );
        apply_tool_result_patch(&mut tr, "c".into(), None);
        assert_eq!(tr.content.len(), 3);
        assert_eq!(tr.content[0], ContentBlock::Text { text: "c".into() });
        assert!(
            matches!(&tr.content[1], ContentBlock::Image { media_type, data, .. }
            if media_type == "image/png"
                && matches!(data, crate::types::ImageData::Inline { data } if data == "X"))
        );
        assert!(
            matches!(&tr.content[2], ContentBlock::Image { media_type, data, .. }
            if media_type == "image/jpeg"
                && matches!(data, crate::types::ImageData::Inline { data } if data == "Y"))
        );
    }

    #[test]
    fn hook_patch_sets_is_error() {
        let mut tr = ToolResult::new("tc_1".into(), "ok".into(), false);
        apply_tool_result_patch(&mut tr, "error".into(), Some(true));
        assert!(tr.is_error);
        assert_eq!(tr.text_content(), "error");
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
    fn hook_tool_result_has_images_roundtrip() {
        let result = HookToolResult {
            tool_use_id: "tc_1".into(),
            name: "tool".into(),
            content: "text".into(),
            is_error: false,
            has_images: true,
        };
        let json = serde_json::to_string(&result).expect("should serialize");
        let decoded: HookToolResult = serde_json::from_str(&json).expect("should deserialize");
        assert!(decoded.has_images);
    }
}

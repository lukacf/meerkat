//! Core types for Meerkat
//!
//! These types form the representation boundary for session persistence and wire formats.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::fmt;
use uuid::Uuid;

use crate::blob::{BlobId, BlobRef};
use crate::image_generation::{
    AssistantImageId, MediaType, ProviderImageMetadata, RevisedPromptDisposition,
};
use crate::schema::{MeerkatSchema, SchemaCompat, SchemaError, SchemaFormat, SchemaWarning};

/// Wall-clock timestamp for canonical transcript messages.
pub type MessageTimestamp = chrono::DateTime<chrono::Utc>;

pub fn message_timestamp_now() -> MessageTimestamp {
    chrono::Utc::now()
}

/// Stable runtime identity for a transcript message.
///
/// These fields are optional so older persisted sessions deserialize without a
/// migration, while new runtime-backed turns can expose the same identity that
/// live event streams carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct TranscriptMessageIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<crate::interaction::InteractionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<crate::lifecycle::RunId>,
    /// Durable delegated-objective causality, shared by every message in a
    /// causally-related turn chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_id: Option<crate::interaction::ObjectiveId>,
}

impl TranscriptMessageIdentity {
    pub fn is_empty(&self) -> bool {
        self.interaction_id.is_none() && self.run_id.is_none() && self.objective_id.is_none()
    }

    pub fn with_run_id(&self, run_id: crate::lifecycle::RunId) -> Self {
        Self {
            interaction_id: self.interaction_id,
            run_id: Some(run_id),
            objective_id: self.objective_id,
        }
    }
}

// ===========================================================================
// Multimodal content blocks
// ===========================================================================

pub const SUPPORTED_VIDEO_MEDIA_TYPES: &[&str] = &["video/mp4", "video/webm", "video/quicktime"];

/// A block of content in user messages and tool results.
///
/// Supports text and images for multimodal agent interactions.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ImageData {
    /// Base64-encoded inline bytes used for ingress and live execution.
    Inline { data: String },
    /// Durable blob reference used by persisted history/runtime state.
    Blob { blob_id: BlobId },
}

impl From<String> for ImageData {
    fn from(data: String) -> Self {
        Self::Inline { data }
    }
}

impl From<&str> for ImageData {
    fn from(data: &str) -> Self {
        Self::Inline {
            data: data.to_string(),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum VideoData {
    /// Base64-encoded inline bytes used for ingress and live execution.
    Inline { data: String },
    /// Provider-readable media URI used for by-reference video input.
    Uri { uri: String },
}

impl From<String> for VideoData {
    fn from(data: String) -> Self {
        Self::Inline { data }
    }
}

impl From<&str> for VideoData {
    fn from(data: &str) -> Self {
        Self::Inline {
            data: data.to_string(),
        }
    }
}

#[non_exhaustive]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content.
    Text { text: String },
    /// Base64-encoded image content.
    Image {
        /// MIME type (e.g. "image/png", "image/jpeg").
        media_type: String,
        /// Image bytes or a durable blob reference.
        #[serde(flatten)]
        data: ImageData,
    },
    /// Base64-encoded video content.
    Video {
        /// MIME type (e.g. "video/mp4", "video/webm").
        media_type: String,
        /// Caller-provided clip duration in milliseconds.
        duration_ms: u64,
        /// Inline video bytes.
        #[serde(flatten)]
        data: VideoData,
    },
    /// Structured JSON content returned by a tool, preserved as JSON rather
    /// than laundered through a stringified `Text` block.
    ///
    /// Tools whose result is structured data (lists, records) emit this so the
    /// canonical transcript carries the typed JSON shape. The payload is opaque
    /// pass-through JSON (`Box<RawValue>`, the allow-listed §3 pattern shared
    /// with `ToolUse` args): Meerkat does not impose a closed schema on
    /// arbitrary tool output. The model-facing text projection is *derived*
    /// from this JSON, never the other way around.
    Structured {
        /// Provider-opaque structured JSON, parsed at most once by consumers.
        #[serde(
            serialize_with = "serialize_structured_data",
            deserialize_with = "deserialize_structured_data"
        )]
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        data: Box<RawValue>,
    },
    /// Rendered skill context injected by per-turn skill activation.
    ///
    /// The typed `skill_key` preserves the activation provenance in the
    /// durable transcript — activation is never folded into anonymous
    /// operator text. `text` is the rendered skill body; the model-facing
    /// representation is the text projection of this block.
    SkillContext {
        /// Canonical identity of the activated skill.
        skill_key: crate::skills::SkillKey,
        /// Rendered skill body delivered to the model.
        text: String,
    },
}

/// Serializes a `Box<RawValue>` structured payload as inline JSON (no double
/// string-encoding).
fn serialize_structured_data<S>(data: &RawValue, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    data.serialize(serializer)
}

/// Deserializes a structured payload via `Value` so it survives the
/// internally-tagged `Message` buffering step, mirroring `deserialize_tool_use_args`.
fn deserialize_structured_data<'de, D>(deserializer: D) -> Result<Box<RawValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
    RawValue::from_string(raw).map_err(serde::de::Error::custom)
}

impl ContentBlock {
    /// Canonical text projection for display, hooks, and indexing.
    ///
    /// Text blocks return their content. Media blocks return `[kind: {media_type}]`.
    /// `source_path` is internal metadata and is NOT included in the projection.
    pub fn text_projection(&self) -> Cow<'_, str> {
        match self {
            ContentBlock::Text { text } => Cow::Borrowed(text),
            ContentBlock::Image { media_type, .. } => Cow::Owned(format!("[image: {media_type}]")),
            ContentBlock::Video { media_type, .. } => Cow::Owned(format!("[video: {media_type}]")),
            ContentBlock::Structured { data } => Cow::Owned(data.get().to_string()),
            ContentBlock::SkillContext { text, .. } => Cow::Borrowed(text),
        }
    }

    /// Convenience: wrap a string into a single-element Vec of Text blocks.
    pub fn text_vec(s: String) -> Vec<ContentBlock> {
        vec![ContentBlock::Text { text: s }]
    }

    /// Construct a structured-JSON content block from any serializable value.
    ///
    /// This is the typed path tools use instead of stringifying structured
    /// output into a `Text` block. Fails closed if the value cannot be
    /// serialized to JSON.
    pub fn structured<T: Serialize>(value: &T) -> Result<Self, serde_json::Error> {
        let raw = serde_json::value::to_raw_value(value)?;
        Ok(ContentBlock::Structured { data: raw })
    }

    /// Borrow the raw structured JSON payload, if this is a `Structured` block.
    pub fn structured_data(&self) -> Option<&RawValue> {
        match self {
            ContentBlock::Structured { data } => Some(data),
            _ => None,
        }
    }

    pub fn image_inline_data(&self) -> Option<(&str, &str)> {
        match self {
            ContentBlock::Image {
                media_type,
                data: ImageData::Inline { data },
            } => Some((media_type, data)),
            _ => None,
        }
    }

    pub fn image_blob_ref(&self) -> Option<(&str, &BlobId)> {
        match self {
            ContentBlock::Image {
                media_type,
                data: ImageData::Blob { blob_id },
            } => Some((media_type, blob_id)),
            _ => None,
        }
    }

    pub fn video_inline_data(&self) -> Option<(&str, u64, &str)> {
        match self {
            ContentBlock::Video {
                media_type,
                duration_ms,
                data: VideoData::Inline { data },
            } => Some((media_type, *duration_ms, data)),
            _ => None,
        }
    }

    pub fn video_uri(&self) -> Option<(&str, u64, &str)> {
        match self {
            ContentBlock::Video {
                media_type,
                duration_ms,
                data: VideoData::Uri { uri },
            } => Some((media_type, *duration_ms, uri)),
            _ => None,
        }
    }
}

impl fmt::Display for ContentBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text_projection())
    }
}

impl PartialEq for ContentBlock {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ContentBlock::Text { text: t1 }, ContentBlock::Text { text: t2 }) => t1 == t2,
            (
                ContentBlock::Image {
                    media_type: mt1,
                    data: d1,
                },
                ContentBlock::Image {
                    media_type: mt2,
                    data: d2,
                },
            ) => mt1 == mt2 && d1 == d2,
            (
                ContentBlock::Video {
                    media_type: mt1,
                    duration_ms: dur1,
                    data: d1,
                },
                ContentBlock::Video {
                    media_type: mt2,
                    duration_ms: dur2,
                    data: d2,
                },
            ) => mt1 == mt2 && dur1 == dur2 && d1 == d2,
            // `RawValue` carries no `PartialEq`; compare the canonical JSON text,
            // mirroring `AssistantBlock::ToolUse` args equality.
            (ContentBlock::Structured { data: d1 }, ContentBlock::Structured { data: d2 }) => {
                d1.get() == d2.get()
            }
            (
                ContentBlock::SkillContext {
                    skill_key: k1,
                    text: t1,
                },
                ContentBlock::SkillContext {
                    skill_key: k2,
                    text: t2,
                },
            ) => k1 == k2 && t1 == t2,
            _ => false,
        }
    }
}

impl Eq for ContentBlock {}

/// Concatenate text projections from a slice of content blocks.
pub fn text_content(blocks: &[ContentBlock]) -> String {
    let mut result = String::new();
    for block in blocks {
        let projection = block.text_projection();
        if !result.is_empty() && !projection.is_empty() {
            result.push('\n');
        }
        result.push_str(&projection);
    }
    result
}

/// Check if any content blocks contain images.
pub fn has_images(blocks: &[ContentBlock]) -> bool {
    blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }))
}

/// Check if any content blocks contain video.
pub fn has_video(blocks: &[ContentBlock]) -> bool {
    blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Video { .. }))
}

/// Check if any content blocks are non-text.
pub fn has_non_text_content(blocks: &[ContentBlock]) -> bool {
    blocks
        .iter()
        .any(|b| !matches!(b, ContentBlock::Text { .. }))
}

pub fn is_supported_video_media_type(media_type: &str) -> bool {
    SUPPORTED_VIDEO_MEDIA_TYPES.contains(&media_type)
}

pub fn validate_inline_video_blocks(blocks: &[ContentBlock]) -> Result<(), String> {
    for block in blocks {
        if let ContentBlock::Video {
            media_type,
            duration_ms,
            data,
        } = block
        {
            if !is_supported_video_media_type(media_type) {
                return Err(format!(
                    "unsupported video media type '{media_type}'; expected one of {}",
                    SUPPORTED_VIDEO_MEDIA_TYPES.join(", ")
                ));
            }
            if *duration_ms == 0 {
                return Err(format!(
                    "video block duration_ms must be greater than 0 for {media_type}"
                ));
            }
            match data {
                VideoData::Inline { data } if data.is_empty() => {
                    return Err(format!(
                        "video block data must not be empty for {media_type}"
                    ));
                }
                VideoData::Inline { .. } => {}
                VideoData::Uri { uri } if uri.trim().is_empty() => {
                    return Err(format!(
                        "video block uri must not be empty for {media_type}"
                    ));
                }
                VideoData::Uri { .. } => {}
            }
        }
    }
    Ok(())
}

/// Custom serde for `Vec<ContentBlock>` fields that provides backwards
/// compatibility with legacy `String` content.
///
/// - Deserialize: `"text"` -> `[Text { text }]`, `[{...}]` -> `[ContentBlock...]`
/// - Serialize: text-only -> `"concatenated"`, mixed -> `[{...}, ...]`
mod content_blocks_serde {
    use super::ContentBlock;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(blocks: &Vec<ContentBlock>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Single text block → plain string (backwards compat).
        // Multiple text blocks (e.g. after compaction replaces images with text
        // placeholders) must serialize as an array to preserve block boundaries.
        if blocks.len() == 1
            && let ContentBlock::Text { text } = &blocks[0]
        {
            return text.serialize(serializer);
        }
        // Multiple blocks or any non-text → array
        blocks.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<ContentBlock>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrBlocks {
            Text(String),
            Blocks(Vec<ContentBlock>),
        }

        match StringOrBlocks::deserialize(deserializer)? {
            StringOrBlocks::Text(s) => Ok(vec![ContentBlock::Text { text: s }]),
            StringOrBlocks::Blocks(blocks) => Ok(blocks),
        }
    }
}

/// Input content that can be either a plain text string or multimodal content blocks.
///
/// Deserializes from either `"text"` or `[{type: "text", text: "..."}, ...]`.
/// Provides `From<String>` and `From<&str>` for ergonomic construction.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentInput {
    /// Plain text input.
    Text(String),
    /// Multimodal content blocks (text + images).
    Blocks(Vec<ContentBlock>),
}

impl ContentInput {
    /// Convert to content blocks.
    pub fn into_blocks(self) -> Vec<ContentBlock> {
        match self {
            ContentInput::Text(s) => ContentBlock::text_vec(s),
            ContentInput::Blocks(blocks) => blocks,
        }
    }

    /// Get text content (text projection for all blocks).
    pub fn text_content(&self) -> String {
        match self {
            ContentInput::Text(s) => s.clone(),
            ContentInput::Blocks(blocks) => text_content(blocks),
        }
    }

    /// Check if this input contains any images.
    pub fn has_images(&self) -> bool {
        match self {
            ContentInput::Text(_) => false,
            ContentInput::Blocks(blocks) => has_images(blocks),
        }
    }

    /// Check if this input contains any video blocks.
    pub fn has_video(&self) -> bool {
        match self {
            ContentInput::Text(_) => false,
            ContentInput::Blocks(blocks) => has_video(blocks),
        }
    }

    /// Check if this input contains any non-text blocks.
    pub fn has_non_text_content(&self) -> bool {
        match self {
            ContentInput::Text(_) => false,
            ContentInput::Blocks(blocks) => has_non_text_content(blocks),
        }
    }
}

impl From<String> for ContentInput {
    fn from(s: String) -> Self {
        ContentInput::Text(s)
    }
}

impl From<&str> for ContentInput {
    fn from(s: &str) -> Self {
        ContentInput::Text(s.to_string())
    }
}

/// Typed input fact for a run boundary.
///
/// A run either starts from caller-provided content or resumes from tool
/// results already staged at the session's pending-continuation boundary.
/// The pending-tail case is its own typed variant — run-boundary events and
/// hooks never fabricate an empty-string prompt to stand in for it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunInput {
    /// The run starts from caller-provided content (text or blocks).
    Content { content: ContentInput },
    /// The run resumes a pending continuation whose transcript tail is a
    /// staged tool-results message; there is no caller prompt.
    PendingToolResults,
}

impl RunInput {
    /// Borrow the caller-provided content, when this run has one.
    pub fn content(&self) -> Option<&ContentInput> {
        match self {
            RunInput::Content { content } => Some(content),
            RunInput::PendingToolResults => None,
        }
    }

    /// Text projection of the caller-provided content, when present.
    /// `PendingToolResults` has no prompt and projects to `None` — never to
    /// a fabricated empty string.
    pub fn prompt_text(&self) -> Option<String> {
        self.content().map(ContentInput::text_content)
    }
}

impl From<ContentInput> for RunInput {
    fn from(content: ContentInput) -> Self {
        RunInput::Content { content }
    }
}

/// Handling mode for ordinary content-bearing work.
///
/// `Queue` means outer-loop / next-turn handling and leaves the current run
/// untouched.
/// `Steer` means inner-loop handling and requests injection at the earliest
/// admissible cooperative boundary, while remaining ordinary work rather than
/// an out-of-band control-plane command.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HandlingMode {
    #[default]
    Queue,
    Steer,
}

/// Standardized rendering class for injected ordinary work.
///
/// This metadata is for normalization/injection rendering, not for core
/// runtime scheduling. It lets the system consistently frame peer messages,
/// external events, tool-scope notices, and similar inputs when they are
/// rendered into model-visible context.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderClass {
    UserPrompt,
    PeerMessage,
    PeerRequest,
    PeerResponse,
    ExternalEvent,
    FlowStep,
    Continuation,
    SystemNotice,
    ToolScopeNotice,
    OpsProgress,
}

/// Constrained salience metadata for injected ordinary work.
///
/// This is intentionally a small closed enum so call sites do not invent
/// arbitrary free-text urgency language.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderSalience {
    Background,
    #[default]
    Normal,
    Important,
    Urgent,
}

/// Optional rendering metadata carried alongside ordinary work content.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderMetadata {
    pub class: RenderClass,
    #[serde(default)]
    pub salience: RenderSalience,
}

// ===========================================================================
// New ordered transcript types (spec section 3.1)
// ===========================================================================

/// Provider-specific metadata for replay continuity.
/// Typed enum prevents runtime "is this an object?" errors.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderMeta {
    /// Anthropic thinking block signature
    Anthropic {
        /// Opaque signature for thinking continuity
        signature: String,
    },
    /// Anthropic redacted thinking (encrypted by safety systems, must be preserved verbatim)
    AnthropicRedacted {
        /// Opaque encrypted data
        data: String,
    },
    /// Anthropic compaction summary (server-side context compression)
    AnthropicCompaction {
        /// Summary text that replaces prior context
        content: String,
    },
    /// Gemini thought signature for tool calls
    Gemini {
        /// Opaque signature for tool call continuity
        #[serde(rename = "thoughtSignature")]
        thought_signature: String,
    },
    /// OpenAI reasoning item metadata for stateless replay.
    /// The `id`, `encrypted_content`, and `phase` fields must be preserved
    /// verbatim when present so Responses API output items can be replayed.
    OpenAi {
        /// Reasoning item ID (required by OpenAI schema)
        id: String,
        /// Encrypted reasoning tokens for continuity
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        /// Responses API phase marker for manual stateless replay.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
        /// Provider response ID for server-side continuation. This is an
        /// optimization hint only; canonical transcript replay does not
        /// depend on it being present or valid.
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
    },
    /// OpenAI response-level continuation metadata for non-reasoning blocks.
    /// The ID is an optimization hint for `previous_response_id`; it is not
    /// semantic transcript content.
    OpenAiResponse {
        /// Provider response ID returned by the Responses API.
        response_id: String,
    },
}

/// Deserializes `Box<RawValue>` from a possibly-buffered serde value.
///
/// `Message` uses internally-tagged serde representation, which buffers enum
/// content during dispatch. In that path, `RawValue` cannot deserialize directly.
/// This converts via `Value` so ToolUse arguments survive that buffering step.
fn deserialize_tool_use_args<'de, D>(deserializer: D) -> Result<Box<RawValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
    RawValue::from_string(raw).map_err(serde::de::Error::custom)
}

/// Source lane for a `Transcript` assistant block.
///
/// Transcripts originate from a non-text output channel (today only spoken
/// audio from realtime providers). Modeled as a typed enum so future lanes
/// (e.g. captions, sign-language gloss) can be added without breaking
/// downstream consumers.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSource {
    /// Spoken-audio transcript (provider audio output → text).
    Spoken,
}

/// Typed semantic kind of a provider-executed (server-side) tool.
///
/// This is the single typed owner of the server-tool identity. Each provider
/// adapter parses its native discriminator into this enum once at the streaming
/// boundary, so downstream consumers never re-classify by matching a
/// `name: String`. Provider-native sub-event detail (e.g. OpenAI's
/// `web_search_call` vs `web_search_result`) is preserved in the accompanying
/// `content` JSON, not in this kind.
///
/// `ProviderNative` is the verbatim escape hatch for dynamic provider tool
/// names (Anthropic `server_tool_use` carries an arbitrary tool name that must
/// round-trip exactly on replay). It is the only variant carrying a string,
/// and that string IS the typed fact — not a re-derivable label.
#[non_exhaustive]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerToolKind {
    /// Provider-hosted web search (Anthropic `web_search*`, OpenAI
    /// `web_search*`). Sub-event detail lives in the evidence `content`.
    WebSearch,
    /// Gemini grounding via Google Search (grounding metadata block).
    GoogleSearch,
    /// A provider-native server tool whose exact name must round-trip verbatim
    /// (Anthropic `server_tool_use` dynamic name and any unrecognized future
    /// server tool). The `name` is provider-owned and replayed unchanged.
    ProviderNative { name: String },
}

impl ServerToolKind {
    /// The provider-native tool name for this kind.
    ///
    /// Replay sites that must reconstruct a provider request item read this
    /// derived projection instead of carrying a parallel `name: String`.
    pub fn provider_name(&self) -> &str {
        match self {
            ServerToolKind::WebSearch => "web_search",
            ServerToolKind::GoogleSearch => "google_search",
            ServerToolKind::ProviderNative { name } => name,
        }
    }

    pub fn from_provider_name(name: impl Into<String>) -> Self {
        let name = name.into();
        match name.as_str() {
            "web_search" => ServerToolKind::WebSearch,
            "google_search" => ServerToolKind::GoogleSearch,
            _ => ServerToolKind::ProviderNative { name },
        }
    }
}

fn deserialize_server_tool_kind<'de, D>(deserializer: D) -> Result<ServerToolKind, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ServerToolKindCompat {
        Current(ServerToolKind),
        LegacyName(String),
    }

    match ServerToolKindCompat::deserialize(deserializer)? {
        ServerToolKindCompat::Current(kind) => Ok(kind),
        ServerToolKindCompat::LegacyName(name) => Ok(ServerToolKind::from_provider_name(name)),
    }
}

impl fmt::Display for ServerToolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.provider_name())
    }
}

/// A block of content in an assistant message, preserving order.
///
/// Uses adjacently tagged serde representation to support `Box<RawValue>` in
/// ToolUse, with a custom deserializer on `args` to tolerate Message-level
/// internal enum buffering.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "block_type", content = "data", rename_all = "snake_case")]
pub enum AssistantBlock {
    /// Plain text output.
    /// Note: Gemini may attach thoughtSignature to trailing text parts for continuity.
    Text {
        text: String,
        /// Provider metadata (Gemini thoughtSignature on text parts)
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },

    /// Spoken-transcript output captured from a non-text lane (realtime
    /// provider audio output → transcribed text). Distinguished from
    /// `Text` so projection sites that care about lane provenance (display
    /// vs. spoken) can render or filter independently. Both project to the
    /// same human-readable text stream.
    Transcript {
        text: String,
        /// Origin lane (today: `Spoken`).
        source: TranscriptSource,
        /// Provider continuity metadata, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },

    /// Visible reasoning/thinking notes emitted by the model
    Reasoning {
        /// Human-readable text (may be empty if only encrypted content)
        #[serde(default)]
        text: String,
        /// Provider continuity metadata (typed, not arbitrary JSON)
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },

    /// Tool use request
    ToolUse {
        id: String,
        name: String,
        /// Arguments as raw JSON - only dispatcher parses this
        #[serde(deserialize_with = "deserialize_tool_use_args")]
        args: Box<RawValue>,
        /// Provider continuity metadata
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },

    /// Provider-executed tool evidence, such as OpenAI web search calls,
    /// Anthropic server-side web search blocks, or Gemini grounding metadata.
    ServerToolContent {
        /// Provider item or tool-use ID when one exists.
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Typed semantic tool kind, parsed once at the provider adapter.
        #[serde(alias = "name", deserialize_with = "deserialize_server_tool_kind")]
        kind: ServerToolKind,
        /// Provider-native JSON payload, preserved for citations and grounding evidence.
        content: Value,
        /// Provider continuity metadata, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },

    /// Canonical generated image output.
    Image {
        image_id: AssistantImageId,
        blob_ref: BlobRef,
        media_type: MediaType,
        width: u32,
        height: u32,
        revised_prompt: RevisedPromptDisposition,
        meta: ProviderImageMetadata,
    },
}

pub fn assistant_blocks_have_visible_or_actionable_output(blocks: &[AssistantBlock]) -> bool {
    blocks.iter().any(|block| match block {
        AssistantBlock::Text { text, .. } | AssistantBlock::Transcript { text, .. } => {
            !text.trim().is_empty()
        }
        AssistantBlock::ToolUse { .. } | AssistantBlock::Image { .. } => true,
        AssistantBlock::Reasoning { text, meta } => !text.trim().is_empty() || meta.is_some(),
        AssistantBlock::ServerToolContent { .. } => false,
    })
}

impl PartialEq for AssistantBlock {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                AssistantBlock::Text { text: t1, meta: m1 },
                AssistantBlock::Text { text: t2, meta: m2 },
            )
            | (
                AssistantBlock::Reasoning { text: t1, meta: m1 },
                AssistantBlock::Reasoning { text: t2, meta: m2 },
            ) => t1 == t2 && m1 == m2,
            (
                AssistantBlock::Transcript {
                    text: t1,
                    source: s1,
                    meta: m1,
                },
                AssistantBlock::Transcript {
                    text: t2,
                    source: s2,
                    meta: m2,
                },
            ) => t1 == t2 && s1 == s2 && m1 == m2,
            (
                AssistantBlock::ToolUse {
                    id: i1,
                    name: n1,
                    args: a1,
                    meta: m1,
                },
                AssistantBlock::ToolUse {
                    id: i2,
                    name: n2,
                    args: a2,
                    meta: m2,
                },
            ) => i1 == i2 && n1 == n2 && a1.get() == a2.get() && m1 == m2,
            (
                AssistantBlock::ServerToolContent {
                    id: i1,
                    kind: k1,
                    content: c1,
                    meta: m1,
                },
                AssistantBlock::ServerToolContent {
                    id: i2,
                    kind: k2,
                    content: c2,
                    meta: m2,
                },
            ) => i1 == i2 && k1 == k2 && c1 == c2 && m1 == m2,
            (
                AssistantBlock::Image {
                    image_id: i1,
                    blob_ref: b1,
                    media_type: mt1,
                    width: w1,
                    height: h1,
                    revised_prompt: rp1,
                    meta: m1,
                },
                AssistantBlock::Image {
                    image_id: i2,
                    blob_ref: b2,
                    media_type: mt2,
                    width: w2,
                    height: h2,
                    revised_prompt: rp2,
                    meta: m2,
                },
            ) => {
                i1 == i2 && b1 == b2 && mt1 == mt2 && w1 == w2 && h1 == h2 && rp1 == rp2 && m1 == m2
            }
            _ => false,
        }
    }
}

impl Eq for AssistantBlock {}

/// Borrowed view into a tool call block. Zero allocations.
#[derive(Debug, Clone, Copy)]
pub struct ToolCallView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub args: &'a RawValue,
}

impl ToolCallView<'_> {
    /// Parse args into a concrete type. Called by dispatcher, not core.
    ///
    /// # Errors
    /// Returns `serde_json::Error` if args don't match expected schema.
    pub fn parse_args<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(self.args.get())
    }
}

/// Iterator over tool calls in a [`BlockAssistantMessage`]. Zero allocations.
pub struct ToolCallIter<'a> {
    inner: std::slice::Iter<'a, AssistantBlock>,
}

impl<'a> Iterator for ToolCallIter<'a> {
    type Item = ToolCallView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let AssistantBlock::ToolUse { id, name, args, .. } = self.inner.next()? {
                return Some(ToolCallView { id, name, args });
            }
        }
    }
}

// ===========================================================================
// Legacy types (will be migrated in later phases)
// ===========================================================================

/// Configuration for structured output extraction.
///
/// When provided to an agent, the agent will perform an extraction turn after
/// completing the agentic work, forcing the LLM to output validated JSON that
/// conforms to the provided schema. [`RunResult::text`] remains the committed
/// main-turn output; extraction populates [`RunResult::structured_output`] on
/// success or [`RunResult::extraction_error`] on failure.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize)]
pub struct OutputSchema {
    /// The JSON schema that the output must conform to
    pub schema: MeerkatSchema,
    /// Optional name for the schema (used by some providers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether to enforce strict schema validation (provider-specific)
    #[serde(default)]
    pub strict: bool,
    /// Compatibility mode for provider lowering (lossy or strict)
    #[serde(default)]
    pub compat: SchemaCompat,
    /// Schema format version
    #[serde(default)]
    pub format: SchemaFormat,
}

impl PartialEq for OutputSchema {
    fn eq(&self, other: &Self) -> bool {
        self.schema == other.schema
            && self.name == other.name
            && self.strict == other.strict
            && self.compat == other.compat
            && self.format == other.format
    }
}

impl Eq for OutputSchema {}

impl OutputSchema {
    /// Create a new output schema
    pub fn new(schema: Value) -> Result<Self, SchemaError> {
        Ok(Self {
            schema: MeerkatSchema::new(schema)?,
            name: None,
            strict: false,
            compat: SchemaCompat::default(),
            format: SchemaFormat::default(),
        })
    }

    /// Set the schema name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Enable strict mode for schema validation
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Set compatibility mode for provider lowering
    pub fn with_compat(mut self, compat: SchemaCompat) -> Self {
        self.compat = compat;
        self
    }

    /// Set the schema format
    pub fn with_format(mut self, format: SchemaFormat) -> Self {
        self.format = format;
        self
    }

    /// Parse from a JSON string (wrapper or raw schema).
    pub fn from_json_str(raw: &str) -> Result<Self, SchemaError> {
        let value: Value = serde_json::from_str(raw).map_err(|_| SchemaError::InvalidRoot)?;
        Self::from_json_value(value)
    }

    /// Parse from a JSON value (wrapper or raw schema).
    pub fn from_json_value(value: Value) -> Result<Self, SchemaError> {
        match value {
            Value::Object(obj) if is_wrapped_schema(&obj) => {
                let wrapped: OutputSchemaWire = serde_json::from_value(Value::Object(obj))
                    .map_err(|_| SchemaError::InvalidRoot)?;
                Ok(Self {
                    schema: MeerkatSchema::new(wrapped.schema)?,
                    name: wrapped.name,
                    strict: wrapped.strict,
                    compat: wrapped.compat,
                    format: wrapped.format,
                })
            }
            other => OutputSchema::new(other),
        }
    }

    /// Create from a Rust type via schemars.
    pub fn from_type<T: schemars::JsonSchema>() -> Result<Self, SchemaError> {
        let schema = schemars::schema_for!(T);
        let mut value = serde_json::to_value(&schema).map_err(|_| SchemaError::InvalidRoot)?;
        // Ensure object schemas always have `properties` and `required` keys.
        if let Value::Object(ref mut obj) = value
            && obj.get("type").and_then(Value::as_str) == Some("object")
        {
            obj.entry("properties".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            obj.entry("required".to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
        }
        OutputSchema::new(value)
    }

    /// Convert to a JSON value for provider params.
    pub fn to_value(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("schema".to_string(), self.schema.as_value().clone());
        if let Some(name) = &self.name {
            obj.insert("name".to_string(), Value::String(name.clone()));
        }
        obj.insert("strict".to_string(), Value::Bool(self.strict));
        obj.insert(
            "compat".to_string(),
            serde_json::to_value(self.compat).unwrap_or(Value::String("lossy".to_string())),
        );
        obj.insert(
            "format".to_string(),
            serde_json::to_value(self.format).unwrap_or(Value::String("meerkat_v1".to_string())),
        );
        Value::Object(obj)
    }
}

impl<'de> Deserialize<'de> for OutputSchema {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        OutputSchema::from_json_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Deserialize)]
struct OutputSchemaWire {
    pub schema: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub compat: SchemaCompat,
    #[serde(default)]
    pub format: SchemaFormat,
}

fn is_wrapped_schema(obj: &Map<String, Value>) -> bool {
    let has_schema = obj.contains_key("schema");
    let has_format_marker = obj
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|f| f == "meerkat_v1");
    let only_wrapper_keys = obj.keys().all(|key| {
        matches!(
            key.as_str(),
            "schema" | "name" | "strict" | "compat" | "format"
        )
    });
    has_schema && (has_format_marker || only_wrapper_keys)
}

/// Unique identifier for a session (UUID v7 for time-ordering)
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl SessionId {
    /// Create a new session ID using UUID v7
    pub fn new() -> Self {
        Self(crate::time_compat::new_uuid_v7())
    }

    /// Create from an existing UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Parse from string
    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A message in the conversation history
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// System prompt (injected at start)
    System(SystemMessage),
    /// Typed synthetic/runtime system notice.
    SystemNotice(SystemNoticeMessage),
    /// User input
    User(UserMessage),
    /// Assistant response with ordered blocks
    BlockAssistant(BlockAssistantMessage),
    /// Results from tool execution
    ToolResults {
        results: Vec<ToolResult>,
        #[allow(missing_docs)]
        created_at: MessageTimestamp,
    },
}

impl Message {
    pub fn tool_results(results: Vec<ToolResult>) -> Self {
        Self::ToolResults {
            results,
            created_at: message_timestamp_now(),
        }
    }

    /// Classify this message for semantic indexing as a typed decision.
    ///
    /// The include/exclude policy is an explicit typed value, not a hidden
    /// empty-string convention: conversation turns (user/assistant) project to
    /// [`MemoryIndexableContent::Indexable`], while system prompts, synthetic
    /// system notices, and tool results carry a typed
    /// [`MemoryIndexExclusion`] reason so a consumer (e.g. the memory store)
    /// can see and own indexability rather than inferring it from an empty
    /// flattened `String`.
    pub fn indexable_content(&self) -> MemoryIndexableContent {
        match self {
            // The typed transcript role is the indexability owner for the user
            // channel: ordinary conversation indexes, while runtime compaction
            // summaries (projections of already-indexed history) and
            // host-attached injected context carry typed exclusion reasons.
            Message::User(u) => match u.transcript_role {
                TranscriptUserRole::Conversational => {
                    MemoryIndexableContent::Indexable(u.text_content())
                }
                TranscriptUserRole::CompactionSummary => {
                    MemoryIndexableContent::Excluded(MemoryIndexExclusion::CompactionSummary)
                }
                TranscriptUserRole::InjectedContext => {
                    MemoryIndexableContent::Excluded(MemoryIndexExclusion::InjectedContext)
                }
            },
            Message::BlockAssistant(ba) => {
                let mut result = String::new();
                for text in ba.text_blocks() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(text);
                }
                MemoryIndexableContent::Indexable(result)
            }
            Message::System(_) => {
                MemoryIndexableContent::Excluded(MemoryIndexExclusion::SystemPrompt)
            }
            Message::SystemNotice(_) => {
                MemoryIndexableContent::Excluded(MemoryIndexExclusion::SystemNotice)
            }
            Message::ToolResults { .. } => {
                MemoryIndexableContent::Excluded(MemoryIndexExclusion::ToolResults)
            }
        }
    }

    /// Extract text content suitable for semantic indexing.
    ///
    /// Text projection of [`Message::indexable_content`]: excluded messages
    /// (system prompts, system notices, tool results) project to an empty
    /// string. Prefer [`Message::indexable_content`] when the indexability
    /// decision itself matters.
    pub fn as_indexable_text(&self) -> String {
        self.indexable_content().into_indexable_text()
    }
}

/// Typed indexability decision for a [`Message`] in semantic-memory indexing.
///
/// Replaces the empty-`String` "this message is not indexable" convention with
/// an explicit, exhaustive decision the consumer can match on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryIndexableContent {
    /// Conversation content to index, projected to plain text.
    Indexable(String),
    /// Message excluded from indexing, with the typed reason.
    Excluded(MemoryIndexExclusion),
}

impl MemoryIndexableContent {
    /// Whether this message contributes content to the index.
    pub fn is_indexable(&self) -> bool {
        matches!(self, MemoryIndexableContent::Indexable(_))
    }

    /// The indexable text, or empty string for excluded messages.
    pub fn into_indexable_text(self) -> String {
        match self {
            MemoryIndexableContent::Indexable(text) => text,
            MemoryIndexableContent::Excluded(_) => String::new(),
        }
    }

    /// Borrow the indexable text, if any.
    pub fn indexable_text(&self) -> Option<&str> {
        match self {
            MemoryIndexableContent::Indexable(text) => Some(text.as_str()),
            MemoryIndexableContent::Excluded(_) => None,
        }
    }
}

/// Typed reason a [`Message`] is excluded from semantic-memory indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryIndexExclusion {
    /// System prompt — not conversation content.
    SystemPrompt,
    /// Synthetic/runtime system notice — not conversation content.
    SystemNotice,
    /// Tool results — structured data, not conversation prose.
    ToolResults,
    /// Runtime compaction summary — a projection of already-indexed history;
    /// re-indexing it would promote the projection to source content.
    CompactionSummary,
    /// Host-attached injected context — ambient material delivered alongside
    /// (not inside) the user's message; not user-authored conversation.
    InjectedContext,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
enum MessageWire {
    System(SystemMessage),
    SystemNotice(SystemNoticeMessage),
    User(UserMessage),
    #[serde(rename = "block_assistant")]
    BlockAssistant(BlockAssistantMessage),
    #[serde(rename = "tool_results")]
    ToolResults {
        results: Vec<ToolResult>,
        #[serde(default = "message_timestamp_now")]
        created_at: MessageTimestamp,
    },
}

impl Serialize for Message {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let wire = match self {
            Self::System(message) => MessageWire::System(message.clone()),
            Self::SystemNotice(message) => MessageWire::SystemNotice(message.clone()),
            Self::User(message) => MessageWire::User(message.clone()),
            Self::BlockAssistant(message) => MessageWire::BlockAssistant(message.clone()),
            Self::ToolResults {
                results,
                created_at,
            } => MessageWire::ToolResults {
                results: results.clone(),
                created_at: *created_at,
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = MessageWire::deserialize(deserializer)?;
        Ok(match wire {
            MessageWire::System(message) => Self::System(message),
            MessageWire::SystemNotice(message) => Self::SystemNotice(message),
            MessageWire::User(message) => Self::User(message),
            MessageWire::BlockAssistant(message) => Self::BlockAssistant(message),
            MessageWire::ToolResults {
                results,
                created_at,
            } => Self::ToolResults {
                results,
                created_at,
            },
        })
    }
}

/// System message content
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemMessage {
    pub content: String,
    /// Typed provenance of the mutation that last produced this system message.
    ///
    /// This is the canonical replacement for `[Runtime System Context]`
    /// string-prefix folklore in the transcript-continuity save-guard: a system
    /// prompt produced by a runtime system-context append carries
    /// [`SystemPromptMutationKind::RuntimeContextAppend`], so the guard admits the
    /// append-shaped divergence from a typed field instead of classifying the
    /// rendered prompt suffix by content.
    #[serde(
        default,
        skip_serializing_if = "SystemPromptMutationKind::is_unspecified"
    )]
    pub mutation_kind: SystemPromptMutationKind,
    #[serde(default = "message_timestamp_now")]
    pub created_at: MessageTimestamp,
}

impl SystemMessage {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            mutation_kind: SystemPromptMutationKind::Unspecified,
            created_at: message_timestamp_now(),
        }
    }

    /// Construct a system message tagging the typed provenance of the mutation
    /// that produced it.
    pub fn with_mutation_kind(
        content: impl Into<String>,
        mutation_kind: SystemPromptMutationKind,
    ) -> Self {
        Self {
            content: content.into(),
            mutation_kind,
            created_at: message_timestamp_now(),
        }
    }
}

/// Typed provenance class for the mutation that last produced a system prompt.
///
/// Carried on [`SystemMessage`] so the transcript-continuity save-guard reads a
/// typed fact instead of classifying the rendered prompt by the
/// `[Runtime System Context]` label. The default `Unspecified` covers system
/// prompts seeded outside the durable-config mutation path (wire reconstruction,
/// direct construction); the runtime context-append path stamps
/// [`Self::RuntimeContextAppend`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemPromptMutationKind {
    /// No typed provenance was recorded for this system prompt.
    #[default]
    Unspecified,
    /// A direct system-prompt mutation (caller-supplied prompt).
    DirectMutation,
    /// A build-time explicit system prompt.
    ExplicitBuild,
    /// A build-time default system prompt.
    DefaultBuild,
    /// A WASM build-time default system prompt.
    WasmDefaultBuild,
    /// A runtime system-context append (appends a `[Runtime System Context]`
    /// block to the existing prompt).
    RuntimeContextAppend,
    /// A runtime-steer cleanup mutation (removes transient steer blocks).
    RuntimeSteerCleanup,
}

impl SystemPromptMutationKind {
    /// Whether this is the default (`Unspecified`) provenance. Used by
    /// `skip_serializing_if` so untagged system prompts serialize without the
    /// field.
    #[must_use]
    pub fn is_unspecified(&self) -> bool {
        matches!(self, Self::Unspecified)
    }

    /// Whether this system prompt was produced by a runtime context append.
    #[must_use]
    pub fn is_runtime_context_append(&self) -> bool {
        matches!(self, Self::RuntimeContextAppend)
    }
}

/// Typed system notice content carried in the transcript.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemNoticeKind {
    Generic,
    Comms,
    ExternalEvent,
    McpPending,
    Mcp,
    BackgroundJob,
    ToolScope,
    ToolScopeWarning,
    /// Auth lease transitioned to `reauth_required` — the active
    /// `auth_binding` needs manual re-authentication before the next LLM
    /// call can proceed (Phase 1.5-rev).
    AuthReauthRequired,
}

impl SystemNoticeKind {
    pub const fn render_class(self) -> RenderClass {
        match self {
            Self::Generic | Self::McpPending | Self::AuthReauthRequired => {
                RenderClass::SystemNotice
            }
            Self::Comms => RenderClass::PeerMessage,
            Self::ExternalEvent => RenderClass::ExternalEvent,
            Self::Mcp => RenderClass::SystemNotice,
            Self::BackgroundJob => RenderClass::OpsProgress,
            Self::ToolScope | Self::ToolScopeWarning => RenderClass::ToolScopeNotice,
        }
    }
}

/// Direction for runtime-authored communication metadata.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemNoticeDirection {
    Incoming,
    Outgoing,
    Internal,
}

/// Closed comms-notice vocabulary carried by [`SystemNoticeBlock::Comms`].
///
/// This is the typed owner of the comms transcript-block discriminator that
/// used to be a raw `kind: String` re-matched (with a silent fail-open arm) in
/// [`SystemNoticeBlock::model_projection_text`]. It mirrors the runtime
/// `PeerConvention` / `InputKind` vocabulary: a peer-to-peer message, a
/// correlated request, an in-flight response progress update, and a terminal
/// response.
///
/// Wire form is a plain string. The canonical tags match the strings the
/// runtime producer has always emitted (`message`, `request`,
/// `response_progress`, `response_terminal`). Any unknown wire value is
/// captured explicitly as [`CommsNoticeKind::Other`] (NOT a silent
/// fall-through) so the projection match stays exhaustive while remaining
/// forward-compatible; an `Other` value round-trips to its original string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommsNoticeKind {
    /// Simple peer-to-peer message (wire tag `message`).
    Message,
    /// Correlated request expecting a response (wire tag `request`).
    Request,
    /// Progress update for an in-flight response (wire tag
    /// `response_progress`).
    ResponseProgress,
    /// Terminal response, completed or failed (wire tag `response_terminal`).
    ResponseTerminal,
    /// Forward-compatible escape hatch for an unrecognized wire kind. Projects
    /// as a plain peer message but is matched explicitly, never silently.
    Other(String),
}

impl CommsNoticeKind {
    /// Canonical wire string for this kind.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Message => "message",
            Self::Request => "request",
            Self::ResponseProgress => "response_progress",
            Self::ResponseTerminal => "response_terminal",
            Self::Other(raw) => raw.as_str(),
        }
    }

    /// Classify a wire string into the typed vocabulary. Unrecognized values
    /// become [`CommsNoticeKind::Other`].
    #[must_use]
    pub fn from_wire(raw: &str) -> Self {
        match raw {
            "message" => Self::Message,
            "request" => Self::Request,
            "response_progress" => Self::ResponseProgress,
            "response_terminal" => Self::ResponseTerminal,
            other => Self::Other(other.to_string()),
        }
    }
}

impl std::fmt::Display for CommsNoticeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for CommsNoticeKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CommsNoticeKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_wire(&raw))
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for CommsNoticeKind {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CommsNoticeKind".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

/// Peer identity carried in a typed comms transcript block.
///
/// `id` is the canonical routing identity ([`crate::comms::PeerId`]), serialized
/// as a hyphenated UUID string on the wire; `display_name` is the presentation
/// label. Keeping `id` typed lets the projection logic consume the identity
/// directly instead of re-parsing a `String` back into a `PeerId`.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemNoticePeer {
    pub id: crate::comms::PeerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// Typed runtime-authored transcript metadata.
///
/// These blocks are the durable contract for comms, tool/MCP state, auth,
/// background work, and other runtime facts. They are never user-authored text.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemNoticeBlock {
    Comms {
        kind: CommsNoticeKind,
        direction: SystemNoticeDirection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer: Option<SystemNoticePeer>,
        /// Sender-declared content taint carried in the signed envelope.
        /// `None` means the sender made no declaration — a real third state
        /// that must never be coalesced into
        /// [`crate::comms::SenderContentTaint::Clean`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_taint: Option<crate::comms::SenderContentTaint>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
    },
    ExternalEvent {
        source: String,
        event_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
    },
    ToolConfig {
        payload: crate::event::ToolConfigChangedPayload,
    },
    Mcp {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation: Option<crate::event::ToolConfigChangeOperation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phase: Option<crate::event::ExternalToolDeltaPhase>,
        #[serde(default)]
        persisted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pending_sources: Vec<String>,
    },
    BackgroundJob {
        job_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
        status: crate::event::BackgroundJobTerminalStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Auth {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binding: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    RuntimeNotice {
        category: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },
    Unknown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<Value>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SystemNoticeBlockKnown {
    Comms {
        kind: CommsNoticeKind,
        direction: SystemNoticeDirection,
        #[serde(default)]
        peer: Option<SystemNoticePeer>,
        #[serde(default)]
        sender_taint: Option<crate::comms::SenderContentTaint>,
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        intent: Option<String>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        payload: Option<Value>,
        #[serde(default)]
        content: Vec<ContentBlock>,
    },
    ExternalEvent {
        source: String,
        event_type: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        payload: Option<Value>,
        #[serde(default)]
        content: Vec<ContentBlock>,
    },
    ToolConfig {
        payload: crate::event::ToolConfigChangedPayload,
    },
    Mcp {
        #[serde(default)]
        server_id: Option<String>,
        #[serde(default)]
        operation: Option<crate::event::ToolConfigChangeOperation>,
        #[serde(default)]
        phase: Option<crate::event::ExternalToolDeltaPhase>,
        #[serde(default)]
        persisted: bool,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        pending_sources: Vec<String>,
    },
    BackgroundJob {
        job_id: String,
        #[serde(default)]
        display_name: Option<String>,
        status: crate::event::BackgroundJobTerminalStatus,
        #[serde(default)]
        detail: Option<String>,
    },
    Auth {
        state: String,
        #[serde(default)]
        binding: Option<String>,
        #[serde(default)]
        detail: Option<String>,
    },
    RuntimeNotice {
        category: String,
        #[serde(default)]
        detail: Option<String>,
        #[serde(default)]
        payload: Option<Value>,
    },
    Unknown {
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        payload: Option<Value>,
    },
}

impl From<SystemNoticeBlockKnown> for SystemNoticeBlock {
    fn from(value: SystemNoticeBlockKnown) -> Self {
        match value {
            SystemNoticeBlockKnown::Comms {
                kind,
                direction,
                peer,
                sender_taint,
                request_id,
                intent,
                status,
                summary,
                payload,
                content,
            } => Self::Comms {
                kind,
                direction,
                peer,
                sender_taint,
                request_id,
                intent,
                status,
                summary,
                payload,
                content,
            },
            SystemNoticeBlockKnown::ExternalEvent {
                source,
                event_type,
                summary,
                body,
                payload,
                content,
            } => Self::ExternalEvent {
                source,
                event_type,
                summary,
                body,
                payload,
                content,
            },
            SystemNoticeBlockKnown::ToolConfig { payload } => Self::ToolConfig { payload },
            SystemNoticeBlockKnown::Mcp {
                server_id,
                operation,
                phase,
                persisted,
                detail,
                pending_sources,
            } => Self::Mcp {
                server_id,
                operation,
                phase,
                persisted,
                detail,
                pending_sources,
            },
            SystemNoticeBlockKnown::BackgroundJob {
                job_id,
                display_name,
                status,
                detail,
            } => Self::BackgroundJob {
                job_id,
                display_name,
                status,
                detail,
            },
            SystemNoticeBlockKnown::Auth {
                state,
                binding,
                detail,
            } => Self::Auth {
                state,
                binding,
                detail,
            },
            SystemNoticeBlockKnown::RuntimeNotice {
                category,
                detail,
                payload,
            } => Self::RuntimeNotice {
                category,
                detail,
                payload,
            },
            SystemNoticeBlockKnown::Unknown { summary, payload } => {
                Self::Unknown { summary, payload }
            }
        }
    }
}

impl<'de> Deserialize<'de> for SystemNoticeBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let block_type = value.get("type").and_then(Value::as_str);
        match block_type {
            Some(
                "comms" | "external_event" | "tool_config" | "mcp" | "background_job" | "auth"
                | "runtime_notice" | "unknown",
            ) => serde_json::from_value::<SystemNoticeBlockKnown>(value)
                .map(Into::into)
                .map_err(serde::de::Error::custom),
            Some(_) => Ok(Self::unknown_from_value(value)),
            None => Err(serde::de::Error::custom("missing system notice block type")),
        }
    }
}

impl SystemNoticeBlock {
    fn unknown_from_value(value: Value) -> Self {
        let summary = value
            .get("summary")
            .or_else(|| value.get("body"))
            .or_else(|| value.get("detail"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Self::Unknown {
            summary,
            payload: Some(value),
        }
    }

    pub fn summary(&self) -> Option<&str> {
        match self {
            Self::Comms { summary, .. } | Self::Unknown { summary, .. } => summary.as_deref(),
            Self::ExternalEvent { summary, body, .. } => summary.as_deref().or(body.as_deref()),
            Self::Mcp { detail, .. }
            | Self::BackgroundJob { detail, .. }
            | Self::Auth { detail, .. }
            | Self::RuntimeNotice { detail, .. } => detail.as_deref(),
            Self::ToolConfig { .. } => None,
        }
    }

    pub fn model_projection_text(&self) -> String {
        match self {
            Self::Comms {
                kind,
                peer,
                sender_taint,
                request_id,
                intent,
                status,
                summary,
                payload,
                content,
                ..
            } => {
                let peer_id = peer.as_ref().map(|peer| peer.id);
                let peer_id_label = peer_id.map(|peer_id| peer_id.as_str());
                let peer_label = peer
                    .as_ref()
                    .and_then(|peer| peer.display_name.as_deref())
                    .or(peer_id_label.as_deref())
                    .unwrap_or("peer");
                let mut body_lines = Vec::new();
                for block in content {
                    let text = block.text_projection();
                    if !text.trim().is_empty() {
                        body_lines.push(text.into_owned());
                    }
                }
                let body = body_lines.join("\n");
                // The carrier is the typed `CommsNoticeKind`, matched
                // exhaustively. The peer identity is the canonical typed
                // `PeerId`, so the correlated-request projection no longer
                // re-parses a string and branches on Ok/Err; the only
                // remaining fallback is the structural "no peer identity
                // present" case.
                let mut lines = match kind {
                    CommsNoticeKind::Request => {
                        let params = payload.as_ref().unwrap_or(&Value::Null);
                        match peer_id {
                            Some(peer_id) => {
                                let mut lines =
                                    vec![crate::interaction::format_peer_request_projection(
                                        peer_id,
                                        Some(peer_label),
                                        request_id.as_deref().unwrap_or_default(),
                                        intent.as_deref().unwrap_or("request"),
                                        params,
                                    )];
                                if !body.trim().is_empty() {
                                    lines.push(body);
                                }
                                lines
                            }
                            None => {
                                vec![format!("Peer request from {peer_label}\n{body}")]
                            }
                        }
                    }
                    CommsNoticeKind::ResponseProgress => vec![format!(
                        "Peer response progress from {peer_label}\nRequest ID: {}",
                        request_id.as_deref().unwrap_or_default()
                    )],
                    CommsNoticeKind::ResponseTerminal => {
                        let mut text = format!(
                            "Peer terminal response from {peer_label}\nRequest ID: {}",
                            request_id.as_deref().unwrap_or_default()
                        );
                        if let Some(status) = status.as_deref() {
                            text.push_str(&format!("\nStatus: {status}"));
                        }
                        if let Some(payload) = payload {
                            text.push_str(&format!("\nResult: {}", format_notice_payload(payload)));
                        }
                        vec![text]
                    }
                    CommsNoticeKind::Message | CommsNoticeKind::Other(_) => {
                        vec![crate::interaction::format_peer_message_projection(
                            peer_label, &body,
                        )]
                    }
                };
                let appends_extras = !matches!(
                    kind,
                    CommsNoticeKind::Request | CommsNoticeKind::ResponseTerminal
                );
                if appends_extras {
                    if let Some(request_id) = request_id {
                        lines.push(format!("Request ID: {request_id}"));
                    }
                    if let Some(intent) = intent {
                        lines.push(format!("Intent: {intent}"));
                    }
                    if let Some(status) = status {
                        lines.push(format!("Status: {status}"));
                    }
                    if let Some(summary) = summary {
                        lines.push(summary.clone());
                    }
                }
                if let Some(payload) = payload
                    && appends_extras
                {
                    lines.push(format!("Payload: {}", format_notice_payload(payload)));
                }
                // Model-visible marker for DECLARED-tainted content only.
                // `Clean` and `None` (no declaration) deliberately render
                // identically: the typed `sender_taint` field is the carrier
                // of the three-way distinction, and the projection flags only
                // the state the model must treat with caution.
                if matches!(
                    sender_taint,
                    Some(crate::comms::SenderContentTaint::Tainted)
                ) {
                    lines.push("[sender declared this content tainted]".to_string());
                }
                lines.join("\n")
            }
            Self::ExternalEvent {
                source,
                event_type,
                body,
                payload,
                content,
                ..
            } => {
                let mut lines = vec![crate::interaction::format_external_event_projection(
                    source,
                    body.as_deref().or(Some(event_type.as_str())),
                )];
                for block in content {
                    let text = block.text_projection();
                    if !text.trim().is_empty() {
                        lines.push(text.into_owned());
                    }
                }
                if let Some(payload) = payload {
                    lines.push(format!("Payload: {}", format_notice_payload(payload)));
                }
                lines.join("\n")
            }
            Self::ToolConfig { payload } => {
                format!("Tool configuration changed: {}", payload.status_text())
            }
            _ => self.summary().unwrap_or_default().to_string(),
        }
    }
}

fn format_notice_payload(payload: &Value) -> String {
    serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string())
}

/// System notice message stored in the canonical transcript.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemNoticeMessage {
    pub kind: SystemNoticeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<SystemNoticeBlock>,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    #[serde(default = "message_timestamp_now")]
    pub created_at: MessageTimestamp,
}

impl SystemNoticeMessage {
    pub fn new(kind: SystemNoticeKind, body: impl Into<String>) -> Self {
        let body = body.into();
        Self {
            kind,
            body: if body.is_empty() { None } else { Some(body) },
            blocks: Vec::new(),
            created_at: message_timestamp_now(),
        }
    }

    pub fn with_blocks(
        kind: SystemNoticeKind,
        body: Option<String>,
        blocks: Vec<SystemNoticeBlock>,
    ) -> Self {
        Self {
            kind,
            body,
            blocks,
            created_at: message_timestamp_now(),
        }
    }

    pub fn with_block(
        kind: SystemNoticeKind,
        body: Option<String>,
        block: SystemNoticeBlock,
    ) -> Self {
        Self::with_blocks(kind, body, vec![block])
    }

    /// Internal model-facing projection for typed runtime facts.
    ///
    /// This is used only while assembling provider-visible prompt/context text.
    /// The projection is not the transcript contract and must not be persisted
    /// as user-authored content.
    pub fn model_projection_text(&self) -> String {
        let mut parts = Vec::new();
        if let Some(body) = self.body.as_deref().filter(|body| !body.trim().is_empty()) {
            parts.push(body.to_string());
        }
        for block in &self.blocks {
            let projection = block.model_projection_text();
            if !projection.trim().is_empty() {
                parts.push(projection);
            }
        }
        parts.join("\n")
    }
}

/// Typed transcript role for a user-channel message.
///
/// This is the canonical replacement for `[Context compacted]` string-prefix
/// folklore in the transcript-continuity save-guard. A user message produced as
/// a runtime compaction boundary carries [`TranscriptUserRole::CompactionSummary`];
/// everything else stays [`TranscriptUserRole::Conversational`]. The producer of
/// the compaction summary sets this typed marker; the save-guard reads the typed
/// field instead of classifying the rendered message body by content.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptUserRole {
    /// Ordinary conversational user input.
    #[default]
    Conversational,
    /// A runtime-produced compaction summary that opens a rebuilt transcript.
    CompactionSummary,
    /// Host-attached ambient context delivered alongside (not inside) the
    /// user's message. Excluded from semantic-memory indexing, and it never
    /// satisfies the transcript-continuity save-guard
    /// ([`Self::is_compaction_summary`] stays `CompactionSummary`-only).
    InjectedContext,
}

impl TranscriptUserRole {
    /// Whether this is the default (`Conversational`) role. Used by
    /// `skip_serializing_if` so ordinary user messages serialize without the
    /// field.
    #[must_use]
    pub fn is_conversational(&self) -> bool {
        matches!(self, Self::Conversational)
    }

    /// Whether this user message is a runtime compaction summary.
    #[must_use]
    pub fn is_compaction_summary(&self) -> bool {
        matches!(self, Self::CompactionSummary)
    }

    /// Whether this user message is host-attached injected context.
    #[must_use]
    pub fn is_injected_context(&self) -> bool {
        matches!(self, Self::InjectedContext)
    }
}

/// User message content
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UserMessage {
    #[serde(with = "content_blocks_serde")]
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<RenderMetadata>,
    #[serde(default, skip_serializing_if = "TranscriptMessageIdentity::is_empty")]
    pub identity: TranscriptMessageIdentity,
    /// Typed transcript role. Defaults to [`TranscriptUserRole::Conversational`];
    /// runtime compaction marks the summary boundary message as
    /// [`TranscriptUserRole::CompactionSummary`].
    #[serde(default, skip_serializing_if = "TranscriptUserRole::is_conversational")]
    pub transcript_role: TranscriptUserRole,
    #[serde(default = "message_timestamp_now")]
    pub created_at: MessageTimestamp,
}

impl UserMessage {
    /// Create a text-only user message.
    pub fn text(content: impl Into<String>) -> Self {
        Self::text_with_render_metadata(content, None)
    }

    /// Create a text-only user message with optional render metadata.
    pub fn text_with_render_metadata(
        content: impl Into<String>,
        render_metadata: Option<RenderMetadata>,
    ) -> Self {
        Self {
            content: ContentBlock::text_vec(content.into()),
            render_metadata,
            identity: TranscriptMessageIdentity::default(),
            transcript_role: TranscriptUserRole::Conversational,
            created_at: message_timestamp_now(),
        }
    }

    /// Create a runtime compaction-summary boundary message.
    ///
    /// The returned message carries [`TranscriptUserRole::CompactionSummary`] so
    /// the transcript-continuity save-guard recognizes the rebuilt-transcript
    /// boundary from a typed field rather than a rendered string prefix.
    pub fn compaction_summary(content: impl Into<String>) -> Self {
        Self {
            content: ContentBlock::text_vec(content.into()),
            render_metadata: None,
            identity: TranscriptMessageIdentity::default(),
            transcript_role: TranscriptUserRole::CompactionSummary,
            created_at: message_timestamp_now(),
        }
    }

    /// Create a text-only host-attached injected-context message.
    ///
    /// The returned message carries [`TranscriptUserRole::InjectedContext`] so
    /// consumers (semantic-memory indexing, wire projections) read the typed
    /// fact instead of classifying the rendered content.
    pub fn injected_context(content: impl Into<String>) -> Self {
        Self {
            content: ContentBlock::text_vec(content.into()),
            render_metadata: None,
            identity: TranscriptMessageIdentity::default(),
            transcript_role: TranscriptUserRole::InjectedContext,
            created_at: message_timestamp_now(),
        }
    }

    /// Create a multimodal host-attached injected-context message.
    ///
    /// Block-bearing sibling of [`UserMessage::injected_context`].
    pub fn injected_context_with_blocks(content: Vec<ContentBlock>) -> Self {
        Self {
            content,
            render_metadata: None,
            identity: TranscriptMessageIdentity::default(),
            transcript_role: TranscriptUserRole::InjectedContext,
            created_at: message_timestamp_now(),
        }
    }

    /// Create a multimodal user message.
    pub fn with_blocks(content: Vec<ContentBlock>) -> Self {
        Self::with_blocks_and_render_metadata(content, None)
    }

    /// Create a multimodal user message with optional render metadata.
    pub fn with_blocks_and_render_metadata(
        content: Vec<ContentBlock>,
        render_metadata: Option<RenderMetadata>,
    ) -> Self {
        Self {
            content,
            render_metadata,
            identity: TranscriptMessageIdentity::default(),
            transcript_role: TranscriptUserRole::Conversational,
            created_at: message_timestamp_now(),
        }
    }

    /// Get concatenated text content (text projection for all blocks).
    pub fn text_content(&self) -> String {
        text_content(&self.content)
    }

    /// Check whether any content blocks are images.
    pub fn has_images(&self) -> bool {
        has_images(&self.content)
    }

    /// Check whether any content blocks are video.
    pub fn has_video(&self) -> bool {
        has_video(&self.content)
    }

    /// Check whether any content blocks are non-text.
    pub fn has_non_text_content(&self) -> bool {
        has_non_text_content(&self.content)
    }
}

/// Assistant message with ordered blocks - no billing metadata.
///
/// The canonical transcript representation for assistant output: an ordered
/// sequence of typed [`AssistantBlock`]s plus the stop reason.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockAssistantMessage {
    /// Ordered sequence of content blocks
    pub blocks: Vec<AssistantBlock>,
    /// How the turn ended
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "TranscriptMessageIdentity::is_empty")]
    pub identity: TranscriptMessageIdentity,
    /// When this assistant message was committed to the transcript.
    #[serde(default = "message_timestamp_now")]
    pub created_at: MessageTimestamp,
}

// Usage is NOT part of BlockAssistantMessage.
// It's billing metadata returned separately from LLM calls.
// See LlmResponse in meerkat-client.

impl BlockAssistantMessage {
    pub fn new(blocks: Vec<AssistantBlock>, stop_reason: StopReason) -> Self {
        Self {
            blocks,
            stop_reason,
            identity: TranscriptMessageIdentity::default(),
            created_at: message_timestamp_now(),
        }
    }

    /// Iterate over tool calls without allocation.
    pub fn tool_calls(&self) -> ToolCallIter<'_> {
        ToolCallIter {
            inner: self.blocks.iter(),
        }
    }

    /// Check if any tool calls are present.
    pub fn has_tool_calls(&self) -> bool {
        self.blocks
            .iter()
            .any(|b| matches!(b, AssistantBlock::ToolUse { .. }))
    }

    pub fn has_visible_or_actionable_output(&self) -> bool {
        assistant_blocks_have_visible_or_actionable_output(&self.blocks)
    }

    /// Get tool use block by ID.
    pub fn get_tool_use(&self, id: &str) -> Option<ToolCallView<'_>> {
        self.tool_calls().find(|tc| tc.id == id)
    }

    /// Iterate over visible text blocks without allocation.
    ///
    /// Includes both `Text` (display lane) and `Transcript` (spoken lane)
    /// since both project to the same human-readable text stream. Lane
    /// provenance is preserved structurally on `AssistantBlock` itself; this
    /// helper is for callers that just want the rendered text.
    pub fn text_blocks(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().filter_map(|b| match b {
            AssistantBlock::Text { text, .. } | AssistantBlock::Transcript { text, .. } => {
                Some(text.as_str())
            }
            _ => None,
        })
    }
}

/// Display implementation for text content - zero intermediate allocations.
///
/// Renders both `Text` (display) and `Transcript` (spoken) blocks since both
/// project to the same human-readable text stream. Lane provenance lives on
/// the `AssistantBlock` enum itself for callers that need it.
impl fmt::Display for BlockAssistantMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for block in &self.blocks {
            match block {
                AssistantBlock::Text { text, .. } | AssistantBlock::Transcript { text, .. } => {
                    f.write_str(text)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// A tool call requested by the model
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolCall {
    /// Unique ID for this tool use (from the model)
    pub id: String,
    /// Name of the tool to invoke
    pub name: String,
    /// Arguments as JSON
    pub args: Value,
}

impl ToolCall {
    /// Create a new tool call
    pub fn new(id: String, name: String, args: Value) -> Self {
        Self { id, name, args }
    }

    /// Return the typed model-facing tool name for internal policy and routing.
    pub fn tool_name(&self) -> ToolName {
        ToolName::new(self.name.clone())
    }
}

/// Result of executing a tool
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolResult {
    /// Matches the tool_call.id
    pub tool_use_id: String,
    /// Content returned by the tool (text or multimodal blocks).
    ///
    /// Backwards-compatible serde: deserializes from plain string or array,
    /// serializes text-only content as string.
    #[serde(with = "content_blocks_serde")]
    pub content: Vec<ContentBlock>,
    /// Whether this is an error result
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    /// Create a text-only tool result.
    pub fn new(tool_use_id: String, content: String, is_error: bool) -> Self {
        Self {
            tool_use_id,
            content: ContentBlock::text_vec(content),
            is_error,
        }
    }

    /// Create a tool result from a ToolCall with text content.
    pub fn from_tool_call(tool_call: &ToolCall, content: String, is_error: bool) -> Self {
        Self {
            tool_use_id: tool_call.id.clone(),
            content: ContentBlock::text_vec(content),
            is_error,
        }
    }

    /// Create a tool result with multimodal content blocks.
    pub fn with_blocks(tool_use_id: String, content: Vec<ContentBlock>, is_error: bool) -> Self {
        Self {
            tool_use_id,
            content,
            is_error,
        }
    }

    /// Get concatenated text content (text projection for all blocks).
    pub fn text_content(&self) -> String {
        text_content(&self.content)
    }

    /// Check whether any content blocks are images.
    pub fn has_images(&self) -> bool {
        has_images(&self.content)
    }

    /// Check whether any content blocks are video.
    pub fn has_video(&self) -> bool {
        has_video(&self.content)
    }

    /// Set text content, replacing all blocks with a single text block.
    pub fn set_text_content(&mut self, text: String) {
        self.content = ContentBlock::text_vec(text);
    }
}

/// Why the model stopped generating
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model finished naturally
    #[default]
    EndTurn,
    /// Model wants to call tools
    ToolUse,
    /// Hit max output tokens
    MaxTokens,
    /// Hit a stop sequence
    StopSequence,
    /// Blocked by content filter
    ContentFilter,
    /// Request was cancelled
    Cancelled,
}

/// Security mode for tool execution (e.g. shell)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    /// No restrictions. (Default)
    #[default]
    Unrestricted,
    /// Only allow commands matching specified patterns.
    AllowList,
    /// Block commands matching specified patterns.
    DenyList,
}

/// Token usage statistics
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
}

impl Usage {
    /// Get total tokens used
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Accumulate usage from another
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        if let Some(c) = other.cache_creation_tokens {
            *self.cache_creation_tokens.get_or_insert(0) += c;
        }
        if let Some(c) = other.cache_read_tokens {
            *self.cache_read_tokens.get_or_insert(0) += c;
        }
    }
}

/// Source kind for tool provenance tracking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSourceKind {
    Builtin,
    Shell,
    Comms,
    Memory,
    Schedule,
    WorkGraph,
    Mob,
    Callback,
    Mcp,
    RustBundle,
}

/// Provenance metadata for a tool definition.
///
/// Tracks which subsystem materialized this tool and the specific source
/// instance (e.g., MCP server name, bundle name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProvenance {
    pub kind: ToolSourceKind,
    pub source_id: ToolSourceId,
}

/// Canonical typed source identifier for tool provenance.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolSourceId(String);

impl ToolSourceId {
    pub fn new(source_id: impl Into<String>) -> Self {
        Self(source_id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for ToolSourceId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<ToolSourceId> for String {
    fn from(value: ToolSourceId) -> Self {
        value.into_string()
    }
}

impl From<&str> for ToolSourceId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for ToolSourceId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for ToolSourceId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for ToolSourceId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for ToolSourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<str> for ToolSourceId {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ToolSourceId {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for ToolSourceId {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<ToolSourceId> for String {
    fn eq(&self, other: &ToolSourceId) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<ToolSourceId> for str {
    fn eq(&self, other: &ToolSourceId) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<ToolSourceId> for &str {
    fn eq(&self, other: &ToolSourceId) -> bool {
        *self == other.as_str()
    }
}

/// Canonical typed name for an LLM-callable tool.
///
/// The model-facing wire shape is still the provider's string tool name, but
/// internal policy, routing, and registry surfaces carry this handle so raw
/// strings do not become the tool identity owner.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for ToolName {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<ToolName> for String {
    fn from(value: ToolName) -> Self {
        value.into_string()
    }
}

impl From<&str> for ToolName {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for ToolName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for ToolName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialEq<str> for ToolName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for ToolName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for ToolName {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<ToolName> for String {
    fn eq(&self, other: &ToolName) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<ToolName> for str {
    fn eq(&self, other: &ToolName) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<ToolName> for &str {
    fn eq(&self, other: &ToolName) -> bool {
        *self == other.as_str()
    }
}

/// Set of canonical tool names.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolNameSet(pub std::collections::HashSet<ToolName>);

impl ToolNameSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<ToolName>) -> bool {
        self.0.insert(name.into())
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.0.remove(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ToolName> {
        self.0.iter()
    }

    pub fn retain(&mut self, mut f: impl FnMut(&ToolName) -> bool) {
        self.0.retain(|name| f(name));
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn into_inner(self) -> std::collections::HashSet<ToolName> {
        self.0
    }

    pub fn to_string_set(&self) -> std::collections::HashSet<String> {
        self.iter().map(|name| name.as_str().to_string()).collect()
    }
}

impl From<ToolName> for ToolNameSet {
    fn from(value: ToolName) -> Self {
        Self(std::iter::once(value).collect())
    }
}

impl std::iter::FromIterator<ToolName> for ToolNameSet {
    fn from_iter<T: IntoIterator<Item = ToolName>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::iter::FromIterator<String> for ToolNameSet {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        Self(iter.into_iter().map(ToolName::from).collect())
    }
}

impl<'a> std::iter::FromIterator<&'a str> for ToolNameSet {
    fn from_iter<T: IntoIterator<Item = &'a str>>(iter: T) -> Self {
        Self(iter.into_iter().map(ToolName::from).collect())
    }
}

impl Extend<ToolName> for ToolNameSet {
    fn extend<T: IntoIterator<Item = ToolName>>(&mut self, iter: T) {
        self.0.extend(iter);
    }
}

impl<'a> Extend<&'a ToolName> for ToolNameSet {
    fn extend<T: IntoIterator<Item = &'a ToolName>>(&mut self, iter: T) {
        self.0.extend(iter.into_iter().cloned());
    }
}

impl Extend<String> for ToolNameSet {
    fn extend<T: IntoIterator<Item = String>>(&mut self, iter: T) {
        self.0.extend(iter.into_iter().map(ToolName::from));
    }
}

impl IntoIterator for ToolNameSet {
    type Item = ToolName;
    type IntoIter = std::collections::hash_set::IntoIter<ToolName>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a ToolNameSet {
    type Item = &'a ToolName;
    type IntoIter = std::collections::hash_set::Iter<'a, ToolName>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Provenance-aware identity for a concrete tool definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolIdentity {
    pub name: ToolName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}

/// Tool definition for the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolDef {
    pub name: ToolName,
    pub description: String,
    pub input_schema: Value,
    /// Optional provenance metadata tracking which subsystem materialized this tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ToolProvenance>,
}

impl ToolDef {
    /// Create a new ToolDef without provenance.
    pub fn new(
        name: impl Into<ToolName>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            provenance: None,
        }
    }

    /// Set provenance on this ToolDef (chainable builder).
    pub fn with_provenance(mut self, provenance: ToolProvenance) -> Self {
        self.provenance = Some(provenance);
        self
    }

    /// Return the typed, provenance-aware identity for this tool definition.
    pub fn identity(&self) -> ToolIdentity {
        ToolIdentity {
            name: self.name.clone(),
            provenance: self.provenance.clone(),
        }
    }

    /// Return the typed model-facing tool name.
    pub fn tool_name(&self) -> ToolName {
        self.name.clone()
    }
}

/// Structured-output extraction failure details for a run whose main agentic
/// turn completed successfully.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExtractionError {
    /// The committed main-turn assistant output that extraction attempted to
    /// transform.
    pub last_output: String,
    /// Number of extraction attempts made before the failure was reported.
    ///
    /// For validation exhaustion this is the number of failed validation
    /// attempts. Other extraction failures may report the attempts completed
    /// before the failing extraction step.
    pub attempts: u32,
    /// Human-readable extraction failure reason.
    pub reason: String,
}

/// Result of a successful agent run
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunResult {
    /// Final committed text response from the main agentic turn.
    pub text: String,
    /// Session ID for resumption
    pub session_id: SessionId,
    /// Total token usage
    pub usage: Usage,
    /// Number of turns taken
    pub turns: u32,
    /// Number of tool calls made
    pub tool_calls: u32,
    /// Machine-owned terminal cause when success represents a typed terminal
    /// condition, such as budget exhaustion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_cause_kind: Option<crate::turn_execution_authority::TurnTerminalCauseKind>,
    /// Structured output (if output_schema was provided and extraction succeeded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    /// Extraction failure details when output_schema extraction was configured
    /// and a post-main-turn extraction step failed after the main run
    /// completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extraction_error: Option<ExtractionError>,
    /// Warnings produced during schema compilation (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_warnings: Option<Vec<SchemaWarning>>,
    /// Skill subsystem diagnostics for operator surfaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_diagnostics: Option<crate::skills::SkillRuntimeDiagnostics>,
}

/// Reference to an artifact stored externally
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArtifactRef {
    /// Unique artifact identifier
    pub id: String,
    /// Session this artifact belongs to
    pub session_id: SessionId,
    /// Size in bytes
    pub size_bytes: u64,
    /// Time-to-live in seconds (None = permanent)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    /// Format version for migrations
    pub version: u32,
}

#[cfg(test)]
mod tests;

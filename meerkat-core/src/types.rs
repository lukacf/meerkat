//! Core types for Meerkat
//!
//! These types form the representation boundary for session persistence and wire formats.

use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
        }
    }

    /// Convenience: wrap a string into a single-element Vec of Text blocks.
    pub fn text_vec(s: String) -> Vec<ContentBlock> {
        vec![ContentBlock::Text { text: s }]
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
}

impl fmt::Display for ContentBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text_projection())
    }
}

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
        /// Provider-native tool name, for example `web_search` or `google_search`.
        name: String,
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
        AssistantBlock::Reasoning { .. } | AssistantBlock::ServerToolContent { .. } => false,
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
                    name: n1,
                    content: c1,
                    meta: m1,
                },
                AssistantBlock::ServerToolContent {
                    id: i2,
                    name: n2,
                    content: c2,
                    meta: m2,
                },
            ) => i1 == i2 && n1 == n2 && c1 == c2 && m1 == m2,
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

/// Iterator over tool calls in an AssistantMessage. Zero allocations.
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
#[derive(Debug, Clone)]
pub enum Message {
    /// System prompt (injected at start)
    System(SystemMessage),
    /// Typed synthetic/runtime system notice.
    SystemNotice(SystemNoticeMessage),
    /// User input
    User(UserMessage),
    /// Assistant response (may include tool calls) - legacy format
    Assistant(AssistantMessage),
    /// Assistant response with ordered blocks - new format (spec v2)
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

    /// Extract text content suitable for semantic indexing.
    ///
    /// Returns user and assistant text content. System messages and
    /// tool results are excluded (system prompts are not conversation
    /// content, tool results are structured data).
    pub fn as_indexable_text(&self) -> String {
        match self {
            Message::SystemNotice(_) => String::new(),
            Message::User(u) => u.text_content(),
            Message::Assistant(a) => a.content.clone(),
            Message::BlockAssistant(ba) => {
                let mut result = String::new();
                for text in ba.text_blocks() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(text);
                }
                result
            }
            Message::System(_) | Message::ToolResults { .. } => String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
enum MessageWire {
    System(SystemMessage),
    SystemNotice(SystemNoticeMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
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
            Self::Assistant(message) => MessageWire::Assistant(message.clone()),
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
            MessageWire::User(message) => SystemNoticeMessage::try_from_legacy_user(&message)
                .map(Self::SystemNotice)
                .unwrap_or(Self::User(message)),
            MessageWire::Assistant(message) => Self::Assistant(message),
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemMessage {
    pub content: String,
    #[serde(default = "message_timestamp_now")]
    pub created_at: MessageTimestamp,
}

impl SystemMessage {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            created_at: message_timestamp_now(),
        }
    }
}

/// Typed system notice content carried in the transcript.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemNoticeKind {
    Generic,
    McpPending,
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
            Self::BackgroundJob => RenderClass::OpsProgress,
            Self::ToolScope | Self::ToolScopeWarning => RenderClass::ToolScopeNotice,
        }
    }

    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Generic => "[SYSTEM NOTICE]",
            Self::McpPending => "[SYSTEM NOTICE][MCP_PENDING]",
            Self::BackgroundJob => "[SYSTEM NOTICE][BG_JOB]",
            Self::ToolScope => "[SYSTEM NOTICE][TOOL_SCOPE]",
            Self::ToolScopeWarning => "[SYSTEM NOTICE][TOOL_SCOPE][WARNING]",
            Self::AuthReauthRequired => "[SYSTEM NOTICE][AUTH_REAUTH_REQUIRED]",
        }
    }

    fn parse_legacy(text: &str) -> Option<(Self, &str)> {
        const PREFIXES: &[(SystemNoticeKind, &str)] = &[
            (
                SystemNoticeKind::ToolScopeWarning,
                "[SYSTEM NOTICE][TOOL_SCOPE][WARNING] ",
            ),
            (SystemNoticeKind::ToolScope, "[SYSTEM NOTICE][TOOL_SCOPE] "),
            (
                SystemNoticeKind::McpPending,
                "[SYSTEM NOTICE][MCP_PENDING] ",
            ),
            (SystemNoticeKind::BackgroundJob, "[SYSTEM NOTICE][BG_JOB] "),
            (
                SystemNoticeKind::AuthReauthRequired,
                "[SYSTEM NOTICE][AUTH_REAUTH_REQUIRED] ",
            ),
            (SystemNoticeKind::Generic, "[SYSTEM NOTICE] "),
        ];

        PREFIXES
            .iter()
            .find_map(|(kind, prefix)| text.strip_prefix(prefix).map(|body| (*kind, body)))
    }
}

/// System notice message stored in the canonical transcript.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemNoticeMessage {
    pub kind: SystemNoticeKind,
    pub body: String,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    #[serde(default = "message_timestamp_now")]
    pub created_at: MessageTimestamp,
}

impl SystemNoticeMessage {
    pub fn new(kind: SystemNoticeKind, body: impl Into<String>) -> Self {
        Self {
            kind,
            body: body.into(),
            created_at: message_timestamp_now(),
        }
    }

    pub fn rendered_text(&self) -> String {
        format!("{} {}", self.kind.prefix(), self.body)
    }

    fn try_from_legacy_user(user: &UserMessage) -> Option<Self> {
        let expected_class = user.render_metadata.as_ref().map(|meta| meta.class)?;
        let text = user.text_content();
        let (kind, body) = SystemNoticeKind::parse_legacy(&text)?;
        if expected_class != kind.render_class() {
            return None;
        }
        Some(Self::new(kind, body))
    }
}

/// User message content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UserMessage {
    #[serde(with = "content_blocks_serde")]
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_metadata: Option<RenderMetadata>,
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

/// New assistant message with ordered blocks - no billing metadata.
///
/// This is the new format for storing assistant messages with full block ordering.
/// The existing `AssistantMessage` is preserved for backwards compatibility during
/// the migration period.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockAssistantMessage {
    /// Ordered sequence of content blocks
    pub blocks: Vec<AssistantBlock>,
    /// How the turn ended
    pub stop_reason: StopReason,
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

/// Assistant message with potential tool calls (legacy format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AssistantMessage {
    /// Text content (may be empty if only tool calls)
    pub content: String,
    /// Tool calls requested by the model
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// How the turn ended
    pub stop_reason: StopReason,
    /// Token usage for this turn
    pub usage: Usage,
    /// When this assistant message was committed to the transcript.
    #[serde(default = "message_timestamp_now")]
    pub created_at: MessageTimestamp,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

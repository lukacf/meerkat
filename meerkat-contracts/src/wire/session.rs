//! Wire session types.

use serde::{Deserialize, Serialize};

fn serialize_raw_json_box<S>(
    value: &serde_json::value::RawValue,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let value: serde_json::Value =
        serde_json::from_str(value.get()).map_err(serde::ser::Error::custom)?;
    value.serialize(serializer)
}

fn deserialize_raw_json_box<'de, D>(
    deserializer: D,
) -> Result<Box<serde_json::value::RawValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    serde_json::value::RawValue::from_string(value.to_string()).map_err(serde::de::Error::custom)
}
use std::collections::BTreeMap;

use meerkat_core::{
    AssistantBlock, BlobId, BlockAssistantMessage, ContentBlock, ContentInput, ImageData, Message,
    ProviderMeta, ServerToolKind, SessionHistoryPage, SessionId, SessionInfo, SessionSummary,
    SessionTranscriptRevisionPage, StopReason, SystemMessage, SystemNoticeKind,
    SystemNoticeMessage, ToolResult, TranscriptEditRunningBehavior, TranscriptReplacement,
    TranscriptRewriteReason, TranscriptRewriteSelection, TranscriptSource, UserMessage, VideoData,
};
use std::convert::TryFrom;

/// Request payload for `session/stream_open`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SessionStreamOpenParams {
    pub session_id: String,
}

/// Response payload for `session/stream_open`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionStreamOpenResult {
    pub stream_id: String,
    pub session_id: String,
    pub opened: bool,
}

/// Request payload for `session/stream_close`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SessionStreamCloseParams {
    pub stream_id: String,
}

/// Response payload for `session/stream_close`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionStreamCloseResult {
    pub stream_id: String,
    pub closed: bool,
    pub already_closed: bool,
}

/// Request payload for `session/fork_at`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ForkSessionAtParams {
    pub session_id: String,
    pub message_index: usize,
    #[serde(default)]
    pub running_behavior: TranscriptEditRunningBehavior,
}

/// Request payload for `session/fork_replace`.
///
/// `replacement` rides as the typed [`WireTranscriptReplacement`] mirror so
/// the emitted JSON schema is the closed replacement enum rather than a bare
/// `serde_json::Value`. The consuming surface lowers it into the core
/// [`TranscriptReplacement`] via [`WireTranscriptReplacement::into_core`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ForkSessionReplaceParams {
    pub session_id: String,
    pub message_index: usize,
    pub replacement: WireTranscriptReplacement,
    #[serde(default)]
    pub running_behavior: TranscriptEditRunningBehavior,
}

/// Public transcript message accepted by same-session rewrite APIs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum TranscriptRewriteMessage {
    System {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<String>,
    },
    SystemNotice {
        kind: SystemNoticeKind,
        #[serde(default, alias = "content", skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blocks: Vec<meerkat_core::SystemNoticeBlock>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<String>,
    },
    User {
        content: WireContentInput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<String>,
    },
    #[serde(rename = "block_assistant")]
    BlockAssistant {
        blocks: Vec<WireAssistantBlock>,
        #[serde(default)]
        stop_reason: WireStopReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<String>,
    },
    #[serde(rename = "tool_results")]
    ToolResults {
        results: Vec<WireToolResult>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_at: Option<String>,
    },
}

/// Request payload for `session/rewrite_transcript`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RewriteSessionTranscriptParams {
    pub session_id: String,
    pub selection: TranscriptRewriteSelection,
    pub replacement: Vec<TranscriptRewriteMessage>,
    pub reason: TranscriptRewriteReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_parent_revision: Option<String>,
    #[serde(default)]
    pub running_behavior: TranscriptEditRunningBehavior,
}

/// Request payload for `session/restore_transcript_revision`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RestoreSessionTranscriptRevisionParams {
    pub session_id: String,
    pub revision: String,
    pub reason: TranscriptRewriteReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_parent_revision: Option<String>,
    #[serde(default)]
    pub running_behavior: TranscriptEditRunningBehavior,
}

/// Request payload for `session/transcript_revision`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReadSessionTranscriptRevisionParams {
    pub session_id: String,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Canonical content-addressed transcript revision identity (e.g. a
/// `sha256:...` digest). A newtype over the wire string so revision identity is
/// never confused with the `"current"` head sentinel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RevisionId(pub String);

impl RevisionId {
    /// The literal revision string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the owned revision string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for RevisionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Wire sentinel that selects the session head revision.
const REVISION_SELECTOR_CURRENT: &str = "current";

/// Typed selector for a transcript revision lookup. The wire carries a bare
/// string where `"current"` is an undocumented sentinel meaning "resolve to the
/// session head". Parsing that string into this enum at the consumer boundary
/// replaces the `revision == "current"` string oracle with a closed type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionSelector {
    /// Resolve to the current session head revision.
    Current,
    /// Use this literal content-addressed revision.
    Specific(RevisionId),
}

impl RevisionSelector {
    /// Parse the wire revision string into a typed selector. The `"current"`
    /// sentinel selects the head; anything else is a specific revision id.
    pub fn parse(revision: impl Into<String>) -> Self {
        let revision = revision.into();
        if revision == REVISION_SELECTOR_CURRENT {
            Self::Current
        } else {
            Self::Specific(RevisionId(revision))
        }
    }
}

/// Canonical session info for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionInfo {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub is_active: bool,
    pub model: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_assistant_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_capabilities: Option<crate::wire::WireResolvedModelCapabilities>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl From<SessionInfo> for WireSessionInfo {
    fn from(info: SessionInfo) -> Self {
        Self {
            session_id: info.session_id,
            session_ref: None,
            created_at: info
                .created_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            updated_at: info
                .updated_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            message_count: info.message_count,
            is_active: info.is_active,
            model: info.model,
            provider: info.provider.as_str().to_string(),
            last_assistant_text: info.last_assistant_text,
            resolved_capabilities: None,
            labels: info.labels,
        }
    }
}

/// Canonical session summary for wire protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionSummary {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub message_count: usize,
    pub total_tokens: u64,
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl From<SessionSummary> for WireSessionSummary {
    fn from(summary: SessionSummary) -> Self {
        Self {
            session_id: summary.session_id,
            session_ref: None,
            created_at: summary
                .created_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            updated_at: summary
                .updated_at
                .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            message_count: summary.message_count,
            total_tokens: summary.total_tokens,
            is_active: summary.is_active,
            labels: summary.labels,
        }
    }
}

/// Provider continuity metadata for transcript blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum WireProviderMeta {
    Anthropic {
        signature: String,
    },
    AnthropicRedacted {
        data: String,
    },
    AnthropicCompaction {
        content: String,
    },
    Gemini {
        #[serde(rename = "thoughtSignature")]
        thought_signature: String,
    },
    OpenAi {
        id: String,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },
    Unknown,
}

impl From<ProviderMeta> for WireProviderMeta {
    fn from(value: ProviderMeta) -> Self {
        match value {
            ProviderMeta::Anthropic { signature } => Self::Anthropic { signature },
            ProviderMeta::AnthropicRedacted { data } => Self::AnthropicRedacted { data },
            ProviderMeta::AnthropicCompaction { content } => Self::AnthropicCompaction { content },
            ProviderMeta::Gemini { thought_signature } => Self::Gemini { thought_signature },
            ProviderMeta::OpenAi {
                id,
                encrypted_content,
                phase,
                ..
            } => Self::OpenAi {
                id,
                encrypted_content,
                phase,
            },
            _ => Self::Unknown,
        }
    }
}

/// Wire projection of `meerkat_core::ServerToolKind`.
///
/// Mirrors the typed semantic owner of a provider-executed tool. The core enum
/// is `#[non_exhaustive]`; a future semantic kind that lacks an explicit arm in
/// the forward `From` surfaces as `Unknown { debug }` rather than silently
/// collapsing into a plausible-but-wrong kind. SDK consumers route on the
/// `kind` discriminator and treat `unknown` as unrecognized.
///
/// **When a new core variant is added, add an explicit arm in the forward
/// `From` impl above the wildcard — `Unknown` is the floor, not the
/// destination.**
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireServerToolKind {
    WebSearch,
    GoogleSearch,
    ProviderNative { name: String },
    Unknown { debug: String },
}

impl From<ServerToolKind> for WireServerToolKind {
    fn from(value: ServerToolKind) -> Self {
        match value {
            ServerToolKind::WebSearch => Self::WebSearch,
            ServerToolKind::GoogleSearch => Self::GoogleSearch,
            ServerToolKind::ProviderNative { name } => Self::ProviderNative { name },
            // Core enum is `#[non_exhaustive]`. Surface unknown semantic kinds
            // explicitly rather than coercing to a plausible-but-wrong kind.
            // **When a new core variant is added, add an explicit arm above.**
            other => Self::Unknown {
                debug: format!("{other:?}"),
            },
        }
    }
}

impl TryFrom<WireServerToolKind> for ServerToolKind {
    type Error = crate::wire::error::WireConversionError;

    fn try_from(value: WireServerToolKind) -> Result<Self, Self::Error> {
        match value {
            WireServerToolKind::WebSearch => Ok(Self::WebSearch),
            WireServerToolKind::GoogleSearch => Ok(Self::GoogleSearch),
            WireServerToolKind::ProviderNative { name } => Ok(Self::ProviderNative { name }),
            WireServerToolKind::Unknown { debug } => {
                Err(crate::wire::error::WireConversionError::AssistantBlock { debug })
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum WireImageData {
    Inline {
        data: String,
    },
    Blob {
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        blob_id: BlobId,
    },
}

impl From<String> for WireImageData {
    fn from(data: String) -> Self {
        Self::Inline { data }
    }
}

impl From<&str> for WireImageData {
    fn from(data: &str) -> Self {
        Self::Inline {
            data: data.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum WireVideoData {
    Inline { data: String },
}

impl From<String> for WireVideoData {
    fn from(data: String) -> Self {
        Self::Inline { data }
    }
}

impl From<&str> for WireVideoData {
    fn from(data: &str) -> Self {
        Self::Inline {
            data: data.to_string(),
        }
    }
}

/// Wire-safe content block (no `source_path` — internal only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        #[serde(flatten)]
        data: WireImageData,
    },
    Video {
        media_type: String,
        duration_ms: u64,
        #[serde(flatten)]
        data: WireVideoData,
    },
    /// Structured JSON tool output, materialized as inline JSON for SDK
    /// consumers (the core side keeps it as opaque `Box<RawValue>`).
    Structured {
        data: serde_json::Value,
    },
    /// Forward-compatibility for unknown block types.
    #[serde(other)]
    Unknown,
}

impl From<ContentBlock> for WireContentBlock {
    fn from(block: ContentBlock) -> Self {
        match block {
            ContentBlock::Text { text } => WireContentBlock::Text { text },
            ContentBlock::Image { media_type, data } => WireContentBlock::Image {
                media_type,
                data: match data {
                    ImageData::Inline { data } => WireImageData::Inline { data },
                    ImageData::Blob { blob_id } => WireImageData::Blob { blob_id },
                },
            },
            ContentBlock::Video {
                media_type,
                duration_ms,
                data,
            } => WireContentBlock::Video {
                media_type,
                duration_ms,
                data: match data {
                    VideoData::Inline { data } => WireVideoData::Inline { data },
                },
            },
            ContentBlock::Structured { data } => WireContentBlock::Structured {
                // Materialize the opaque `Box<RawValue>` into a wire JSON value.
                // The payload is valid JSON by construction; preserve the raw
                // bytes losslessly on the unreachable parse-failure path rather
                // than fabricating a success-shaped null.
                data: serde_json::from_str(data.get())
                    .unwrap_or_else(|_| serde_json::Value::String(data.get().to_owned())),
            },
            _ => WireContentBlock::Unknown,
        }
    }
}

impl TryFrom<WireContentBlock> for ContentBlock {
    type Error = &'static str;

    fn try_from(block: WireContentBlock) -> Result<Self, Self::Error> {
        match block {
            WireContentBlock::Text { text } => Ok(ContentBlock::Text { text }),
            WireContentBlock::Image { media_type, data } => Ok(ContentBlock::Image {
                media_type,
                data: match data {
                    WireImageData::Inline { data } => ImageData::Inline { data },
                    WireImageData::Blob { blob_id } => ImageData::Blob { blob_id },
                },
            }),
            WireContentBlock::Video {
                media_type,
                duration_ms,
                data,
            } => Ok(ContentBlock::Video {
                media_type,
                duration_ms,
                data: match data {
                    WireVideoData::Inline { data } => VideoData::Inline { data },
                },
            }),
            WireContentBlock::Structured { data } => Ok(ContentBlock::Structured {
                data: serde_json::value::to_raw_value(&data)
                    .map_err(|_| "structured content block is not serializable JSON")?,
            }),
            WireContentBlock::Unknown => Err("unknown content block type"),
        }
    }
}

/// Wire-safe content input (mirrors `ContentInput`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum WireContentInput {
    Text(String),
    Blocks(Vec<WireContentBlock>),
}

impl From<ContentInput> for WireContentInput {
    fn from(input: ContentInput) -> Self {
        match input {
            ContentInput::Text(s) => WireContentInput::Text(s),
            ContentInput::Blocks(blocks) => {
                WireContentInput::Blocks(blocks.into_iter().map(Into::into).collect())
            }
        }
    }
}

impl TryFrom<WireContentInput> for ContentInput {
    type Error = &'static str;

    fn try_from(input: WireContentInput) -> Result<Self, Self::Error> {
        match input {
            WireContentInput::Text(text) => Ok(ContentInput::Text(text)),
            WireContentInput::Blocks(blocks) => Ok(ContentInput::Blocks(
                blocks
                    .into_iter()
                    .map(ContentBlock::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
        }
    }
}

/// Wire-safe tool result content that handles both legacy string and array formats.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum WireToolResultContent {
    Text(String),
    Blocks(Vec<WireContentBlock>),
}

/// Transcript block inside a block-assistant message.
///
/// Not `PartialEq`: `ToolUse.args` is `Box<RawValue>` for pass-through
/// fidelity (core invariant — opaque from provider to dispatcher), and
/// `RawValue` does not derive equality. Equivalence checks should
/// round-trip through serialization and compare the serialized bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "block_type", content = "data", rename_all = "snake_case")]
pub enum WireAssistantBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    /// Spoken-transcript output (provider audio output → text). Distinct
    /// from `Text` so callers can render or filter by lane provenance.
    Transcript {
        text: String,
        source: WireTranscriptSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    Reasoning {
        #[serde(default)]
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    ToolUse {
        id: String,
        name: String,
        /// Wire-projected tool-call args. Carries the opaque JSON payload
        /// as `Box<RawValue>` to mirror the core `AssistantBlock::ToolUse`
        /// invariant ("tool-call args pass through un-parsed from provider
        /// to dispatcher") — see `CLAUDE.md` Rust design principles §3.
        /// Allow-listed by dogma-blind-spots §7.
        #[serde(
            serialize_with = "serialize_raw_json_box",
            deserialize_with = "deserialize_raw_json_box"
        )]
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        args: Box<serde_json::value::RawValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    ServerToolContent {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        kind: WireServerToolKind,
        content: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<WireProviderMeta>,
    },
    Image {
        image_id: meerkat_core::AssistantImageId,
        blob_ref: meerkat_core::BlobRef,
        media_type: meerkat_core::MediaType,
        width: u32,
        height: u32,
        revised_prompt: meerkat_core::RevisedPromptDisposition,
        meta: meerkat_core::ProviderImageMetadata,
    },
    #[serde(other)]
    Unknown,
}

/// Wire projection of `meerkat_core::TranscriptSource`. Lane provenance
/// for spoken-transcript blocks.
///
/// R7-4 (P3 dogma): the previous shape used a wildcard arm in
/// `From<TranscriptSource>` that fell through to `Spoken`, silently
/// misattributing future core variants as user-spoken transcript. The
/// fix mirrors the live-wire pattern from R5-3 / R6-5 — future core
/// variants surface as `Unknown { debug }` (a fail-loud sentinel), and
/// the reverse direction is `TryFrom` returning
/// `WireConversionError::TranscriptSource { debug }` for the `Unknown`
/// case rather than fabricating a typed `Spoken`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WireTranscriptSource {
    Spoken,
    /// R7-4 (P3 dogma): explicit fail-loud variant for unknown core
    /// variants. The core [`TranscriptSource`] enum is `#[non_exhaustive]`;
    /// when a future variant lands without an explicit arm in the wire
    /// `From` impl, the conversion surfaces as `Unknown { debug }` rather
    /// than silently coercing into `Spoken` — a "plausible lie" that would
    /// mark a non-spoken provenance as user-voice on the wire. SDK
    /// consumers route on the `kind: "unknown"` discriminator and treat
    /// it as unrecognized — never as `Spoken`.
    ///
    /// **When a new core variant is added, add an explicit arm in the
    /// forward `From` impl above this variant — `Unknown` is the floor,
    /// not the destination.**
    Unknown {
        debug: String,
    },
}

impl From<TranscriptSource> for WireTranscriptSource {
    fn from(value: TranscriptSource) -> Self {
        match value {
            TranscriptSource::Spoken => Self::Spoken,
            // Core enum is `#[non_exhaustive]`. R7-4 (P3 dogma): surface
            // unknown variants explicitly via `Unknown { debug }` rather
            // than silently coercing to `Spoken`. **When a new core
            // variant is added, add an explicit arm above this comment.**
            other => Self::Unknown {
                debug: format!("{other:?}"),
            },
        }
    }
}

impl TryFrom<WireTranscriptSource> for TranscriptSource {
    type Error = crate::wire::error::WireConversionError;

    fn try_from(value: WireTranscriptSource) -> Result<Self, Self::Error> {
        match value {
            WireTranscriptSource::Spoken => Ok(Self::Spoken),
            WireTranscriptSource::Unknown { debug } => {
                Err(crate::wire::error::WireConversionError::TranscriptSource { debug })
            }
        }
    }
}

impl From<AssistantBlock> for WireAssistantBlock {
    fn from(value: AssistantBlock) -> Self {
        match value {
            AssistantBlock::Text { text, meta } => Self::Text {
                text,
                meta: meta.map(|m| (*m).into()),
            },
            AssistantBlock::Transcript { text, source, meta } => Self::Transcript {
                text,
                source: source.into(),
                meta: meta.map(|m| (*m).into()),
            },
            AssistantBlock::Reasoning { text, meta } => Self::Reasoning {
                text,
                meta: meta.map(|m| (*m).into()),
            },
            AssistantBlock::ToolUse {
                id,
                name,
                args,
                meta,
            } => Self::ToolUse {
                id,
                name,
                // Pass-through: no re-parse, no silent `Value::Null`
                // downgrade on malformed bytes. Core invariant — tool-call
                // args are opaque from provider to dispatcher and this
                // wire type preserves that invariant.
                args,
                meta: meta.map(|m| (*m).into()),
            },
            AssistantBlock::ServerToolContent {
                id,
                kind,
                content,
                meta,
            } => Self::ServerToolContent {
                id,
                kind: kind.into(),
                content,
                meta: meta.map(|m| (*m).into()),
            },
            AssistantBlock::Image {
                image_id,
                blob_ref,
                media_type,
                width,
                height,
                revised_prompt,
                meta,
            } => Self::Image {
                image_id,
                blob_ref,
                media_type,
                width,
                height,
                revised_prompt,
                meta,
            },
            _ => Self::Unknown,
        }
    }
}

/// CC1 (FIX-A): inverse of `From<ProviderMeta> for WireProviderMeta`.
///
/// Returns `None` for `WireProviderMeta::Unknown` (the wire-side sink for
/// future provider variants we don't yet recognize). Caller should drop
/// the meta in that case rather than fabricate a typed shape. Returns
/// `Some(...)` for every typed variant — the conversion is lossless for
/// the four typed cases.
fn wire_provider_meta_to_core(value: WireProviderMeta) -> Option<ProviderMeta> {
    match value {
        WireProviderMeta::Anthropic { signature } => Some(ProviderMeta::Anthropic { signature }),
        WireProviderMeta::AnthropicRedacted { data } => {
            Some(ProviderMeta::AnthropicRedacted { data })
        }
        WireProviderMeta::AnthropicCompaction { content } => {
            Some(ProviderMeta::AnthropicCompaction { content })
        }
        WireProviderMeta::Gemini { thought_signature } => {
            Some(ProviderMeta::Gemini { thought_signature })
        }
        WireProviderMeta::OpenAi {
            id,
            encrypted_content,
            phase,
        } => Some(ProviderMeta::OpenAi {
            id,
            encrypted_content,
            phase,
        }),
        WireProviderMeta::Unknown => None,
    }
}

/// CC1 (FIX-A) / R7-5 (P3 dogma): inverse of
/// `From<AssistantBlock> for WireAssistantBlock`.
///
/// Mirrors the forward direction arm-for-arm. `WireAssistantBlock::Unknown`
/// is the wire-side sink for future core variants we don't yet recognize;
/// the inverse cannot fabricate a typed `AssistantBlock` from it.
///
/// R7-4 (P3 dogma) promoted this from `From` to `TryFrom` so the inner
/// `WireTranscriptSource::Unknown` can propagate via `?` instead of
/// silently fabricating a `Spoken` provenance.
///
/// R7-5 (P3 dogma) replaced the previous fabrication of an empty
/// `AssistantBlock::Text { "" }` for the `Unknown` arm with a typed
/// [`WireConversionError::AssistantBlock`] error. SDK consumers and
/// upstream callers now see a typed conversion failure rather than a
/// zero-length text block silently injected into the canonical
/// transcript.
///
/// Pre-1.0 dogma: when a new `AssistantBlock` variant lands, add an
/// explicit arm both here and in the forward direction.
impl TryFrom<WireAssistantBlock> for AssistantBlock {
    type Error = crate::wire::error::WireConversionError;

    fn try_from(value: WireAssistantBlock) -> Result<Self, Self::Error> {
        Ok(match value {
            WireAssistantBlock::Text { text, meta } => Self::Text {
                text,
                meta: meta.and_then(wire_provider_meta_to_core).map(Box::new),
            },
            WireAssistantBlock::Transcript { text, source, meta } => Self::Transcript {
                text,
                source: TranscriptSource::try_from(source)?,
                meta: meta.and_then(wire_provider_meta_to_core).map(Box::new),
            },
            WireAssistantBlock::Reasoning { text, meta } => Self::Reasoning {
                text,
                meta: meta.and_then(wire_provider_meta_to_core).map(Box::new),
            },
            WireAssistantBlock::ToolUse {
                id,
                name,
                args,
                meta,
            } => Self::ToolUse {
                id,
                name,
                // Pass-through: opaque from provider to dispatcher; mirrors
                // the forward direction's preservation of `Box<RawValue>`.
                args,
                meta: meta.and_then(wire_provider_meta_to_core).map(Box::new),
            },
            WireAssistantBlock::ServerToolContent {
                id,
                kind,
                content,
                meta,
            } => Self::ServerToolContent {
                id,
                kind: ServerToolKind::try_from(kind)?,
                content,
                meta: meta.and_then(wire_provider_meta_to_core).map(Box::new),
            },
            WireAssistantBlock::Image {
                image_id,
                blob_ref,
                media_type,
                width,
                height,
                revised_prompt,
                meta,
            } => Self::Image {
                image_id,
                blob_ref,
                media_type,
                width,
                height,
                revised_prompt,
                meta,
            },
            WireAssistantBlock::Unknown => {
                return Err(crate::wire::error::WireConversionError::AssistantBlock {
                    debug: "WireAssistantBlock::Unknown".to_string(),
                });
            }
        })
    }
}

/// Canonical stop reason for transcript messages.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireStopReason {
    #[default]
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    ContentFilter,
    Cancelled,
}

impl From<StopReason> for WireStopReason {
    fn from(value: StopReason) -> Self {
        match value {
            StopReason::EndTurn => Self::EndTurn,
            StopReason::ToolUse => Self::ToolUse,
            StopReason::MaxTokens => Self::MaxTokens,
            StopReason::StopSequence => Self::StopSequence,
            StopReason::ContentFilter => Self::ContentFilter,
            StopReason::Cancelled => Self::Cancelled,
        }
    }
}

impl From<WireStopReason> for StopReason {
    fn from(value: WireStopReason) -> Self {
        match value {
            WireStopReason::EndTurn => Self::EndTurn,
            WireStopReason::ToolUse => Self::ToolUse,
            WireStopReason::MaxTokens => Self::MaxTokens,
            WireStopReason::StopSequence => Self::StopSequence,
            WireStopReason::ContentFilter => Self::ContentFilter,
            WireStopReason::Cancelled => Self::Cancelled,
        }
    }
}

/// Tool result payload in a transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireToolResult {
    pub tool_use_id: String,
    pub content: WireToolResultContent,
    #[serde(default)]
    pub is_error: bool,
}

fn transcript_message_timestamp(
    created_at: Option<String>,
) -> Result<meerkat_core::types::MessageTimestamp, crate::wire::error::WireConversionError> {
    match created_at {
        Some(value) => chrono::DateTime::parse_from_rfc3339(&value)
            .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
            .map_err(
                |err| crate::wire::error::WireConversionError::TranscriptMessage {
                    debug: format!("invalid created_at {value:?}: {err}"),
                },
            ),
        None => Ok(meerkat_core::types::message_timestamp_now()),
    }
}

impl TranscriptRewriteMessage {
    pub fn into_core(self) -> Result<Message, crate::wire::error::WireConversionError> {
        match self {
            Self::System {
                content,
                created_at,
            } => Ok(Message::System(SystemMessage {
                content,
                mutation_kind: meerkat_core::types::SystemPromptMutationKind::default(),
                created_at: transcript_message_timestamp(created_at)?,
            })),
            Self::SystemNotice {
                kind,
                body,
                blocks,
                created_at,
            } => Ok(Message::SystemNotice(SystemNoticeMessage {
                kind,
                body,
                blocks,
                created_at: transcript_message_timestamp(created_at)?,
            })),
            Self::User {
                content,
                created_at,
            } => {
                let content = ContentInput::try_from(content)
                    .map_err(
                        |err| crate::wire::error::WireConversionError::TranscriptMessage {
                            debug: err.to_string(),
                        },
                    )?
                    .into_blocks();
                Ok(Message::User(UserMessage {
                    content,
                    render_metadata: None,
                    transcript_role: meerkat_core::types::TranscriptUserRole::default(),
                    created_at: transcript_message_timestamp(created_at)?,
                }))
            }
            Self::BlockAssistant {
                blocks,
                stop_reason,
                created_at,
            } => {
                let blocks = blocks
                    .into_iter()
                    .map(AssistantBlock::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Message::BlockAssistant(BlockAssistantMessage {
                    blocks,
                    stop_reason: stop_reason.into(),
                    created_at: transcript_message_timestamp(created_at)?,
                }))
            }
            Self::ToolResults {
                results,
                created_at,
            } => {
                let results = results
                    .into_iter()
                    .map(|result| {
                        let content = match result.content {
                            WireToolResultContent::Text(text) => ContentBlock::text_vec(text),
                            WireToolResultContent::Blocks(blocks) => blocks
                                .into_iter()
                                .map(ContentBlock::try_from)
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|err| {
                                    crate::wire::error::WireConversionError::TranscriptMessage {
                                        debug: err.to_string(),
                                    }
                                })?,
                        };
                        Ok(ToolResult {
                            tool_use_id: result.tool_use_id,
                            content,
                            is_error: result.is_error,
                        })
                    })
                    .collect::<Result<Vec<_>, crate::wire::error::WireConversionError>>()?;
                Ok(Message::ToolResults {
                    results,
                    created_at: transcript_message_timestamp(created_at)?,
                })
            }
        }
    }
}

/// Typed wire mirror of [`meerkat_core::TranscriptReplacement`].
///
/// R-#87 (P3 dogma): `ForkSessionReplaceParams.replacement` previously carried
/// the core `TranscriptReplacement` directly with a
/// `#[schemars(with = "serde_json::Value")]` override, so the emitted JSON
/// schema collapsed to a bare `Value` (artifacts/schemas/params.json) and the
/// SDK codegen saw an untyped bag. The core enum cannot derive `JsonSchema`
/// directly: it embeds `Message` and `AssistantBlock`, neither of which has a
/// `JsonSchema` impl in `meerkat-core` (and `Message::BlockAssistant` carries
/// `Box<RawValue>` tool-call args).
///
/// This wire mirror is schema-emittable because it is built entirely from
/// types that already carry `JsonSchema` derives in this module:
/// [`TranscriptRewriteMessage`] (the public message shape, with its own
/// `into_core`), [`WireContentBlock`], and [`WireAssistantBlock`]. Conversion
/// into the core enum is fallible — every inner conversion already returns a
/// typed [`WireConversionError`](crate::wire::error::WireConversionError), so
/// malformed payloads surface a typed parse fault at the boundary rather than
/// being smuggled through as an opaque `Value`.
///
/// The serde tag/rename mirror the core enum (`#[serde(tag = "type",
/// rename_all = "snake_case")]`) so the wire bytes are unchanged: the only
/// observable delta is that the schema is now the closed enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireTranscriptReplacement {
    /// Replace the addressed message with a full canonical message.
    Message { message: TranscriptRewriteMessage },
    /// Replace one user-message content block.
    UserContentBlock {
        block_index: usize,
        block: WireContentBlock,
    },
    /// Replace one block in a block-assistant message.
    AssistantBlock {
        block_index: usize,
        block: WireAssistantBlock,
    },
    /// Replace one content block inside one tool-result payload.
    ToolResultContentBlock {
        result_index: usize,
        block_index: usize,
        block: WireContentBlock,
    },
}

impl WireTranscriptReplacement {
    /// Lower the typed wire replacement into the core
    /// [`TranscriptReplacement`].
    ///
    /// Each inner conversion is fallible and surfaces a typed
    /// [`WireConversionError`](crate::wire::error::WireConversionError):
    /// `Message` delegates to [`TranscriptRewriteMessage::into_core`];
    /// `ContentBlock`/`AssistantBlock` conversions wrap their `&'static str` /
    /// typed failures into [`WireConversionError::TranscriptMessage`] so the
    /// surface sees one closed error type.
    pub fn into_core(
        self,
    ) -> Result<TranscriptReplacement, crate::wire::error::WireConversionError> {
        match self {
            Self::Message { message } => Ok(TranscriptReplacement::Message {
                message: message.into_core()?,
            }),
            Self::UserContentBlock { block_index, block } => {
                Ok(TranscriptReplacement::UserContentBlock {
                    block_index,
                    block: ContentBlock::try_from(block).map_err(|err| {
                        crate::wire::error::WireConversionError::TranscriptMessage {
                            debug: format!("invalid user content block: {err}"),
                        }
                    })?,
                })
            }
            Self::AssistantBlock { block_index, block } => {
                Ok(TranscriptReplacement::AssistantBlock {
                    block_index,
                    block: AssistantBlock::try_from(block)?,
                })
            }
            Self::ToolResultContentBlock {
                result_index,
                block_index,
                block,
            } => Ok(TranscriptReplacement::ToolResultContentBlock {
                result_index,
                block_index,
                block: ContentBlock::try_from(block).map_err(|err| {
                    crate::wire::error::WireConversionError::TranscriptMessage {
                        debug: format!("invalid tool-result content block: {err}"),
                    }
                })?,
            }),
        }
    }
}

/// Canonical transcript message for public wire surfaces.
///
/// Not `PartialEq`: the `BlockAssistant.blocks` variant carries
/// `WireAssistantBlock`s which hold opaque tool-call args as
/// `Box<RawValue>`. See `WireAssistantBlock` doc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum WireSessionMessage {
    System {
        content: String,
        created_at: String,
    },
    SystemNotice {
        kind: SystemNoticeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blocks: Vec<meerkat_core::SystemNoticeBlock>,
        created_at: String,
    },
    User {
        content: WireContentInput,
        created_at: String,
    },
    #[serde(rename = "block_assistant")]
    BlockAssistant {
        blocks: Vec<WireAssistantBlock>,
        stop_reason: WireStopReason,
        created_at: String,
    },
    #[serde(rename = "tool_results")]
    ToolResults {
        results: Vec<WireToolResult>,
        created_at: String,
    },
}

impl From<Message> for WireSessionMessage {
    fn from(value: Message) -> Self {
        match value {
            Message::System(message) => Self::System {
                content: message.content,
                created_at: message.created_at.to_rfc3339(),
            },
            Message::SystemNotice(message) => Self::SystemNotice {
                kind: message.kind,
                body: message.body,
                blocks: message.blocks,
                created_at: message.created_at.to_rfc3339(),
            },
            Message::User(message) => {
                let created_at = message.created_at.to_rfc3339();
                let content = if message.content.len() == 1
                    && matches!(&message.content[0], ContentBlock::Text { .. })
                {
                    WireContentInput::Text(message.text_content())
                } else {
                    WireContentInput::Blocks(message.content.into_iter().map(Into::into).collect())
                };
                Self::User {
                    content,
                    created_at,
                }
            }
            Message::BlockAssistant(message) => Self::BlockAssistant {
                blocks: message.blocks.into_iter().map(Into::into).collect(),
                stop_reason: message.stop_reason.into(),
                created_at: message.created_at.to_rfc3339(),
            },
            Message::ToolResults {
                results,
                created_at,
            } => Self::ToolResults {
                results: results
                    .into_iter()
                    .map(|result| {
                        let content = if result.content.len() == 1
                            && matches!(&result.content[0], ContentBlock::Text { .. })
                        {
                            WireToolResultContent::Text(result.text_content())
                        } else {
                            WireToolResultContent::Blocks(
                                result.content.into_iter().map(Into::into).collect(),
                            )
                        };
                        WireToolResult {
                            tool_use_id: result.tool_use_id,
                            content,
                            is_error: result.is_error,
                        }
                    })
                    .collect(),
                created_at: created_at.to_rfc3339(),
            },
        }
    }
}

/// Full session history in canonical wire format.
///
/// Not `PartialEq`: `messages` carries `WireSessionMessage` which
/// transitively holds opaque tool-call args; compare via serialization
/// round-trip rather than field equality.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionHistory {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub message_count: usize,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    pub has_more: bool,
    pub messages: Vec<WireSessionMessage>,
}

impl From<SessionHistoryPage> for WireSessionHistory {
    fn from(page: SessionHistoryPage) -> Self {
        Self {
            session_id: page.session_id,
            session_ref: None,
            message_count: page.message_count,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            messages: page.messages.into_iter().map(Into::into).collect(),
        }
    }
}

/// Full transcript revision body in canonical wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireSessionTranscriptRevision {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
    pub revision: String,
    pub head_revision: String,
    pub message_count: usize,
    pub offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    pub has_more: bool,
    pub messages: Vec<WireSessionMessage>,
}

impl From<SessionTranscriptRevisionPage> for WireSessionTranscriptRevision {
    fn from(page: SessionTranscriptRevisionPage) -> Self {
        Self {
            session_id: page.session_id,
            session_ref: None,
            revision: page.revision,
            head_revision: page.head_revision,
            message_count: page.message_count,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            messages: page.messages.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::time_compat::SystemTime;
    use meerkat_core::{BlockAssistantMessage, Message, SystemMessage, UserMessage};

    #[test]
    fn test_fork_session_params_roundtrip_typed_replacement() {
        let fork_at: ForkSessionAtParams = serde_json::from_value(serde_json::json!({
            "session_id": "session_123",
            "message_index": 2,
            "running_behavior": "reject"
        }))
        .unwrap();
        assert_eq!(fork_at.session_id, "session_123");
        assert_eq!(fork_at.message_index, 2);
        assert_eq!(
            fork_at.running_behavior,
            TranscriptEditRunningBehavior::Reject
        );

        let fork_replace: ForkSessionReplaceParams = serde_json::from_value(serde_json::json!({
            "session_id": "session_123",
            "message_index": 2,
            "replacement": {
                "type": "message",
                "message": {
                    "role": "user",
                    "content": "edited follow up"
                }
            }
        }))
        .unwrap();
        // The wire field is now the typed `WireTranscriptReplacement` mirror
        // (so the schema is the closed enum, not a bare Value).
        assert!(matches!(
            &fork_replace.replacement,
            WireTranscriptReplacement::Message {
                message: TranscriptRewriteMessage::User { content, .. },
            } if matches!(content, WireContentInput::Text(text) if text == "edited follow up")
        ));

        // Lowering into the core enum is typed and fallible; the user
        // message round-trips into a core `Message::User`.
        let core = fork_replace
            .replacement
            .clone()
            .into_core()
            .expect("typed wire replacement lowers into core");
        assert!(matches!(
            core,
            TranscriptReplacement::Message {
                message: Message::User(user),
            } if user.text_content() == "edited follow up"
        ));

        let json = serde_json::to_value(&fork_replace).unwrap();
        assert_eq!(json["replacement"]["type"], "message");
        assert_eq!(json["running_behavior"], "reject");
    }

    #[test]
    fn test_rewrite_session_transcript_params_roundtrip() {
        let params: RewriteSessionTranscriptParams = serde_json::from_value(serde_json::json!({
            "session_id": "session_123",
            "selection": {
                "type": "message_range",
                "start": 1,
                "end": 4
            },
            "replacement": [
                {
                    "role": "block_assistant",
                    "blocks": [
                        {
                            "block_type": "text",
                            "data": {
                                "text": "compacted trace"
                            }
                        }
                    ],
                    "stop_reason": "end_turn"
                }
            ],
            "reason": {
                "kind": "compaction",
                "note": "replace N-3 trace"
            },
            "actor": "test",
            "expected_parent_revision": "sha256:parent",
            "running_behavior": "reject"
        }))
        .unwrap();
        assert_eq!(params.session_id, "session_123");
        assert!(matches!(
            params.selection,
            TranscriptRewriteSelection::MessageRange { start: 1, end: 4 }
        ));
        assert_eq!(params.replacement.len(), 1);
        assert_eq!(params.reason.kind, "compaction");

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["selection"]["type"], "message_range");
        assert_eq!(json["replacement"][0]["role"], "block_assistant");
        assert_eq!(json["running_behavior"], "reject");
    }

    #[test]
    fn test_rewrite_session_transcript_rejects_legacy_assistant_role() {
        // The text-shaped `role: "assistant"` rewrite message was folded into
        // `block_assistant`; the legacy wire form must fail closed at parse.
        let err = serde_json::from_value::<RewriteSessionTranscriptParams>(serde_json::json!({
            "session_id": "session_123",
            "selection": {
                "type": "message_range",
                "start": 1,
                "end": 2
            },
            "replacement": [
                {
                    "role": "assistant",
                    "content": "Compacted assistant trace"
                }
            ],
            "reason": {
                "kind": "compaction"
            }
        }))
        .expect_err("legacy assistant rewrite role must be rejected");
        assert!(
            err.to_string().contains("assistant"),
            "rejection should name the unknown role: {err}"
        );
    }

    #[test]
    fn test_rewrite_session_transcript_accepts_public_system_notice() {
        let params: RewriteSessionTranscriptParams = serde_json::from_value(serde_json::json!({
            "session_id": "session_123",
            "selection": {
                "type": "message_range",
                "start": 0,
                "end": 1
            },
            "replacement": [
                {
                    "role": "system_notice",
                    "kind": "background_job",
                    "body": "still running"
                }
            ],
            "reason": {
                "kind": "correction"
            }
        }))
        .unwrap();

        let message = params
            .replacement
            .into_iter()
            .next()
            .expect("replacement exists")
            .into_core()
            .expect("public system notice converts");
        assert!(matches!(
            message,
            Message::SystemNotice(meerkat_core::SystemNoticeMessage {
                kind: meerkat_core::SystemNoticeKind::BackgroundJob,
                body: Some(body),
                ..
            }) if body == "still running"
        ));
    }

    #[test]
    fn test_read_session_transcript_revision_params_roundtrip() {
        let params: ReadSessionTranscriptRevisionParams =
            serde_json::from_value(serde_json::json!({
                "session_id": "session_123",
                "revision": "sha256:parent",
                "offset": 1,
                "limit": 2
            }))
            .unwrap();
        assert_eq!(params.session_id, "session_123");
        assert_eq!(params.revision, "sha256:parent");
        assert_eq!(params.offset, Some(1));
        assert_eq!(params.limit, Some(2));

        let page = SessionTranscriptRevisionPage::from_messages(
            SessionId::new(),
            "sha256:parent".to_string(),
            "sha256:head".to_string(),
            &[Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "recover me".to_string(),
                },
            ]))],
            0,
            None,
        );
        let wire: WireSessionTranscriptRevision = page.into();
        let encoded = serde_json::to_value(wire).unwrap();
        assert!(encoded["session_id"].as_str().is_some());
        assert_eq!(encoded["revision"], "sha256:parent");
        assert_eq!(encoded["head_revision"], "sha256:head");
        assert_eq!(encoded["messages"][0]["role"], "user");
    }

    #[test]
    fn test_restore_session_transcript_revision_params_roundtrip() {
        let params: RestoreSessionTranscriptRevisionParams =
            serde_json::from_value(serde_json::json!({
                "session_id": "session_123",
                "revision": "sha256:parent",
                "reason": {
                    "kind": "restore",
                    "note": "recover prior truth"
                },
                "actor": "test",
                "expected_parent_revision": "sha256:current",
                "running_behavior": "reject"
            }))
            .unwrap();
        assert_eq!(params.session_id, "session_123");
        assert_eq!(params.revision, "sha256:parent");
        assert_eq!(params.reason.kind, "restore");
        assert_eq!(params.actor.as_deref(), Some("test"));
        assert_eq!(
            params.expected_parent_revision.as_deref(),
            Some("sha256:current")
        );
        assert_eq!(
            params.running_behavior,
            TranscriptEditRunningBehavior::Reject
        );
    }

    #[test]
    fn test_wire_session_summary_labels_roundtrip() {
        let mut labels = BTreeMap::new();
        labels.insert("env".to_string(), "prod".to_string());
        labels.insert("team".to_string(), "infra".to_string());

        let wire = WireSessionSummary {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 5,
            total_tokens: 100,
            is_active: true,
            labels: labels.clone(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireSessionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.labels, labels);
    }

    #[test]
    fn test_wire_session_summary_empty_labels_omitted() {
        let wire = WireSessionSummary {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 0,
            total_tokens: 0,
            is_active: false,
            labels: BTreeMap::new(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        assert!(
            !json.contains("\"labels\""),
            "empty labels should be omitted from JSON"
        );
    }

    #[test]
    fn test_wire_session_info_labels_roundtrip() {
        let mut labels = BTreeMap::new();
        labels.insert("role".to_string(), "orchestrator".to_string());

        let wire = WireSessionInfo {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 3,
            is_active: true,
            model: "claude-sonnet-4-5".to_string(),
            provider: "anthropic".to_string(),
            last_assistant_text: None,
            resolved_capabilities: None,
            labels: labels.clone(),
        };
        let json = serde_json::to_string(&wire).unwrap();
        let parsed: WireSessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.labels, labels);
    }

    #[test]
    fn test_wire_session_info_resolved_capabilities_roundtrip() {
        let capabilities = crate::wire::WireResolvedModelCapabilities {
            vision: true,
            image_input: true,
            image_tool_results: true,
            inline_video: false,
            realtime: true,
            web_search: true,
            image_generation: true,
        };
        let wire = WireSessionInfo {
            session_id: SessionId::new(),
            session_ref: None,
            created_at: 1000,
            updated_at: 2000,
            message_count: 3,
            is_active: true,
            model: "gpt-realtime-2".to_string(),
            provider: "openai".to_string(),
            last_assistant_text: None,
            labels: BTreeMap::new(),
            resolved_capabilities: Some(capabilities.clone()),
        };

        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains("\"resolved_capabilities\""));
        let parsed: WireSessionInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.resolved_capabilities, Some(capabilities));
    }

    #[test]
    fn test_wire_session_info_from_session_info_maps_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("env".to_string(), "staging".to_string());

        let info = SessionInfo {
            session_id: SessionId::new(),
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: 2,
            is_active: false,
            model: "claude-sonnet-4-5".to_string(),
            provider: meerkat_core::Provider::Anthropic,
            last_assistant_text: Some("hello".to_string()),
            labels: labels.clone(),
        };
        let wire: WireSessionInfo = info.into();
        assert_eq!(wire.labels, labels);
    }

    #[test]
    fn test_wire_session_summary_from_session_summary_maps_labels() {
        let mut labels = BTreeMap::new();
        labels.insert("project".to_string(), "meerkat".to_string());

        let summary = SessionSummary {
            session_id: SessionId::new(),
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
            message_count: 10,
            total_tokens: 500,
            is_active: true,
            labels: labels.clone(),
        };
        let wire: WireSessionSummary = summary.into();
        assert_eq!(wire.labels, labels);
    }

    #[test]
    fn test_wire_session_info_backward_compat_no_labels() {
        let json = r#"{
            "session_id": "019405c8-1234-7000-8000-000000000001",
            "created_at": 1000,
            "updated_at": 2000,
            "message_count": 0,
            "is_active": false,
            "model": "claude-sonnet-4-5",
            "provider": "anthropic"
        }"#;
        let parsed: WireSessionInfo = serde_json::from_str(json).unwrap();
        assert!(parsed.labels.is_empty());
    }

    #[test]
    fn test_wire_session_summary_backward_compat_no_labels() {
        let json = r#"{
            "session_id": "019405c8-1234-7000-8000-000000000001",
            "created_at": 1000,
            "updated_at": 2000,
            "message_count": 0,
            "total_tokens": 0,
            "is_active": false
        }"#;
        let parsed: WireSessionSummary = serde_json::from_str(json).unwrap();
        assert!(parsed.labels.is_empty());
    }

    #[test]
    fn test_wire_session_history_roundtrip_mixed_messages() {
        let history = WireSessionHistory {
            session_id: SessionId::new(),
            session_ref: Some("session://example".to_string()),
            message_count: 4,
            offset: 0,
            limit: Some(4),
            has_more: false,
            messages: vec![
                WireSessionMessage::System {
                    content: "You are helpful".to_string(),
                    created_at: "2026-04-27T00:00:00Z".to_string(),
                },
                WireSessionMessage::User {
                    content: WireContentInput::Text("hello".to_string()),
                    created_at: "2026-04-27T00:00:01Z".to_string(),
                },
                WireSessionMessage::BlockAssistant {
                    blocks: vec![
                        WireAssistantBlock::Reasoning {
                            text: "thinking".to_string(),
                            meta: None,
                        },
                        WireAssistantBlock::ToolUse {
                            id: "tool-1".to_string(),
                            name: "search".to_string(),
                            args: serde_json::value::RawValue::from_string(
                                r#"{"q":"rust"}"#.to_string(),
                            )
                            .expect("fixture args literal is valid JSON"),
                            meta: None,
                        },
                        WireAssistantBlock::Text {
                            text: "done".to_string(),
                            meta: None,
                        },
                        // T3-extension (FIX-A): pin spoken-transcript
                        // round-trip alongside Text + Reasoning.
                        WireAssistantBlock::Transcript {
                            text: "spoken hi".to_string(),
                            source: WireTranscriptSource::Spoken,
                            meta: None,
                        },
                    ],
                    stop_reason: WireStopReason::EndTurn,
                    created_at: "2026-04-27T00:00:03Z".to_string(),
                },
                WireSessionMessage::ToolResults {
                    results: vec![WireToolResult {
                        tool_use_id: "tool-1".to_string(),
                        content: WireToolResultContent::Text("ok".to_string()),
                        is_error: false,
                    }],
                    created_at: "2026-04-27T00:00:04Z".to_string(),
                },
            ],
        };
        let json = serde_json::to_string(&history).unwrap();
        let parsed: WireSessionHistory = serde_json::from_str(&json).unwrap();
        // `WireSessionHistory` is not `PartialEq` post-C-2 because
        // tool-call args ride as `Box<RawValue>`. Round-trip via JSON
        // string equivalence is the canonical wire comparison.
        let reserialized = serde_json::to_string(&parsed).unwrap();
        assert_eq!(reserialized, json);
    }

    #[test]
    fn test_wire_session_history_from_page_maps_messages() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 3,
            offset: 0,
            limit: None,
            has_more: false,
            messages: vec![
                Message::System(SystemMessage::new("sys")),
                Message::SystemNotice(meerkat_core::SystemNoticeMessage::new(
                    meerkat_core::SystemNoticeKind::BackgroundJob,
                    "still running",
                )),
                Message::BlockAssistant(BlockAssistantMessage::new(
                    vec![
                        AssistantBlock::Text {
                            text: "hello".to_string(),
                            meta: None,
                        },
                        AssistantBlock::ToolUse {
                            id: "call-1".to_string(),
                            name: "search".to_string(),
                            args: serde_json::value::RawValue::from_string(
                                r#"{"q":"meerkat"}"#.to_string(),
                            )
                            .expect("fixture args literal is valid JSON"),
                            meta: None,
                        },
                    ],
                    StopReason::ToolUse,
                )),
            ],
        };
        let wire: WireSessionHistory = page.into();
        assert_eq!(wire.messages.len(), 3);
        assert!(matches!(
            wire.messages[0],
            WireSessionMessage::System { .. }
        ));
        assert!(matches!(
            wire.messages[1],
            WireSessionMessage::SystemNotice { .. }
        ));
        assert!(matches!(
            wire.messages[2],
            WireSessionMessage::BlockAssistant { .. }
        ));
    }

    #[test]
    fn test_wire_session_history_from_page_maps_block_assistant_and_tool_results() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 2,
            offset: 0,
            limit: Some(2),
            has_more: false,
            messages: vec![
                Message::BlockAssistant(BlockAssistantMessage::new(
                    vec![AssistantBlock::Text {
                        text: "hi".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                )),
                Message::tool_results(vec![meerkat_core::ToolResult::new(
                    "tool-2".to_string(),
                    "done".to_string(),
                    false,
                )]),
            ],
        };
        let wire: WireSessionHistory = page.into();
        assert!(matches!(
            wire.messages[0],
            WireSessionMessage::BlockAssistant { .. }
        ));
        assert!(matches!(
            wire.messages[1],
            WireSessionMessage::ToolResults { .. }
        ));
    }

    #[test]
    fn test_wire_content_block_text_roundtrip() {
        let block = WireContentBlock::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: WireContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, block);
    }

    #[test]
    fn test_wire_content_block_image_roundtrip() {
        let block = WireContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "iVBOR...".into(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: WireContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, block);
    }

    #[test]
    fn test_wire_content_block_video_roundtrip() {
        let block = WireContentBlock::Video {
            media_type: "video/mp4".to_string(),
            duration_ms: 12_000,
            data: "AAAA".into(),
        };
        let json = serde_json::to_string(&block).unwrap();
        let parsed: WireContentBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, block);
    }

    #[test]
    fn test_wire_content_block_unknown_forward_compat() {
        let json = r#"{"type":"hologram","url":"https://example.com/v.mp4"}"#;
        let parsed: WireContentBlock = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, WireContentBlock::Unknown);
    }

    #[test]
    fn test_wire_content_block_from_core_strips_source_path() {
        let core_block = ContentBlock::Image {
            media_type: "image/jpeg".to_string(),
            data: "base64data".into(),
        };
        let wire: WireContentBlock = core_block.into();
        assert_eq!(
            wire,
            WireContentBlock::Image {
                media_type: "image/jpeg".to_string(),
                data: "base64data".into(),
            }
        );
    }

    #[test]
    fn test_wire_content_block_from_core_video_roundtrip() {
        let core_block = ContentBlock::Video {
            media_type: "video/mp4".to_string(),
            duration_ms: 12_000,
            data: VideoData::Inline {
                data: "base64video".to_string(),
            },
        };
        let wire: WireContentBlock = core_block.clone().into();
        assert_eq!(
            wire,
            WireContentBlock::Video {
                media_type: "video/mp4".to_string(),
                duration_ms: 12_000,
                data: "base64video".into(),
            }
        );
        let restored = ContentBlock::try_from(wire).unwrap();
        assert_eq!(restored, core_block);
    }

    #[test]
    fn test_wire_content_input_text_roundtrip() {
        let input = WireContentInput::Text("hello world".to_string());
        let json = serde_json::to_string(&input).unwrap();
        assert_eq!(json, r#""hello world""#);
        let parsed: WireContentInput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn test_wire_content_input_blocks_roundtrip() {
        let input = WireContentInput::Blocks(vec![
            WireContentBlock::Text {
                text: "look at this".to_string(),
            },
            WireContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "abc123".into(),
            },
        ]);
        let json = serde_json::to_string(&input).unwrap();
        let parsed: WireContentInput = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn test_wire_tool_result_content_text_roundtrip() {
        let content = WireToolResultContent::Text("result text".to_string());
        let json = serde_json::to_string(&content).unwrap();
        assert_eq!(json, r#""result text""#);
        let parsed: WireToolResultContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn test_wire_tool_result_content_blocks_roundtrip() {
        let content = WireToolResultContent::Blocks(vec![
            WireContentBlock::Text {
                text: "output".to_string(),
            },
            WireContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "data".into(),
            },
        ]);
        let json = serde_json::to_string(&content).unwrap();
        let parsed: WireToolResultContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, content);
    }

    #[test]
    fn test_wire_tool_result_backward_compat_string() {
        let json = r#"{"tool_use_id":"t1","content":"hello","is_error":false}"#;
        let parsed: WireToolResult = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.content,
            WireToolResultContent::Text("hello".to_string())
        );
    }

    #[test]
    fn test_wire_user_message_text_backward_compat() {
        let json = r#"{"role":"user","content":"hello","created_at":"2026-04-27T00:00:00Z"}"#;
        let parsed: WireSessionMessage = serde_json::from_str(json).unwrap();
        match parsed {
            WireSessionMessage::User { content, .. } => {
                assert_eq!(content, WireContentInput::Text("hello".to_string()));
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_wire_user_message_blocks() {
        let json = r#"{"role":"user","content":[{"type":"text","text":"look"},{"type":"image","media_type":"image/png","source":"inline","data":"abc"}],"created_at":"2026-04-27T00:00:00Z"}"#;
        let parsed: WireSessionMessage = serde_json::from_str(json).unwrap();
        match parsed {
            WireSessionMessage::User { content, .. } => {
                assert_eq!(
                    content,
                    WireContentInput::Blocks(vec![
                        WireContentBlock::Text {
                            text: "look".to_string()
                        },
                        WireContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "abc".into()
                        },
                    ])
                );
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_wire_user_message_from_multimodal_core() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 1,
            offset: 0,
            limit: None,
            has_more: false,
            messages: vec![Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "describe this".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "base64data".into(),
                },
            ]))],
        };
        let wire: WireSessionHistory = page.into();
        match &wire.messages[0] {
            WireSessionMessage::User { content, .. } => {
                assert_eq!(
                    *content,
                    WireContentInput::Blocks(vec![
                        WireContentBlock::Text {
                            text: "describe this".to_string()
                        },
                        WireContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "base64data".into()
                        },
                    ])
                );
            }
            _ => panic!("expected User message"),
        }
    }

    #[test]
    fn test_wire_tool_result_from_multimodal_core() {
        let page = SessionHistoryPage {
            session_id: SessionId::new(),
            message_count: 1,
            offset: 0,
            limit: None,
            has_more: false,
            messages: vec![Message::tool_results(vec![
                meerkat_core::ToolResult::with_blocks(
                    "tool-1".to_string(),
                    vec![
                        ContentBlock::Text {
                            text: "screenshot:".to_string(),
                        },
                        ContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "imgdata".into(),
                        },
                    ],
                    false,
                ),
            ])],
        };
        let wire: WireSessionHistory = page.into();
        match &wire.messages[0] {
            WireSessionMessage::ToolResults { results, .. } => {
                assert_eq!(
                    results[0].content,
                    WireToolResultContent::Blocks(vec![
                        WireContentBlock::Text {
                            text: "screenshot:".to_string()
                        },
                        WireContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "imgdata".into()
                        },
                    ])
                );
            }
            _ => panic!("expected ToolResults message"),
        }
    }

    /// CC1 (FIX-A): assert `From<AssistantBlock> for WireAssistantBlock`
    /// is symmetric with `From<WireAssistantBlock> for AssistantBlock`
    /// for every typed variant. Round-trip core → wire → core must
    /// preserve `Text`, `Reasoning`, `Transcript` (with `meta`),
    /// `ToolUse` (opaque args), `ServerToolContent`, and `Image`.
    #[test]
    fn test_assistant_block_core_wire_core_round_trip_symmetric() {
        use meerkat_core::{
            AssistantImageId, BlobId, BlobRef, MediaType, ProviderImageMetadata,
            RevisedPromptDisposition,
        };
        use uuid::Uuid;

        let cases: Vec<AssistantBlock> = vec![
            AssistantBlock::Text {
                text: "display lane".to_string(),
                meta: Some(Box::new(ProviderMeta::Anthropic {
                    signature: "sig-1".to_string(),
                })),
            },
            AssistantBlock::Reasoning {
                text: "thinking".to_string(),
                meta: Some(Box::new(ProviderMeta::OpenAi {
                    id: "rs_1".to_string(),
                    encrypted_content: Some("enc".to_string()),
                    phase: Some("draft".to_string()),
                })),
            },
            AssistantBlock::Transcript {
                text: "spoken".to_string(),
                source: TranscriptSource::Spoken,
                meta: Some(Box::new(ProviderMeta::Gemini {
                    thought_signature: "ts-1".to_string(),
                })),
            },
            AssistantBlock::ToolUse {
                id: "call-1".to_string(),
                name: "search".to_string(),
                args: serde_json::value::RawValue::from_string(r#"{"q":"rust"}"#.to_string())
                    .expect("fixture args literal is valid JSON"),
                meta: None,
            },
            AssistantBlock::ServerToolContent {
                id: Some("st-1".to_string()),
                kind: ServerToolKind::WebSearch,
                content: serde_json::json!({"hits": 3}),
                meta: None,
            },
            AssistantBlock::Image {
                image_id: AssistantImageId::new(Uuid::nil()),
                blob_ref: BlobRef {
                    blob_id: BlobId::new("blob-fixture"),
                    media_type: "image/png".to_string(),
                },
                media_type: MediaType::new("image/png"),
                width: 64,
                height: 64,
                revised_prompt: RevisedPromptDisposition::NotRequested,
                meta: ProviderImageMetadata::NotEmitted,
            },
        ];

        for case in cases {
            let snapshot = format!("{case:?}");
            let wire: WireAssistantBlock = case.clone().into();
            let back: AssistantBlock =
                AssistantBlock::try_from(wire).expect("typed wire variants round-trip");
            // `AssistantBlock` is `PartialEq` (custom impl handles
            // `ToolUse.args` via byte-equivalence on the underlying JSON).
            assert_eq!(case, back, "round trip lost data for {snapshot}");
        }
    }

    /// R7-4 (P3 dogma): the wire enum must carry an explicit `Unknown`
    /// variant so future core variants surface as fail-loud sentinels
    /// rather than silently coercing into `Spoken`. This test pins the
    /// shape: `Unknown { debug }` exists, round-trips through JSON with
    /// the `kind: "unknown"` discriminator, and is *not* equal to
    /// `Spoken` after a round-trip.
    #[test]
    fn unknown_transcript_source_does_not_become_spoken() {
        let wire = WireTranscriptSource::Unknown {
            debug: "future_lane".to_string(),
        };
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["kind"], "unknown");
        let parsed: WireTranscriptSource = serde_json::from_value(json).unwrap();
        assert!(matches!(parsed, WireTranscriptSource::Unknown { ref debug }
            if debug == "future_lane"));
        assert!(!matches!(parsed, WireTranscriptSource::Spoken));
    }

    /// R7-4 (P3 dogma): the reverse direction `Wire -> core` returns a
    /// typed error for `Unknown`, never silently fabricating a typed
    /// `Spoken` value.
    #[test]
    fn wire_to_core_transcript_source_unknown_returns_typed_error() {
        let wire = WireTranscriptSource::Unknown {
            debug: "FutureSpokenSource".to_string(),
        };
        let err = TranscriptSource::try_from(wire).unwrap_err();
        assert!(matches!(
            err,
            crate::wire::error::WireConversionError::TranscriptSource { ref debug }
                if debug == "FutureSpokenSource"
        ));
    }

    /// R7-4 (P3 dogma): the typed `Unknown` variant must not poison
    /// known-variant round-trips.
    #[test]
    fn known_transcript_sources_round_trip() {
        let wire: WireTranscriptSource = TranscriptSource::Spoken.into();
        assert!(matches!(wire, WireTranscriptSource::Spoken));
        let back = TranscriptSource::try_from(wire).unwrap();
        assert!(matches!(back, TranscriptSource::Spoken));
    }

    /// R7-5 (P3 dogma): the reverse `WireAssistantBlock::Unknown -> core`
    /// path must surface a typed [`WireConversionError::AssistantBlock`]
    /// rather than fabricating an empty `AssistantBlock::Text { "" }`.
    /// Closes the silent zero-length-text injection regression that the
    /// previous `From` impl exhibited for unknown future wire variants.
    #[test]
    fn wire_to_core_assistant_block_unknown_returns_typed_error() {
        let wire = WireAssistantBlock::Unknown;
        let err = AssistantBlock::try_from(wire).unwrap_err();
        assert!(
            matches!(
                err,
                crate::wire::error::WireConversionError::AssistantBlock { ref debug }
                    if debug == "WireAssistantBlock::Unknown"
            ),
            "expected WireConversionError::AssistantBlock, got {err:?}"
        );
    }
}

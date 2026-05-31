//! Session management for Meerkat
//!
//! A session represents a conversation history that can be persisted and resumed.
//!
//! # Performance
//!
//! Sessions use Arc-based copy-on-write for message storage:
//! - `fork()` shares the message buffer (O(1), no clone)
//! - Mutation (push) triggers CoW only when refcount > 1
//! - `push_batch()` adds multiple messages with a single timestamp update

use crate::Provider;
use crate::generated::{
    session_document, session_durable_config_authority, session_persistence_version_authority,
    session_realtime_transcript_authority,
    session_realtime_transcript_authority::SessionRealtimeTranscriptState,
};
use crate::peer_meta::PeerMeta;
use crate::realtime_transcript::{
    RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent, SESSION_REALTIME_TRANSCRIPT_STATE_KEY,
};
use crate::service::{AppendSystemContextRequest, MobToolAuthorityContext};
use crate::time_compat::SystemTime;
use crate::tool_scope::ToolFilter;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ContentInput, Message, SessionId,
    StopReason, ToolDef, ToolProvenance, ToolResult, Usage, UserMessage,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Current session format version.
///
/// Version history:
/// - v1 — pre-wave-c. `SessionMetadata.auth_binding` inner fields were
///   untyped strings (`realm_id`, `binding_id`, `profile`); no per-entity
///   schema version byte on `SessionMetadata`.
/// - v2 — wave-c C-3. `AuthBindingRef` inner fields are typed
///   `RealmId`/`BindingId`/`ProfileId` newtypes; `SessionMetadata` carries
///   a `schema_version` byte. Opportunistic upgrade-on-read —
///   `meerkat_session::persistent::migrations::migrate` rewrites v1 rows
///   into v2 shape; the next `save()` persists v2.
pub use crate::generated::session_persistence_version_authority::SESSION_VERSION;

/// Current `SessionMetadata` schema version. Distinct from `SESSION_VERSION`
/// so `SessionMetadata` can evolve independently of the Session envelope.
///
/// - v1 — pre-wave-c. Default on read for rows written before the byte
///   was introduced.
/// - v2 — wave-c C-3. Typed `AuthBindingRef` inner fields; any future
///   `SessionMetadata`-local shape change bumps this without moving
///   `SESSION_VERSION`.
pub use crate::generated::session_persistence_version_authority::SESSION_METADATA_SCHEMA_VERSION;

/// Current session format version accepted by generated persistence authority.
pub fn session_version() -> u32 {
    session_persistence_version_authority::session_envelope_version()
}

/// Current `SessionMetadata` schema version accepted by generated persistence authority.
pub fn session_metadata_schema_version() -> u32 {
    session_persistence_version_authority::session_metadata_schema_version()
}

/// Typed transcript replacement used to create an edited fork.
///
/// Replacements never mutate the source session in place. The owning service
/// applies this to a forked prefix, producing a new `SessionId`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptReplacement {
    /// Replace the addressed message with a full canonical message.
    Message { message: Message },
    /// Replace one user-message content block.
    UserContentBlock {
        block_index: usize,
        block: ContentBlock,
    },
    /// Replace one block in a block-assistant message.
    AssistantBlock {
        block_index: usize,
        block: AssistantBlock,
    },
    /// Replace one content block inside one tool-result payload.
    ToolResultContentBlock {
        result_index: usize,
        block_index: usize,
        block: ContentBlock,
    },
}

/// Session metadata key for the typed transcript revision graph head.
pub const SESSION_TRANSCRIPT_HISTORY_STATE_KEY: &str = "session_transcript_history_state_v1";

/// A concrete transcript span selected for same-session rewrite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptRewriteSelection {
    /// Replace messages in `[start, end)`.
    MessageRange { start: usize, end: usize },
}

impl TranscriptRewriteSelection {
    fn bounds(&self) -> (usize, usize) {
        match self {
            Self::MessageRange { start, end } => (*start, *end),
        }
    }
}

/// Audit annotation carried with a transcript rewrite commit.
///
/// The free-form kind is for review, debugging, and provenance. It is not a
/// second policy authority; rewrite admission is enforced by the typed
/// selection, digest, parent-revision, and store-guard contracts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewriteReason {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl TranscriptRewriteReason {
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            note: None,
        }
    }
}

/// Immutable rewrite commit that advances a session transcript head.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewriteCommit {
    pub parent_revision: String,
    pub revision: String,
    pub selection: TranscriptRewriteSelection,
    pub original_span_digest: String,
    pub replacement_digest: String,
    pub messages_before: usize,
    pub messages_after: usize,
    pub reason: TranscriptRewriteReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "SchemaSystemTime"))]
    pub committed_at: SystemTime,
}

/// Immutable transcript revision body retained by the session-local graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRevisionBody {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_revision: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    pub messages: Vec<Message>,
    #[cfg_attr(feature = "schema", schemars(with = "SchemaSystemTime"))]
    pub created_at: SystemTime,
}

#[cfg(feature = "schema")]
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[schemars(rename = "SystemTime")]
struct SchemaSystemTime {
    secs_since_epoch: u64,
    nanos_since_epoch: u32,
}

/// Self-contained append-only transcript rewrite record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewriteRecord {
    pub commit: TranscriptRewriteCommit,
    pub parent_body: TranscriptRevisionBody,
    pub revision_body: TranscriptRevisionBody,
}

impl TranscriptRewriteRecord {
    pub fn new(
        commit: TranscriptRewriteCommit,
        parent_body: TranscriptRevisionBody,
        revision_body: TranscriptRevisionBody,
    ) -> Result<Self, TranscriptEditError> {
        validate_transcript_rewrite_record(&commit, &parent_body, &revision_body)?;
        Ok(Self {
            commit,
            parent_body,
            revision_body,
        })
    }
}

/// Typed session-local transcript revision graph state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptHistoryState {
    pub head: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<TranscriptRewriteCommit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<TranscriptRevisionBody>,
}

impl TranscriptHistoryState {
    /// Rebuild transcript revision graph state from append-only rewrite records.
    pub fn from_rewrite_records<I>(records: I) -> Result<Option<Self>, TranscriptEditError>
    where
        I: IntoIterator<Item = TranscriptRewriteRecord>,
    {
        let mut state: Option<Self> = None;
        for record in records {
            validate_transcript_rewrite_record(
                &record.commit,
                &record.parent_body,
                &record.revision_body,
            )?;
            let state = state.get_or_insert_with(|| Self {
                head: record.commit.parent_revision.clone(),
                commits: Vec::new(),
                revisions: Vec::new(),
            });
            if record.commit.parent_revision != state.head {
                if revision_body_extends_head(&record.parent_body, &state.revisions, &state.head)? {
                    state.head = record.commit.parent_revision.clone();
                } else {
                    return Err(TranscriptEditError::HistoryStateMalformed(format!(
                        "rewrite record parent {} does not extend transcript head {}",
                        record.commit.parent_revision, state.head
                    )));
                }
            }
            if !state
                .revisions
                .iter()
                .any(|body| body.revision == record.parent_body.revision)
            {
                state.revisions.push(record.parent_body);
            }
            if !state
                .revisions
                .iter()
                .any(|body| body.revision == record.revision_body.revision)
            {
                state.revisions.push(record.revision_body);
            }
            state.head = record.commit.revision.clone();
            state.commits.push(record.commit);
        }
        Ok(state)
    }
}

/// Invalid typed transcript edit request.
#[derive(Debug, Clone, thiserror::Error)]
pub enum TranscriptEditError {
    #[error("message index {message_index} out of bounds for {message_count} messages")]
    MessageIndexOutOfBounds {
        message_index: usize,
        message_count: usize,
    },
    #[error("{block_kind} index {block_index} out of bounds for {block_count} blocks")]
    BlockIndexOutOfBounds {
        block_kind: &'static str,
        block_index: usize,
        block_count: usize,
    },
    #[error("replacement expected {expected} at message index {message_index}, found {actual}")]
    MessageRoleMismatch {
        message_index: usize,
        expected: &'static str,
        actual: &'static str,
    },
    #[error("invalid transcript rewrite range {start}..{end} for {message_count} messages")]
    InvalidRewriteRange {
        start: usize,
        end: usize,
        message_count: usize,
    },
    #[error("transcript rewrite does not change transcript revision {revision}")]
    NoOpRewrite { revision: String },
    #[error("transcript rewrite parent revision mismatch: expected {expected}, actual {actual}")]
    RevisionConflict { expected: String, actual: String },
    #[error("transcript history state is malformed: {0}")]
    HistoryStateMalformed(String),
    #[error("invalid transcript shape after rewrite: {0}")]
    InvalidTranscriptShape(String),
}

fn message_role_name(message: &Message) -> &'static str {
    match message {
        Message::System(_) => "system",
        Message::SystemNotice(_) => "system_notice",
        Message::User(_) => "user",
        Message::Assistant(_) => "assistant",
        Message::BlockAssistant(_) => "block_assistant",
        Message::ToolResults { .. } => "tool_results",
    }
}

fn assistant_tool_use_ids(message: &Message) -> Vec<&str> {
    match message {
        Message::Assistant(assistant) => assistant
            .tool_calls
            .iter()
            .map(|tool_call| tool_call.id.as_str())
            .collect(),
        Message::BlockAssistant(assistant) => assistant
            .blocks
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn validate_transcript_tool_result_shape(messages: &[Message]) -> Result<(), TranscriptEditError> {
    for (index, message) in messages.iter().enumerate() {
        if let Message::ToolResults { results, .. } = message {
            let Some(previous) = index
                .checked_sub(1)
                .and_then(|previous| messages.get(previous))
            else {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} has no preceding assistant tool-use message"
                )));
            };
            let expected = assistant_tool_use_ids(previous);
            if expected.is_empty() {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} follows {}, not an assistant tool-use message",
                    message_role_name(previous)
                )));
            }
            let actual = results
                .iter()
                .map(|result| result.tool_use_id.as_str())
                .collect::<Vec<_>>();
            let actual_set = actual.iter().copied().collect::<BTreeSet<_>>();
            let expected_set = expected.iter().copied().collect::<BTreeSet<_>>();
            if actual.len() != actual_set.len() {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} contains duplicate tool ids"
                )));
            }
            if expected.len() != expected_set.len() {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "assistant tool-use message before tool_results at message {index} contains duplicate tool ids"
                )));
            }
            if actual_set != expected_set {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "tool_results at message {index} resolve tool ids {actual_set:?}, expected {expected_set:?}"
                )));
            }
        }

        let tool_use_ids = assistant_tool_use_ids(message);
        if tool_use_ids.is_empty() {
            continue;
        }
        let Some(next) = messages.get(index + 1) else {
            return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                "assistant tool-use message {index} has no following tool_results"
            )));
        };
        if !matches!(next, Message::ToolResults { .. }) {
            return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                "assistant tool-use message {index} is followed by {}, not tool_results",
                message_role_name(next)
            )));
        }
    }
    Ok(())
}

pub fn transcript_messages_digest(messages: &[Message]) -> Result<String, serde_json::Error> {
    sha256_json_digest(messages)
}

fn validate_transcript_rewrite_record(
    commit: &TranscriptRewriteCommit,
    parent_body: &TranscriptRevisionBody,
    revision_body: &TranscriptRevisionBody,
) -> Result<(), TranscriptEditError> {
    if parent_body.revision != commit.parent_revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "parent body revision {} does not match commit parent {}",
            parent_body.revision, commit.parent_revision
        )));
    }
    if revision_body.revision != commit.revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "revision body {} does not match commit revision {}",
            revision_body.revision, commit.revision
        )));
    }
    if commit.parent_revision == commit.revision {
        return Err(TranscriptEditError::NoOpRewrite {
            revision: commit.revision.clone(),
        });
    }
    let parent_digest = transcript_messages_digest(&parent_body.messages)
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_digest != commit.parent_revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "parent body digest {parent_digest} does not match commit parent {}",
            commit.parent_revision
        )));
    }
    let revision_digest = transcript_messages_digest(&revision_body.messages)
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if revision_digest != commit.revision {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "revision body digest {revision_digest} does not match commit revision {}",
            commit.revision
        )));
    }
    let (start, end) = commit.selection.bounds();
    if start > end || end > parent_body.messages.len() {
        return Err(TranscriptEditError::InvalidRewriteRange {
            start,
            end,
            message_count: parent_body.messages.len(),
        });
    }
    if commit.messages_before != parent_body.messages.len()
        || commit.messages_after != revision_body.messages.len()
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "commit message counts {} -> {} do not match revision bodies {} -> {}",
            commit.messages_before,
            commit.messages_after,
            parent_body.messages.len(),
            revision_body.messages.len()
        )));
    }
    let original_span_digest = sha256_json_digest(&parent_body.messages[start..end])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if original_span_digest != commit.original_span_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "original span digest {original_span_digest} does not match commit digest {}",
            commit.original_span_digest
        )));
    }
    let removed_len = end - start;
    let retained_len = commit
        .messages_before
        .checked_sub(removed_len)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "commit removed more messages than it recorded before rewrite".to_string(),
            )
        })?;
    let replacement_len = commit
        .messages_after
        .checked_sub(retained_len)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "commit message counts cannot describe a replacement span".to_string(),
            )
        })?;
    let replacement_end = start.checked_add(replacement_len).ok_or_else(|| {
        TranscriptEditError::HistoryStateMalformed("replacement span end overflowed".to_string())
    })?;
    if replacement_end > revision_body.messages.len() {
        return Err(TranscriptEditError::InvalidRewriteRange {
            start,
            end: replacement_end,
            message_count: revision_body.messages.len(),
        });
    }
    let parent_prefix_digest = transcript_messages_digest(&parent_body.messages[..start])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    let revision_prefix_digest = transcript_messages_digest(&revision_body.messages[..start])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_prefix_digest != revision_prefix_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "rewrite revision changed messages before the selected span".to_string(),
        ));
    }
    let parent_suffix_digest = transcript_messages_digest(&parent_body.messages[end..])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    let revision_suffix_digest =
        transcript_messages_digest(&revision_body.messages[replacement_end..])
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if parent_suffix_digest != revision_suffix_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "rewrite revision changed messages after the selected span".to_string(),
        ));
    }
    let replacement_digest = sha256_json_digest(&revision_body.messages[start..replacement_end])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    if replacement_digest != commit.replacement_digest {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "replacement span digest {replacement_digest} does not match commit digest {}",
            commit.replacement_digest
        )));
    }
    Ok(())
}

fn validate_transcript_history_state(
    state: &TranscriptHistoryState,
) -> Result<(), TranscriptEditError> {
    if state
        .revisions
        .iter()
        .all(|body| body.revision != state.head)
    {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "missing transcript head body {}",
            state.head
        )));
    }
    for body in &state.revisions {
        let digest = transcript_messages_digest(&body.messages)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        if digest != body.revision {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "transcript revision body {} has digest {digest}",
                body.revision
            )));
        }
    }
    for commit in &state.commits {
        let parent_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.parent_revision)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "missing parent transcript body {}",
                    commit.parent_revision
                ))
            })?;
        let revision_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.revision)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "missing transcript revision body {}",
                    commit.revision
                ))
            })?;
        validate_transcript_rewrite_record(commit, parent_body, revision_body)?;
    }
    let Some(first_commit) = state.commits.first() else {
        return Ok(());
    };
    let mut expected_head = first_commit.parent_revision.clone();
    for commit in &state.commits {
        let parent_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.parent_revision)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "missing parent transcript body {}",
                    commit.parent_revision
                ))
            })?;
        if commit.parent_revision != expected_head
            && !revision_body_extends_head(parent_body, &state.revisions, &expected_head)?
        {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite commit parent {} does not extend transcript head {}",
                commit.parent_revision, expected_head
            )));
        }
        expected_head = commit.revision.clone();
    }
    let mut cursor = state.head.clone();
    while cursor != expected_head {
        let Some(head_body) = state.revisions.iter().find(|body| body.revision == cursor) else {
            break;
        };
        match head_body.parent_revision.as_deref() {
            Some(parent) => cursor = parent.to_string(),
            None => break,
        }
    }
    if cursor != expected_head {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "transcript head {} does not extend the rewrite chain",
            state.head
        )));
    }
    Ok(())
}

fn revision_body_extends_head(
    candidate: &TranscriptRevisionBody,
    revisions: &[TranscriptRevisionBody],
    head: &str,
) -> Result<bool, TranscriptEditError> {
    if candidate.parent_revision.as_deref() == Some(head) {
        return Ok(true);
    }
    let Some(head_body) = revisions.iter().find(|body| body.revision == head) else {
        return Ok(false);
    };
    if candidate.messages.len() < head_body.messages.len() {
        return Ok(false);
    }
    let prefix_digest = transcript_messages_digest(&candidate.messages[..head_body.messages.len()])
        .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
    Ok(prefix_digest == head)
}

fn sha256_json_digest<T: Serialize + ?Sized>(value: &T) -> Result<String, serde_json::Error> {
    let bytes = serde_json::to_vec(value)?;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(format!("sha256:{out}"))
}

/// A conversation session with full history
///
/// Uses Arc<Vec<Message>> internally for efficient forking (copy-on-write).
#[derive(Debug, Clone)]
pub struct Session {
    /// Format version for migrations
    version: u32,
    /// Unique identifier
    id: SessionId,
    /// All messages in order (Arc for CoW on fork)
    pub(crate) messages: Arc<Vec<Message>>,
    /// When the session was created
    created_at: SystemTime,
    /// When the session was last updated
    updated_at: SystemTime,
    /// Arbitrary metadata
    metadata: serde_json::Map<String, serde_json::Value>,
    /// Cumulative token usage across all LLM calls in this session
    usage: Usage,
}

/// Serde helper for Session serialization (flattens Arc)
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct SessionSerde {
    #[serde(default = "default_version")]
    version: u32,
    id: SessionId,
    messages: Vec<Message>,
    created_at: SystemTime,
    updated_at: SystemTime,
    #[serde(default)]
    metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    usage: Usage,
}

impl Serialize for Session {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let serde_repr = SessionSerde {
            version: self.version,
            id: self.id.clone(),
            messages: (*self.messages).clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            metadata: self.metadata.clone(),
            usage: self.usage.clone(),
        };
        serde_repr.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Session {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let serde_repr = SessionSerde::deserialize(deserializer)?;
        let version = session_persistence_version_authority::restore_session_envelope_version(
            serde_repr.version,
        )
        .map_err(<D::Error as serde::de::Error>::custom)?;
        Ok(Session {
            version,
            id: serde_repr.id,
            messages: Arc::new(serde_repr.messages),
            created_at: serde_repr.created_at,
            updated_at: serde_repr.updated_at,
            metadata: serde_repr.metadata,
            usage: serde_repr.usage,
        })
    }
}

fn default_version() -> u32 {
    session_persistence_version_authority::legacy_session_envelope_version()
}

/// Metadata key used to store durable system-context control state.
pub const SESSION_SYSTEM_CONTEXT_STATE_KEY: &str = "session_system_context_state";

/// Metadata key used to store deferred-turn control state.
pub const SESSION_DEFERRED_TURN_STATE_KEY: &str = "session_deferred_turn_state";

/// Metadata key used to store recoverable build-only session state.
pub const SESSION_BUILD_STATE_KEY: &str = "session_build_state";

/// Metadata key used to store durable session-local tool visibility intent.
pub const SESSION_TOOL_VISIBILITY_STATE_KEY: &str = "session_tool_visibility_state_v1";

/// Canonical tool name gated by `image_tool_results` capability.
pub const VIEW_IMAGE_TOOL_NAME: &str = "view_image";

/// Canonical separator between appended runtime system-context blocks.
pub const SYSTEM_CONTEXT_SEPARATOR: &str = "\n\n---\n\n";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("metadata key `{key}` is reserved for session authority")]
pub struct ReservedSessionMetadataKey {
    key: String,
}

impl ReservedSessionMetadataKey {
    fn new(key: &str) -> Self {
        Self {
            key: key.to_string(),
        }
    }
}

fn is_session_authority_metadata_key(key: &str) -> bool {
    matches!(
        key,
        SESSION_METADATA_KEY
            | SESSION_BUILD_STATE_KEY
            | SESSION_SYSTEM_CONTEXT_STATE_KEY
            | SESSION_DEFERRED_TURN_STATE_KEY
            | SESSION_TOOL_VISIBILITY_STATE_KEY
            | SESSION_TRANSCRIPT_HISTORY_STATE_KEY
            | SESSION_REALTIME_TRANSCRIPT_STATE_KEY
    )
}

#[allow(clippy::panic)]
fn fail_closed_generated_restore(authority: &'static str, err: serde_json::Error) -> ! {
    tracing::error!(
        authority,
        error = %err,
        "generated authority rejected durable restore"
    );
    panic!("generated {authority} authority rejected durable restore: {err}");
}

/// Shared runtime system-context authority handle.
///
/// This handle is intentionally narrower than `Arc<Mutex<SessionSystemContextState>>`:
/// callers can read snapshots or request generated-authority transitions, but
/// cannot replace the machine-owned state by taking a mutable guard.
#[derive(Clone)]
pub struct SystemContextStateHandle {
    inner: Arc<std::sync::Mutex<SessionSystemContextState>>,
}

impl std::fmt::Debug for SystemContextStateHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemContextStateHandle")
            .field("inner", &"<Arc<Mutex<SessionSystemContextState>>>")
            .finish()
    }
}

impl SystemContextStateHandle {
    pub fn new(state: SessionSystemContextState) -> Result<Self, serde_json::Error> {
        let state = system_context_authority::restore_system_context_state(state)
            .map_err(<serde_json::Error as serde::de::Error>::custom)?;
        Ok(Self {
            inner: Arc::new(std::sync::Mutex::new(state)),
        })
    }

    pub fn from_shared_authority_state(
        inner: Arc<std::sync::Mutex<SessionSystemContextState>>,
    ) -> Self {
        Self { inner }
    }

    pub fn snapshot(&self) -> SessionSystemContextState {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                tracing::warn!("system-context state lock poisoned while reading snapshot");
                poisoned.into_inner().clone()
            }
        }
    }

    pub fn replace_from_generated_restore(
        &self,
        state: SessionSystemContextState,
    ) -> Result<(), serde_json::Error> {
        let state = system_context_authority::restore_system_context_state(state)
            .map_err(<serde_json::Error as serde::de::Error>::custom)?;
        match self.inner.lock() {
            Ok(mut guard) => {
                *guard = state;
            }
            Err(poisoned) => {
                tracing::warn!("system-context state lock poisoned while restoring state");
                *poisoned.into_inner() = state;
            }
        }
        Ok(())
    }

    pub fn replace_from_generated_restore_if_changed(
        &self,
        state: SessionSystemContextState,
    ) -> Result<bool, serde_json::Error> {
        let state = system_context_authority::restore_system_context_state(state)
            .map_err(<serde_json::Error as serde::de::Error>::custom)?;
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!(
                    "system-context state lock poisoned while replacing generated-restored state"
                );
                poisoned.into_inner()
            }
        };
        if *guard == state {
            return Ok(false);
        }
        *guard = state;
        Ok(true)
    }

    pub fn replace_from_generated_restore_if_current(
        &self,
        current: &SessionSystemContextState,
        replacement: SessionSystemContextState,
    ) -> Result<bool, serde_json::Error> {
        let replacement = system_context_authority::restore_system_context_state(replacement)
            .map_err(<serde_json::Error as serde::de::Error>::custom)?;
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!(
                    "system-context state lock poisoned while conditionally replacing generated-restored state"
                );
                poisoned.into_inner()
            }
        };
        if *guard != *current {
            return Ok(false);
        }
        *guard = replacement;
        Ok(true)
    }

    pub fn stage_append_with_snapshot(
        &self,
        req: &AppendSystemContextRequest,
        accepted_at: SystemTime,
    ) -> Result<
        (
            crate::service::AppendSystemContextStatus,
            SessionSystemContextState,
            SessionSystemContextState,
        ),
        SystemContextStageError,
    > {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("system-context state lock poisoned while staging append");
                poisoned.into_inner()
            }
        };
        let snapshot = guard.clone();
        let status = guard.stage_append(req, accepted_at)?;
        let staged = guard.clone();
        Ok((status, snapshot, staged))
    }

    pub fn stage_active_turn_appends_with_snapshot(
        &self,
        appends: Vec<(AppendSystemContextRequest, SystemTime)>,
    ) -> Result<(SessionSystemContextState, SessionSystemContextState), SystemContextStageError>
    {
        let mut guard = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!(
                    "system-context state lock poisoned while staging active-turn appends"
                );
                poisoned.into_inner()
            }
        };
        let snapshot = guard.clone();
        let mut candidate = snapshot.clone();
        for (req, accepted_at) in appends {
            candidate.stage_active_turn_append(&req, accepted_at)?;
        }
        *guard = candidate.clone();
        let staged = candidate;
        Ok((snapshot, staged))
    }

    pub fn discard_unapplied_active_turn_pending(&self) -> usize {
        let discarded = match self.inner.lock() {
            Ok(mut guard) => guard.discard_unapplied_active_turn_pending(),
            Err(poisoned) => {
                tracing::warn!(
                    "system-context state lock poisoned while discarding active-turn context"
                );
                poisoned
                    .into_inner()
                    .discard_unapplied_active_turn_pending()
            }
        };
        discarded.len()
    }

    pub fn discard_active_turn_pending_by_keys(
        &self,
        idempotency_keys: &[String],
    ) -> Vec<PendingSystemContextAppend> {
        match self.inner.lock() {
            Ok(mut guard) => guard.discard_active_turn_pending_by_keys(idempotency_keys),
            Err(poisoned) => {
                tracing::warn!(
                    "system-context state lock poisoned while discarding active-turn pending appends"
                );
                poisoned
                    .into_inner()
                    .discard_active_turn_pending_by_keys(idempotency_keys)
            }
        }
    }

    pub fn stage_active_turn_append(
        &self,
        req: &AppendSystemContextRequest,
        accepted_at: SystemTime,
    ) -> Result<crate::service::AppendSystemContextStatus, SystemContextStageError> {
        match self.inner.lock() {
            Ok(mut guard) => guard.stage_active_turn_append(req, accepted_at),
            Err(poisoned) => {
                tracing::warn!(
                    "system-context state lock poisoned while staging active-turn context"
                );
                poisoned
                    .into_inner()
                    .stage_active_turn_append(req, accepted_at)
            }
        }
    }
}

/// Durable control state for runtime system-context append requests.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SessionSystemContextState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) pending: Vec<PendingSystemContextAppend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) applied: Vec<PendingSystemContextAppend>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub(crate) seen: std::collections::BTreeMap<String, SeenSystemContextKey>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub(crate) active_turn_pending_keys: std::collections::BTreeSet<String>,
}

/// Typed provenance class for a runtime system-context append.
///
/// Canonical replacement for the retired `runtime:steer:` string-prefix
/// folklore. The PRODUCER of a runtime-steer append (the runtime input
/// projection in `meerkat-runtime`) constructs it with
/// [`SystemContextSource::RuntimeSteer`]; everything else is
/// [`SystemContextSource::Normal`]. No code reclassifies a `source` string
/// into this fact — it is set once at construction and the machine guards the
/// typed field.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemContextSource {
    /// A durable, non-transient runtime context append (peer responses, etc.).
    #[default]
    Normal,
    /// A transient operator/peer steer append that must not survive past the
    /// turn it steers and must not be promoted to the durable applied set.
    RuntimeSteer,
}

impl From<SystemContextSource> for session_document::SystemContextSource {
    fn from(value: SystemContextSource) -> Self {
        match value {
            SystemContextSource::Normal => Self::Normal,
            SystemContextSource::RuntimeSteer => Self::RuntimeSteer,
        }
    }
}

impl SystemContextSource {
    /// Whether this is the default (`Normal`) provenance. Used by
    /// `skip_serializing_if` so durable appends serialize without the field.
    #[must_use]
    pub fn is_normal(&self) -> bool {
        matches!(self, Self::Normal)
    }

    /// Whether this append is a transient runtime steer.
    #[must_use]
    pub fn is_runtime_steer(&self) -> bool {
        matches!(self, Self::RuntimeSteer)
    }
}

/// Pending append request accepted by the control plane but not yet applied at an LLM boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct PendingSystemContextAppend {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Typed provenance: whether this append is a transient runtime steer.
    #[serde(default, skip_serializing_if = "SystemContextSource::is_normal")]
    pub source_kind: SystemContextSource,
    pub accepted_at: SystemTime,
}

/// Durable control state for deferred first-turn prompt and staged callback tool results.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SessionDeferredTurnState {
    #[serde(default, skip_serializing_if = "DeferredFirstTurnPhase::is_inactive")]
    pub(crate) first_turn_phase: DeferredFirstTurnPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_initial_prompt: Option<PendingDeferredPrompt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) pending_tool_results: Vec<PendingToolResultsMessage>,
}

/// Canonical lifecycle phase for the session's deferred first turn.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeferredFirstTurnPhase {
    /// The session was not created in deferred-first-turn mode.
    #[default]
    Inactive,
    /// The session exists durably but the first turn has not started yet.
    Pending,
    /// The first turn has started; build-only overrides are no longer legal.
    Consumed,
}

impl DeferredFirstTurnPhase {
    pub fn is_inactive(&self) -> bool {
        matches!(self, Self::Inactive)
    }
}

impl From<DeferredFirstTurnPhase> for session_document::SessionFirstTurnPhase {
    fn from(value: DeferredFirstTurnPhase) -> Self {
        match value {
            DeferredFirstTurnPhase::Inactive => Self::Inactive,
            DeferredFirstTurnPhase::Pending => Self::Pending,
            DeferredFirstTurnPhase::Consumed => Self::Consumed,
        }
    }
}

impl From<session_document::SessionFirstTurnPhase> for DeferredFirstTurnPhase {
    fn from(value: session_document::SessionFirstTurnPhase) -> Self {
        match value {
            session_document::SessionFirstTurnPhase::Inactive => Self::Inactive,
            session_document::SessionFirstTurnPhase::Pending => Self::Pending,
            session_document::SessionFirstTurnPhase::Consumed => Self::Consumed,
        }
    }
}

fn is_default_hook_run_overrides(value: &crate::HookRunOverrides) -> bool {
    value == &crate::HookRunOverrides::default()
}

fn is_default_call_timeout_override(value: &crate::CallTimeoutOverride) -> bool {
    value == &crate::CallTimeoutOverride::default()
}

fn is_tool_filter_all(value: &ToolFilter) -> bool {
    matches!(value, ToolFilter::All)
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// Derive the machine-owned capability base filter from the current image-tool-results support.
pub fn capability_base_filter_for_image_tool_results(image_tool_results: bool) -> ToolFilter {
    if image_tool_results {
        ToolFilter::All
    } else {
        ToolFilter::Deny([VIEW_IMAGE_TOOL_NAME.to_string()].into_iter().collect())
    }
}

/// Persisted witness for a durable tool-visibility name.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ToolVisibilityWitness {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_owner_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_provenance: Option<ToolProvenance>,
}

impl ToolVisibilityWitness {
    pub fn has_identity_witness(&self) -> bool {
        self.stable_owner_key.is_some() || self.last_seen_provenance.is_some()
    }

    pub fn has_provenance_identity_witness(&self) -> bool {
        self.last_seen_provenance.is_some()
    }
}

/// Typed authority value for a deferred-tool load request.
///
/// The public/effect seam carries the requested route name and provenance
/// witness as one value. Canonical owners may project this into name-indexed
/// maps internally, but callers do not get to make a map key the authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct DeferredToolLoadAuthority {
    pub name: String,
    pub witness: ToolVisibilityWitness,
}

impl DeferredToolLoadAuthority {
    pub fn new(name: impl Into<String>, witness: ToolVisibilityWitness) -> Self {
        Self {
            name: name.into(),
            witness,
        }
    }

    pub fn into_parts(self) -> (String, ToolVisibilityWitness) {
        (self.name, self.witness)
    }
}

/// Durable tool-filter intent paired with the witnesses that made the names
/// authoritative at capture time.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct WitnessedToolFilter {
    pub filter: ToolFilter,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub witnesses: BTreeMap<String, ToolVisibilityWitness>,
}

impl WitnessedToolFilter {
    pub fn new(filter: ToolFilter, witnesses: BTreeMap<String, ToolVisibilityWitness>) -> Self {
        Self { filter, witnesses }
    }

    pub fn into_parts(self) -> (ToolFilter, BTreeMap<String, ToolVisibilityWitness>) {
        (self.filter, self.witnesses)
    }
}

/// Opaque parent/composition-authorized inherited tool visibility handoff.
///
/// The filter and witnesses are intentionally not public fields. Callers that
/// need to hand inherited visibility to a child build must obtain this from an
/// AgentFactory-minted parent composition authority; they cannot write
/// canonical session visibility state directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InheritedToolVisibilityAuthority {
    filter: ToolFilter,
    witnesses: BTreeMap<String, ToolVisibilityWitness>,
}

impl InheritedToolVisibilityAuthority {
    pub(crate) fn from_generated_composition_authority(
        filter: ToolFilter,
        witnesses: BTreeMap<String, ToolVisibilityWitness>,
    ) -> Self {
        Self { filter, witnesses }
    }

    pub fn filter(&self) -> &ToolFilter {
        &self.filter
    }

    pub fn witnesses(&self) -> &BTreeMap<String, ToolVisibilityWitness> {
        &self.witnesses
    }

    pub(crate) fn into_initial_visibility_state(self) -> SessionToolVisibilityState {
        SessionToolVisibilityState {
            inherited_base_filter: self.filter,
            filter_witnesses: self.witnesses,
            ..Default::default()
        }
    }
}

/// Canonical durable session-local tool visibility intent.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SessionToolVisibilityState {
    #[serde(default, skip_serializing_if = "is_tool_filter_all")]
    pub capability_base_filter: ToolFilter,
    #[serde(default, skip_serializing_if = "is_tool_filter_all")]
    pub inherited_base_filter: ToolFilter,
    #[serde(default, skip_serializing_if = "is_tool_filter_all")]
    pub active_filter: ToolFilter,
    #[serde(default, skip_serializing_if = "is_tool_filter_all")]
    pub staged_filter: ToolFilter,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub active_requested_deferred_names: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub staged_requested_deferred_names: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub active_revision: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub staged_revision: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requested_witnesses: BTreeMap<String, ToolVisibilityWitness>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub filter_witnesses: BTreeMap<String, ToolVisibilityWitness>,
}

/// Generated-authority-approved durable tool visibility projection.
///
/// Session metadata stores this as a projection of the generated visibility
/// owner. Code that only has raw `SessionToolVisibilityState` must first route
/// it through a `ToolVisibilityOwner`/`ToolScope` restore path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedSessionToolVisibilityState {
    state: SessionToolVisibilityState,
}

impl AuthorizedSessionToolVisibilityState {
    pub(crate) fn from_generated_authority(state: SessionToolVisibilityState) -> Self {
        Self { state }
    }

    pub fn as_state(&self) -> &SessionToolVisibilityState {
        &self.state
    }

    pub fn into_state(self) -> SessionToolVisibilityState {
        self.state
    }
}

/// Durable build-only session state required to faithfully recover and rebuild
/// a persisted session without surface-local shadow config.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionBuildState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<crate::OutputSchema>,
    #[serde(default, skip_serializing_if = "is_default_hook_run_overrides")]
    pub hooks_override: crate::HookRunOverrides,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<crate::BudgetLimits>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recoverable_tool_defs: Vec<ToolDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub silent_comms_intents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_peer_notifications: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_instructions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_env: Option<HashMap<String, String>>,
    /// Compatibility projection of mob operator authority.
    ///
    /// `MobToolAuthorityContext` deliberately loses its generated authority
    /// seal when serialized; restored behavior must be approved by the
    /// generated runtime bridge before this projection can affect tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_tool_authority_context: Option<MobToolAuthorityContext>,
    #[serde(default, skip_serializing_if = "is_default_call_timeout_override")]
    pub call_timeout_override: crate::CallTimeoutOverride,
}

/// Deferred create-time prompt staged for the next turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct PendingDeferredPrompt {
    pub prompt: ContentInput,
    pub accepted_at: SystemTime,
}

/// Staged callback tool results waiting to be admitted on the next turn seam.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PendingToolResultsMessage {
    pub results: Vec<ToolResult>,
    pub accepted_at: SystemTime,
}

impl PartialEq for PendingToolResultsMessage {
    fn eq(&self, other: &Self) -> bool {
        self.accepted_at == other.accepted_at
            && serde_json::to_value(&self.results).ok() == serde_json::to_value(&other.results).ok()
    }
}

/// Deferred first-turn inputs consumed at the generated start-turn authority seam.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConsumedDeferredTurnInputs {
    pub(crate) restore_first_turn_pending: bool,
    pub(crate) pending_initial_prompt: Option<PendingDeferredPrompt>,
    pub(crate) pending_tool_results: Vec<PendingToolResultsMessage>,
}

impl ConsumedDeferredTurnInputs {
    pub fn is_empty(&self) -> bool {
        !self.restore_first_turn_pending
            && self.pending_initial_prompt.is_none()
            && self.pending_tool_results.is_empty()
    }

    pub fn pending_initial_prompt(&self) -> Option<&PendingDeferredPrompt> {
        self.pending_initial_prompt.as_ref()
    }

    pub fn pending_tool_results(&self) -> &[PendingToolResultsMessage] {
        &self.pending_tool_results
    }
}

/// Seen idempotency-key entry for system-context append requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SeenSystemContextKey {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Typed provenance carried from the append, so runtime-steer cleanup can
    /// match seen entries by the typed marker rather than a `source` prefix.
    #[serde(default, skip_serializing_if = "SystemContextSource::is_normal")]
    pub source_kind: SystemContextSource,
    pub state: SeenSystemContextState,
}

/// Lifecycle state for an accepted idempotency key.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SeenSystemContextState {
    Pending,
    Applied,
}

impl SessionSystemContextState {
    pub fn pending(&self) -> &[PendingSystemContextAppend] {
        &self.pending
    }

    pub fn applied(&self) -> &[PendingSystemContextAppend] {
        &self.applied
    }

    pub fn seen(&self) -> &BTreeMap<String, SeenSystemContextKey> {
        &self.seen
    }

    pub fn active_turn_pending_keys(&self) -> &BTreeSet<String> {
        &self.active_turn_pending_keys
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn applied_len(&self) -> usize {
        self.applied.len()
    }

    pub fn active_turn_pending_len(&self) -> usize {
        self.active_turn_pending_keys.len()
    }

    pub fn realtime_projection_appends(&self) -> Vec<PendingSystemContextAppend> {
        self.applied
            .iter()
            .chain(self.pending.iter())
            .cloned()
            .collect()
    }

    /// Stage an append request, enforcing per-session idempotency.
    pub fn stage_append(
        &mut self,
        req: &AppendSystemContextRequest,
        accepted_at: SystemTime,
    ) -> Result<crate::service::AppendSystemContextStatus, SystemContextStageError> {
        system_context_authority::stage_append(self, req, accepted_at, false)
    }

    fn stage_append_with_generated_authority(
        &mut self,
        req: &AppendSystemContextRequest,
        accepted_at: SystemTime,
        active_turn_scoped: bool,
    ) -> Result<crate::service::AppendSystemContextStatus, SystemContextStageError> {
        system_context_authority::stage_append(self, req, accepted_at, active_turn_scoped)
    }

    /// Stage an append that is scoped to the currently-active turn only.
    ///
    /// If the active turn reaches another model boundary, normal pending
    /// consumption moves it to `applied`. If the turn completes first, callers
    /// should discard the still-pending active-turn keys so the context cannot
    /// leak into an unrelated later run.
    pub fn stage_active_turn_append(
        &mut self,
        req: &AppendSystemContextRequest,
        accepted_at: SystemTime,
    ) -> Result<crate::service::AppendSystemContextStatus, SystemContextStageError> {
        self.stage_append_with_generated_authority(req, accepted_at, true)
    }

    /// Mark all currently-pending appends as applied and clear the pending queue.
    pub fn mark_pending_applied(&mut self) {
        system_context_authority::mark_pending_applied(self);
    }

    /// Discard active-turn-only appends that were not consumed by the turn's
    /// next LLM boundary.
    pub fn discard_unapplied_active_turn_pending(&mut self) -> Vec<PendingSystemContextAppend> {
        system_context_authority::discard_unapplied_active_turn_pending(self)
    }

    /// Discard specific active-turn-only appends that are still pending.
    ///
    /// This is the rollback companion for live-boundary staging. The runtime
    /// owns the accepted input, so if that commit fails after the session has
    /// staged context, the session-side projection must be removed by the same
    /// idempotency keys before the caller reports failure.
    pub fn discard_active_turn_pending_by_keys(
        &mut self,
        idempotency_keys: &[String],
    ) -> Vec<PendingSystemContextAppend> {
        system_context_authority::discard_active_turn_pending_by_keys(self, idempotency_keys)
    }

    /// Authorize this snapshot through the canonical
    /// [`session_document::SessionDocumentMachine`] system-context restore
    /// transition, returning the state unchanged on success.
    pub fn restore_from_snapshot(self) -> Result<Self, SystemContextStageError> {
        system_context_authority::restore_system_context_state(self)
    }

    /// Record the machine-authorized applied system-context blocks, returning
    /// the appends that are newly applied (and thus need rendering into the
    /// system prompt by the caller).
    pub fn record_applied_blocks(
        &mut self,
        appends: &[PendingSystemContextAppend],
        current_system_prompt: &str,
    ) -> Vec<PendingSystemContextAppend> {
        system_context_authority::record_applied_system_context_blocks(
            self,
            appends,
            current_system_prompt,
        )
    }
}

/// Per-session registry key for the first-turn region of the
/// [`session_document::SessionDocumentMachine`]. Each
/// [`SessionDeferredTurnState`] is a single session's projection, so its
/// machine instance carries exactly one registry entry under this key.
const SESSION_DOCUMENT_FIRST_TURN_KEY: &str = "first_turn";

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Authorize a durable deferred-turn snapshot through the canonical
/// [`session_document::SessionDocumentMachine`] recovery transition.
///
/// The machine validates that the persisted first-turn phase is a legal
/// recovery target and adopts it into its per-session registry, emitting
/// `SessionFirstTurnPhaseRecovered`. The snapshot is returned unchanged on
/// success; the machine — not this shell — owns the recovery legality.
fn validate_deferred_turn_snapshot(
    state: SessionDeferredTurnState,
) -> Result<SessionDeferredTurnState, session_document::SessionDocumentError> {
    let mut authority = session_document::SessionDocumentMachineAuthority::new();
    let key = session_document::SessionDocumentKey::new(SESSION_DOCUMENT_FIRST_TURN_KEY);
    // The recovery transition fails closed for any illegal first-turn phase
    // (its guard admits only the three known phases); a rejection surfaces as
    // `Err` here. On success the machine has adopted the snapshot.
    authority.recover_session_first_turn_phase(
        key,
        state.first_turn_phase.into(),
        state.pending_initial_prompt.is_some(),
        usize_to_u64(state.pending_tool_results.len()),
    )?;
    Ok(state)
}

impl SessionDeferredTurnState {
    pub fn first_turn_phase(&self) -> DeferredFirstTurnPhase {
        self.first_turn_phase
    }

    pub fn pending_initial_prompt(&self) -> Option<&PendingDeferredPrompt> {
        self.pending_initial_prompt.as_ref()
    }

    pub fn pending_tool_results(&self) -> &[PendingToolResultsMessage] {
        &self.pending_tool_results
    }

    pub fn pending_tool_results_len(&self) -> usize {
        self.pending_tool_results.len()
    }

    pub(crate) fn pending_initial_prompt_mut_for_blob_rewrite(
        &mut self,
    ) -> Option<&mut PendingDeferredPrompt> {
        self.pending_initial_prompt.as_mut()
    }

    pub(crate) fn pending_tool_results_mut_for_blob_rewrite(
        &mut self,
    ) -> &mut [PendingToolResultsMessage] {
        &mut self.pending_tool_results
    }

    /// Build a [`SessionDocumentMachineAuthority`] seeded with this session's
    /// current durable first-turn projection.
    ///
    /// The machine owns the canonical first-turn phase + presence/count in its
    /// own per-session `Map`; the durable [`SessionDeferredTurnState`] is its
    /// projection. We recover the machine-owned registry from that projection
    /// before driving an operation so every subsequent decision reads the
    /// machine's own state — the shell never passes a phase conclusion as an
    /// operation input.
    fn document_authority(
        &self,
    ) -> (
        session_document::SessionDocumentMachineAuthority,
        session_document::SessionDocumentKey,
    ) {
        let mut authority = session_document::SessionDocumentMachineAuthority::new();
        let key = session_document::SessionDocumentKey::new(SESSION_DOCUMENT_FIRST_TURN_KEY);
        if let Err(err) = authority.recover_session_first_turn_phase(
            key.clone(),
            self.first_turn_phase.into(),
            self.pending_initial_prompt.is_some(),
            usize_to_u64(self.pending_tool_results.len()),
        ) {
            tracing::warn!(
                error = %err,
                "generated session document authority rejected first-turn recovery"
            );
        }
        (authority, key)
    }

    /// Mirror the machine-resolved first-turn phase from one effect batch onto
    /// the durable projection, returning `was_pending` when present.
    fn mirror_first_turn_phase(
        &mut self,
        effects: &[session_document::SessionDocumentEffect],
    ) -> Option<bool> {
        for effect in effects {
            if let session_document::SessionDocumentEffect::SessionFirstTurnPhaseResolved {
                phase,
                was_pending,
            } = effect
            {
                self.first_turn_phase = (*phase).into();
                return Some(*was_pending);
            }
        }
        None
    }

    /// Mark that this session has a deferred first turn waiting to start.
    pub fn mark_initial_turn_pending(&mut self) {
        let (mut authority, key) = self.document_authority();
        match authority.mark_session_initial_turn_pending(key) {
            Ok(effects) => {
                self.mirror_first_turn_phase(&effects);
            }
            Err(err) => tracing::warn!(
                error = %err,
                "generated session document authority rejected pending mark"
            ),
        }
    }

    /// Mark the deferred first turn as started.
    ///
    /// Returns true when the phase transitioned from `Pending`.
    pub fn mark_initial_turn_started(&mut self) -> bool {
        let (mut authority, key) = self.document_authority();
        match authority.start_session_initial_turn(key) {
            Ok(effects) => self.mirror_first_turn_phase(&effects).unwrap_or(false),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated session document authority rejected first-turn start"
                );
                false
            }
        }
    }

    /// Restore the deferred first-turn pending phase after a failed pre-run setup.
    pub fn restore_initial_turn_pending(&mut self) {
        // The restore-to-pending decision is the machine's
        // `RestoreSessionConsumedInputs` transition with phase rollback
        // requested; presence/count mirrors are left untouched here because the
        // bulky payloads are restored separately by the caller.
        let (mut authority, key) = self.document_authority();
        match authority.restore_session_consumed_inputs(
            key.clone(),
            true,
            self.pending_initial_prompt.is_some(),
            usize_to_u64(self.pending_tool_results.len()),
        ) {
            Ok(_) => {
                // Mirror the machine-owned phase the restore transition wrote
                // into its per-session registry rather than re-deriving it.
                if let Some(phase) = authority.session_first_turn_phase_for(&key) {
                    self.first_turn_phase = phase.into();
                }
            }
            Err(err) => tracing::warn!(
                error = %err,
                "generated session document authority rejected pending restore"
            ),
        }
    }

    /// Whether build-only first-turn overrides are still legal for this session.
    pub fn allows_initial_turn_overrides(&self) -> bool {
        let (mut authority, key) = self.document_authority();
        match authority.resolve_session_first_turn_overrides_allowed(key) {
            Ok(effects) => effects
                .iter()
                .find_map(|effect| {
                    match effect {
                session_document::SessionDocumentEffect::SessionFirstTurnOverridesResolved {
                    allowed,
                } => Some(*allowed),
                _ => None,
            }
                })
                .unwrap_or(false),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated session document authority rejected override resolution"
                );
                false
            }
        }
    }

    /// Stage the create-time prompt for a later first turn.
    pub fn stage_initial_prompt(&mut self, prompt: ContentInput, accepted_at: SystemTime) {
        let prompt_has_content = prompt.has_images() || !prompt.text_content().trim().is_empty();
        let (mut authority, key) = self.document_authority();
        match authority.stage_session_initial_prompt(key, prompt_has_content) {
            Ok(effects) => {
                let decision = effects.iter().find_map(|effect| {
                    match effect {
                    session_document::SessionDocumentEffect::SessionInitialPromptStageResolved {
                        decision,
                    } => Some(*decision),
                    _ => None,
                }
                });
                match decision {
                    Some(session_document::SessionInitialPromptStageDecision::Store) => {
                        self.pending_initial_prompt = Some(PendingDeferredPrompt {
                            prompt,
                            accepted_at,
                        });
                    }
                    Some(session_document::SessionInitialPromptStageDecision::Clear) => {
                        self.pending_initial_prompt = None;
                    }
                    None => tracing::warn!(
                        "generated session document authority returned no prompt-stage decision"
                    ),
                }
            }
            Err(err) => tracing::warn!(
                error = %err,
                "generated session document authority rejected initial prompt stage"
            ),
        }
    }

    /// Stage one callback tool-results message for the next turn.
    pub fn stage_tool_results(
        &mut self,
        results: Vec<ToolResult>,
        accepted_at: SystemTime,
    ) -> usize {
        let (mut authority, key) = self.document_authority();
        let accepted = match authority.stage_session_tool_results(key, usize_to_u64(results.len()))
        {
            Ok(effects) => effects.iter().find_map(|effect| match effect {
                session_document::SessionDocumentEffect::SessionToolResultsStageResolved {
                    accepted_count,
                } => Some(*accepted_count),
                _ => None,
            }),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated session document authority rejected tool-results stage"
                );
                return 0;
            }
        };
        let Some(accepted) = accepted else {
            tracing::warn!(
                "generated session document authority returned no tool-results decision"
            );
            return 0;
        };
        if accepted == 0 {
            return 0;
        }
        let accepted = usize::try_from(accepted).unwrap_or(usize::MAX);
        self.pending_tool_results.push(PendingToolResultsMessage {
            results,
            accepted_at,
        });
        accepted
    }

    /// Whether any callback tool results are currently staged.
    pub fn has_pending_tool_results(&self) -> bool {
        !self.pending_tool_results.is_empty()
    }

    /// Start a turn and consume all inputs generated-authorized for that seam.
    pub fn consume_for_started_turn(&mut self) -> ConsumedDeferredTurnInputs {
        let (mut authority, key) = self.document_authority();
        let was_pending = match authority.consume_session_deferred_inputs(key) {
            Ok(effects) => self.mirror_first_turn_phase(&effects).unwrap_or(false),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated session document authority rejected started-turn consumption"
                );
                return ConsumedDeferredTurnInputs::default();
            }
        };
        ConsumedDeferredTurnInputs {
            restore_first_turn_pending: was_pending,
            pending_initial_prompt: self.pending_initial_prompt.take(),
            pending_tool_results: std::mem::take(&mut self.pending_tool_results),
        }
    }

    /// Restore inputs previously consumed by `consume_for_started_turn`.
    pub fn restore_consumed_turn_inputs(&mut self, consumed: ConsumedDeferredTurnInputs) {
        if consumed.is_empty() {
            return;
        }
        let (mut authority, key) = self.document_authority();
        let effects = match authority.restore_session_consumed_inputs(
            key,
            consumed.restore_first_turn_pending,
            consumed.pending_initial_prompt.is_some(),
            usize_to_u64(consumed.pending_tool_results.len()),
        ) {
            Ok(effects) => effects,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated session document authority rejected consumed input restore"
                );
                return;
            }
        };
        let Some((restore_first_turn_pending, restore_initial_prompt, restore_tool_results)) =
            effects.iter().find_map(|effect| match effect {
                session_document::SessionDocumentEffect::SessionConsumedInputsRestoreResolved {
                    restore_first_turn_pending,
                    restore_initial_prompt,
                    restore_tool_results,
                } => Some((
                    *restore_first_turn_pending,
                    *restore_initial_prompt,
                    *restore_tool_results,
                )),
                _ => None,
            })
        else {
            tracing::warn!(
                "generated session document authority returned no consumed-input restore decision"
            );
            return;
        };
        if restore_first_turn_pending {
            self.restore_initial_turn_pending();
        }
        if restore_initial_prompt && self.pending_initial_prompt.is_none() {
            self.pending_initial_prompt = consumed.pending_initial_prompt;
        }
        if restore_tool_results {
            let mut restored = consumed.pending_tool_results;
            restored.extend(std::mem::take(&mut self.pending_tool_results));
            self.pending_tool_results = restored;
        }
    }
}

/// Failure when staging a system-context append request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemContextStageError {
    InvalidRequest(String),
    Conflict {
        key: String,
        existing_text: String,
        existing_source: Option<String>,
    },
}

impl std::fmt::Display for SystemContextStageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(message) => {
                write!(f, "invalid system-context append request: {message}")
            }
            Self::Conflict { key, .. } => {
                write!(
                    f,
                    "system-context append conflict for idempotency key `{key}`"
                )
            }
        }
    }
}

impl std::error::Error for SystemContextStageError {}

/// Mechanical PRESENTATION helper: render a system-context append into the
/// display block string that is concatenated into the model-facing system
/// prompt. This is NOT a decision — it builds the `[Runtime System Context]`
/// label text for OUTPUT only. The authority for which appends to render and
/// whether one is a runtime steer lives in the
/// [`session_document::SessionDocumentMachine`]; this function never inspects
/// the `source` string to classify anything.
fn render_system_context_block(append: &PendingSystemContextAppend) -> String {
    let mut rendered = String::from(SYSTEM_CONTEXT_RENDER_LABEL);
    if let Some(source) = &append.source {
        rendered.push_str("\nsource: ");
        rendered.push_str(source);
    }
    rendered.push_str("\n\n");
    rendered.push_str(&append.text);
    rendered
}

/// Display label prefix for a rendered runtime system-context block.
///
/// PRESENTATION only — this is the human/model-facing heading, not a
/// classification key. Nothing reads this back to make a semantic decision.
const SYSTEM_CONTEXT_RENDER_LABEL: &str = "[Runtime System Context]";

/// Shell adapter that drives the canonical
/// [`session_document::SessionDocumentMachine`] system-context region and
/// mirrors its emitted decisions onto the bulky `SessionSystemContextState`.
///
/// The machine owns every SEMANTIC decision (append disposition, per-append
/// apply/discard from the typed [`SystemContextSource`] marker, snapshot
/// restore legality). This module performs only the mechanical collection
/// work — iterating the shell's pending/applied/seen collections and applying
/// the machine's per-item verdict. It never decides; in particular it never
/// inspects a `source` string to classify a runtime steer.
mod system_context_authority {
    use super::{
        AppendSystemContextRequest, BTreeSet, PendingSystemContextAppend, SeenSystemContextKey,
        SeenSystemContextState, SessionSystemContextState, SystemContextSource,
        SystemContextStageError, SystemTime, render_system_context_block, session_document,
        usize_to_u64,
    };
    use crate::service::AppendSystemContextStatus;

    fn document_authority() -> session_document::SessionDocumentMachineAuthority {
        session_document::SessionDocumentMachineAuthority::new()
    }

    /// Resolve the four-way append disposition through the machine.
    fn resolve_append_decision(
        trimmed_text_byte_count: u64,
        idempotency_key_present: bool,
        existing_key_matches: bool,
        existing_key_conflicts: bool,
        active_turn_scoped: bool,
    ) -> Result<session_document::SystemContextAppendDecision, SystemContextStageError> {
        let mut authority = document_authority();
        let effects = authority
            .resolve_system_context_append(
                trimmed_text_byte_count,
                idempotency_key_present,
                existing_key_matches,
                existing_key_conflicts,
                active_turn_scoped,
            )
            .map_err(|err| SystemContextStageError::InvalidRequest(err.to_string()))?;
        effects
            .into_iter()
            .find_map(|effect| match effect {
                session_document::SessionDocumentEffect::SystemContextAppendResolved {
                    decision,
                    ..
                } => Some(decision),
                _ => None,
            })
            .ok_or_else(|| {
                SystemContextStageError::InvalidRequest(
                    "generated session document authority returned no append decision".to_string(),
                )
            })
    }

    /// Per-pending-append apply verdict, decided by the machine from the typed
    /// `source_kind` marker (NOT a `source` string prefix).
    fn pending_apply_item(source_kind: SystemContextSource) -> Option<(bool, bool, bool)> {
        let mut authority = document_authority();
        match authority.resolve_system_context_pending_apply_item(source_kind.into()) {
            Ok(effects) => effects.into_iter().find_map(|effect| {
                match effect {
                session_document::SessionDocumentEffect::SystemContextPendingApplyItemResolved {
                    promote_to_applied,
                    mark_seen_applied,
                    remove_seen,
                } => Some((promote_to_applied, mark_seen_applied, remove_seen)),
                _ => None,
            }
            }),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated session document authority rejected system-context apply item"
                );
                None
            }
        }
    }

    /// Per-item transient-steer discard verdict, decided by the machine from
    /// the typed `source_kind` marker.
    fn steer_cleanup_discards(source_kind: SystemContextSource) -> bool {
        let mut authority = document_authority();
        match authority.resolve_system_context_steer_cleanup_item(source_kind.into()) {
            Ok(effects) => effects
                .into_iter()
                .find_map(|effect| {
                    match effect {
                    session_document::SessionDocumentEffect::SystemContextSteerCleanupItemResolved {
                        discard,
                    } => Some(discard),
                    _ => None,
                }
                })
                .unwrap_or(false),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated session document authority rejected system-context steer cleanup item"
                );
                false
            }
        }
    }

    pub(super) fn restore_system_context_state(
        state: SessionSystemContextState,
    ) -> Result<SessionSystemContextState, SystemContextStageError> {
        let active_keys_have_known_pending_or_seen =
            state.active_turn_pending_keys.iter().all(|key| {
                state.seen.contains_key(key)
                    || state
                        .pending
                        .iter()
                        .any(|append| append.idempotency_key.as_ref() == Some(key))
            });
        let seen_keys_match_known_appends = state.seen.iter().all(|(key, seen)| {
            state
                .pending
                .iter()
                .chain(state.applied.iter())
                .any(|append| {
                    append.idempotency_key.as_ref() == Some(key)
                        && seen.text == append.text
                        && seen.source.as_deref() == append.source.as_deref()
                })
        });
        let mut authority = document_authority();
        authority
            .restore_system_context_snapshot(
                active_keys_have_known_pending_or_seen,
                seen_keys_match_known_appends,
            )
            .map_err(|err| SystemContextStageError::InvalidRequest(err.to_string()))?;
        Ok(state)
    }

    pub(super) fn stage_append(
        state: &mut SessionSystemContextState,
        req: &AppendSystemContextRequest,
        accepted_at: SystemTime,
        active_turn_scoped: bool,
    ) -> Result<AppendSystemContextStatus, SystemContextStageError> {
        let text = req.text.trim();
        let existing = req
            .idempotency_key
            .as_ref()
            .and_then(|key| state.seen.get(key));
        let existing_key_matches = existing.is_some_and(|existing| {
            existing.text == text && existing.source.as_deref() == req.source.as_deref()
        });
        let existing_key_conflicts = existing.is_some() && !existing_key_matches;
        let decision = resolve_append_decision(
            usize_to_u64(text.len()),
            req.idempotency_key.is_some(),
            existing_key_matches,
            existing_key_conflicts,
            active_turn_scoped,
        )?;

        match decision {
            session_document::SystemContextAppendDecision::RejectEmpty => {
                return Err(SystemContextStageError::InvalidRequest(
                    "system context text must not be empty".to_string(),
                ));
            }
            session_document::SystemContextAppendDecision::RejectConflict => {
                let Some(key) = req.idempotency_key.as_ref() else {
                    return Err(SystemContextStageError::InvalidRequest(
                        "generated system-context authority rejected append without a key"
                            .to_string(),
                    ));
                };
                let Some(existing) = existing else {
                    return Err(SystemContextStageError::InvalidRequest(
                        "generated system-context authority rejected append without a conflict"
                            .to_string(),
                    ));
                };
                return Err(SystemContextStageError::Conflict {
                    key: key.clone(),
                    existing_text: existing.text.clone(),
                    existing_source: existing.source.clone(),
                });
            }
            session_document::SystemContextAppendDecision::Duplicate => {
                return Ok(AppendSystemContextStatus::Duplicate);
            }
            session_document::SystemContextAppendDecision::Staged => {}
        }

        let append = PendingSystemContextAppend {
            text: text.to_string(),
            source: req.source.clone(),
            idempotency_key: req.idempotency_key.clone(),
            source_kind: req.source_kind,
            accepted_at,
        };
        if let Some(key) = req.idempotency_key.as_ref() {
            state.seen.insert(
                key.clone(),
                SeenSystemContextKey {
                    text: append.text.clone(),
                    source: append.source.clone(),
                    source_kind: append.source_kind,
                    state: SeenSystemContextState::Pending,
                },
            );
        }
        if active_turn_scoped && let Some(key) = req.idempotency_key.as_ref() {
            state.active_turn_pending_keys.insert(key.clone());
        }
        state.pending.push(append);
        Ok(AppendSystemContextStatus::Staged)
    }

    pub(super) fn mark_pending_applied(state: &mut SessionSystemContextState) {
        // Promote pending appends to applied per the machine's per-item
        // verdict (keyed on the typed `source_kind`).
        let pending = std::mem::take(&mut state.pending);
        let mut seen_to_remove = Vec::new();
        for append in &pending {
            let Some((promote_to_applied, mark_seen_applied, remove_seen)) =
                pending_apply_item(append.source_kind)
            else {
                continue;
            };
            if promote_to_applied && !state.applied.contains(append) {
                state.applied.push(append.clone());
            }
            if let Some(key) = append.idempotency_key.as_ref() {
                if remove_seen {
                    seen_to_remove.push(key.clone());
                } else if mark_seen_applied && let Some(seen) = state.seen.get_mut(key) {
                    seen.state = SeenSystemContextState::Applied;
                }
            }
        }
        for key in seen_to_remove {
            state.seen.remove(&key);
        }
        state.active_turn_pending_keys.clear();
    }

    pub(super) fn discard_unapplied_active_turn_pending(
        state: &mut SessionSystemContextState,
    ) -> Vec<PendingSystemContextAppend> {
        if state.active_turn_pending_keys.is_empty() {
            return Vec::new();
        }
        let active_keys = std::mem::take(&mut state.active_turn_pending_keys);
        let mut discarded = Vec::new();
        state.pending.retain(|append| {
            let should_discard = append
                .idempotency_key
                .as_ref()
                .is_some_and(|key| active_keys.contains(key));
            if should_discard {
                discarded.push(append.clone());
            }
            !should_discard
        });

        for append in &discarded {
            if let Some(key) = append.idempotency_key.as_ref()
                && state
                    .seen
                    .get(key)
                    .is_some_and(|seen| seen.state == SeenSystemContextState::Pending)
            {
                state.seen.remove(key);
            }
        }

        discarded
    }

    pub(super) fn discard_active_turn_pending_by_keys(
        state: &mut SessionSystemContextState,
        idempotency_keys: &[String],
    ) -> Vec<PendingSystemContextAppend> {
        if idempotency_keys.is_empty() || state.active_turn_pending_keys.is_empty() {
            return Vec::new();
        }
        let requested_keys: BTreeSet<&str> = idempotency_keys.iter().map(String::as_str).collect();
        let mut discarded = Vec::new();
        let mut discarded_keys = Vec::new();
        state.pending.retain(|append| {
            let should_discard = append.idempotency_key.as_ref().is_some_and(|key| {
                requested_keys.contains(key.as_str())
                    && state.active_turn_pending_keys.contains(key)
            });
            if should_discard {
                if let Some(key) = append.idempotency_key.as_ref() {
                    discarded_keys.push(key.clone());
                }
                discarded.push(append.clone());
            }
            !should_discard
        });

        for key in discarded_keys {
            state.active_turn_pending_keys.remove(&key);
            if state
                .seen
                .get(&key)
                .is_some_and(|seen| seen.state == SeenSystemContextState::Pending)
            {
                state.seen.remove(&key);
            }
        }

        discarded
    }

    pub(super) fn discard_transient_runtime_steer_state(
        state: &mut SessionSystemContextState,
    ) -> usize {
        let mut removed = 0usize;

        let before_pending = state.pending.len();
        state
            .pending
            .retain(|append| !steer_cleanup_discards(append.source_kind));
        removed += before_pending.saturating_sub(state.pending.len());

        let before_applied = state.applied.len();
        state
            .applied
            .retain(|append| !steer_cleanup_discards(append.source_kind));
        removed += before_applied.saturating_sub(state.applied.len());

        let before_seen = state.seen.len();
        state
            .seen
            .retain(|_key, seen| !steer_cleanup_discards(seen.source_kind));
        removed += before_seen.saturating_sub(state.seen.len());

        // Active-turn keys are tracked only by idempotency key, so an active
        // key is a runtime steer iff its seen entry (or pending append) was.
        // Recompute the surviving steer keys from the typed seen markers.
        let before_active = state.active_turn_pending_keys.len();
        let steer_keys: BTreeSet<String> = state
            .seen
            .iter()
            .filter(|(_key, seen)| steer_cleanup_discards(seen.source_kind))
            .map(|(key, _seen)| key.clone())
            .collect();
        // Any active key whose seen entry was already removed above (because it
        // was a steer) is no longer present in `seen`; drop those, plus any
        // still-present steer keys.
        state
            .active_turn_pending_keys
            .retain(|key| state.seen.contains_key(key) && !steer_keys.contains(key));
        removed += before_active.saturating_sub(state.active_turn_pending_keys.len());

        removed
    }

    pub(super) fn remove_runtime_steer_blocks_for_rendered(
        system_prompt: &str,
        runtime_steer_appends: &[PendingSystemContextAppend],
    ) -> (String, usize) {
        if runtime_steer_appends.is_empty() {
            return (system_prompt.to_string(), 0);
        }
        // Build the set of rendered blocks for the typed runtime-steer appends,
        // then remove those exact rendered blocks from the prompt. The typed
        // marker is the authority; rendering is mechanical presentation.
        let steer_blocks: BTreeSet<String> = runtime_steer_appends
            .iter()
            .map(render_system_context_block)
            .collect();
        let parts = system_prompt
            .split(super::SYSTEM_CONTEXT_SEPARATOR)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let original_len = parts.len();
        let retained = parts
            .into_iter()
            .filter(|part| !steer_blocks.contains(part))
            .collect::<Vec<_>>();
        let removed = original_len.saturating_sub(retained.len());
        (retained.join(super::SYSTEM_CONTEXT_SEPARATOR), removed)
    }

    pub(super) fn record_applied_system_context_blocks(
        state: &mut SessionSystemContextState,
        appends: &[PendingSystemContextAppend],
        current_system_prompt: &str,
    ) -> Vec<PendingSystemContextAppend> {
        let mut new_appends: Vec<PendingSystemContextAppend> = Vec::new();
        for append in appends {
            if append.text.trim().is_empty() {
                continue;
            }
            let rendered = render_system_context_block(append);
            if let Some(key) = append.idempotency_key.as_ref() {
                if let Some(existing) = state.seen.get(key)
                    && !seen_system_context_matches(existing, append)
                {
                    tracing::warn!(
                        idempotency_key = %key,
                        "skipping conflicting runtime system-context append"
                    );
                    continue;
                }
                if let Some(existing) = state
                    .applied
                    .iter()
                    .find(|applied| applied.idempotency_key.as_ref() == Some(key))
                    && !pending_system_context_matches(existing, append)
                {
                    tracing::warn!(
                        idempotency_key = %key,
                        "skipping conflicting runtime system-context append"
                    );
                    continue;
                }
                if let Some(existing) = new_appends
                    .iter()
                    .find(|pending| pending.idempotency_key.as_ref() == Some(key))
                {
                    if !pending_system_context_matches(existing, append) {
                        tracing::warn!(
                            idempotency_key = %key,
                            "skipping conflicting runtime system-context append"
                        );
                    }
                    continue;
                }
                if current_system_prompt.contains(&rendered) {
                    record_applied_append(state, append);
                    continue;
                }
            } else if new_appends.contains(append) || current_system_prompt.contains(&rendered) {
                continue;
            }
            record_applied_append(state, append);
            new_appends.push(append.clone());
        }
        new_appends
    }

    fn record_applied_append(
        state: &mut SessionSystemContextState,
        append: &PendingSystemContextAppend,
    ) {
        if let Some(key) = append.idempotency_key.as_ref() {
            state.seen.insert(
                key.clone(),
                SeenSystemContextKey {
                    text: append.text.clone(),
                    source: append.source.clone(),
                    source_kind: append.source_kind,
                    state: SeenSystemContextState::Applied,
                },
            );
            if state
                .applied
                .iter()
                .any(|applied| applied.idempotency_key.as_ref() == Some(key))
            {
                return;
            }
        } else if state.applied.contains(append) {
            return;
        }
        state.applied.push(append.clone());
    }

    fn seen_system_context_matches(
        seen: &SeenSystemContextKey,
        append: &PendingSystemContextAppend,
    ) -> bool {
        seen.text == append.text && seen.source.as_deref() == append.source.as_deref()
    }

    fn pending_system_context_matches(
        existing: &PendingSystemContextAppend,
        append: &PendingSystemContextAppend,
    ) -> bool {
        existing.text == append.text && existing.source.as_deref() == append.source.as_deref()
    }
}

impl Session {
    /// Create a new empty session
    pub fn new() -> Self {
        let now = SystemTime::now();
        Self {
            version: session_version(),
            id: SessionId::new(),
            messages: Arc::new(Vec::new()),
            created_at: now,
            updated_at: now,
            metadata: serde_json::Map::new(),
            usage: Usage::default(),
        }
    }

    /// Create a session with a specific ID (for loading)
    pub fn with_id(id: SessionId) -> Self {
        let mut session = Self::new();
        session.id = id;
        session
    }

    /// Get the session ID
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Get the session version
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Get all messages.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Replace the message buffer for core-owned internal transcript rewrites.
    ///
    /// Intentionally `pub(crate)`: cross-crate consumers must route same-session
    /// rewrites through transcript-edit APIs so the revision graph remains the
    /// semantic owner of message history.
    pub(crate) fn replace_messages_internal(
        &mut self,
        messages: Vec<Message>,
        reason: TranscriptRewriteReason,
    ) -> Result<Option<TranscriptRewriteCommit>, TranscriptEditError> {
        if transcript_messages_digest(self.messages()).ok()
            == transcript_messages_digest(&messages).ok()
        {
            return Ok(None);
        }
        let commit = self.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: 0,
                end: self.messages.len(),
            },
            messages,
            reason,
            Some("meerkat-core".to_string()),
            None,
        )?;
        Ok(Some(commit))
    }

    /// Retain messages for core-owned synthetic-notice projection cleanup.
    pub(crate) fn retain_messages_internal<F>(
        &mut self,
        mut retain: F,
        reason: TranscriptRewriteReason,
    ) -> Result<Option<TranscriptRewriteCommit>, TranscriptEditError>
    where
        F: FnMut(&Message) -> bool,
    {
        let retained = self
            .messages
            .iter()
            .filter(|message| retain(message))
            .cloned()
            .collect::<Vec<_>>();
        if retained.len() == self.messages.len()
            && transcript_messages_digest(self.messages()).ok()
                == transcript_messages_digest(&retained).ok()
        {
            return Ok(None);
        }
        self.replace_messages_internal(retained, reason)
    }

    /// Get creation time
    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }

    /// Get last update time
    pub fn updated_at(&self) -> SystemTime {
        self.updated_at
    }

    /// Add a message to the session
    ///
    /// Updates the timestamp. For adding multiple messages, prefer `push_batch`.
    pub fn push(&mut self, message: Message) {
        Arc::make_mut(&mut self.messages).push(message);
        self.updated_at = SystemTime::now();
        self.refresh_transcript_head_after_message_mutation();
    }

    /// Add multiple messages in one operation (single timestamp update)
    ///
    /// More efficient than multiple `push` calls when adding many messages.
    pub fn push_batch(&mut self, messages: Vec<Message>) {
        if messages.is_empty() {
            return;
        }
        let inner = Arc::make_mut(&mut self.messages);
        inner.extend(messages);
        self.updated_at = SystemTime::now();
        self.refresh_transcript_head_after_message_mutation();
    }

    /// Rewrite inline media payloads in-place as `BlobRef` pointers.
    ///
    /// Message count is invariant across this operation — `externalize`
    /// only swaps inline image/media bytes for opaque blob references.
    /// This is the cross-crate-legitimate rewrite operation that used
    /// to require public `messages_mut()`; post-C-H1 callers in
    /// `meerkat-session` go through this typed method.
    ///
    /// Does not touch `updated_at` — externalization is bookkeeping, not
    /// a semantic session mutation.
    pub async fn externalize_media(
        &mut self,
        blob_store: &dyn crate::BlobStore,
        start: usize,
    ) -> Result<(), crate::blob::BlobStoreError> {
        let previous_digest = if self
            .metadata
            .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
        {
            transcript_messages_digest(self.messages()).ok()
        } else {
            None
        };
        let messages = Arc::make_mut(&mut self.messages);
        crate::image_content::externalize_messages_from(blob_store, messages, start).await?;
        if let Some(previous_digest) = previous_digest
            && transcript_messages_digest(self.messages()).ok().as_ref() != Some(&previous_digest)
        {
            self.refresh_transcript_head_after_message_mutation();
        }
        Ok(())
    }

    /// Explicitly update the timestamp
    ///
    /// Call this after bulk operations that don't update timestamps automatically.
    pub fn touch(&mut self) {
        self.updated_at = SystemTime::now();
    }

    /// Get the last N messages
    pub fn last_n(&self, n: usize) -> &[Message] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Count total tokens used.
    pub fn total_tokens(&self) -> u64 {
        self.usage.total_tokens()
    }

    /// Get total usage statistics for the session.
    pub fn total_usage(&self) -> Usage {
        self.usage.clone()
    }

    /// Update cumulative usage after an LLM call.
    pub fn record_usage(&mut self, turn_usage: Usage) {
        self.usage.add(&turn_usage);
        self.updated_at = SystemTime::now();
    }

    /// Append externally-produced user content to the canonical transcript.
    pub fn append_external_user_content(&mut self, content: ContentInput) {
        self.push(Message::User(UserMessage::with_blocks(
            content.into_blocks(),
        )));
    }

    /// Append externally-produced assistant output to the canonical transcript.
    pub fn append_external_assistant_blocks(
        &mut self,
        blocks: Vec<AssistantBlock>,
        stop_reason: StopReason,
        usage: Usage,
    ) {
        if !blocks.is_empty() {
            self.push(Message::BlockAssistant(BlockAssistantMessage::new(
                blocks,
                stop_reason,
            )));
        }
        if usage != Usage::default() {
            self.record_usage(usage);
        }
    }

    /// Apply an identity-bearing provider realtime transcript event.
    ///
    /// This is the canonical append authority for provider-managed realtime
    /// turns: provider item ids, predecessor links, and content segment ids are
    /// persisted in session metadata so duplicate websocket delivery,
    /// reconnect replay, and causally equivalent event ordering cannot create
    /// duplicate or misordered canonical messages.
    pub fn append_realtime_transcript_event(
        &mut self,
        event: RealtimeTranscriptEvent,
    ) -> RealtimeTranscriptApplyOutcome {
        let mut state = self.realtime_transcript_state();
        let commit = session_realtime_transcript_authority::apply_realtime_transcript_event(
            &mut state, event,
        )
        .unwrap_or_else(|err| {
            fail_closed_generated_restore(
                "realtime-transcript",
                <serde_json::Error as serde::de::Error>::custom(err),
            )
        });
        self.store_realtime_transcript_state(&state);
        self.push_batch(commit.messages);
        if commit.usage != Usage::default() {
            self.record_usage(commit.usage);
        }
        commit.outcome
    }

    /// Return every distinct provider `response_id` currently staged in the
    /// realtime-transcript metadata that has at least one **unmaterialized**
    /// assistant item and is **not already discarded**.
    ///
    /// CC4 (Round-4 architectural reconciliation): when the live boundary
    /// signals a barge-in (`TurnInterrupted`), the projection sink does not
    /// know which provider response_ids have streaming deltas staged in
    /// session metadata. This accessor lets the sink fan
    /// [`RealtimeTranscriptEvent::AssistantTurnInterrupted`] events out to
    /// each in-flight response so staged-but-not-yet-materialized transcript
    /// fragments are discarded — preventing them from silently committing
    /// when the *next* turn's `AssistantTurnCompleted` (synthesized by the
    /// CC2 fix in `signal_turn_completed`) sweeps the materializer.
    ///
    /// Order is the [`SessionRealtimeTranscriptState::first_seen_order`]
    /// projection so callers see deterministic iteration. Items already
    /// materialized or skipped are excluded — only response_ids with at
    /// least one live unmaterialized assistant item are returned.
    #[must_use]
    pub fn in_flight_realtime_assistant_response_ids(&self) -> Vec<String> {
        let state = self.realtime_transcript_state();
        session_realtime_transcript_authority::in_flight_realtime_assistant_response_ids(&state)
    }

    fn realtime_transcript_state(&self) -> SessionRealtimeTranscriptState {
        match self.try_realtime_transcript_state() {
            Ok(Some(state)) => state,
            Ok(None) => SessionRealtimeTranscriptState::default(),
            Err(err) => fail_closed_generated_restore("realtime-transcript", err),
        }
    }

    fn try_realtime_transcript_state(
        &self,
    ) -> Result<Option<SessionRealtimeTranscriptState>, serde_json::Error> {
        self.metadata
            .get(SESSION_REALTIME_TRANSCRIPT_STATE_KEY)
            .map(|value| {
                let state = serde_json::from_value(value.clone())?;
                session_realtime_transcript_authority::restore_realtime_transcript_state(state)
                    .map_err(<serde_json::Error as serde::de::Error>::custom)
            })
            .transpose()
    }

    fn store_realtime_transcript_state(&mut self, state: &SessionRealtimeTranscriptState) {
        match serde_json::to_value(state) {
            Ok(value) => self.set_metadata_unchecked(SESSION_REALTIME_TRANSCRIPT_STATE_KEY, value),
            Err(error) => {
                tracing::warn!(error = %error, "failed to serialize realtime transcript state");
            }
        }
    }

    fn apply_authorized_system_prompt(
        &mut self,
        prompt: session_durable_config_authority::AuthorizedSystemPrompt,
    ) {
        use crate::types::SystemMessage;

        let (prompt, _replacing_existing) = prompt.into_parts();
        let inner = Arc::make_mut(&mut self.messages);
        // Check if first message is system
        if let Some(Message::System(_)) = inner.first() {
            inner[0] = Message::System(SystemMessage::new(prompt));
        } else {
            inner.insert(0, Message::System(SystemMessage::new(prompt)));
        }
        self.updated_at = SystemTime::now();
        self.refresh_transcript_head_after_message_mutation();
    }

    /// Set a system prompt through generated durable-config authority.
    pub fn set_system_prompt_with_source(
        &mut self,
        prompt: String,
        source: session_durable_config_authority::SessionSystemPromptSource,
    ) -> Result<(), session_durable_config_authority::SessionDurableConfigAuthorityError> {
        let replacing_existing = matches!(self.messages.first(), Some(Message::System(_)));
        let prompt = session_durable_config_authority::authorize_system_prompt_mutation(
            prompt,
            source,
            replacing_existing,
        )?;
        self.apply_authorized_system_prompt(prompt);
        Ok(())
    }

    /// Set a system prompt (adds or replaces System message at start).
    pub fn set_system_prompt(&mut self, prompt: String) {
        if let Err(err) = self.set_system_prompt_with_source(
            prompt,
            session_durable_config_authority::SessionSystemPromptSource::DirectMutation,
        ) {
            tracing::warn!(error = %err, "generated session durable-config authority rejected system prompt mutation");
        }
    }

    /// Remove transient active-turn steer context from persisted session state.
    ///
    /// Operator steers accepted into an already-running turn are request-local:
    /// they should be visible to that turn's next model boundary, then vanish
    /// instead of replaying into later turns after persistence or resume.
    pub fn discard_transient_runtime_steer_context(&mut self) -> usize {
        let mut removed = 0usize;

        let mut state = match self.try_system_context_state() {
            Ok(state) => state.unwrap_or_default(),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated system-context authority rejected runtime steer cleanup state"
                );
                return removed;
            }
        };

        // The typed `source_kind` marker on persisted appends is the authority
        // for which rendered prompt blocks are transient runtime steers. Gather
        // the runtime-steer appends, then remove their exact rendered blocks
        // from the system prompt — no `runtime:steer:` string classification.
        let runtime_steer_appends = state
            .pending
            .iter()
            .chain(state.applied.iter())
            .filter(|append| append.source_kind.is_runtime_steer())
            .cloned()
            .collect::<Vec<_>>();
        if let Some(Message::System(system)) = self.messages.first() {
            let (retained_prompt, removed_blocks) =
                system_context_authority::remove_runtime_steer_blocks_for_rendered(
                    &system.content,
                    &runtime_steer_appends,
                );
            if removed_blocks > 0 {
                removed += removed_blocks;
                if let Err(err) = self.set_system_prompt_with_source(
                    retained_prompt,
                    session_durable_config_authority::SessionSystemPromptSource::RuntimeSteerCleanup,
                ) {
                    tracing::warn!(
                        error = %err,
                        "generated session durable-config authority rejected runtime steer prompt cleanup"
                    );
                }
            }
        }

        removed += system_context_authority::discard_transient_runtime_steer_state(&mut state);

        if removed > 0
            && let Err(err) = self.set_system_context_state(state)
        {
            tracing::warn!(
                error = %err,
                "failed to persist runtime steer context cleanup"
            );
        }

        removed
    }

    /// Append one or more runtime system-context blocks to the canonical system prompt.
    pub fn append_system_context_blocks(&mut self, appends: &[PendingSystemContextAppend]) {
        if appends.is_empty() {
            return;
        }

        let current_system_prompt = self
            .messages
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        let mut state = match self.try_system_context_state() {
            Ok(state) => state.unwrap_or_default(),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "generated system-context authority rejected applied context state"
                );
                return;
            }
        };
        let new_appends = system_context_authority::record_applied_system_context_blocks(
            &mut state,
            appends,
            current_system_prompt,
        );
        if new_appends.is_empty() {
            if let Err(err) = self.set_system_context_state(state) {
                tracing::warn!(error = %err, "failed to persist applied system-context state");
            }
            return;
        }

        let rendered = new_appends
            .iter()
            .map(render_system_context_block)
            .collect::<Vec<_>>()
            .join(SYSTEM_CONTEXT_SEPARATOR);

        let next = match self.messages.first() {
            Some(Message::System(sys)) if !sys.content.is_empty() => {
                format!("{}{}{}", sys.content, SYSTEM_CONTEXT_SEPARATOR, rendered)
            }
            _ => rendered,
        };
        if let Err(err) = self.set_system_prompt_with_source(
            next,
            session_durable_config_authority::SessionSystemPromptSource::RuntimeContextAppend,
        ) {
            tracing::warn!(
                error = %err,
                "generated session durable-config authority rejected system-context prompt append"
            );
            return;
        }
        if let Err(err) = self.set_system_context_state(state) {
            tracing::warn!(error = %err, "failed to persist applied system-context state");
        }
    }

    /// Get the last assistant message text content.
    ///
    /// Concatenates both `Text` (display) and `Transcript` (spoken) blocks
    /// in document order, since both lanes project to the same human-readable
    /// stream. Lane provenance is preserved on the underlying `AssistantBlock`
    /// for callers that need it.
    pub fn last_assistant_text(&self) -> Option<String> {
        self.messages.iter().rev().find_map(|m| match m {
            Message::BlockAssistant(a) => {
                let mut buf = String::new();
                for block in &a.blocks {
                    match block {
                        crate::types::AssistantBlock::Text { text, .. }
                        | crate::types::AssistantBlock::Transcript { text, .. } => {
                            buf.push_str(text);
                        }
                        _ => {}
                    }
                }
                if buf.is_empty() { None } else { Some(buf) }
            }
            Message::Assistant(a) if !a.content.is_empty() => Some(a.content.clone()),
            _ => None,
        })
    }

    /// Count tool calls made
    pub fn tool_call_count(&self) -> usize {
        self.messages
            .iter()
            .filter_map(|m| match m {
                Message::BlockAssistant(a) => Some(
                    a.blocks
                        .iter()
                        .filter(|b| matches!(b, crate::types::AssistantBlock::ToolUse { .. }))
                        .count(),
                ),
                Message::Assistant(a) => Some(a.tool_calls.len()),
                _ => None,
            })
            .sum()
    }

    /// Get metadata
    pub fn metadata(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.metadata
    }

    fn set_metadata_unchecked(&mut self, key: &str, value: serde_json::Value) {
        self.metadata.insert(key.to_string(), value);
        self.updated_at = SystemTime::now();
    }

    #[cfg(test)]
    pub(crate) fn set_metadata_unchecked_for_test(&mut self, key: &str, value: serde_json::Value) {
        self.set_metadata_unchecked(key, value);
    }

    fn fork_metadata_projection(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut metadata = self.metadata.clone();
        metadata.retain(|key, _| !is_session_authority_metadata_key(key));
        metadata
    }

    fn remove_metadata_unchecked(&mut self, key: &str) {
        self.metadata.remove(key);
        self.updated_at = SystemTime::now();
    }

    /// Set a metadata value when the key is not reserved for generated authority.
    pub fn try_set_metadata(
        &mut self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), ReservedSessionMetadataKey> {
        if is_session_authority_metadata_key(key) {
            return Err(ReservedSessionMetadataKey::new(key));
        }
        self.set_metadata_unchecked(key, value);
        Ok(())
    }

    /// Set a metadata value.
    ///
    /// Reserved generated-authority metadata keys fail closed and are left
    /// untouched. Use the typed setters for those keys.
    pub fn set_metadata(&mut self, key: &str, value: serde_json::Value) {
        if let Err(err) = self.try_set_metadata(key, value) {
            tracing::warn!(error = %err, "rejected raw session metadata mutation");
        }
    }

    /// Backfill a missing metadata value without changing `updated_at`.
    ///
    /// This is only for compatibility reads that need to hydrate metadata from
    /// an older projection. Semantic metadata mutations must use
    /// [`Session::set_metadata`] so the session timestamp advances.
    pub fn backfill_metadata_if_absent(&mut self, key: &str, value: serde_json::Value) -> bool {
        if is_session_authority_metadata_key(key) {
            tracing::warn!(
                metadata_key = key,
                "rejected raw session metadata backfill for authority key"
            );
            return false;
        }
        if self.metadata.contains_key(key) {
            false
        } else {
            self.metadata.insert(key.to_string(), value);
            true
        }
    }

    /// Remove a metadata value.
    pub fn remove_metadata(&mut self, key: &str) {
        if is_session_authority_metadata_key(key) {
            tracing::warn!(
                metadata_key = key,
                "rejected raw session metadata removal for authority key"
            );
            return;
        }
        self.metadata.remove(key);
        self.updated_at = SystemTime::now();
    }

    /// Store SessionMetadata in the session metadata map.
    pub fn set_session_metadata(
        &mut self,
        metadata: SessionMetadata,
    ) -> Result<(), serde_json::Error> {
        let metadata =
            session_durable_config_authority::authorize_session_metadata_persist(metadata)
                .map_err(<serde_json::Error as serde::ser::Error>::custom)?
                .into_metadata();
        let value = serde_json::to_value(metadata)?;
        self.set_metadata_unchecked(SESSION_METADATA_KEY, value);
        Ok(())
    }

    /// Load SessionMetadata from the session metadata map.
    ///
    /// If the reserved key exists but cannot pass typed generated restore,
    /// fail closed instead of treating corrupted machine facts as absent.
    pub fn session_metadata(&self) -> Option<SessionMetadata> {
        match self.try_session_metadata() {
            Ok(metadata) => metadata,
            Err(err) => fail_closed_generated_restore("session-metadata", err),
        }
    }

    /// Try to load SessionMetadata through generated restore authority.
    pub fn try_session_metadata(&self) -> Result<Option<SessionMetadata>, serde_json::Error> {
        let Some(value) = self.metadata.get(SESSION_METADATA_KEY) else {
            return Ok(None);
        };
        let mut metadata = serde_json::from_value::<SessionMetadata>(value.clone())?;
        metadata.schema_version =
            session_persistence_version_authority::restore_session_metadata_schema_version(
                metadata.schema_version,
            )
            .map_err(<serde_json::Error as serde::de::Error>::custom)?;
        session_durable_config_authority::restore_session_metadata(metadata)
            .map(Some)
            .map_err(<serde_json::Error as serde::de::Error>::custom)
    }

    /// Store durable system-context control state in the session metadata map.
    pub fn set_system_context_state(
        &mut self,
        state: SessionSystemContextState,
    ) -> Result<(), serde_json::Error> {
        let state = system_context_authority::restore_system_context_state(state)
            .map_err(<serde_json::Error as serde::ser::Error>::custom)?;
        let value = serde_json::to_value(state)?;
        self.set_metadata_unchecked(SESSION_SYSTEM_CONTEXT_STATE_KEY, value);
        Ok(())
    }

    /// Try to load durable system-context control state through generated restore authority.
    pub fn try_system_context_state(
        &self,
    ) -> Result<Option<SessionSystemContextState>, serde_json::Error> {
        self.metadata
            .get(SESSION_SYSTEM_CONTEXT_STATE_KEY)
            .map(|value| {
                let state = serde_json::from_value(value.clone())?;
                system_context_authority::restore_system_context_state(state)
                    .map_err(<serde_json::Error as serde::de::Error>::custom)
            })
            .transpose()
    }

    /// Load durable system-context control state from the session metadata map.
    ///
    /// Rejected durable facts fail closed through the generated restore
    /// authority. Callers that need the typed rejection must use
    /// [`Self::try_system_context_state`].
    pub fn system_context_state(&self) -> Option<SessionSystemContextState> {
        match self.try_system_context_state() {
            Ok(state) => state,
            Err(err) => fail_closed_generated_restore("system-context", err),
        }
    }

    /// Store durable deferred-turn control state in the session metadata map.
    pub fn set_deferred_turn_state(
        &mut self,
        state: SessionDeferredTurnState,
    ) -> Result<(), serde_json::Error> {
        let state = validate_deferred_turn_snapshot(state)
            .map_err(<serde_json::Error as serde::ser::Error>::custom)?;
        let value = serde_json::to_value(state)?;
        self.set_metadata_unchecked(SESSION_DEFERRED_TURN_STATE_KEY, value);
        Ok(())
    }

    /// Try to load durable deferred-turn control state through generated restore authority.
    pub fn try_deferred_turn_state(
        &self,
    ) -> Result<Option<SessionDeferredTurnState>, serde_json::Error> {
        self.metadata
            .get(SESSION_DEFERRED_TURN_STATE_KEY)
            .map(|value| {
                let state = serde_json::from_value(value.clone())?;
                validate_deferred_turn_snapshot(state)
                    .map_err(<serde_json::Error as serde::de::Error>::custom)
            })
            .transpose()
    }

    /// Load durable deferred-turn control state from the session metadata map.
    ///
    /// Rejected durable facts fail closed through the generated restore
    /// authority. Callers that need the typed rejection must use
    /// [`Self::try_deferred_turn_state`].
    pub fn deferred_turn_state(&self) -> Option<SessionDeferredTurnState> {
        match self.try_deferred_turn_state() {
            Ok(state) => state,
            Err(err) => fail_closed_generated_restore("deferred-turn", err),
        }
    }

    /// Store recoverable build-only session state in the session metadata map.
    pub fn set_build_state(&mut self, state: SessionBuildState) -> Result<(), serde_json::Error> {
        let state = session_durable_config_authority::authorize_session_build_state_persist(state)
            .map_err(<serde_json::Error as serde::ser::Error>::custom)?
            .into_state();
        let value = serde_json::to_value(state)?;
        self.set_metadata_unchecked(SESSION_BUILD_STATE_KEY, value);
        Ok(())
    }

    /// Load recoverable build-only session state from the session metadata map.
    ///
    /// If the reserved key exists but cannot pass typed generated restore,
    /// fail closed instead of treating corrupted machine facts as absent.
    pub fn build_state(&self) -> Option<SessionBuildState> {
        match self.try_build_state() {
            Ok(state) => state,
            Err(err) => fail_closed_generated_restore("session-build-state", err),
        }
    }

    /// Try to load recoverable build-only session state through generated restore authority.
    pub fn try_build_state(&self) -> Result<Option<SessionBuildState>, serde_json::Error> {
        let Some(value) = self.metadata.get(SESSION_BUILD_STATE_KEY) else {
            return Ok(None);
        };
        let state = serde_json::from_value::<SessionBuildState>(value.clone())?;
        session_durable_config_authority::restore_session_build_state(state)
            .map(Some)
            .map_err(<serde_json::Error as serde::de::Error>::custom)
    }

    /// Store durable tool-visibility control state in the session metadata map.
    pub fn set_tool_visibility_state(
        &mut self,
        state: AuthorizedSessionToolVisibilityState,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(state.into_state())?;
        self.set_metadata_unchecked(SESSION_TOOL_VISIBILITY_STATE_KEY, value);
        Ok(())
    }

    /// Test-only metadata clear for compatibility assertions.
    ///
    /// Production paths persist an explicit generated-authority projection
    /// rather than making durable absence carry semantic default truth.
    #[cfg(test)]
    pub(crate) fn clear_tool_visibility_state(&mut self) {
        self.remove_metadata_unchecked(SESSION_TOOL_VISIBILITY_STATE_KEY);
    }

    /// Load durable tool-visibility control state from the session metadata map.
    pub fn tool_visibility_state(
        &self,
    ) -> Result<Option<SessionToolVisibilityState>, serde_json::Error> {
        self.try_tool_visibility_state()
    }

    /// Load durable tool-visibility control state while distinguishing absent
    /// metadata from malformed canonical metadata.
    pub fn try_tool_visibility_state(
        &self,
    ) -> Result<Option<SessionToolVisibilityState>, serde_json::Error> {
        self.metadata
            .get(SESSION_TOOL_VISIBILITY_STATE_KEY)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
    }

    /// Load typed transcript revision state from metadata.
    pub fn transcript_history_state(
        &self,
    ) -> Result<Option<TranscriptHistoryState>, serde_json::Error> {
        self.metadata
            .get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
    }

    /// Validate the retained transcript revision graph, when present.
    pub fn validate_transcript_history_state(&self) -> Result<(), TranscriptEditError> {
        let Some(state) = self
            .transcript_history_state()
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?
        else {
            return Ok(());
        };
        validate_transcript_history_state(&state)
    }

    /// Clear retained transcript revision metadata after a caller has
    /// materialized the desired message projection.
    pub fn clear_transcript_history_state(&mut self) {
        self.remove_metadata_unchecked(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    }

    /// Return the retained immutable body for a transcript revision.
    pub fn transcript_revision_body(
        &self,
        revision: &str,
    ) -> Result<Option<TranscriptRevisionBody>, serde_json::Error> {
        Ok(self.transcript_history_state()?.and_then(|state| {
            state
                .revisions
                .into_iter()
                .find(|body| body.revision == revision)
        }))
    }

    /// Return the ordered messages for a retained transcript revision.
    pub fn transcript_revision_messages(
        &self,
        revision: &str,
    ) -> Result<Option<Vec<Message>>, serde_json::Error> {
        Ok(self
            .transcript_revision_body(revision)?
            .map(|body| body.messages))
    }

    /// Materialize this session projection from a typed transcript history graph.
    pub fn apply_transcript_history_state(
        &mut self,
        state: TranscriptHistoryState,
    ) -> Result<(), TranscriptEditError> {
        validate_transcript_history_state(&state)?;
        let head_body = state
            .revisions
            .iter()
            .find(|body| body.revision == state.head)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "missing transcript head body {}",
                    state.head
                ))
            })?
            .clone();
        let value = serde_json::to_value(&state)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        self.set_metadata_unchecked(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, value);
        let mut updated_at = head_body.created_at;
        for commit in &state.commits {
            if commit.committed_at > updated_at {
                updated_at = commit.committed_at;
            }
        }
        self.messages = Arc::new(head_body.messages);
        self.updated_at = updated_at;
        Ok(())
    }

    /// Current transcript head revision. Rows written before transcript
    /// revisions derive their implicit head from the current message snapshot.
    pub fn transcript_revision(&self) -> Result<String, serde_json::Error> {
        if let Some(state) = self.transcript_history_state()? {
            Ok(state.head)
        } else {
            transcript_messages_digest(self.messages())
        }
    }

    /// Commit a same-session transcript rewrite and advance the transcript head.
    pub fn commit_transcript_rewrite(
        &mut self,
        selection: TranscriptRewriteSelection,
        replacement: Vec<Message>,
        reason: TranscriptRewriteReason,
        actor: Option<String>,
        expected_parent_revision: Option<String>,
    ) -> Result<TranscriptRewriteCommit, TranscriptEditError> {
        let parent_revision = self
            .transcript_revision()
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        if let Some(expected) = expected_parent_revision
            && expected != parent_revision
        {
            return Err(TranscriptEditError::RevisionConflict {
                expected,
                actual: parent_revision,
            });
        }

        let (start, end) = selection.bounds();
        let message_count = self.messages.len();
        if start > end || end > message_count {
            return Err(TranscriptEditError::InvalidRewriteRange {
                start,
                end,
                message_count,
            });
        }

        let replacement_len = replacement.len();
        let mut rewritten = Vec::with_capacity(
            start
                .saturating_add(replacement_len)
                .saturating_add(message_count.saturating_sub(end)),
        );
        rewritten.extend_from_slice(&self.messages[..start]);
        rewritten.extend(replacement);
        rewritten.extend_from_slice(&self.messages[end..]);
        validate_transcript_tool_result_shape(&rewritten)?;

        let original_span_digest = sha256_json_digest(&self.messages[start..end])
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        let replacement_digest = sha256_json_digest(&rewritten[start..start + replacement_len])
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        let revision = transcript_messages_digest(&rewritten)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        if revision == parent_revision {
            return Err(TranscriptEditError::NoOpRewrite { revision });
        }

        let commit = TranscriptRewriteCommit {
            parent_revision,
            revision: revision.clone(),
            selection,
            original_span_digest,
            replacement_digest,
            messages_before: message_count,
            messages_after: rewritten.len(),
            reason,
            actor,
            committed_at: SystemTime::now(),
        };

        let mut state = self
            .transcript_history_state()
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?
            .unwrap_or_else(|| TranscriptHistoryState {
                head: commit.parent_revision.clone(),
                commits: Vec::new(),
                revisions: Vec::new(),
            });
        if !state
            .revisions
            .iter()
            .any(|body| body.revision == commit.parent_revision)
        {
            state.revisions.push(TranscriptRevisionBody {
                revision: commit.parent_revision.clone(),
                parent_revision: None,
                messages: self.messages().to_vec(),
                created_at: self.updated_at,
            });
        }
        if !state
            .revisions
            .iter()
            .any(|body| body.revision == commit.revision)
        {
            state.revisions.push(TranscriptRevisionBody {
                revision: commit.revision.clone(),
                parent_revision: Some(commit.parent_revision.clone()),
                messages: rewritten.clone(),
                created_at: commit.committed_at,
            });
        }
        state.head = revision;
        state.commits.push(commit.clone());
        let value = serde_json::to_value(state)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        self.set_metadata_unchecked(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, value);

        self.messages = Arc::new(rewritten);
        self.updated_at = SystemTime::now();
        Ok(commit)
    }

    fn refresh_transcript_head_after_message_mutation(&mut self) {
        if !self
            .metadata
            .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
        {
            return;
        }
        let Ok(Some(mut state)) = self.transcript_history_state() else {
            tracing::warn!(
                session_id = %self.id,
                "transcript history state is malformed; leaving head unchanged after message mutation"
            );
            return;
        };
        let Ok(head) = transcript_messages_digest(self.messages()) else {
            tracing::warn!(
                session_id = %self.id,
                "failed to digest transcript after message mutation; leaving head unchanged"
            );
            return;
        };
        let previous_head = state.head.clone();
        if !state.revisions.iter().any(|body| body.revision == head) {
            state.revisions.push(TranscriptRevisionBody {
                revision: head.clone(),
                parent_revision: Some(previous_head),
                messages: self.messages().to_vec(),
                created_at: SystemTime::now(),
            });
        }
        state.head = head;
        match serde_json::to_value(state) {
            Ok(value) => self.set_metadata_unchecked(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, value),
            Err(error) => {
                tracing::warn!(
                    session_id = %self.id,
                    error = %error,
                    "failed to serialize transcript history state after message mutation"
                );
            }
        }
    }

    /// Store typed mob operator authority inside canonical build-state metadata.
    ///
    /// Store the mob operator authority projection inside build-state metadata.
    ///
    /// The projection is durable compatibility data only: serialization drops
    /// the generated authority seal, so behavior must re-enter generated
    /// authority before using restored facts.
    pub fn set_mob_tool_authority_context(
        &mut self,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), serde_json::Error> {
        if let Some(authority_context) = authority_context.as_ref()
            && !authority_context.is_generated_authority_context()
        {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "mob authority context was not minted by generated authority",
            ));
        }
        let mut build_state = self.build_state().ok_or_else(|| {
            <serde_json::Error as serde::de::Error>::custom(format!(
                "session {} is missing session build state",
                self.id
            ))
        })?;
        build_state.mob_tool_authority_context = authority_context;
        self.set_build_state(build_state)
    }

    /// Load the in-memory generated mob operator authority, if still present.
    ///
    /// Stored/deserialized contexts deliberately fail this check and are not
    /// returned as behavior authority.
    pub fn mob_tool_authority_context(&self) -> Option<MobToolAuthorityContext> {
        self.build_state()
            .and_then(|state| state.mob_tool_authority_context)
            .filter(MobToolAuthorityContext::is_generated_authority_context)
    }

    /// Fork the session at a specific message index
    ///
    /// Creates a new session with a subset of messages. The messages are copied
    /// (not shared) since the new session has a different prefix.
    pub fn fork_at(&self, index: usize) -> Self {
        let now = SystemTime::now();
        let truncated = self.messages[..index.min(self.messages.len())].to_vec();
        Self {
            version: session_version(),
            id: SessionId::new(),
            messages: Arc::new(truncated),
            created_at: now,
            updated_at: now,
            metadata: self.fork_metadata_projection(),
            usage: self.usage.clone(),
        }
    }

    /// Fork the session and replace the message at `message_index`.
    ///
    /// The returned session contains the original prefix before
    /// `message_index`, followed by the typed replacement. Later source
    /// messages are intentionally omitted so follow-up work continues from the
    /// edited branch rather than replaying stale descendants.
    pub fn fork_replacing(
        &self,
        message_index: usize,
        replacement: TranscriptReplacement,
    ) -> Result<Self, TranscriptEditError> {
        let Some(original) = self.messages.get(message_index) else {
            return Err(TranscriptEditError::MessageIndexOutOfBounds {
                message_index,
                message_count: self.messages.len(),
            });
        };

        let replacement_message = match replacement {
            TranscriptReplacement::Message { message } => message,
            TranscriptReplacement::UserContentBlock { block_index, block } => {
                let Message::User(user) = original else {
                    return Err(TranscriptEditError::MessageRoleMismatch {
                        message_index,
                        expected: "user",
                        actual: message_role_name(original),
                    });
                };
                if block_index >= user.content.len() {
                    return Err(TranscriptEditError::BlockIndexOutOfBounds {
                        block_kind: "user content block",
                        block_index,
                        block_count: user.content.len(),
                    });
                }
                let mut edited = user.clone();
                edited.content[block_index] = block;
                Message::User(edited)
            }
            TranscriptReplacement::AssistantBlock { block_index, block } => {
                let Message::BlockAssistant(assistant) = original else {
                    return Err(TranscriptEditError::MessageRoleMismatch {
                        message_index,
                        expected: "block_assistant",
                        actual: message_role_name(original),
                    });
                };
                if block_index >= assistant.blocks.len() {
                    return Err(TranscriptEditError::BlockIndexOutOfBounds {
                        block_kind: "assistant block",
                        block_index,
                        block_count: assistant.blocks.len(),
                    });
                }
                let mut edited = assistant.clone();
                edited.blocks[block_index] = block;
                Message::BlockAssistant(edited)
            }
            TranscriptReplacement::ToolResultContentBlock {
                result_index,
                block_index,
                block,
            } => {
                let Message::ToolResults {
                    results,
                    created_at,
                } = original
                else {
                    return Err(TranscriptEditError::MessageRoleMismatch {
                        message_index,
                        expected: "tool_results",
                        actual: message_role_name(original),
                    });
                };
                let Some(result) = results.get(result_index) else {
                    return Err(TranscriptEditError::BlockIndexOutOfBounds {
                        block_kind: "tool result",
                        block_index: result_index,
                        block_count: results.len(),
                    });
                };
                if block_index >= result.content.len() {
                    return Err(TranscriptEditError::BlockIndexOutOfBounds {
                        block_kind: "tool result content block",
                        block_index,
                        block_count: result.content.len(),
                    });
                }
                let mut edited_results = results.clone();
                edited_results[result_index].content[block_index] = block;
                Message::ToolResults {
                    results: edited_results,
                    created_at: *created_at,
                }
            }
        };

        let mut forked = self.fork_at(message_index);
        forked.push(replacement_message);
        Ok(forked)
    }

    /// Fork the entire session (full history)
    ///
    /// This is O(1) - the new session shares the message buffer via Arc.
    /// Copy-on-write occurs when either session mutates its messages.
    pub fn fork(&self) -> Self {
        let now = SystemTime::now();
        Self {
            version: session_version(),
            id: SessionId::new(),
            messages: Arc::clone(&self.messages),
            created_at: now,
            updated_at: now,
            metadata: self.fork_metadata_projection(),
            usage: self.usage.clone(),
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary metadata for listing sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionMeta {
    pub id: SessionId,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub total_tokens: u64,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// Metadata required to reliably resume a session across interfaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionMetadata {
    /// Per-entity schema version byte.
    ///
    /// Defaults to `1` on read so pre-wave-c rows without the field
    /// deserialize cleanly; rewritten as `SESSION_METADATA_SCHEMA_VERSION`
    /// on the next `save()` after a successful migration pass.
    #[serde(default = "default_session_metadata_schema_version")]
    pub schema_version: u32,
    pub model: String,
    pub max_tokens: u32,
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_hosted_server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
    pub tooling: SessionTooling,
    #[serde(default)]
    pub keep_alive: bool,
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (populated when comms is enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_meta: Option<PeerMeta>,
    /// Realm identity for cross-surface storage sharing/isolation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    /// Optional process/agent instance identifier within a realm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// Backend pinned by the realm manifest (e.g. "sqlite", "jsonl").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Config generation used when this session was created/resumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_generation: Option<u64>,
    /// Realm-scoped auth binding (Phase 3 provider-auth redesign).
    ///
    /// Persisted intent for the auth/backend binding this session resolved
    /// through. On resume, `apply_resumed_session_metadata` writes this
    /// back into `AgentBuildConfig.auth_binding` so the same realm
    /// binding is re-resolved. Never carries secret material — leases
    /// are rebuilt from the active realm connection set at resume time.
    /// Older persisted sessions without the field deserialize as `None`
    /// (backward compatible via `#[serde(default)]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<crate::AuthBindingRef>,
}

fn default_structured_output_retries() -> u32 {
    2
}

fn default_session_metadata_schema_version() -> u32 {
    session_persistence_version_authority::legacy_session_metadata_schema_version()
}

/// Canonical durable LLM identity for a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SessionLlmIdentity {
    pub model: String,
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_hosted_server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
    /// Realm-scoped auth binding this session resolves credentials
    /// through. Carried on the identity so mid-session hot-swaps
    /// (`apply_live_session_llm_identity`) re-resolve against the
    /// same realm the session was created with — preventing
    /// cross-realm credential bleed in multi-tenant setups. Dogma
    /// §12 (dynamic policy follows dynamic identity): on swap the
    /// factory re-enters `ProviderRuntimeRegistry::resolve` against
    /// this binding, not a new synthesized env-default realm.
    ///
    /// Projection (dogma §1/§13): canonical owner is
    /// `SessionMetadata.auth_binding`; this field is the
    /// read/write projection used by hot-swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<crate::AuthBindingRef>,
}

/// Typed per-turn override request for a session LLM identity.
pub struct SessionLlmIdentityOverride<'a> {
    pub model: Option<&'a str>,
    pub provider: Option<Provider>,
    pub provider_params: Option<&'a serde_json::Value>,
    pub clear_provider_params: bool,
    pub auth_binding: Option<&'a crate::AuthBindingRef>,
    pub clear_auth_binding: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionLlmIdentityOverrideError {
    #[error("provider override requires model on an existing session")]
    ProviderRequiresModel,
    #[error("clear_provider_params cannot be combined with provider_params")]
    SetAndClearProviderParams,
    #[error("clear_auth_binding cannot be combined with auth_binding")]
    SetAndClearAuthBinding,
    #[error("{0}")]
    ProviderModelMismatch(String),
    #[error("self-hosted provider requires a registered model alias; '{model}' is not configured")]
    MissingSelfHostedAlias { model: String },
}

/// Resolve a turn-time model/provider/auth override against the current
/// durable session identity.
///
/// The model registry is the authority for catalog ownership. A model-only
/// override follows catalog ownership when the target model is registered;
/// uncatalogued models keep the current provider so custom aliases remain
/// possible.
pub fn resolve_session_llm_identity_override(
    current: &SessionLlmIdentity,
    registry: &crate::ModelRegistry,
    overrides: SessionLlmIdentityOverride<'_>,
) -> Result<SessionLlmIdentity, SessionLlmIdentityOverrideError> {
    if overrides.provider.is_some() && overrides.model.is_none() {
        return Err(SessionLlmIdentityOverrideError::ProviderRequiresModel);
    }
    if overrides.clear_provider_params && overrides.provider_params.is_some() {
        return Err(SessionLlmIdentityOverrideError::SetAndClearProviderParams);
    }
    if overrides.clear_auth_binding && overrides.auth_binding.is_some() {
        return Err(SessionLlmIdentityOverrideError::SetAndClearAuthBinding);
    }

    let model = overrides
        .model
        .map(str::to_string)
        .unwrap_or_else(|| current.model.clone());
    let provider = if let Some(provider) = overrides.provider {
        provider
    } else if overrides.model.is_some() {
        registry
            .entry(&model)
            .map_or(current.provider, |entry| entry.provider)
    } else {
        current.provider
    };

    if (overrides.model.is_some() || overrides.provider.is_some())
        && let Some(reason) = registry.provider_override_mismatch_reason(provider, &model)
    {
        return Err(SessionLlmIdentityOverrideError::ProviderModelMismatch(
            reason,
        ));
    }

    let provider_params = if overrides.clear_provider_params {
        None
    } else {
        overrides
            .provider_params
            .cloned()
            .or_else(|| current.provider_params.clone())
    };
    let self_hosted_server_id = if provider == Provider::SelfHosted {
        if overrides.model.is_none() {
            current.self_hosted_server_id.clone().or_else(|| {
                registry
                    .entry_for_provider(Provider::SelfHosted, &model)
                    .and_then(|entry| entry.self_hosted.as_ref())
                    .map(|server| server.server_id.clone())
            })
        } else {
            let entry = registry
                .entry_for_provider(Provider::SelfHosted, &model)
                .ok_or_else(|| SessionLlmIdentityOverrideError::MissingSelfHostedAlias {
                    model: model.clone(),
                })?;
            entry
                .self_hosted
                .as_ref()
                .map(|server| server.server_id.clone())
        }
    } else {
        None
    };

    let auth_binding = if overrides.clear_auth_binding
        || (provider != current.provider && overrides.auth_binding.is_none())
    {
        None
    } else {
        overrides
            .auth_binding
            .cloned()
            .or_else(|| current.auth_binding.clone())
    };

    Ok(SessionLlmIdentity {
        model,
        provider,
        self_hosted_server_id,
        provider_params,
        auth_binding,
    })
}

/// Live request policy paired with a session LLM identity hot-swap.
///
/// `SessionLlmIdentity` is the durable semantic identity. This projection is
/// the per-turn request policy the live agent must use for the next LLM call,
/// including provider params and provider-native tool defaults resolved for
/// the same target model/provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SessionLlmRequestPolicy {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_tool_defaults: Option<serde_json::Value>,
}

impl SessionMetadata {
    /// Return the current durable LLM identity for this session.
    pub fn llm_identity(&self) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: self.model.clone(),
            provider: self.provider,
            self_hosted_server_id: self.self_hosted_server_id.clone(),
            provider_params: self.provider_params.clone(),
            auth_binding: self.auth_binding.clone(),
        }
    }

    /// Overwrite the durable LLM identity while preserving unrelated session metadata.
    pub fn apply_llm_identity(&mut self, identity: &SessionLlmIdentity) {
        self.model = identity.model.clone();
        self.provider = identity.provider;
        self.self_hosted_server_id = identity.self_hosted_server_id.clone();
        self.provider_params = identity.provider_params.clone();
        self.auth_binding = identity.auth_binding.clone();
    }
}

/// Key used to store SessionMetadata in Session metadata map.
pub const SESSION_METADATA_KEY: &str = "session_metadata";

/// Caller intent for a tool category.
///
/// Distinguishes "no opinion / didn't exist" (`Inherit`) from explicit
/// `Enable` / `Disable` so that resumed sessions don't freeze tool
/// availability at the capabilities of the Meerkat version that created them.
///
/// **Dogma §10:** Inherit, disable, and set are different facts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategoryOverride {
    /// No explicit intent — inherit runtime/factory default.
    #[default]
    Inherit,
    /// Explicitly enabled by caller.
    Enable,
    /// Explicitly disabled by caller.
    Disable,
}

impl ToolCategoryOverride {
    /// Resolve this override against a runtime default.
    ///
    /// - `Enable` → `true`
    /// - `Disable` → `false`
    /// - `Inherit` → `runtime_default`
    #[must_use]
    pub fn resolve(self, runtime_default: bool) -> bool {
        match self {
            Self::Enable => true,
            Self::Disable => false,
            Self::Inherit => runtime_default,
        }
    }

    /// Convert to `Option<bool>` for feeding `AgentBuildConfig` override fields.
    ///
    /// - `Enable` → `Some(true)`
    /// - `Disable` → `Some(false)`
    /// - `Inherit` → `None` (factory default wins)
    #[must_use]
    pub fn to_override(self) -> Option<bool> {
        match self {
            Self::Enable => Some(true),
            Self::Disable => Some(false),
            Self::Inherit => None,
        }
    }

    /// Construct from a resolved effective bool.
    ///
    /// **Warning:** this collapses `Inherit` into `Enable`/`Disable`. Prefer
    /// [`from_override`] when persisting session metadata so that `Inherit`
    /// survives across save/resume cycles. Only use `from_effective` in test
    /// helpers or when constructing metadata from external sources that only
    /// provide a resolved bool.
    #[must_use]
    pub fn from_effective(enabled: bool) -> Self {
        if enabled { Self::Enable } else { Self::Disable }
    }

    /// Construct from an `Option<bool>` override field, preserving `Inherit`.
    ///
    /// - `Some(true)` → `Enable`
    /// - `Some(false)` → `Disable`
    /// - `None` → `Inherit` (factory default was used, no explicit intent)
    ///
    /// This is the inverse of [`to_override`] and should be used when persisting
    /// session tooling metadata so that `Inherit` survives across save/resume
    /// cycles.
    #[must_use]
    pub fn from_override(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Enable,
            Some(false) => Self::Disable,
            None => Self::Inherit,
        }
    }
}

/// Backward-compatible deserializer: accepts both old `bool` JSON and new
/// tri-state `"inherit"` / `"enable"` / `"disable"` strings.
///
/// Old persisted sessions have `"mob": false` or `"builtins": true`.
/// - `true`  → `Enable`  (user explicitly had it on)
/// - `false` → `Inherit` (can't distinguish "disabled" from "didn't exist")
/// - string  → normal enum deserialization
fn deserialize_tool_category_compat<'de, D>(
    deserializer: D,
) -> Result<ToolCategoryOverride, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct ToolCategoryVisitor;

    impl de::Visitor<'_> for ToolCategoryVisitor {
        type Value = ToolCategoryOverride;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a boolean or one of \"inherit\", \"enable\", \"disable\"")
        }

        fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(if v {
                ToolCategoryOverride::Enable
            } else {
                ToolCategoryOverride::Inherit
            })
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            match v {
                "inherit" => Ok(ToolCategoryOverride::Inherit),
                "enable" => Ok(ToolCategoryOverride::Enable),
                "disable" => Ok(ToolCategoryOverride::Disable),
                _ => Err(de::Error::unknown_variant(
                    v,
                    &["inherit", "enable", "disable"],
                )),
            }
        }
    }

    deserializer.deserialize_any(ToolCategoryVisitor)
}

/// Tooling intent captured at session creation time.
///
/// Fields use [`ToolCategoryOverride`] to distinguish "no opinion" from
/// explicit enable/disable (Dogma §10). On resume, `Inherit` falls through
/// to the factory's current runtime default, allowing new tool categories
/// to become available without re-creating the session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionTooling {
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub builtins: ToolCategoryOverride,
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub shell: ToolCategoryOverride,
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub comms: ToolCategoryOverride,
    /// Mob (multi-agent orchestration) tools.
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub mob: ToolCategoryOverride,
    /// Semantic memory.
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub memory: ToolCategoryOverride,
    /// Scheduler tools.
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub schedule: ToolCategoryOverride,
    /// WorkGraph durable work tools.
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub workgraph: ToolCategoryOverride,
    /// Assistant image generation.
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub image_generation: ToolCategoryOverride,
    /// Meerkat-owned fallback web search.
    #[serde(default, deserialize_with = "deserialize_tool_category_compat")]
    pub web_search: ToolCategoryOverride,
    /// Active skills at session creation time (for deterministic resume).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_skills: Option<Vec<crate::skills::SkillKey>>,
}

impl From<&Session> for SessionMeta {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            message_count: session.messages.len(),
            total_tokens: session.total_tokens(),
            metadata: session.metadata.clone(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::realtime_transcript::RealtimeTranscriptRole;
    use crate::types::{
        AssistantMessage, BlockAssistantMessage, ContentBlock, StopReason, SystemMessage, Usage,
        UserMessage,
    };
    use std::sync::Arc;

    fn block_assistant_text(message: &BlockAssistantMessage) -> String {
        message
            .blocks
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn transcript_rewrite_preserves_full_assistant_block_trace() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text(
            "run the trace".to_string(),
        )));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "original assistant trace".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));

        let parent_revision = session.transcript_revision().expect("parent revision");
        let replacement = vec![
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![
                    AssistantBlock::Text {
                        text: "compacted assistant trace".to_string(),
                        meta: None,
                    },
                    AssistantBlock::ToolUse {
                        id: "toolu_trace".to_string(),
                        name: "trace_probe".to_string(),
                        args: serde_json::value::RawValue::from_string(
                            r#"{"path":"N-3"}"#.to_string(),
                        )
                        .expect("valid tool args"),
                        meta: None,
                    },
                ],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::new(
                "toolu_trace".to_string(),
                "trace complete".to_string(),
                false,
            )]),
        ];

        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                replacement,
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision.clone()),
            )
            .expect("rewrite should commit");

        assert_eq!(commit.parent_revision, parent_revision);
        let current = session
            .transcript_revision_messages(&commit.revision)
            .expect("history state should decode")
            .expect("current revision should be retained");
        let Message::BlockAssistant(assistant) = &current[1] else {
            panic!("replacement should remain a block assistant message");
        };
        assert!(assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::ToolUse { name, args, .. }
                if name == "trace_probe" && args.get().contains("\"N-3\"")
        )));

        let parent = session
            .transcript_revision_messages(&parent_revision)
            .expect("history state should decode")
            .expect("parent revision should remain retained");
        assert!(matches!(
            &parent[1],
            Message::BlockAssistant(assistant)
                if block_assistant_text(assistant).contains("original assistant trace")
        ));
    }

    #[test]
    fn transcript_rewrite_rejects_trailing_block_assistant_tool_call() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "plain answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = session.transcript_revision().expect("parent revision");

        let err = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage::new(
                    vec![AssistantBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "lookup".to_string(),
                        args: serde_json::value::RawValue::from_string("{}".to_string())
                            .expect("valid args"),
                        meta: None,
                    }],
                    StopReason::ToolUse,
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision),
            )
            .expect_err("rewrite should reject trailing unresolved block-assistant tool call");
        assert!(matches!(
            err,
            TranscriptEditError::InvalidTranscriptShape(_)
        ));
    }

    #[test]
    fn transcript_rewrite_rejects_no_op_self_edge() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text(
            "keep this exact transcript".to_string(),
        )));
        session.push(Message::Assistant(AssistantMessage {
            content: "unchanged".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent_revision = session.transcript_revision().expect("parent revision");
        let err = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![session.messages()[1].clone()],
                TranscriptRewriteReason::new("retry"),
                Some("unit-test".to_string()),
                Some(parent_revision.clone()),
            )
            .expect_err("same-content rewrite should not emit a self-edge commit");

        assert!(matches!(
            err,
            TranscriptEditError::NoOpRewrite { revision } if revision == parent_revision
        ));
        assert!(
            session
                .transcript_history_state()
                .expect("history state should decode")
                .is_none()
        );
    }

    #[test]
    fn transcript_rewrite_run_boundary_guard_accepts_rewrite_then_append() {
        let mut original = Session::new();
        original.push(Message::User(UserMessage::text("question".to_string())));
        original.push(Message::Assistant(AssistantMessage {
            content: "verbose answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent_revision = original.transcript_revision().expect("parent revision");
        let mut incoming = original.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite should commit");
        incoming.push(Message::User(UserMessage::text("follow-up".to_string())));
        incoming.push(Message::Assistant(AssistantMessage {
            content: "follow-up answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        crate::session_store::run_boundary_snapshot_save_guard(&incoming, Some(&original))
            .expect("rewrite plus appended turn should be a valid run-boundary commit");
    }

    #[test]
    fn transcript_rewrite_rejects_orphaned_tool_results() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("use a tool".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: "toolu_1".to_string(),
                name: "lookup".to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string())
                    .expect("valid args"),
                meta: None,
            }],
            StopReason::ToolUse,
        )));
        session.push(Message::tool_results(vec![ToolResult::new(
            "toolu_1".to_string(),
            "done".to_string(),
            false,
        )]));
        let parent_revision = session.transcript_revision().expect("parent revision");

        let err = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "no tool after all".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision),
            )
            .expect_err("rewrite should reject stranded tool results");
        assert!(matches!(
            err,
            TranscriptEditError::InvalidTranscriptShape(_)
        ));
    }

    #[test]
    fn transcript_rewrite_rejects_trailing_assistant_tool_call() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "plain answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = session.transcript_revision().expect("parent revision");

        let err = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: String::new(),
                    tool_calls: vec![crate::types::ToolCall::new(
                        "toolu_1".to_string(),
                        "lookup".to_string(),
                        serde_json::json!({}),
                    )],
                    stop_reason: StopReason::ToolUse,
                    usage: Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision),
            )
            .expect_err("rewrite should reject trailing unresolved tool call");
        assert!(matches!(
            err,
            TranscriptEditError::InvalidTranscriptShape(_)
        ));
    }

    #[test]
    fn transcript_rewrite_rejects_duplicate_tool_results() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("use a tool".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "plain answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = session.transcript_revision().expect("parent revision");

        let err = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![
                    Message::BlockAssistant(BlockAssistantMessage::new(
                        vec![AssistantBlock::ToolUse {
                            id: "toolu_1".to_string(),
                            name: "lookup".to_string(),
                            args: serde_json::value::RawValue::from_string("{}".to_string())
                                .expect("valid args"),
                            meta: None,
                        }],
                        StopReason::ToolUse,
                    )),
                    Message::tool_results(vec![
                        ToolResult::new("toolu_1".to_string(), "one".to_string(), false),
                        ToolResult::new("toolu_1".to_string(), "two".to_string(), false),
                    ]),
                ],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision),
            )
            .expect_err("rewrite should reject duplicate tool results");
        assert!(matches!(
            err,
            TranscriptEditError::InvalidTranscriptShape(_)
        ));
    }

    #[test]
    fn transcript_rewrite_record_rejects_prefix_or_suffix_tampering() {
        let mut session = Session::new();
        session.push(Message::System(SystemMessage::new("keep prefix")));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        session.push(Message::User(UserMessage::text("keep suffix".to_string())));

        let parent_revision = session.transcript_revision().expect("parent revision");
        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite should commit");
        let state = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        let parent_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.parent_revision)
            .expect("parent body retained")
            .clone();
        let revision_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.revision)
            .expect("revision body retained")
            .clone();

        let mut forged_body = revision_body;
        forged_body.messages[0] = Message::System(SystemMessage::new("tampered prefix"));
        forged_body.revision =
            transcript_messages_digest(&forged_body.messages).expect("forged digest");
        let mut forged_commit = commit;
        forged_commit.revision = forged_body.revision.clone();
        let err = TranscriptRewriteRecord::new(forged_commit, parent_body, forged_body)
            .expect_err("record validation must reject changes outside selected span");
        assert!(
            err.to_string().contains("before the selected span"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn transcript_rewrite_replay_allows_normal_turn_revisions_between_rewrites() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("first".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose first answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: crate::types::Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let first_parent = session.transcript_revision().expect("first parent");
        let first_commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "compact first answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::types::Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(first_parent),
            )
            .expect("first rewrite");

        session.push(Message::User(UserMessage::text("normal turn".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose second answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: crate::types::Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let bridge_parent = session
            .transcript_revision()
            .expect("normal turn should advance transcript head");
        assert_ne!(bridge_parent, first_commit.revision);
        validate_transcript_history_state(
            &session
                .transcript_history_state()
                .expect("history state should decode")
                .expect("history state should exist"),
        )
        .expect("normal turn head may legitimately differ from last rewrite commit");

        let second_commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 3, end: 4 },
                vec![Message::Assistant(AssistantMessage {
                    content: "compact second answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::types::Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(bridge_parent.clone()),
            )
            .expect("second rewrite");

        let state = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        let records = state.commits.iter().map(|commit| {
            let parent_body = state
                .revisions
                .iter()
                .find(|body| body.revision == commit.parent_revision)
                .expect("parent body retained")
                .clone();
            let revision_body = state
                .revisions
                .iter()
                .find(|body| body.revision == commit.revision)
                .expect("revision body retained")
                .clone();
            TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body)
                .expect("record should validate")
        });

        let replayed = TranscriptHistoryState::from_rewrite_records(records)
            .expect("rewrite replay should accept normal-turn bridge revisions")
            .expect("rewrite records should exist");
        assert_eq!(replayed.head, second_commit.revision);
        assert!(
            replayed
                .revisions
                .iter()
                .any(|body| body.revision == bridge_parent)
        );
    }

    #[test]
    fn transcript_rewrite_replay_rejects_branched_rewrite_records() {
        let mut base = Session::new();
        base.push(Message::User(UserMessage::text("question".to_string())));
        base.push(Message::Assistant(AssistantMessage {
            content: "verbose answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: crate::types::Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent = base.transcript_revision().expect("parent revision");

        let mut first = base.clone();
        let first_commit = first
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "first compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::types::Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent.clone()),
            )
            .expect("first rewrite");
        let first_state = first
            .transcript_history_state()
            .expect("first state decodes")
            .expect("first state exists");

        let mut second = base;
        let second_commit = second
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "second compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::types::Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent),
            )
            .expect("second rewrite");
        let second_state = second
            .transcript_history_state()
            .expect("second state decodes")
            .expect("second state exists");

        let record = |state: &TranscriptHistoryState, commit: &TranscriptRewriteCommit| {
            let parent_body = state
                .revisions
                .iter()
                .find(|body| body.revision == commit.parent_revision)
                .expect("parent body retained")
                .clone();
            let revision_body = state
                .revisions
                .iter()
                .find(|body| body.revision == commit.revision)
                .expect("revision body retained")
                .clone();
            TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body)
                .expect("record should validate")
        };

        let err = TranscriptHistoryState::from_rewrite_records(vec![
            record(&first_state, &first_commit),
            record(&second_state, &second_commit),
        ])
        .expect_err("branched rewrite records must not replay as a linear source history");
        assert!(
            err.to_string().contains("does not extend transcript head"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn internal_message_rewrites_refresh_transcript_history_head() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: crate::types::Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent = session.transcript_revision().expect("parent revision");
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::types::Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent),
            )
            .expect("rewrite should commit");

        session.push(Message::User(UserMessage::text(
            "notice-bearing turn".to_string(),
        )));
        session
            .retain_messages_internal(
                |message| {
                    !matches!(
                        message,
                        Message::User(user)
                            if user.content.iter().any(|block| matches!(
                                block,
                                ContentBlock::Text { text } if text.contains("notice-bearing")
                            ))
                    )
                },
                TranscriptRewriteReason::new("synthetic_notice_cleanup"),
            )
            .expect("retain should commit internal rewrite");
        let retained_digest =
            transcript_messages_digest(session.messages()).expect("retained digest");
        assert_eq!(
            session.transcript_revision().expect("retained head"),
            retained_digest
        );

        session
            .replace_messages_internal(
                vec![
                    Message::User(UserMessage::text("compacted question".to_string())),
                    Message::Assistant(AssistantMessage {
                        content: "compacted answer".to_string(),
                        tool_calls: Vec::new(),
                        stop_reason: StopReason::EndTurn,
                        usage: crate::types::Usage::default(),
                        created_at: crate::types::message_timestamp_now(),
                    }),
                ],
                TranscriptRewriteReason::new("compaction"),
            )
            .expect("replace should commit internal rewrite");
        let replaced_digest =
            transcript_messages_digest(session.messages()).expect("replaced digest");
        assert_eq!(
            session.transcript_revision().expect("replaced head"),
            replaced_digest
        );
        let state = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        assert!(
            state
                .revisions
                .iter()
                .any(|body| body.revision == replaced_digest)
        );
        validate_transcript_history_state(&state).expect("history state remains valid");
    }

    #[test]
    fn set_system_prompt_refreshes_transcript_history_head_after_rewrite() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: crate::types::Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent = session.transcript_revision().expect("parent revision");
        let rewrite = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::types::Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent),
            )
            .expect("rewrite should commit");

        session.set_system_prompt("durable system prompt".to_string());

        let head = session
            .transcript_revision()
            .expect("system prompt should refresh transcript head");
        assert_ne!(head, rewrite.revision);
        assert_eq!(
            head,
            transcript_messages_digest(session.messages()).expect("current digest")
        );
        let head_messages = session
            .transcript_revision_messages(&head)
            .expect("history state should decode")
            .expect("refreshed head body should be retained");
        assert_eq!(
            serde_json::to_value(&head_messages).expect("head serializes"),
            serde_json::to_value(session.messages()).expect("session serializes")
        );
        validate_transcript_history_state(
            &session
                .transcript_history_state()
                .expect("history state should decode")
                .expect("history state should exist"),
        )
        .expect("history state remains valid after system prompt update");
    }

    #[test]
    fn apply_transcript_history_state_uses_latest_commit_time_for_restored_head() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: crate::types::Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let original_messages = session.messages().to_vec();
        let parent = session.transcript_revision().expect("parent revision");
        let compact = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::Assistant(AssistantMessage {
                    content: "compact answer".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: StopReason::EndTurn,
                    usage: crate::types::Usage::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent.clone()),
            )
            .expect("rewrite should commit");

        std::thread::sleep(std::time::Duration::from_millis(2));
        let restore = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 0,
                    end: session.messages().len(),
                },
                original_messages.clone(),
                TranscriptRewriteReason::new("restore"),
                Some("unit-test".to_string()),
                Some(compact.revision),
            )
            .expect("restore should commit");
        assert_eq!(restore.revision, parent);

        let state = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        let restored_body_created_at = state
            .revisions
            .iter()
            .find(|body| body.revision == restore.revision)
            .expect("restored body should be retained")
            .created_at;
        assert!(
            restored_body_created_at < restore.committed_at,
            "test requires restore commit to be newer than retained body"
        );

        let mut replayed = Session::new();
        replayed
            .apply_transcript_history_state(state)
            .expect("replay should materialize restored head");
        assert_eq!(
            serde_json::to_value(replayed.messages()).expect("replayed serializes"),
            serde_json::to_value(&original_messages).expect("original serializes")
        );
        assert_eq!(replayed.updated_at(), restore.committed_at);
    }

    #[test]
    fn test_session_new() {
        let session = Session::new();
        assert_eq!(session.version(), SESSION_VERSION);
        assert!(session.messages().is_empty());
        assert!(session.created_at() <= session.updated_at());
    }

    #[test]
    fn llm_identity_model_override_switches_to_catalog_provider() {
        let registry = crate::Config::default().model_registry().unwrap();
        let current = SessionLlmIdentity {
            model: "claude-sonnet-4-5".to_string(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(crate::AuthBindingRef {
                realm: crate::RealmId::parse("tenant_a").unwrap(),
                binding: crate::BindingId::parse("anthropic_default").unwrap(),
                profile: None,
            }),
        };

        let resolved = resolve_session_llm_identity_override(
            &current,
            &registry,
            SessionLlmIdentityOverride {
                model: Some("gpt-5.5"),
                provider: None,
                provider_params: None,
                clear_provider_params: false,
                auth_binding: None,
                clear_auth_binding: false,
            },
        )
        .unwrap();

        assert_eq!(resolved.model, "gpt-5.5");
        assert_eq!(resolved.provider, Provider::OpenAI);
        assert!(
            resolved.auth_binding.is_none(),
            "provider switches must not inherit a binding from the previous provider"
        );
    }

    #[test]
    fn llm_identity_model_override_keeps_uncatalogued_model_on_current_provider() {
        let registry = crate::Config::default().model_registry().unwrap();
        let current = SessionLlmIdentity {
            model: "custom-model".to_string(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };

        let resolved = resolve_session_llm_identity_override(
            &current,
            &registry,
            SessionLlmIdentityOverride {
                model: Some("uncatalogued-custom-model"),
                provider: None,
                provider_params: None,
                clear_provider_params: false,
                auth_binding: None,
                clear_auth_binding: false,
            },
        )
        .unwrap();

        assert_eq!(resolved.model, "uncatalogued-custom-model");
        assert_eq!(resolved.provider, Provider::Anthropic);
    }

    #[test]
    fn realtime_transcript_append_is_idempotent_by_provider_item_and_delta_id() {
        let mut session = Session::new();

        let user = RealtimeTranscriptEvent::UserTranscriptFinal {
            item_id: "item_user".to_string(),
            previous_item_id: None,
            content_index: 0,
            text: "hello".to_string(),
        };
        assert!(
            !session
                .append_realtime_transcript_event(user.clone())
                .is_inert()
        );
        assert!(session.append_realtime_transcript_event(user).is_inert());

        let delta = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: "resp_assistant".to_string(),
            delta_id: "evt_delta_1".to_string(),
            item_id: "item_assistant".to_string(),
            previous_item_id: Some("item_user".to_string()),
            content_index: 0,
            delta: "hi".to_string(),
        };
        assert!(
            session
                .append_realtime_transcript_event(delta.clone())
                .is_inert()
        );
        assert!(session.append_realtime_transcript_event(delta).is_inert());

        let terminal = RealtimeTranscriptEvent::AssistantTurnCompleted {
            response_id: "resp_assistant".to_string(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        assert!(
            !session
                .append_realtime_transcript_event(terminal.clone())
                .is_inert()
        );
        assert!(
            session
                .append_realtime_transcript_event(terminal)
                .is_inert()
        );

        assert_eq!(session.messages().len(), 2);
        assert!(matches!(
            &session.messages()[0],
            Message::User(user) if user.text_content() == "hello"
        ));
        assert!(matches!(
            &session.messages()[1],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "hi"
        ));
    }

    /// R5-7: `AssistantTranscriptFinalText` injects authoritative final text
    /// into the staged item. Verifies the override semantics: a partial
    /// delta is replaced, not concatenated, and the item promotes to the
    /// Spoken lane so flush emits `AssistantBlock::Transcript`.
    #[test]
    fn realtime_transcript_final_text_overrides_partial_delta_and_promotes_to_spoken_lane() {
        let mut session = Session::new();

        // Partial delta accumulates "incom" — simulating delta loss before
        // the final arrives.
        assert!(
            session
                .append_realtime_transcript_event(
                    RealtimeTranscriptEvent::AssistantTranscriptDelta {
                        response_id: "resp_a".to_string(),
                        delta_id: "evt_1".to_string(),
                        item_id: "item_a".to_string(),
                        previous_item_id: None,
                        content_index: 0,
                        delta: "incom".to_string(),
                    }
                )
                .is_inert()
        );

        // Authoritative final text overrides the staged content.
        assert!(
            session
                .append_realtime_transcript_event(
                    RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                        response_id: "resp_a".to_string(),
                        item_id: "item_a".to_string(),
                        content_index: 0,
                        text: "complete answer".to_string(),
                    }
                )
                .is_inert()
        );

        // Turn completion drives the flush.
        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert!(!outcome.is_inert());

        // Verify the materialized block has the final's authoritative text
        // (not the partial "incom") and the Spoken lane.
        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::BlockAssistant(assistant) => {
                let mut found_transcript = false;
                for block in &assistant.blocks {
                    if let AssistantBlock::Transcript { text, .. } = block {
                        assert_eq!(text, "complete answer");
                        found_transcript = true;
                    }
                }
                assert!(
                    found_transcript,
                    "AssistantTranscriptFinalText must promote to the Spoken lane and \
                     materialize as AssistantBlock::Transcript"
                );
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }

    /// R5-7: `AssistantTranscriptFinalText` works for final-only providers
    /// where no prior delta has staged an item.
    #[test]
    fn realtime_transcript_final_text_creates_item_when_no_delta_staged() {
        let mut session = Session::new();

        assert!(
            session
                .append_realtime_transcript_event(
                    RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                        response_id: "resp_a".to_string(),
                        item_id: "item_a".to_string(),
                        content_index: 0,
                        text: "spoken-final-only".to_string(),
                    }
                )
                .is_inert()
        );

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert!(!outcome.is_inert());

        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::BlockAssistant(assistant) => {
                let has_transcript = assistant.blocks.iter().any(|b| {
                    matches!(b, AssistantBlock::Transcript { text, .. } if text == "spoken-final-only")
                });
                assert!(
                    has_transcript,
                    "final-only provider path must materialize as Transcript on the Spoken lane"
                );
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }

    #[test]
    fn realtime_transcript_append_orders_causally_equivalent_out_of_order_items() {
        let mut session = Session::new();

        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                    response_id: "resp_assistant".to_string(),
                    delta_id: "evt_delta_1".to_string(),
                    item_id: "item_assistant".to_string(),
                    previous_item_id: Some("item_user".to_string()),
                    content_index: 0,
                    delta: "answer".to_string(),
                })
                .is_inert()
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: "resp_assistant".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
                .is_inert()
        );

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "question".to_string(),
            },
        );

        assert_eq!(outcome.materialized_messages.len(), 2);
        assert_eq!(session.messages().len(), 2);
        assert!(matches!(
            &session.messages()[0],
            Message::User(user) if user.text_content() == "question"
        ));
        assert!(matches!(
            &session.messages()[1],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "answer"
        ));
    }

    #[test]
    fn realtime_transcript_replay_of_seen_provider_items_is_inert() {
        let mut session = Session::new();
        let events = vec![
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "hello".to_string(),
            },
            RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_assistant".to_string(),
                delta_id: "evt_delta_1".to_string(),
                item_id: "item_assistant".to_string(),
                previous_item_id: Some("item_user".to_string()),
                content_index: 0,
                delta: "world".to_string(),
            },
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_assistant".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ];

        for event in events.iter().cloned() {
            let _ = session.append_realtime_transcript_event(event);
        }
        let first_messages = serde_json::to_value(session.messages()).unwrap();

        for event in events {
            assert!(session.append_realtime_transcript_event(event).is_inert());
        }

        assert_eq!(
            serde_json::to_value(session.messages()).unwrap(),
            first_messages
        );
    }

    #[test]
    fn realtime_transcript_user_final_replay_cannot_erase_existing_segment() {
        let mut session = Session::new();

        let user = RealtimeTranscriptEvent::UserTranscriptFinal {
            item_id: "item_user".to_string(),
            previous_item_id: None,
            content_index: 0,
            text: "remember amber lantern".to_string(),
        };
        assert!(
            !session
                .append_realtime_transcript_event(user.clone())
                .is_inert()
        );
        let first_messages = serde_json::to_value(session.messages()).unwrap();

        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: "item_user".to_string(),
                    previous_item_id: None,
                    content_index: 0,
                    text: String::new(),
                })
                .is_inert()
        );
        assert!(session.append_realtime_transcript_event(user).is_inert());
        assert_eq!(
            serde_json::to_value(session.messages()).unwrap(),
            first_messages
        );
    }

    #[test]
    fn realtime_transcript_empty_user_final_can_be_filled_by_later_nonempty_replay() {
        let mut session = Session::new();

        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: "item_user".to_string(),
                    previous_item_id: None,
                    content_index: 0,
                    text: String::new(),
                })
                .is_inert()
        );
        assert!(session.messages().is_empty());

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "remember amber lantern".to_string(),
            },
        );
        assert_eq!(outcome.materialized_messages.len(), 1);
        assert_eq!(session.messages().len(), 1);
        assert!(matches!(
            &session.messages()[0],
            Message::User(user) if user.text_content() == "remember amber lantern"
        ));
    }

    #[test]
    fn realtime_transcript_skipped_provider_items_preserve_causal_order_without_content() {
        let mut session = Session::new();

        let assistant_delta = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: "resp_assistant".to_string(),
            delta_id: "evt_delta_1".to_string(),
            item_id: "item_assistant".to_string(),
            previous_item_id: Some("item_tool".to_string()),
            content_index: 0,
            delta: "done".to_string(),
        };
        assert!(
            session
                .append_realtime_transcript_event(assistant_delta.clone())
                .is_inert()
        );
        let assistant_complete = RealtimeTranscriptEvent::AssistantTurnCompleted {
            response_id: "resp_assistant".to_string(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        assert!(
            session
                .append_realtime_transcript_event(assistant_complete.clone())
                .is_inert()
        );

        let skipped = RealtimeTranscriptEvent::ItemSkipped {
            item_id: "item_tool".to_string(),
            previous_item_id: Some("item_user".to_string()),
        };
        assert!(
            session
                .append_realtime_transcript_event(skipped.clone())
                .is_inert(),
            "a skipped provider item must not append transcript content"
        );
        assert!(session.messages().is_empty());

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "please use the tool".to_string(),
            },
        );
        assert_eq!(outcome.materialized_messages.len(), 2);
        assert_eq!(session.messages().len(), 2);
        assert!(matches!(
            &session.messages()[0],
            Message::User(user) if user.text_content() == "please use the tool"
        ));
        assert!(matches!(
            &session.messages()[1],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "done"
        ));

        let first_messages = serde_json::to_value(session.messages()).unwrap();
        assert!(session.append_realtime_transcript_event(skipped).is_inert());
        assert!(
            session
                .append_realtime_transcript_event(assistant_delta)
                .is_inert()
        );
        assert!(
            session
                .append_realtime_transcript_event(assistant_complete)
                .is_inert()
        );
        assert_eq!(
            serde_json::to_value(session.messages()).unwrap(),
            first_messages
        );
    }

    #[test]
    fn realtime_transcript_interrupted_assistant_item_unblocks_later_provider_items() {
        // R5-5 (Round-5): the staged assistant content is a Display-lane item
        // (`AssistantTextDelta`). Under the new lane-aware barge-in contract,
        // the Display lane survives interruption and materializes. The User
        // "Stop." item, gated on the chained Display item being materialized,
        // also unblocks. Round-4's "must stay non-canonical" assertion was
        // wrong — that contract was lane-blind.
        let mut session = Session::new();

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_repeat".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "repeat until stop".to_string(),
            },
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                    response_id: "resp_loop".to_string(),
                    delta_id: "evt_loop_1".to_string(),
                    item_id: "item_loop".to_string(),
                    previous_item_id: Some("item_repeat".to_string()),
                    content_index: 0,
                    delta: "Looping now".to_string(),
                })
                .is_inert()
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: "item_stop".to_string(),
                    previous_item_id: Some("item_loop".to_string()),
                    content_index: 0,
                    text: "Stop.".to_string(),
                })
                .is_inert(),
            "the stop turn waits until the interrupted assistant provider item is resolved"
        );

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnInterrupted {
                response_id: "resp_loop".to_string(),
            },
        );

        // R5-5: materializer commits 2 messages (the retained Display item +
        // the unblocked "Stop." User message).
        assert_eq!(outcome.materialized_messages.len(), 2);
        // Canonical history: User-repeat, BlockAssistant(Display "Looping now"), User-Stop.
        assert_eq!(session.messages().len(), 3);
        assert!(matches!(
            &session.messages()[0],
            Message::User(user) if user.text_content() == "repeat until stop"
        ));
        match &session.messages()[1] {
            Message::BlockAssistant(assistant) => {
                let text = block_assistant_text(assistant);
                assert_eq!(text, "Looping now");
            }
            other => unreachable!(
                "Display lane assistant item must be retained on Interrupted, got {other:?}"
            ),
        }
        assert!(matches!(
            &session.messages()[2],
            Message::User(user) if user.text_content() == "Stop."
        ));
    }

    #[test]
    fn realtime_transcript_late_interrupted_assistant_delta_stays_noncanonical() {
        let mut session = Session::new();

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_repeat".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "repeat until stop".to_string(),
            },
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::ItemObserved {
                    item_id: "item_loop".to_string(),
                    previous_item_id: Some("item_repeat".to_string()),
                    role: RealtimeTranscriptRole::Assistant,
                    response_id: None,
                })
                .is_inert(),
            "provider can observe an assistant item before the adapter learns its response id"
        );
        assert!(
            session
                .append_realtime_transcript_event(
                    RealtimeTranscriptEvent::AssistantTurnInterrupted {
                        response_id: "resp_loop".to_string(),
                    }
                )
                .is_inert(),
            "an interruption can arrive before delayed transcript deltas for the response"
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::UserTranscriptFinal {
                    item_id: "item_stop".to_string(),
                    previous_item_id: Some("item_loop".to_string()),
                    content_index: 0,
                    text: "Stop.".to_string(),
                })
                .is_inert(),
            "the stop turn waits for the provider's interrupted assistant item anchor"
        );

        let late_delta_outcome =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_loop".to_string(),
                delta_id: "evt_loop_late".to_string(),
                item_id: "item_loop".to_string(),
                previous_item_id: Some("item_repeat".to_string()),
                content_index: 0,
                delta: "Looping now".to_string(),
            });
        assert_eq!(late_delta_outcome.materialized_messages.len(), 1);
        assert!(matches!(
            &session.messages()[1],
            Message::User(user) if user.text_content() == "Stop."
        ));
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: "resp_loop".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
                .is_inert(),
            "late completion for an interrupted response must not resurrect its deltas"
        );
        assert!(
            session
                .messages()
                .iter()
                .filter_map(|message| match message {
                    Message::BlockAssistant(assistant) => Some(block_assistant_text(assistant)),
                    _ => None,
                })
                .all(|text| !text.contains("Looping now")),
            "late interrupted assistant text must remain non-canonical"
        );
    }

    #[test]
    fn realtime_transcript_completion_only_finalizes_matching_response() {
        let mut session = Session::new();

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "question".to_string(),
            },
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                    response_id: "resp_a".to_string(),
                    delta_id: "evt_a".to_string(),
                    item_id: "item_a".to_string(),
                    previous_item_id: Some("item_user".to_string()),
                    content_index: 0,
                    delta: "answer a".to_string(),
                })
                .is_inert()
        );

        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: "resp_b".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
                .is_inert(),
            "a completion for another response must not finalize buffered assistant text"
        );
        assert_eq!(session.messages().len(), 1);

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert_eq!(outcome.materialized_messages.len(), 1);
        assert_eq!(session.messages().len(), 2);
        assert!(matches!(
            &session.messages()[1],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "answer a"
        ));
    }

    #[test]
    fn realtime_transcript_completion_before_later_delta_is_response_scoped() {
        let mut session = Session::new();

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "question".to_string(),
            },
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: "resp_a".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
                .is_inert()
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                    response_id: "resp_b".to_string(),
                    delta_id: "evt_b".to_string(),
                    item_id: "item_b".to_string(),
                    previous_item_id: Some("item_user".to_string()),
                    content_index: 0,
                    delta: "wrong response".to_string(),
                })
                .is_inert(),
            "a later delta for another response must not be finalized by resp_a's pending completion"
        );

        let outcome =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "evt_a".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: Some("item_user".to_string()),
                content_index: 0,
                delta: "right response".to_string(),
            });

        assert_eq!(outcome.materialized_messages.len(), 1);
        assert_eq!(session.messages().len(), 2);
        assert!(matches!(
            &session.messages()[1],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "right response"
        ));
    }

    #[test]
    fn realtime_transcript_late_duplicate_completion_cannot_finalize_unrelated_response() {
        let mut session = Session::new();

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "question".to_string(),
            },
        );
        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "evt_a".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: Some("item_user".to_string()),
                content_index: 0,
                delta: "first".to_string(),
            });
        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert_eq!(session.messages().len(), 2);

        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                    response_id: "resp_b".to_string(),
                    delta_id: "evt_b".to_string(),
                    item_id: "item_b".to_string(),
                    previous_item_id: Some("item_a".to_string()),
                    content_index: 0,
                    delta: "second".to_string(),
                })
                .is_inert()
        );
        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: "resp_a".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
                .is_inert(),
            "a duplicate late terminal for resp_a must not finalize resp_b"
        );
        assert_eq!(session.messages().len(), 2);

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_b".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert_eq!(outcome.materialized_messages.len(), 1);
        assert_eq!(session.messages().len(), 3);
    }

    #[test]
    fn realtime_transcript_interruption_discards_only_matching_response() {
        // R5-5: cross-response isolation invariant — Interrupted on resp_a
        // does NOT touch resp_b's staged content. Both responses use
        // `AssistantTextDelta` (Display lane); under R5-5 resp_a's Display
        // item is RETAINED at Interrupted time and resp_b's continues
        // unaffected, materializing on its later TurnCompleted.
        let mut session = Session::new();

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "item_user".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "question".to_string(),
            },
        );
        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "evt_a".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: Some("item_user".to_string()),
                content_index: 0,
                delta: "interrupted display".to_string(),
            });
        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_b".to_string(),
                delta_id: "evt_b".to_string(),
                item_id: "item_b".to_string(),
                previous_item_id: Some("item_user".to_string()),
                content_index: 0,
                delta: "keep me".to_string(),
            });

        // R5-5: Interrupted commits the resp_a Display item; resp_b
        // remains untouched.
        let interrupt_outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnInterrupted {
                response_id: "resp_a".to_string(),
            },
        );
        assert_eq!(
            interrupt_outcome.materialized_messages.len(),
            1,
            "resp_a's Display item commits on Interrupted"
        );

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_b".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert_eq!(
            outcome.materialized_messages.len(),
            1,
            "resp_b commits on its TurnCompleted, untouched by resp_a's Interrupted"
        );

        // 1 user + 2 assistant messages.
        assert_eq!(session.messages().len(), 3);
        assert!(matches!(
            &session.messages()[1],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "interrupted display"
        ));
        assert!(matches!(
            &session.messages()[2],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "keep me"
        ));
    }

    // Performance tests for Arc-based CoW

    #[test]
    fn test_fork_shares_arc_no_clone() {
        let mut session = Session::new();
        for i in 0..100 {
            session.push(Message::User(UserMessage::text(format!("Message {i}"))));
        }

        // Fork should share the same Arc, not clone messages
        let forked = session.fork();

        // Both should point to the same underlying data (Arc refcount > 1)
        assert!(Arc::ptr_eq(&session.messages, &forked.messages));
        assert_eq!(forked.messages().len(), 100);
    }

    #[test]
    fn test_fork_at_shares_arc_prefix() {
        let mut session = Session::new();
        for i in 0..100 {
            session.push(Message::User(UserMessage::text(format!("Message {i}"))));
        }

        // Fork at 50 should create new Arc with copied prefix
        let forked = session.fork_at(50);
        assert_eq!(forked.messages().len(), 50);

        // Original should be unchanged
        assert_eq!(session.messages().len(), 100);
    }

    #[test]
    fn test_fork_at_resets_transcript_history_state_for_branch_identity() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text(
            "summarize this".to_string(),
        )));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "long assistant trace".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
        let parent_revision = session.transcript_revision().expect("parent revision");
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage::new(
                    vec![AssistantBlock::Text {
                        text: "compact trace".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                ))],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite should commit");

        let source_head = session.transcript_revision().expect("source head");
        let mut forked = session.fork_at(1);
        assert_ne!(forked.id(), session.id());
        assert!(
            !forked
                .metadata()
                .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
        );
        assert_eq!(
            forked.transcript_revision().expect("fork head"),
            transcript_messages_digest(forked.messages()).expect("fork digest")
        );
        assert!(
            forked
                .transcript_revision_messages(&source_head)
                .expect("fork history lookup")
                .is_none()
        );

        let fork_parent = forked.transcript_revision().expect("fork parent");
        let commit = forked
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text(
                    "branch prompt".to_string(),
                ))],
                TranscriptRewriteReason::new("branch_edit"),
                Some("test".to_string()),
                Some(fork_parent.clone()),
            )
            .expect("fork rewrite should use fork-local parent");
        assert_eq!(commit.parent_revision, fork_parent);
    }

    #[test]
    fn test_push_cow_behavior() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("First".to_string())));

        // Fork shares the Arc
        let forked = session.fork();
        assert!(Arc::ptr_eq(&session.messages, &forked.messages));

        // Push on original triggers CoW - original gets new Arc
        session.push(Message::User(UserMessage::text("Second".to_string())));

        // Now they should have different Arcs
        assert!(!Arc::ptr_eq(&session.messages, &forked.messages));
        assert_eq!(session.messages().len(), 2);
        assert_eq!(forked.messages().len(), 1);
    }

    // Performance tests for lazy timestamp updates

    #[test]
    fn test_push_batch_single_timestamp() {
        let mut session = Session::new();
        let initial_updated = session.updated_at();

        // Use push_batch to add multiple messages without repeated syscalls
        session.push_batch(vec![
            Message::User(UserMessage::text("First".to_string())),
            Message::User(UserMessage::text("Second".to_string())),
            Message::User(UserMessage::text("Third".to_string())),
        ]);

        assert_eq!(session.messages().len(), 3);
        // Timestamp should have been updated once
        assert!(session.updated_at() >= initial_updated);
    }

    #[test]
    fn test_touch_updates_timestamp() {
        let mut session = Session::new();
        let initial = session.updated_at();

        std::thread::sleep(std::time::Duration::from_millis(10));

        // Explicit touch to update timestamp
        session.touch();

        assert!(session.updated_at() > initial);
    }

    #[test]
    fn test_session_push() {
        let mut session = Session::new();
        let initial_updated = session.updated_at();

        // Small delay to ensure time changes
        std::thread::sleep(std::time::Duration::from_millis(10));

        session.push(Message::User(UserMessage::text("Hello".to_string())));

        assert_eq!(session.messages().len(), 1);
        assert!(session.updated_at() > initial_updated);
    }

    #[test]
    fn test_session_fork() {
        let mut session = Session::new();
        session.push(Message::System(SystemMessage::new("System prompt")));
        session.push(Message::User(UserMessage::text("Hello".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "Hi!".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        // Fork at index 2 (system + user)
        let forked = session.fork_at(2);
        assert_eq!(forked.messages().len(), 2);
        assert_ne!(forked.id(), session.id());

        // Full fork
        let full_fork = session.fork();
        assert_eq!(full_fork.messages().len(), 3);
    }

    #[test]
    fn test_session_forks_drop_generated_authority_metadata() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("original")));
        session.set_metadata("ordinary", serde_json::json!("keep"));
        session
            .set_build_state(SessionBuildState::default())
            .expect("build state should serialize");
        session
            .set_system_context_state(SessionSystemContextState::default())
            .expect("system-context state should serialize");
        session
            .set_deferred_turn_state(SessionDeferredTurnState::default())
            .expect("deferred-turn state should serialize");
        session
            .set_tool_visibility_state(
                AuthorizedSessionToolVisibilityState::from_generated_authority(
                    SessionToolVisibilityState::default(),
                ),
            )
            .expect("visibility state should serialize");
        let _ = session.append_realtime_transcript_event(RealtimeTranscriptEvent::ItemObserved {
            item_id: "rt-item".to_string(),
            previous_item_id: None,
            role: RealtimeTranscriptRole::User,
            response_id: None,
        });
        assert!(
            session
                .metadata()
                .contains_key(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
            "test setup should install realtime transcript authority state"
        );

        let forked_at = session.fork_at(1);
        let full_fork = session.fork();
        let replaced = session
            .fork_replacing(
                0,
                TranscriptReplacement::Message {
                    message: Message::User(UserMessage::text("replacement")),
                },
            )
            .expect("replacement fork should succeed");

        for forked in [&forked_at, &full_fork, &replaced] {
            assert_eq!(forked.metadata().get("ordinary").unwrap(), "keep");
            assert!(
                !forked.metadata().contains_key(SESSION_BUILD_STATE_KEY),
                "forked sessions must not raw-copy durable build-state authority"
            );
            assert!(
                !forked
                    .metadata()
                    .contains_key(SESSION_SYSTEM_CONTEXT_STATE_KEY),
                "forked sessions must not raw-copy system-context authority state"
            );
            assert!(
                !forked
                    .metadata()
                    .contains_key(SESSION_DEFERRED_TURN_STATE_KEY),
                "forked sessions must not raw-copy deferred-turn authority state"
            );
            assert!(
                !forked
                    .metadata()
                    .contains_key(SESSION_TOOL_VISIBILITY_STATE_KEY),
                "forked sessions must not raw-copy tool-visibility authority state"
            );
            assert!(
                !forked
                    .metadata()
                    .contains_key(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
                "forked sessions must not raw-copy realtime transcript authority state"
            );
        }
    }

    #[test]
    fn test_session_metadata() {
        let mut session = Session::new();
        session.set_metadata("key", serde_json::json!("value"));

        assert_eq!(session.metadata().get("key").unwrap(), "value");
    }

    #[test]
    fn test_session_metadata_backfill_preserves_timestamp() {
        let mut session = Session::new();
        let initial_updated = session.updated_at();

        std::thread::sleep(std::time::Duration::from_millis(10));

        assert!(session.backfill_metadata_if_absent("key", serde_json::json!("value")));
        assert_eq!(session.metadata().get("key").unwrap(), "value");
        assert_eq!(session.updated_at(), initial_updated);
        assert!(!session.backfill_metadata_if_absent("key", serde_json::json!("other")));
        assert_eq!(session.metadata().get("key").unwrap(), "value");
        assert_eq!(session.updated_at(), initial_updated);
    }

    #[test]
    fn test_reserved_generated_authority_metadata_rejects_raw_mutation() {
        let mut session = Session::new();

        assert!(
            session
                .try_set_metadata(SESSION_SYSTEM_CONTEXT_STATE_KEY, serde_json::json!({}))
                .is_err()
        );
        assert!(
            session
                .try_set_metadata(SESSION_METADATA_KEY, serde_json::json!({}))
                .is_err()
        );
        assert!(
            session
                .try_set_metadata(SESSION_BUILD_STATE_KEY, serde_json::json!({}))
                .is_err()
        );
        session
            .set_session_metadata(SessionMetadata {
                schema_version: SESSION_METADATA_SCHEMA_VERSION,
                model: "test-model".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
            })
            .expect("typed metadata setter should route through generated authority");
        session
            .set_build_state(SessionBuildState::default())
            .expect("typed build-state setter should route through generated authority");
        session.remove_metadata(SESSION_METADATA_KEY);
        session.remove_metadata(SESSION_BUILD_STATE_KEY);
        assert!(
            session.metadata().contains_key(SESSION_METADATA_KEY),
            "raw removal must not delete generated-authority session metadata"
        );
        assert!(
            session.metadata().contains_key(SESSION_BUILD_STATE_KEY),
            "raw removal must not delete generated-authority build state"
        );
        session.set_metadata(SESSION_DEFERRED_TURN_STATE_KEY, serde_json::json!({}));
        assert!(
            !session
                .metadata()
                .contains_key(SESSION_DEFERRED_TURN_STATE_KEY)
        );
        assert!(
            !session.backfill_metadata_if_absent(
                SESSION_SYSTEM_CONTEXT_STATE_KEY,
                serde_json::json!({})
            )
        );

        let state = SessionSystemContextState::default();
        session
            .set_system_context_state(state.clone())
            .expect("typed setter should route through generated authority");
        session.remove_metadata(SESSION_SYSTEM_CONTEXT_STATE_KEY);
        assert_eq!(
            session
                .try_system_context_state()
                .expect("typed state should restore"),
            Some(state)
        );

        session.metadata.insert(
            SESSION_SYSTEM_CONTEXT_STATE_KEY.to_string(),
            serde_json::json!("not-a-state"),
        );
        assert!(
            session.try_system_context_state().is_err(),
            "malformed generated authority state must not decode as absent/default"
        );

        session.metadata.insert(
            SESSION_METADATA_KEY.to_string(),
            serde_json::json!("not-metadata"),
        );
        assert!(
            session.try_session_metadata().is_err(),
            "malformed session metadata must not decode as absent/default"
        );

        session.metadata.insert(
            SESSION_BUILD_STATE_KEY.to_string(),
            serde_json::json!("not-build-state"),
        );
        assert!(
            session.try_build_state().is_err(),
            "malformed build state must not decode as absent/default"
        );

        assert!(
            session
                .try_set_metadata(SESSION_TOOL_VISIBILITY_STATE_KEY, serde_json::json!({}))
                .is_err()
        );
        session
            .set_tool_visibility_state(
                AuthorizedSessionToolVisibilityState::from_generated_authority(
                    SessionToolVisibilityState::default(),
                ),
            )
            .expect("typed visibility setter should route through typed authority handoff");
        session.remove_metadata(SESSION_TOOL_VISIBILITY_STATE_KEY);
        assert!(
            session
                .metadata()
                .contains_key(SESSION_TOOL_VISIBILITY_STATE_KEY)
        );
        session.clear_tool_visibility_state();
        assert!(
            !session
                .metadata()
                .contains_key(SESSION_TOOL_VISIBILITY_STATE_KEY)
        );
        assert!(
            session
                .try_set_metadata(SESSION_REALTIME_TRANSCRIPT_STATE_KEY, serde_json::json!({}))
                .is_err()
        );
        let _ = session.append_realtime_transcript_event(RealtimeTranscriptEvent::ItemObserved {
            item_id: "rt-item".to_string(),
            previous_item_id: None,
            role: RealtimeTranscriptRole::User,
            response_id: None,
        });
        assert!(
            session
                .metadata()
                .contains_key(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
            "typed realtime transcript append should retain authority to persist its state"
        );
        session.metadata.insert(
            SESSION_REALTIME_TRANSCRIPT_STATE_KEY.to_string(),
            serde_json::json!("not-a-state"),
        );
        assert!(
            session.try_realtime_transcript_state().is_err(),
            "malformed realtime generated authority state must not decode as absent/default"
        );
    }

    #[test]
    fn test_session_mob_tool_authority_context_persists_projection_without_authority_seal() {
        let mut session = Session::new();
        session
            .set_build_state(SessionBuildState::default())
            .expect("session build state should serialize");
        let authority = MobToolAuthorityContext::generated_for_test(
            crate::service::OpaquePrincipalToken::new("opaque-principal"),
            false,
            false,
            std::collections::BTreeSet::from(["mob-a".to_string()]),
            std::collections::BTreeMap::new(),
            None,
            Some("audit-1".to_string()),
        );

        session
            .set_mob_tool_authority_context(Some(authority))
            .expect("authority should serialize");
        assert!(session.mob_tool_authority_context().is_none());
        let stored = session
            .build_state()
            .and_then(|state| state.mob_tool_authority_context)
            .expect("stored projection should deserialize");
        assert!(!stored.is_generated_authority_context());
        assert!(!stored.can_manage_mob("mob-a"));

        session
            .set_mob_tool_authority_context(None)
            .expect("authority should clear");
        assert!(session.mob_tool_authority_context().is_none());
    }

    #[test]
    fn test_session_build_state_rejects_forged_mob_authority_projection() {
        let mut session = Session::new();
        let authority = MobToolAuthorityContext::generated_for_test(
            crate::service::OpaquePrincipalToken::new("opaque-principal"),
            false,
            false,
            std::collections::BTreeSet::from(["mob-a".to_string()]),
            std::collections::BTreeMap::new(),
            None,
            Some("audit-1".to_string()),
        );
        let forged_projection: MobToolAuthorityContext =
            serde_json::from_value(serde_json::to_value(authority).expect("serialize authority"))
                .expect("deserialize projection");
        assert!(!forged_projection.is_generated_authority_context());

        let err = session
            .set_build_state(SessionBuildState {
                mob_tool_authority_context: Some(forged_projection),
                ..Default::default()
            })
            .expect_err("forged build state must be rejected by generated authority");
        assert!(
            err.to_string()
                .contains("generated session durable-config authority rejected"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_session_tool_visibility_state_roundtrip() {
        let mut session = Session::new();
        let state = SessionToolVisibilityState {
            inherited_base_filter: ToolFilter::Allow(["visible".to_string()].into_iter().collect()),
            active_filter: ToolFilter::Allow(
                ["visible".to_string(), "missing".to_string()]
                    .into_iter()
                    .collect(),
            ),
            staged_filter: ToolFilter::Allow(
                ["visible".to_string(), "missing".to_string()]
                    .into_iter()
                    .collect(),
            ),
            active_revision: 1,
            staged_revision: 2,
            ..Default::default()
        };

        session
            .set_tool_visibility_state(
                AuthorizedSessionToolVisibilityState::from_generated_authority(state.clone()),
            )
            .expect("tool visibility state should serialize");
        assert_eq!(session.tool_visibility_state().unwrap(), Some(state));
    }

    #[test]
    fn test_session_tool_visibility_state_malformed_returns_error() {
        let mut session = Session::new();
        session.metadata.insert(
            SESSION_TOOL_VISIBILITY_STATE_KEY.to_string(),
            serde_json::json!({
                "active_filter": {
                    "unexpected_filter_kind": ["secret"]
                }
            }),
        );

        assert!(
            session.tool_visibility_state().is_err(),
            "malformed canonical visibility metadata must not decode as absent/default"
        );
    }

    #[test]
    fn test_session_serialization() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Test".to_string())));

        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id(), session.id());
        assert_eq!(parsed.messages().len(), 1);
        assert_eq!(parsed.version(), SESSION_VERSION);
    }

    #[test]
    fn test_session_meta_from_session() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Hello".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "Hi!".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
            created_at: crate::types::message_timestamp_now(),
        }));
        session.record_usage(Usage {
            input_tokens: 10,
            output_tokens: 5,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        });

        let meta = SessionMeta::from(&session);
        assert_eq!(meta.id, *session.id());
        assert_eq!(meta.message_count, 2);
        assert_eq!(meta.total_tokens, 15);
    }

    #[test]
    fn system_context_state_preserves_applied_runtime_context() {
        let accepted_at = SystemTime::UNIX_EPOCH;
        let mut state = SessionSystemContextState::default();
        state
            .stage_append(
                &AppendSystemContextRequest {
                    text: "Authoritative peer token is birch seventeen.".to_string(),
                    source: Some(
                        "peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
                            .to_string(),
                    ),
                    idempotency_key: Some("018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
                    source_kind: SystemContextSource::Normal,
                },
                accepted_at,
            )
            .expect("append should stage");

        state.mark_pending_applied();

        assert!(state.pending.is_empty());
        assert_eq!(state.applied.len(), 1);
        assert_eq!(
            state.applied[0].text,
            "Authoritative peer token is birch seventeen."
        );
        assert_eq!(
            state.applied[0].source.as_deref(),
            Some("peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1")
        );

        let round_tripped: SessionSystemContextState =
            serde_json::from_value(serde_json::to_value(&state).expect("serialize state"))
                .expect("deserialize state");
        assert_eq!(round_tripped.applied, state.applied);
    }

    #[test]
    fn active_turn_system_context_is_discarded_when_not_applied() {
        let mut state = SessionSystemContextState::default();
        state
            .stage_active_turn_append(
                &AppendSystemContextRequest {
                    text: "only for the active run".to_string(),
                    source: Some("runtime:steer:input-1".to_string()),
                    idempotency_key: Some("runtime:steer:input-1".to_string()),
                    source_kind: SystemContextSource::RuntimeSteer,
                },
                SystemTime::UNIX_EPOCH,
            )
            .expect("active context should stage");

        let discarded = state.discard_unapplied_active_turn_pending();

        assert_eq!(discarded.len(), 1);
        assert!(state.pending.is_empty());
        assert!(state.applied.is_empty());
        assert!(state.active_turn_pending_keys.is_empty());
        assert!(
            state.seen.is_empty(),
            "discarded active-turn context should not block later idempotency keys"
        );
    }

    #[test]
    fn active_turn_system_context_can_roll_back_targeted_keys() {
        let mut state = SessionSystemContextState::default();
        for key in ["runtime:steer:input-1", "runtime:steer:input-2"] {
            state
                .stage_active_turn_append(
                    &AppendSystemContextRequest {
                        text: format!("context for {key}"),
                        source: Some(key.to_string()),
                        idempotency_key: Some(key.to_string()),
                        source_kind: SystemContextSource::RuntimeSteer,
                    },
                    SystemTime::UNIX_EPOCH,
                )
                .expect("active context should stage");
        }

        let discarded =
            state.discard_active_turn_pending_by_keys(&["runtime:steer:input-1".to_string()]);

        assert_eq!(discarded.len(), 1);
        assert_eq!(
            discarded[0].idempotency_key.as_deref(),
            Some("runtime:steer:input-1")
        );
        assert_eq!(state.pending.len(), 1);
        assert_eq!(
            state.pending[0].idempotency_key.as_deref(),
            Some("runtime:steer:input-2")
        );
        assert!(!state.seen.contains_key("runtime:steer:input-1"));
        assert!(state.seen.contains_key("runtime:steer:input-2"));
        assert!(
            !state
                .active_turn_pending_keys
                .contains("runtime:steer:input-1")
        );
        assert!(
            state
                .active_turn_pending_keys
                .contains("runtime:steer:input-2")
        );
    }

    #[test]
    fn active_turn_system_context_is_transient_when_boundary_consumes_it() {
        let mut state = SessionSystemContextState::default();
        state
            .stage_active_turn_append(
                &AppendSystemContextRequest {
                    text: "visible to this run".to_string(),
                    source: Some("runtime:steer:input-2".to_string()),
                    idempotency_key: Some("runtime:steer:input-2".to_string()),
                    source_kind: SystemContextSource::RuntimeSteer,
                },
                SystemTime::UNIX_EPOCH,
            )
            .expect("active context should stage");

        state.mark_pending_applied();
        let discarded = state.discard_unapplied_active_turn_pending();

        assert!(discarded.is_empty());
        assert!(state.pending.is_empty());
        assert!(state.applied.is_empty());
        assert!(state.active_turn_pending_keys.is_empty());
        assert_eq!(
            state.seen.get("runtime:steer:input-2"),
            None,
            "consumed active-turn steer context must not become durable state"
        );
    }

    #[test]
    fn discard_transient_runtime_steer_context_removes_steer_via_typed_marker() {
        let mut session = Session::new();
        // The runtime-steer fact is carried by the typed `source_kind`, not by
        // the `source` string. The durable peer fact uses the same `source`
        // string scheme but is marked `Normal`, so only the steers are removed.
        session.set_system_prompt(format!(
            "base{}{}{}{}",
            SYSTEM_CONTEXT_SEPARATOR,
            render_system_context_block(&PendingSystemContextAppend {
                text: "old steer".to_string(),
                source: Some("steer-source-old".to_string()),
                idempotency_key: Some("steer-key-old".to_string()),
                source_kind: SystemContextSource::RuntimeSteer,
                accepted_at: SystemTime::UNIX_EPOCH,
            }),
            SYSTEM_CONTEXT_SEPARATOR,
            render_system_context_block(&PendingSystemContextAppend {
                text: "durable peer fact".to_string(),
                source: Some("peer_response_terminal:analyst:req".to_string()),
                idempotency_key: Some("peer_response_terminal:analyst:req".to_string()),
                source_kind: SystemContextSource::Normal,
                accepted_at: SystemTime::UNIX_EPOCH,
            })
        ));
        session
            .set_system_context_state(SessionSystemContextState {
                pending: vec![PendingSystemContextAppend {
                    text: "pending steer".to_string(),
                    source: Some("steer-source-pending".to_string()),
                    idempotency_key: Some("steer-key-pending".to_string()),
                    source_kind: SystemContextSource::RuntimeSteer,
                    accepted_at: SystemTime::UNIX_EPOCH,
                }],
                applied: vec![
                    PendingSystemContextAppend {
                        text: "old steer".to_string(),
                        source: Some("steer-source-old".to_string()),
                        idempotency_key: Some("steer-key-old".to_string()),
                        source_kind: SystemContextSource::RuntimeSteer,
                        accepted_at: SystemTime::UNIX_EPOCH,
                    },
                    PendingSystemContextAppend {
                        text: "durable peer fact".to_string(),
                        source: Some("peer_response_terminal:analyst:req".to_string()),
                        idempotency_key: Some("peer_response_terminal:analyst:req".to_string()),
                        source_kind: SystemContextSource::Normal,
                        accepted_at: SystemTime::UNIX_EPOCH,
                    },
                ],
                seen: BTreeMap::from([(
                    "steer-key-old".to_string(),
                    SeenSystemContextKey {
                        text: "old steer".to_string(),
                        source: Some("steer-source-old".to_string()),
                        source_kind: SystemContextSource::RuntimeSteer,
                        state: SeenSystemContextState::Applied,
                    },
                )]),
                active_turn_pending_keys: BTreeSet::from(["steer-key-pending".to_string()]),
            })
            .expect("system context state should serialize");

        let removed = session.discard_transient_runtime_steer_context();

        assert!(removed >= 4);
        let system_prompt = match session.messages().first() {
            Some(Message::System(system)) => system.content.as_str(),
            other => panic!("expected system prompt, got {other:?}"),
        };
        assert!(!system_prompt.contains("old steer"));
        assert!(system_prompt.contains("durable peer fact"));
        let state = session.system_context_state().unwrap_or_default();
        assert!(state.pending.is_empty());
        assert_eq!(state.applied.len(), 1);
        assert_eq!(state.applied[0].text, "durable peer fact");
        assert!(state.seen.is_empty());
        assert!(state.active_turn_pending_keys.is_empty());
    }

    #[test]
    fn append_system_context_blocks_records_typed_applied_context() {
        let append = PendingSystemContextAppend {
            text: "Authoritative peer token is birch seventeen.".to_string(),
            source: Some(
                "peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string(),
            ),
            idempotency_key: Some("018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
            source_kind: SystemContextSource::Normal,
            accepted_at: SystemTime::UNIX_EPOCH,
        };
        let mut session = Session::new();

        session.append_system_context_blocks(std::slice::from_ref(&append));

        let state = session
            .system_context_state()
            .expect("append should persist typed context state");
        assert_eq!(state.applied, vec![append]);
    }

    #[test]
    fn append_system_context_blocks_renders_pre_marked_pending_context() {
        let accepted_at = SystemTime::UNIX_EPOCH;
        let mut state = SessionSystemContextState::default();
        state
            .stage_append(
                &AppendSystemContextRequest {
                    text: "Apply this staged context at the request boundary.".to_string(),
                    source: Some("rpc/session_inject_context".to_string()),
                    idempotency_key: Some("ctx-boundary".to_string()),
                    source_kind: SystemContextSource::Normal,
                },
                accepted_at,
            )
            .expect("append should stage");
        let pending = state.pending.clone();
        state.mark_pending_applied();
        let mut session = Session::new();
        session
            .set_system_context_state(state)
            .expect("state should serialize");

        session.append_system_context_blocks(&pending);

        let system_prompt = session
            .messages()
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(system_prompt.contains("Apply this staged context at the request boundary."));
        let state = session
            .system_context_state()
            .expect("append should persist typed context state");
        assert_eq!(state.applied.len(), 1);
        assert_eq!(
            state.seen["ctx-boundary"].state,
            SeenSystemContextState::Applied
        );
    }

    #[test]
    fn append_system_context_blocks_renders_pre_marked_context_without_idempotency_key() {
        let accepted_at = SystemTime::UNIX_EPOCH;
        let mut state = SessionSystemContextState::default();
        state
            .stage_append(
                &AppendSystemContextRequest {
                    text: "Apply this unkeyed staged context at the request boundary.".to_string(),
                    source: Some("rpc/session_inject_context".to_string()),
                    idempotency_key: None,
                    source_kind: SystemContextSource::Normal,
                },
                accepted_at,
            )
            .expect("append should stage");
        let pending = state.pending.clone();
        state.mark_pending_applied();
        let mut session = Session::new();
        session
            .set_system_context_state(state)
            .expect("state should serialize");

        session.append_system_context_blocks(&pending);

        let system_prompt = session
            .messages()
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            system_prompt.contains("Apply this unkeyed staged context at the request boundary.")
        );
    }

    #[test]
    fn append_system_context_blocks_skips_duplicate_idempotency_key() {
        let first = PendingSystemContextAppend {
            text: "Authoritative peer token is birch seventeen.".to_string(),
            source: Some("peer_response_terminal:analyst:req-1".to_string()),
            idempotency_key: Some("req-1".to_string()),
            source_kind: SystemContextSource::Normal,
            accepted_at: SystemTime::UNIX_EPOCH,
        };
        let duplicate = PendingSystemContextAppend {
            accepted_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
            ..first.clone()
        };
        let mut session = Session::new();

        session.append_system_context_blocks(std::slice::from_ref(&first));
        session.append_system_context_blocks(std::slice::from_ref(&duplicate));

        let state = session
            .system_context_state()
            .expect("append should persist typed context state");
        assert_eq!(state.applied, vec![first]);
        let system_prompt = session
            .messages()
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        assert_eq!(
            system_prompt
                .matches("Authoritative peer token is birch seventeen.")
                .count(),
            1
        );
    }

    #[test]
    fn append_system_context_blocks_skips_conflicting_duplicate_idempotency_key() {
        let first = PendingSystemContextAppend {
            text: "Authoritative peer token is birch seventeen.".to_string(),
            source: Some("peer_response_terminal:analyst:req-1".to_string()),
            idempotency_key: Some("req-1".to_string()),
            source_kind: SystemContextSource::Normal,
            accepted_at: SystemTime::UNIX_EPOCH,
        };
        let conflicting = PendingSystemContextAppend {
            text: "Conflicting peer token should not reach the prompt.".to_string(),
            accepted_at: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
            ..first.clone()
        };
        let mut session = Session::new();

        session.append_system_context_blocks(std::slice::from_ref(&first));
        session.append_system_context_blocks(std::slice::from_ref(&conflicting));

        let state = session
            .system_context_state()
            .expect("append should persist typed context state");
        assert_eq!(state.applied, vec![first]);
        let system_prompt = session
            .messages()
            .first()
            .and_then(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(system_prompt.contains("Authoritative peer token is birch seventeen."));
        assert!(!system_prompt.contains("Conflicting peer token should not reach the prompt."));
    }

    // ------------------------------------------------------------------
    // T9/T10: realtime transcript lane materialization.
    //
    // The display-text lane (`AssistantTextDelta`) materializes as
    // `AssistantBlock::Text`; the spoken-transcript lane
    // (`AssistantTranscriptDelta`) materializes as
    // `AssistantBlock::Transcript { source: TranscriptSource::Spoken }`.
    // These regressions pin both flushes and prove the materializer
    // dispatches on the per-item `TranscriptLane`.
    // ------------------------------------------------------------------

    #[test]
    fn realtime_transcript_assistant_transcript_delta_materializes_transcript_block() {
        let mut session = Session::new();

        let delta = RealtimeTranscriptEvent::AssistantTranscriptDelta {
            response_id: "resp_spoken".to_string(),
            delta_id: "evt_delta_spoken_1".to_string(),
            item_id: "item_spoken".to_string(),
            previous_item_id: None,
            content_index: 0,
            delta: "I said hi".to_string(),
        };
        assert!(
            session.append_realtime_transcript_event(delta).is_inert(),
            "delta alone is inert until turn-completed flushes"
        );

        let terminal = RealtimeTranscriptEvent::AssistantTurnCompleted {
            response_id: "resp_spoken".to_string(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let outcome = session.append_realtime_transcript_event(terminal);
        assert_eq!(outcome.materialized_messages.len(), 1);

        // T9/T10: must be a Transcript block, NOT Text.
        let messages = session.messages();
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            Message::BlockAssistant(assistant) => {
                assert_eq!(assistant.blocks.len(), 1);
                match &assistant.blocks[0] {
                    AssistantBlock::Transcript { text, source, .. } => {
                        assert_eq!(text, "I said hi");
                        assert_eq!(*source, crate::types::TranscriptSource::Spoken);
                    }
                    other => unreachable!(
                        "AssistantTranscriptDelta must materialize as AssistantBlock::Transcript, got {other:?}"
                    ),
                }
            }
            other => unreachable!("expected BlockAssistant message, got {other:?}"),
        }
    }

    #[test]
    fn round4_cc4_in_flight_response_ids_lists_distinct_unmaterialized_responses() {
        // CC4 (Round-4 architectural reconciliation): the helper that
        // powers `signal_turn_interrupt`'s cross-layer fan-out must
        // return every distinct provider response_id that has at least
        // one unmaterialized assistant item, EXCLUDING already-discarded
        // responses and EXCLUDING the user role.
        let mut session = Session::new();

        // Two transcript-delta items on resp_a (different content_index
        // ranges), one on resp_b. resp_c gets a delta and is then
        // discarded explicitly via AssistantTurnInterrupted.
        for (i, response_id) in [
            ("resp_a", "resp_a"),
            ("resp_a_extra", "resp_a"),
            ("resp_b", "resp_b"),
            ("resp_c", "resp_c"),
        ]
        .iter()
        .enumerate()
        {
            let event = RealtimeTranscriptEvent::AssistantTranscriptDelta {
                response_id: response_id.1.to_string(),
                delta_id: format!("delta_{i}"),
                item_id: response_id.0.to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "x".to_string(),
            };
            let _ = session.append_realtime_transcript_event(event);
        }

        // Discard resp_c — it should not appear in the in-flight list.
        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnInterrupted {
                response_id: "resp_c".to_string(),
            },
        );

        // User-role item should never appear (CC4 only fans interrupts
        // to assistant responses).
        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: "u_item".to_string(),
                previous_item_id: None,
                content_index: 0,
                text: "hi".to_string(),
            },
        );

        let in_flight = session.in_flight_realtime_assistant_response_ids();
        assert!(in_flight.contains(&"resp_a".to_string()), "{in_flight:?}");
        assert!(in_flight.contains(&"resp_b".to_string()), "{in_flight:?}");
        assert!(
            !in_flight.contains(&"resp_c".to_string()),
            "discarded response must not appear in in_flight: {in_flight:?}"
        );
        // resp_a appears exactly once even though two items reference it.
        assert_eq!(
            in_flight.iter().filter(|r| *r == "resp_a").count(),
            1,
            "distinct response_ids only: {in_flight:?}"
        );
    }

    #[test]
    fn round4_cc2_assistant_turn_completed_after_transcript_deltas_materializes_transcript() {
        // CC2 (Round-4 architectural reconciliation): once
        // `signal_turn_completed` synthesizes
        // `RealtimeTranscriptEvent::AssistantTurnCompleted`, the staging
        // materializer commits every staged transcript-delta item for
        // that response_id as `AssistantBlock::Transcript { Spoken }`.
        // This pins the production end-to-end shape the sink relies on.
        let mut session = Session::new();

        let delta = RealtimeTranscriptEvent::AssistantTranscriptDelta {
            response_id: "resp_cc2".to_string(),
            delta_id: "delta_cc2_1".to_string(),
            item_id: "item_cc2".to_string(),
            previous_item_id: None,
            content_index: 0,
            delta: "hello world".to_string(),
        };
        assert!(session.append_realtime_transcript_event(delta).is_inert());

        // Pre-completion: in-flight list reports resp_cc2.
        assert_eq!(
            session.in_flight_realtime_assistant_response_ids(),
            vec!["resp_cc2".to_string()]
        );

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_cc2".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert_eq!(outcome.materialized_messages.len(), 1);

        // Post-completion: in-flight list is empty (item is materialized).
        assert!(
            session
                .in_flight_realtime_assistant_response_ids()
                .is_empty(),
            "materialized items must not appear in in_flight_realtime_assistant_response_ids"
        );

        let messages = session.messages();
        let assistant = messages.iter().find_map(|m| match m {
            Message::BlockAssistant(a) => Some(a),
            _ => None,
        });
        let assistant = assistant.expect("assistant block message expected");
        assert_eq!(assistant.blocks.len(), 1);
        assert!(matches!(
            &assistant.blocks[0],
            AssistantBlock::Transcript {
                source: crate::types::TranscriptSource::Spoken,
                ..
            }
        ));
    }

    #[test]
    fn realtime_transcript_assistant_text_delta_still_materializes_text_block() {
        // Counter-regression: the display-text lane must continue to
        // produce `AssistantBlock::Text` after T9/T10. Prevents an
        // accidental cross-lane flip.
        let mut session = Session::new();

        let delta = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: "resp_display".to_string(),
            delta_id: "evt_delta_display_1".to_string(),
            item_id: "item_display".to_string(),
            previous_item_id: None,
            content_index: 0,
            delta: "I wrote".to_string(),
        };
        let _ = session.append_realtime_transcript_event(delta);

        let terminal = RealtimeTranscriptEvent::AssistantTurnCompleted {
            response_id: "resp_display".to_string(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let outcome = session.append_realtime_transcript_event(terminal);
        assert_eq!(outcome.materialized_messages.len(), 1);

        let messages = session.messages();
        match &messages[0] {
            Message::BlockAssistant(assistant) => match &assistant.blocks[0] {
                AssistantBlock::Text { text, .. } => assert_eq!(text, "I wrote"),
                other => unreachable!(
                    "AssistantTextDelta must keep materializing AssistantBlock::Text, got {other:?}"
                ),
            },
            other => unreachable!("expected BlockAssistant message, got {other:?}"),
        }
    }

    #[test]
    fn round4_cc7_mixed_response_persists_text_and_transcript_in_order() {
        // CC7 (Round-4 adversarial-verifier follow-up): a single mixed-modality
        // realtime response that emits BOTH display-text deltas
        // (`AssistantTextDelta`) AND spoken-transcript deltas
        // (`AssistantTranscriptDelta`) under the same response_id must
        // materialize as ONE `Message::BlockAssistant` whose `blocks` field
        // contains exactly two ordered entries:
        //   1. AssistantBlock::Text       (display-text lane)
        //   2. AssistantBlock::Transcript { source: Spoken } (spoken lane)
        // Pre-fix the materializer emitted one Message::BlockAssistant per
        // staged item, splitting the mixed response into two messages.
        //
        // This test drives the production materializer end-to-end: deltas
        // stage in `SessionRealtimeTranscriptState`; `AssistantTurnCompleted`
        // triggers the materializer; canonical history is the assertion
        // surface — exactly the same code path that
        // `SessionServiceProjectionSink::signal_turn_completed` invokes via
        // `runtime.append_realtime_transcript_event` in production.
        let mut session = Session::new();

        // Provider-arrival order: display first, then spoken.
        let display_a = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: "resp_mixed_1".to_string(),
            delta_id: "delta_disp_1".to_string(),
            item_id: "item_display".to_string(),
            previous_item_id: None,
            content_index: 0,
            delta: "Here's the report:".to_string(),
        };
        assert!(
            session
                .append_realtime_transcript_event(display_a)
                .is_inert()
        );

        let display_b = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: "resp_mixed_1".to_string(),
            delta_id: "delta_disp_2".to_string(),
            item_id: "item_display".to_string(),
            previous_item_id: None,
            content_index: 0,
            delta: " (still writing)".to_string(),
        };
        assert!(
            session
                .append_realtime_transcript_event(display_b)
                .is_inert()
        );

        // Spoken items chain after the display item to mirror provider
        // arrival semantics — `previous_item_id` carries arrival ordering
        // that the materializer must preserve as block ordering inside the
        // single emitted message.
        let spoken_a = RealtimeTranscriptEvent::AssistantTranscriptDelta {
            response_id: "resp_mixed_1".to_string(),
            delta_id: "delta_spoken_1".to_string(),
            item_id: "item_spoken".to_string(),
            previous_item_id: Some("item_display".to_string()),
            content_index: 0,
            delta: "I'm reading the report aloud:".to_string(),
        };
        assert!(
            session
                .append_realtime_transcript_event(spoken_a)
                .is_inert()
        );

        let spoken_b = RealtimeTranscriptEvent::AssistantTranscriptDelta {
            response_id: "resp_mixed_1".to_string(),
            delta_id: "delta_spoken_2".to_string(),
            item_id: "item_spoken".to_string(),
            previous_item_id: Some("item_display".to_string()),
            content_index: 0,
            delta: " sentence two.".to_string(),
        };
        assert!(
            session
                .append_realtime_transcript_event(spoken_b)
                .is_inert()
        );

        // TurnCompleted triggers the materializer to flush all staged items
        // for this response_id into ONE BlockAssistant message.
        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_mixed_1".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 11,
                    output_tokens: 22,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
        );
        // Materializer reports two staged items got materialized.
        assert_eq!(outcome.materialized_messages.len(), 2);

        // Canonical history MUST contain exactly ONE BlockAssistant message
        // (the CC7 fix: mixed lanes interleave into one message, not two).
        let messages = session.messages();
        let assistants: Vec<&BlockAssistantMessage> = messages
            .iter()
            .filter_map(|m| match m {
                Message::BlockAssistant(a) => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(
            assistants.len(),
            1,
            "mixed display+spoken response under one response_id must produce exactly ONE BlockAssistant message, got: {assistants:?}"
        );
        let assistant = assistants[0];
        assert_eq!(
            assistant.blocks.len(),
            2,
            "mixed response message must carry both blocks: {:?}",
            assistant.blocks
        );

        // Block 0: display-text (concatenated deltas).
        match &assistant.blocks[0] {
            AssistantBlock::Text { text, .. } => {
                assert_eq!(text, "Here's the report: (still writing)");
            }
            other => unreachable!(
                "first block must be AssistantBlock::Text (display lane), got {other:?}"
            ),
        }
        // Block 1: spoken transcript (concatenated deltas), tagged Spoken.
        match &assistant.blocks[1] {
            AssistantBlock::Transcript { text, source, .. } => {
                assert_eq!(text, "I'm reading the report aloud: sentence two.");
                assert_eq!(*source, crate::types::TranscriptSource::Spoken);
            }
            other => unreachable!(
                "second block must be AssistantBlock::Transcript {{ source: Spoken }}, got {other:?}"
            ),
        }

        // Usage was recorded once for the turn.
        assert_eq!(session.usage.input_tokens, 11);
        assert_eq!(session.usage.output_tokens, 22);
    }

    #[test]
    fn round5_r55_mixed_response_barge_in_preserves_display_drops_spoken() {
        // R5-5 (Round-5 contract update): barge-in MUST filter staged items
        // by lane — `Spoken` is invalidated (the user spoke over the audio
        // they were hearing) but `Display` survives as committed history
        // (sideband display text from the same response is not "spoken
        // over"). Round-4's `round4_cc7_mixed_response_barge_in_discards_*`
        // pinned the wrong invariant; this test replaces it.
        //
        // Architectural decision: `AssistantTurnInterrupted` is terminal for
        // the response on the realtime-staging path — any later
        // `AssistantTurnCompleted { stop_reason: Cancelled }` short-circuits
        // via the `discarded_assistant_response_ids` guard. So the
        // Interrupted handler must seed a synthetic
        // `assistant_completions` entry (`StopReason::Cancelled`,
        // `Usage::default()`) so retained Display items materialize
        // immediately rather than stranding forever.
        let mut session = Session::new();

        let display = RealtimeTranscriptEvent::AssistantTextDelta {
            response_id: "resp_mixed_2".to_string(),
            delta_id: "delta_disp_1".to_string(),
            item_id: "item_display_2".to_string(),
            previous_item_id: None,
            content_index: 0,
            delta: "Working on the report...".to_string(),
        };
        let _ = session.append_realtime_transcript_event(display);

        let spoken = RealtimeTranscriptEvent::AssistantTranscriptDelta {
            response_id: "resp_mixed_2".to_string(),
            delta_id: "delta_spoken_1".to_string(),
            item_id: "item_spoken_2".to_string(),
            previous_item_id: Some("item_display_2".to_string()),
            content_index: 0,
            delta: "I'm reading the report".to_string(),
        };
        let _ = session.append_realtime_transcript_event(spoken);

        // Barge-in arrives BEFORE TurnCompleted. The Display item with
        // staged content materializes immediately under the synthetic
        // Cancelled completion.
        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnInterrupted {
                response_id: "resp_mixed_2".to_string(),
            },
        );
        assert_eq!(
            outcome.materialized_messages.len(),
            1,
            "Display lane item must materialize on Interrupted: {outcome:?}"
        );

        // A late `AssistantTurnCompleted` (the provider's response.done
        // emitted after cancel) must be a no-op: the Display item is
        // already materialized; the Spoken item was dropped at Interrupted.
        let late_completion = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_mixed_2".to_string(),
                stop_reason: StopReason::Cancelled,
                usage: Usage::default(),
            },
        );
        assert_eq!(
            late_completion.materialized_messages.len(),
            0,
            "post-barge-in TurnCompleted must not resurrect anything"
        );

        // Canonical history: exactly one BlockAssistant carrying the
        // Display text (no Transcript block — Spoken was dropped).
        let messages = session.messages();
        let assistants: Vec<&BlockAssistantMessage> = messages
            .iter()
            .filter_map(|m| match m {
                Message::BlockAssistant(a) => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(
            assistants.len(),
            1,
            "barge-in must commit exactly one BlockAssistant containing the Display lane: {assistants:?}"
        );
        let assistant = assistants[0];
        assert_eq!(assistant.blocks.len(), 1, "blocks: {:?}", assistant.blocks);
        match &assistant.blocks[0] {
            AssistantBlock::Text { text, .. } => {
                assert_eq!(text, "Working on the report...");
            }
            other => {
                unreachable!("Display lane must materialize as AssistantBlock::Text, got {other:?}")
            }
        }
        // No Transcript block — Spoken lane was dropped.
        assert!(
            !assistant
                .blocks
                .iter()
                .any(|b| matches!(b, AssistantBlock::Transcript { .. })),
            "Spoken lane must be dropped on barge-in"
        );

        // The in-flight tracker reports the response as no longer in flight
        // (the Display item is materialized; the Spoken item is skipped).
        assert!(
            !session
                .in_flight_realtime_assistant_response_ids()
                .contains(&"resp_mixed_2".to_string()),
            "barged-in response must not appear in in_flight_realtime_assistant_response_ids"
        );
    }

    #[test]
    fn round5_r55_barge_in_preserves_display_lane_drops_spoken() {
        // R5-5 unit test: pin the lane-filter behavior at the staged-item
        // level (no chained predecessor). One Display item, one Spoken item,
        // both unchained, both staged before Interrupted.
        let mut session = Session::new();

        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_d_1".to_string(),
                item_id: "item_display".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "display-text".to_string(),
            });
        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTranscriptDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_s_1".to_string(),
                item_id: "item_spoken".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "spoken-transcript".to_string(),
            },
        );

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnInterrupted {
                response_id: "resp_a".to_string(),
            },
        );
        // Display materializes, Spoken does not.
        assert_eq!(outcome.materialized_messages.len(), 1);

        let messages = session.messages();
        let assistants: Vec<&BlockAssistantMessage> = messages
            .iter()
            .filter_map(|m| match m {
                Message::BlockAssistant(a) => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(assistants.len(), 1);
        // Single Text block (the Display lane) — no Transcript.
        assert_eq!(assistants[0].blocks.len(), 1);
        match &assistants[0].blocks[0] {
            AssistantBlock::Text { text, .. } => assert_eq!(text, "display-text"),
            other => unreachable!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn round5_r55_barge_in_finalizes_retained_display_into_committed_block() {
        // R5-5: the architectural decision — Interrupted is terminal for the
        // response. Display lane must commit at Interrupted time, not wait
        // on a hypothetical AssistantTurnCompleted that may never arrive
        // (or arrives Cancelled and short-circuits).
        let mut session = Session::new();

        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_d_1".to_string(),
                item_id: "item_display".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "committed-display-text".to_string(),
            });

        // Pre-condition: nothing committed yet.
        assert!(session.messages().is_empty());

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnInterrupted {
                response_id: "resp_a".to_string(),
            },
        );
        assert_eq!(
            outcome.materialized_messages.len(),
            1,
            "Interrupted must finalize retained Display lane immediately"
        );

        // Post-condition: BlockAssistant in canonical history, no Transcript.
        let messages = session.messages();
        assert_eq!(messages.len(), 1);
        match &messages[0] {
            Message::BlockAssistant(assistant) => {
                assert_eq!(assistant.blocks.len(), 1);
                match &assistant.blocks[0] {
                    AssistantBlock::Text { text, .. } => {
                        assert_eq!(text, "committed-display-text");
                    }
                    other => unreachable!("expected Text, got {other:?}"),
                }
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }

    #[test]
    fn round5_r56_truncation_promotes_default_lane_item_to_spoken() {
        // R5-6: when truncation is the first content-bearing event for an
        // item (no prior delta), the staged item's lane MUST be promoted to
        // Spoken so the materializer commits as `AssistantBlock::Transcript`.
        // Without the explicit promotion, the lane stays `Display` (the
        // default) and the heard audio transcript persists as
        // `AssistantBlock::Text`.
        let mut session = Session::new();

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                response_id: "resp_a".to_string(),
                item_id: "item_a".to_string(),
                content_index: 0,
                text: "what was actually heard".to_string(),
            },
        );

        let outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert_eq!(outcome.materialized_messages.len(), 1);

        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::BlockAssistant(assistant) => {
                assert_eq!(assistant.blocks.len(), 1);
                match &assistant.blocks[0] {
                    AssistantBlock::Transcript { text, source, .. } => {
                        assert_eq!(text, "what was actually heard");
                        assert_eq!(*source, crate::types::TranscriptSource::Spoken);
                    }
                    other => unreachable!(
                        "truncation-only path must materialize as AssistantBlock::Transcript, got {other:?}"
                    ),
                }
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }

    #[test]
    fn round5_r56_truncation_after_display_delta_is_no_op_keeping_display_content() {
        // R5-6 edge case: a Display delta arrived first and staged Display
        // content; a truncation event arrives for the SAME item id
        // (provider bug — truncation only applies to spoken/audio output).
        // Contract: the staged Display content must NOT be clobbered by
        // the truncation text. `promote_item_lane` keeps the existing
        // Display lane and emits a `tracing::warn!`; the truncation arm
        // sees the lane stayed Display and skips the segment-write.
        let mut session = Session::new();

        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_d_1".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "display-text-from-delta".to_string(),
            });

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                response_id: "resp_a".to_string(),
                item_id: "item_a".to_string(),
                content_index: 0,
                text: "spoken-truncation-text".to_string(),
            },
        );

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );

        // Display content survives unchanged — the truncation text was
        // refused. Materializes as `AssistantBlock::Text` (Display lane).
        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::BlockAssistant(assistant) => {
                assert_eq!(assistant.blocks.len(), 1);
                match &assistant.blocks[0] {
                    AssistantBlock::Text { text, .. } => {
                        assert_eq!(text, "display-text-from-delta");
                    }
                    other => unreachable!(
                        "Display content must survive misrouted truncation, got {other:?}"
                    ),
                }
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }

    /// R5-6 sibling: a Spoken-classified item (transcript-truncation
    /// arrived first and locked the lane to Spoken) must reject a later
    /// `AssistantTextDelta` rather than silently appending the Display
    /// text into the Spoken-locked content_segment. Pre-fix the delta
    /// arm called `promote_item_lane` and unconditionally pushed the
    /// delta — clobbering the lane invariant. Post-fix the delta is
    /// dropped (warn fires) and the Spoken-truncation text survives.
    #[test]
    fn round5_r56_sibling_display_delta_skipped_on_spoken_item() {
        let mut session = Session::new();

        // Truncation arrives first and locks the item to the Spoken lane.
        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                response_id: "resp_a".to_string(),
                item_id: "item_a".to_string(),
                content_index: 0,
                text: "what was actually heard".to_string(),
            },
        );

        // A Display delta arrives later for the SAME item id (provider
        // lane-classification bug). It MUST be dropped.
        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_d_1".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "should-not-appear".to_string(),
            });

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );

        // The Spoken-truncation text survives intact; no Display text
        // leaked into the Spoken lane content.
        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::BlockAssistant(assistant) => {
                assert_eq!(assistant.blocks.len(), 1);
                match &assistant.blocks[0] {
                    AssistantBlock::Transcript { text, source, .. } => {
                        assert_eq!(text, "what was actually heard");
                        assert_eq!(*source, crate::types::TranscriptSource::Spoken);
                    }
                    other => unreachable!(
                        "Spoken-locked item must materialize as Transcript, got {other:?}"
                    ),
                }
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }

    /// R5-6 sibling: a Display-classified item (a Display delta arrived
    /// first and locked the lane to Display) must reject a later
    /// `AssistantTranscriptDelta` rather than appending the Spoken text
    /// into the Display-locked content_segment. Pre-fix the transcript
    /// delta arm called `promote_item_lane` and unconditionally pushed —
    /// silently mixing a Spoken stream into a Display block.
    #[test]
    fn round5_r56_sibling_spoken_delta_skipped_on_display_item() {
        let mut session = Session::new();

        // Display delta arrives first and locks the item to the Display lane.
        let _ =
            session.append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_d_1".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "display-locked-text".to_string(),
            });

        // A spoken-transcript delta arrives later for the SAME item id
        // (provider lane-classification bug). It MUST be dropped.
        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTranscriptDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_s_1".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "should-not-appear".to_string(),
            },
        );

        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );

        // The Display text survives intact; no Spoken text leaked in.
        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::BlockAssistant(assistant) => {
                assert_eq!(assistant.blocks.len(), 1);
                match &assistant.blocks[0] {
                    AssistantBlock::Text { text, .. } => {
                        assert_eq!(text, "display-locked-text");
                    }
                    other => {
                        unreachable!("Display-locked item must materialize as Text, got {other:?}")
                    }
                }
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }

    /// R5-7: a late `AssistantTranscriptFinalText` arriving AFTER
    /// `AssistantTurnCompleted` already materialized the item must NOT
    /// mutate `content_segments` and must NOT rewrite the canonical
    /// `Message::BlockAssistant` (append-only history is a stronger
    /// invariant than typed text repair). The committed message keeps
    /// the delta-accumulated text; the late final is dropped with a
    /// warn; the materializer outcome is inert (no new messages).
    #[test]
    fn round5_r57_late_final_text_after_turn_completed_warns_and_skips() {
        let mut session = Session::new();

        // Delta accumulates partial text on the Spoken lane.
        let _ = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTranscriptDelta {
                response_id: "resp_a".to_string(),
                delta_id: "delta_s_1".to_string(),
                item_id: "item_a".to_string(),
                previous_item_id: None,
                content_index: 0,
                delta: "delta-accumulated".to_string(),
            },
        );

        // TurnCompleted materializes the item with the delta-accumulated text.
        let commit_outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id: "resp_a".to_string(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        );
        assert_eq!(commit_outcome.materialized_messages.len(), 1);

        // Late FinalText arrives — provider-side ordering bug. It MUST
        // be dropped: no canonical message rewrite, no segment mutation,
        // outcome is inert.
        let late_outcome = session.append_realtime_transcript_event(
            RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                response_id: "resp_a".to_string(),
                item_id: "item_a".to_string(),
                content_index: 0,
                text: "authoritative-final-that-must-not-land".to_string(),
            },
        );
        assert!(
            late_outcome.is_inert(),
            "late FinalText after materialization must produce inert outcome"
        );

        // Canonical history: still one message with the original
        // delta-accumulated text — NOT the authoritative final.
        assert_eq!(session.messages().len(), 1);
        match &session.messages()[0] {
            Message::BlockAssistant(assistant) => {
                assert_eq!(assistant.blocks.len(), 1);
                match &assistant.blocks[0] {
                    AssistantBlock::Transcript { text, .. } => {
                        assert_eq!(
                            text, "delta-accumulated",
                            "canonical message must preserve delta-accumulated text; \
                             append-only history forbids late FinalText repair"
                        );
                    }
                    other => unreachable!("expected Transcript, got {other:?}"),
                }
            }
            other => unreachable!("expected BlockAssistant, got {other:?}"),
        }
    }
}

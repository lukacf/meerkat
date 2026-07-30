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
use crate::generated::{session_document, session_persistence_version_authority};
use crate::lifecycle::run_primitive::{TurnMetadataOverride, TurnRequestContext};
use crate::lifecycle::{CoreBoundaryStageError, RunId};
use crate::peer_meta::PeerMeta;
use crate::realtime_transcript::{
    RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent, RealtimeUserContentIdentity,
    SESSION_REALTIME_TRANSCRIPT_STATE_KEY,
};
use crate::realtime_transcript_revision::{self, SessionRealtimeTranscriptState};
use crate::realtime_transcript_sidecar::{
    PreparedRealtimeTranscriptRebase, RealtimeTranscriptSidecarError,
    RealtimeTranscriptSnapshotReasonV1, SessionRealtimeTranscriptProjection,
};
use crate::service::MobToolAuthorityContext;
use crate::session_durable_config_authority;
use crate::time_compat::SystemTime;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_scope::ToolFilter;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ContentInput, Message, SessionId,
    StopReason, ToolDef, ToolName, ToolProvenance, ToolResult, Usage, UserMessage,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Stable logical lineage selected for session identity and fork semantics.
///
/// This is domain identity only. It carries no persistence, verification, or
/// store-currentness authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionLineageId(String);

impl SessionLineageId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidSessionLineageId> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(InvalidSessionLineageId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn for_session(session_id: &SessionId) -> Self {
        Self(format!("session:{session_id}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionLineageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for SessionLineageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSessionLineageId;

impl std::fmt::Display for InvalidSessionLineageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("session lineage id must not be empty")
    }
}

impl std::error::Error for InvalidSessionLineageId {}

/// Logical generation inside one session lineage.
///
/// Runtime restarts and store revisions do not change this value.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct SessionGeneration(u64);

impl SessionGeneration {
    pub const INITIAL: Self = Self(0);

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

mod digest_accumulator;
mod head_metadata;
mod import_0810;
mod transcript_history;

pub(crate) use digest_accumulator::TranscriptMessages;
pub use head_metadata::{
    SessionHeadMetadataCell, SessionHeadMetadataCellIdentity, SessionHeadMetadataCellMutation,
    SessionHeadMetadataDigest, SessionHeadMetadataIdentity, SessionHeadMetadataProjection,
    SessionHeadMetadataValueDigest,
};
pub(crate) use import_0810::is_released_checkpoint_metadata_key;
pub use import_0810::{
    ImportedReleased0810Session, Released0810ImportError, Released0810ImportEvidence,
    Released0810ImportReceipt, import_released_0810_session,
};
pub(crate) use transcript_history::graph::{
    TRANSCRIPT_DIGEST_FORMAT_CURRENT, import_released_0810_history,
};
pub(crate) use transcript_history::validate::validate_transcript_history_state;
use transcript_history::validate::{
    assistant_tool_use_ids, message_role_name, validate_transcript_tool_result_shape,
};
pub use transcript_history::{
    TRANSCRIPT_HISTORY_FORMAT_CURRENT, TranscriptEndpointWitness, TranscriptGraphPrefixAccumulator,
    TranscriptHistoryState, TranscriptParentAdvance, TranscriptRevisionBody,
    TranscriptRevisionEdge, TranscriptRewriteAuditReceiptBatch, TranscriptRewriteCommit,
    TranscriptRewriteParentTransition, TranscriptRewritePatch, TranscriptRewritePrefixAccumulator,
    TranscriptRewriteRecord, ValidatedTranscriptHistory, ValidatedTranscriptRewriteSuffix,
    extend_transcript_rewrite_prefix_accumulator, transcript_history_full_body_materializations,
    transcript_rewrite_prefix_digest,
};

/// Current session format version.
///
/// The persisted `version` byte is mandatory and fail-closed: a stored row
/// with a missing or non-current version is rejected at the serde boundary by
/// the generated persistence version authority. The exact released 0.8.10
/// envelope crosses only the explicit one-time importer; ordinary reads never
/// silently default or upgrade an envelope.
pub use crate::generated::session_persistence_version_authority::SESSION_VERSION;

/// Current `SessionMetadata` schema version. Distinct from `SESSION_VERSION`
/// so `SessionMetadata` can evolve independently of the Session envelope.
///
/// Mandatory and fail-closed on read, same contract as `SESSION_VERSION`.
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

/// Rolling identity of the exact ordered rewrite-commit
/// prefix represented by this session.
///
/// Kept outside the bulky graph value so a head-canonical cold read can prove
/// replay coverage from the small head row without materializing commit
/// history. Typed graph writers update it atomically with the graph.
pub const SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY: &str =
    "session_transcript_rewrite_prefix_authority_v1";

/// A concrete transcript span selected for same-session rewrite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TranscriptRewriteSelection {
    /// Pre-semantic-marker range retained for source/API compatibility and
    /// decoding prior durable records. New commits canonicalize this input to
    /// [`TranscriptRewriteSelection::EditMessageRange`] before persistence.
    MessageRange { start: usize, end: usize },
    /// Current typed ordinary-edit semantic.
    EditMessageRange { range: TranscriptEditRewriteRange },
    /// Replace a full transcript from a core-validated compaction rebuild.
    ///
    /// The range payload has no public constructor. New values are minted only
    /// by the validated compaction path; deserialization exists solely for the
    /// durable transcript graph and is revalidated against its retained bodies.
    CompactionMessageRange { range: CompactionRewriteRange },
}

/// Opaque current-format range carried by an ordinary transcript edit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct TranscriptEditRewriteRange {
    start: usize,
    end: usize,
}

/// Opaque range carried by the typed compaction rewrite semantic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CompactionRewriteRange {
    start: usize,
    end: usize,
}

/// Canonical semantic class of a transcript rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRewriteSemantic {
    /// Ordinary same-session edit.
    Edit,
    /// Core-validated context compaction.
    Compaction,
}

impl TranscriptRewriteSelection {
    /// Return the selected half-open message range without exposing the
    /// authority-bearing representation used to classify the rewrite.
    pub fn bounds(&self) -> (usize, usize) {
        match self {
            Self::MessageRange { start, end } => (*start, *end),
            Self::EditMessageRange { range } => (range.start, range.end),
            Self::CompactionMessageRange { range } => (range.start, range.end),
        }
    }

    pub fn semantic(&self) -> TranscriptRewriteSemantic {
        match self {
            Self::MessageRange { .. } | Self::EditMessageRange { .. } => {
                TranscriptRewriteSemantic::Edit
            }
            Self::CompactionMessageRange { .. } => TranscriptRewriteSemantic::Compaction,
        }
    }

    fn into_current_edit_semantic(self) -> Self {
        match self {
            Self::MessageRange { start, end } => Self::EditMessageRange {
                range: TranscriptEditRewriteRange { start, end },
            },
            current => current,
        }
    }

    fn is_legacy_untyped(&self) -> bool {
        matches!(self, Self::MessageRange { .. })
    }

    fn validated_compaction(
        start: usize,
        end: usize,
        _authority: &crate::agent::compact::ValidatedCompactionRewrite,
    ) -> Self {
        Self::CompactionMessageRange {
            range: CompactionRewriteRange { start, end },
        }
    }

    fn migrated_legacy_compaction(start: usize, end: usize) -> Self {
        Self::CompactionMessageRange {
            range: CompactionRewriteRange { start, end },
        }
    }

    #[cfg(test)]
    pub(crate) fn typed_compaction_for_test(start: usize, end: usize) -> Self {
        Self::CompactionMessageRange {
            range: CompactionRewriteRange { start, end },
        }
    }
}

/// Audit annotation carried with a transcript rewrite commit.
///
/// The free-form kind is for review, debugging, and provenance only. It never
/// classifies a rewrite as compaction; [`TranscriptRewriteSelection`] owns that
/// semantic through its opaque typed compaction range.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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

impl std::fmt::Display for TranscriptRewriteReason {
    /// Human-facing projection consumed by revision-list reads. The typed
    /// `{kind, note}` audit value is retained; this rendering is derived only
    /// and never supplies rewrite semantic authority.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.note {
            Some(note) => write!(f, "{}: {note}", self.kind),
            None => f.write_str(&self.kind),
        }
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

fn canonicalize_digest_image_blocks(blocks: &mut [crate::types::ContentBlock]) {
    for block in blocks.iter_mut() {
        if let crate::types::ContentBlock::Image {
            media_type,
            data: crate::types::ImageData::Inline { data },
        } = block
        {
            // An inline image hydrates from its blob's own bytes, so its
            // content-addressed identity equals the blob id the store minted.
            let blob_id = crate::blob::content_blob_id(media_type, data);
            *block = crate::types::ContentBlock::Image {
                media_type: media_type.clone(),
                data: crate::types::ImageData::Blob { blob_id },
            };
        }
    }
}

/// Canonicalize image payloads to their content-addressed blob identity so the
/// transcript digest is invariant to inline-vs-blob representation.
///
/// The same image hydrated inline for model execution and externalized to a
/// blob for persistence must share one transcript revision; otherwise a live
/// session and its durable snapshot would appear "diverged" purely because of
/// image storage form, and a runtime-backed live session would be discarded as
/// stale mid-turn.
fn canonicalize_message_images_for_digest(messages: &[Message]) -> Vec<Message> {
    let mut canonical = messages.to_vec();
    for message in &mut canonical {
        canonicalize_message_images_for_digest_in_place(message);
    }
    canonical
}

fn canonicalize_message_images_for_digest_in_place(message: &mut Message) {
    match message {
        Message::User(user) => canonicalize_digest_image_blocks(&mut user.content),
        Message::ToolResults { results, .. } => {
            for result in results.iter_mut() {
                canonicalize_digest_image_blocks(&mut result.content);
            }
        }
        Message::SystemNotice(notice) => {
            for block in &mut notice.blocks {
                match block {
                    crate::types::SystemNoticeBlock::Comms { content, .. }
                    | crate::types::SystemNoticeBlock::ExternalEvent { content, .. } => {
                        canonicalize_digest_image_blocks(content);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Validate only the compact current graph from a raw slice.
///
/// Released full-body history refuses before body decoding, making this the
/// bounded doctor/diagnostic seam.
pub fn validate_current_persisted_transcript_history_slice(
    bytes: &[u8],
) -> Result<u64, serde_json::Error> {
    let rewrite_count =
        transcript_history::graph::validate_current_transcript_history_slice(bytes)?;
    u64::try_from(rewrite_count).map_err(|_| {
        persisted_session_decode_error("persisted transcript-history occurrence count exceeds u64")
    })
}

/// Shared parsed form of the current transcript-history graph.
///
/// Guards and the per-append head refresh need the TYPED graph; parsing the
/// metadata value is O(graph), and a turn boundary parsed it twice (incoming
/// and previous) plus once more per append. The typed installer caches the
/// exact state it just serialized; readers share it by `Arc`. Every unchecked
/// write to the history key clears the parsed state.
#[derive(Debug, Default)]
pub(crate) struct SharedTranscriptHistoryState {
    inner: std::sync::Mutex<Option<std::sync::Arc<TranscriptHistoryState>>>,
}

impl Clone for SharedTranscriptHistoryState {
    fn clone(&self) -> Self {
        Self {
            inner: std::sync::Mutex::new(self.locked().clone()),
        }
    }
}

impl SharedTranscriptHistoryState {
    fn locked(&self) -> std::sync::MutexGuard<'_, Option<std::sync::Arc<TranscriptHistoryState>>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn clear(&self) {
        *self.locked() = None;
    }

    fn set(&self, state: std::sync::Arc<TranscriptHistoryState>) {
        *self.locked() = Some(state);
    }

    fn get(&self) -> Option<std::sync::Arc<TranscriptHistoryState>> {
        self.locked().clone()
    }
}

/// Timestamp sentinel used when erasing construction bookkeeping from the
/// digest form. `created_at` always serializes, so a fixed value keeps the
/// canonical bytes deterministic.
fn digest_timestamp_sentinel() -> crate::types::MessageTimestamp {
    chrono::DateTime::<chrono::Utc>::UNIX_EPOCH
}

/// Canonicalize messages to their conversational content before hashing so the
/// transcript revision is a content address, not a construction record.
///
/// Two normalizations compose:
/// - image payloads collapse to their content-addressed blob identity
///   ([`canonicalize_message_images_for_digest`]);
/// - per-construction bookkeeping is erased: [`TranscriptMessageIdentity`]
///   (run/interaction ids are runtime-binding atoms — a re-created authority
///   re-stamps them) and `created_at` timestamps. A resume that re-projects
///   the same conversation through a new runtime authority must digest to the
///   same revision as the persisted row, or the append-only save guard
///   strands the session on restart (fails closed with
///   `TranscriptContinuityViolation`).
///
/// Typed semantic facts stay in the digest — `transcript_role`,
/// `render_metadata`, notice kinds and blocks — because changing them changes
/// the transcript's meaning.
pub(crate) fn canonicalize_messages_for_digest(messages: &[Message]) -> Vec<Message> {
    let mut canonical = canonicalize_message_images_for_digest(messages);
    for message in &mut canonical {
        erase_message_construction_bookkeeping(message);
    }
    canonical
}

fn erase_message_construction_bookkeeping(message: &mut Message) {
    match message {
        Message::System(system) => {
            system.created_at = digest_timestamp_sentinel();
        }
        Message::SystemNotice(notice) => {
            notice.created_at = digest_timestamp_sentinel();
        }
        Message::User(user) => {
            user.identity = crate::types::TranscriptMessageIdentity::default();
            user.created_at = digest_timestamp_sentinel();
        }
        Message::BlockAssistant(assistant) => {
            assistant.identity = crate::types::TranscriptMessageIdentity::default();
            assistant.created_at = digest_timestamp_sentinel();
        }
        Message::ToolResults { created_at, .. } => {
            *created_at = digest_timestamp_sentinel();
        }
    }
}

/// Per-message projection of [`canonicalize_messages_for_digest`].
///
/// Transcript canonicalization is element-wise, so the identity byte stream a
/// transcript digest hashes is `"[" + json(c(m0)) + "," + json(c(m1)) + ... +
/// "]"`. [`digest_accumulator`] folds exactly these per-message bytes, which
/// is why an incremental midstate reproduces the format-2 digest value
/// unchanged. `canonicalize_messages_for_digest_is_element_wise` pins the
/// equivalence.
pub(crate) fn canonicalize_message_for_digest(message: &Message) -> Message {
    let mut canonical = message.clone();
    canonicalize_message_images_for_digest_in_place(&mut canonical);
    erase_message_construction_bookkeeping(&mut canonical);
    canonical
}

pub fn transcript_messages_digest(messages: &[Message]) -> Result<String, serde_json::Error> {
    sha256_json_digest(&canonicalize_messages_for_digest(messages))
}

/// Full transcript digest that does NOT bump the content-digest budget
/// counter.
///
/// Reserved for focused meerkat-core unit-test witness cross-checks. Downstream
/// debug/integration builds deliberately do not execute it: verification
/// scaffolding must not turn ordinary runtime work back into O(document).
pub(crate) fn transcript_messages_digest_uncounted(
    messages: &[Message],
) -> Result<String, serde_json::Error> {
    let canonical = canonicalize_messages_for_digest(messages);
    let bytes = serde_json::to_vec(&canonical)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn sha256_json_digest<T: Serialize + ?Sized>(value: &T) -> Result<String, serde_json::Error> {
    crate::digest_observability::record_content_digest_computation();
    let bytes = serde_json::to_vec(value)?;
    crate::digest_observability::record_content_digest_bytes(bytes.len() as u64);
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
/// Process-local derived caches for the transcript-history graph.
///
/// Grouped behind ONE pointer deliberately. `Session` is embedded throughout
/// the agent's nested async state machine, whose futures compose sizes
/// additively, so every inline byte here is paid again at each spawn depth —
/// and the CLI's full-tools spawn runs against a literal 2 MB production stack
/// budget, pinned by
/// `tools_full_with_explicit_auth_binding_can_spawn_within_production_stack_budget`.
/// Holding these caches inline grew `Session` from 136 to 528 bytes and
/// overflowed that stack. None is persisted or part of a session's identity;
/// all are rebuildable from durable session fields.
#[derive(Debug, Default, Clone)]
pub(crate) struct SessionHistoryCaches {
    /// Shared parsed form of the current history graph.
    shared_state: SharedTranscriptHistoryState,
    /// Actor-local authenticated-map baseline and coalesced dirty-key set for
    /// metadata carried out of line by HeadCanonical persistence.
    ///
    /// This is structural continuation state rather than a value cache:
    /// ordinary preparation canonicalizes only changed cells, and a durable
    /// acknowledgement advances the exact sparse-Merkle baseline. Cold
    /// materialization and explicit 0.8.10 activation install a fully verified
    /// snapshot before ordinary delta writes are admitted.
    head_canonical_metadata: head_metadata::SessionHeadMetadataTracker,
}

fn head_canonical_metadata_cell_carries_key(key: &str) -> bool {
    !import_0810::is_released_checkpoint_metadata_key(key)
        && !matches!(
            key,
            SESSION_TRANSCRIPT_HISTORY_STATE_KEY
                | SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY
                | SESSION_REALTIME_TRANSCRIPT_STATE_KEY
        )
}

#[cfg(test)]
std::thread_local! {
    /// Per-test-thread observability for exact metadata-value canonicalization.
    ///
    /// The Rust test harness runs independent tests concurrently in one
    /// process. A process-global counter lets unrelated HeadCanonical fixture
    /// construction inflate another test's O(delta) budget, so it cannot
    /// certify how much work the measured caller performed.
    static SESSION_HEAD_METADATA_CANONICALIZATION_COUNT: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_session_head_metadata_canonicalization_count() {
    SESSION_HEAD_METADATA_CANONICALIZATION_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn session_head_metadata_canonicalization_count() -> u64 {
    SESSION_HEAD_METADATA_CANONICALIZATION_COUNT.get()
}

#[cfg(test)]
pub(crate) fn record_session_head_metadata_canonicalization() {
    SESSION_HEAD_METADATA_CANONICALIZATION_COUNT.set(
        SESSION_HEAD_METADATA_CANONICALIZATION_COUNT
            .get()
            .saturating_add(1),
    );
}

#[derive(Debug, Clone)]
pub struct Session {
    /// Persisted envelope format version, validated fail-closed on read by
    /// the generated persistence version authority.
    version: u32,
    /// Unique identifier
    id: SessionId,
    /// All messages in order (Arc for CoW on fork) plus the incremental
    /// transcript-digest accumulator that owns them.
    ///
    /// The buffer is deliberately wrapped: [`TranscriptMessages`] exposes no
    /// `DerefMut`, so every message mutation must name one of its typed
    /// mutators, and each mutator states whether the retained digest midstate
    /// survives. That makes the accumulator's invalidation set exhaustive by
    /// construction instead of by convention.
    pub(crate) messages: TranscriptMessages,
    /// When the session was created
    created_at: SystemTime,
    /// When the session was last updated
    updated_at: SystemTime,
    /// Arbitrary metadata
    metadata: serde_json::Map<String, serde_json::Value>,
    /// Typed in-memory realtime reducer projection plus its authenticated
    /// HeadCanonical component-event suffix.
    ///
    /// The accumulated reducer state is deliberately absent from `metadata`
    /// during ordinary operation. WholeBlob serialization injects it only at
    /// that exceptional O(document) representation boundary; HeadCanonical
    /// binds the compact event-prefix authority and persists only new typed
    /// records.
    realtime_transcript: Box<SessionRealtimeTranscriptProjection>,
    /// Derived actor-local indexes and structural continuation state.
    ///
    /// The transcript and authenticated store projections remain authority.
    /// These caches accelerate terminal-notice membership, share the already
    /// validated compact graph, and retain the sparse HeadCanonical metadata
    /// baseline.
    history_caches: Box<SessionHistoryCaches>,
    /// Whether transcript-history metadata has already crossed a validating,
    /// compacting authority boundary in this in-memory session.
    ///
    /// This is derived cache state only, never persisted authority. Typed
    /// transcript mutations install validated state; deserialization validates
    /// before setting it. Any unchecked history mutation invalidates the cache
    /// so serialization retains the fail-closed corrupt-snapshot contract.
    transcript_history_metadata_validation: TranscriptHistoryMetadataValidation,
    /// Cumulative token usage across all LLM calls in this session
    usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptHistoryMetadataValidation {
    Validated,
    RequiresValidation,
}

/// Serde helper for Session serialization (flattens Arc)
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct SessionSerde {
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

/// Borrowed serialization view for Session. The persisted shape deliberately
/// stays lockstep with `SessionSerde`, but large transcripts and metadata are
/// streamed directly instead of being deep-cloned before serde sees them.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct SessionSerdeRef<'a> {
    version: u32,
    id: &'a SessionId,
    messages: &'a [Message],
    created_at: &'a SystemTime,
    updated_at: &'a SystemTime,
    metadata: &'a serde_json::Map<String, serde_json::Value>,
    usage: &'a Usage,
}

/// Borrowed transient WholeBlob metadata overlay.
///
/// The live metadata map never owns the transcript graph or realtime
/// projection. WholeBlob is the one representation that needs those values
/// inline, so this serializer streams the base map and the typed projections
/// into one object without cloning the map or constructing a graph-sized
/// `serde_json::Value` shadow.
struct SessionWholeBlobMetadataRef<'a> {
    base: &'a serde_json::Map<String, serde_json::Value>,
    history: Option<&'a TranscriptHistoryState>,
    realtime: Option<&'a SessionRealtimeTranscriptState>,
}

impl Serialize for SessionWholeBlobMetadataRef<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;
        for (key, value) in self.base {
            if key == SESSION_REALTIME_TRANSCRIPT_STATE_KEY
                || (self.history.is_some()
                    && (key == SESSION_TRANSCRIPT_HISTORY_STATE_KEY
                        || key == SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY))
            {
                continue;
            }
            map.serialize_entry(key, value)?;
        }
        if let Some(history) = self.history {
            map.serialize_entry(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, history)?;
            map.serialize_entry(
                SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY,
                history.rewrite_prefix(),
            )?;
        }
        if let Some(realtime) = self.realtime {
            map.serialize_entry(SESSION_REALTIME_TRANSCRIPT_STATE_KEY, realtime)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct SessionWholeBlobSerdeRef<'a> {
    version: u32,
    id: &'a SessionId,
    messages: &'a [Message],
    created_at: &'a SystemTime,
    updated_at: &'a SystemTime,
    metadata: SessionWholeBlobMetadataRef<'a>,
    usage: &'a Usage,
}

/// Bind every persisted field of `session` into the borrowed encode view.
///
/// This is the exhaustiveness anchor for the durable envelope: a persisted
/// field added to [`SessionSerdeRef`] stops this construction from compiling,
/// and every site that destructures the returned view must then classify the
/// addition instead of silently dropping it. `metadata_override` substitutes
/// the compacted map the snapshot seam builds in place of the live one.
fn persisted_envelope_ref<'a>(
    session: &'a Session,
    metadata_override: Option<&'a serde_json::Map<String, serde_json::Value>>,
) -> SessionSerdeRef<'a> {
    SessionSerdeRef {
        version: session.version,
        id: &session.id,
        messages: session.messages(),
        created_at: &session.created_at,
        updated_at: &session.updated_at,
        metadata: metadata_override.unwrap_or(&session.metadata),
        usage: &session.usage,
    }
}

impl Serialize for Session {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let _digest_site = crate::digest_observability::enter_digest_site(
            crate::digest_observability::DIGEST_SITE_ENCODE,
        );
        if import_0810::contains_released_checkpoint_metadata(&self.metadata) {
            return Err(<S::Error as serde::ser::Error>::custom(
                "released checkpoint metadata cannot be serialized by the current Session domain",
            ));
        }
        if self.transcript_history_metadata_validation
            == TranscriptHistoryMetadataValidation::RequiresValidation
            && (self
                .metadata
                .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
                || self.history_caches.shared_state.get().is_some())
        {
            return Err(<S::Error as serde::ser::Error>::custom(
                "transcript-history graph lacks verified materialization or construction authority",
            ));
        }
        let history = self.history_caches.shared_state.get();
        let serde_repr = SessionWholeBlobSerdeRef {
            version: self.version,
            id: &self.id,
            messages: self.messages(),
            created_at: &self.created_at,
            updated_at: &self.updated_at,
            metadata: SessionWholeBlobMetadataRef {
                base: &self.metadata,
                history: history.as_deref(),
                realtime: self.realtime_transcript.whole_blob_projection(),
            },
            usage: &self.usage,
        };
        serde_repr.serialize(serializer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptHistoryWireKind {
    Released0810,
    Current,
}

fn transcript_history_wire_kind(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<TranscriptHistoryWireKind>, String> {
    let Some(value) = metadata.get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY) else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| "transcript-history graph must be an object".to_string())?;
    match object.get("format") {
        None => Ok(Some(TranscriptHistoryWireKind::Released0810)),
        Some(serde_json::Value::String(format)) if format == TRANSCRIPT_HISTORY_FORMAT_CURRENT => {
            Ok(Some(TranscriptHistoryWireKind::Current))
        }
        Some(serde_json::Value::String(format)) => {
            Err(format!("unsupported transcript graph format {format}"))
        }
        Some(_) => Err("transcript graph format must be a string".to_string()),
    }
}

/// Decode and compact the transient transcript-history wire value, removing it
/// from ordinary metadata and returning the singular typed in-memory graph.
///
/// Returning the proof is the sealed-capability seam: the graph this function
/// just validated used to be dropped on the floor, so the first consumer after
/// a decode re-parsed the very value serialized from it one statement earlier.
/// `Ok(None)` means the metadata carries no transcript-history graph at all.
fn compact_transcript_history_metadata_for_snapshot(
    metadata: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<Option<std::sync::Arc<TranscriptHistoryState>>, String> {
    let Some(value) = metadata.remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY) else {
        return Ok(None);
    };
    let state: TranscriptHistoryState =
        serde_json::from_value(value).map_err(|error| error.to_string())?;
    // `TranscriptHistoryState::deserialize` has already performed the full
    // current-graph validation. Current graphs carry no mechanical revision
    // bodies to prune, so validating again here only repeats every retained
    // rewrite-prefix serialization before installing the exact state that was
    // just proved.
    metadata.remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    Ok(Some(std::sync::Arc::new(state)))
}

impl ValidatedTranscriptHistory {
    /// Seal one compact transcript graph reconstructed from exact
    /// HeadCanonical rows and persisted graph edges.
    ///
    /// This is a store-ingress capability, not a general graph constructor.
    /// The graph implementation revalidates the anchor, ordered edges, and both
    /// physical-head prefix authorities before this proof can be minted.
    #[doc(hidden)]
    pub fn from_store_replayed_compact_graph(
        anchor_revision: String,
        anchor_messages: Vec<Message>,
        anchor_row_prefix: crate::session_store::SessionMessageRowPrefixAccumulator,
        edges: Vec<TranscriptRevisionEdge>,
        expected_rewrite_prefix: &TranscriptRewritePrefixAccumulator,
        expected_graph_prefix: &TranscriptGraphPrefixAccumulator,
    ) -> Result<Self, TranscriptEditError> {
        let state = TranscriptHistoryState::from_store_replayed_compact_graph(
            anchor_revision,
            anchor_messages,
            anchor_row_prefix,
            edges,
            expected_rewrite_prefix,
            expected_graph_prefix,
        )?;
        Ok(Self::adopt_session_validated(std::sync::Arc::new(state)))
    }

    /// Rebuild and seal a graph from generation-bearing rewrite records while
    /// reusing an optional already-proved prefix.
    ///
    /// The record builder validates every unproved endpoint body and edit
    /// relation, preserves only byte-equal bodies/commits from `proved`,
    /// checks occurrence contiguity and every bridge, and derives the rolling
    /// prefix authority as it appends. Those are exactly the facts the full
    /// graph validator would re-derive over the result, so this returns the
    /// proof-bearing capability directly instead of immediately hashing every
    /// retained body a second time.
    pub fn from_rewrite_records_with_proved<I>(
        records: I,
        proved: Option<&ValidatedTranscriptHistory>,
    ) -> Result<Option<Self>, TranscriptEditError>
    where
        I: IntoIterator<Item = TranscriptRewriteRecord>,
    {
        Ok(
            TranscriptHistoryState::from_rewrite_records_with_proved(records, proved)?.map(
                |state| {
                    ValidatedTranscriptHistory::adopt_session_validated(std::sync::Arc::new(state))
                },
            ),
        )
    }
}

impl<'de> Deserialize<'de> for Session {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let _digest_site = crate::digest_observability::enter_digest_site(
            crate::digest_observability::DIGEST_SITE_DECODE,
        );
        let serde_repr = SessionSerde::deserialize(deserializer)?;
        let version = session_persistence_version_authority::restore_session_envelope_version(
            serde_repr.version,
        )
        .map_err(<D::Error as serde::de::Error>::custom)?;
        let mut metadata = serde_repr.metadata;
        if import_0810::contains_released_checkpoint_metadata(&metadata) {
            return Err(<D::Error as serde::de::Error>::custom(
                "embedded released checkpoint metadata requires the explicit one-time 0.8.10 importer",
            ));
        }
        let realtime_transcript = match metadata.remove(SESSION_REALTIME_TRANSCRIPT_STATE_KEY) {
            Some(value) => {
                let state = serde_json::from_value(value)
                    .map_err(<D::Error as serde::de::Error>::custom)?;
                SessionRealtimeTranscriptProjection::from_inline_snapshot(&serde_repr.id, state)
                    .map_err(<D::Error as serde::de::Error>::custom)?
            }
            None => SessionRealtimeTranscriptProjection::empty(&serde_repr.id),
        };
        let history_wire_kind = transcript_history_wire_kind(&metadata)
            .map_err(<D::Error as serde::de::Error>::custom)?;
        if matches!(
            history_wire_kind,
            Some(TranscriptHistoryWireKind::Released0810)
        ) {
            return Err(<D::Error as serde::de::Error>::custom(
                "released 0.8.10 transcript history requires the explicit one-time importer",
            ));
        }
        let history_caches = Box::<SessionHistoryCaches>::default();
        let mut session = Session {
            version,
            id: serde_repr.id,
            messages: TranscriptMessages::from_vec(serde_repr.messages),
            created_at: serde_repr.created_at,
            updated_at: serde_repr.updated_at,
            metadata,
            realtime_transcript: Box::new(realtime_transcript),
            history_caches,
            transcript_history_metadata_validation: if history_wire_kind.is_some() {
                TranscriptHistoryMetadataValidation::RequiresValidation
            } else {
                TranscriptHistoryMetadataValidation::Validated
            },
            usage: serde_repr.usage,
        };
        if let Some(TranscriptHistoryWireKind::Current) = history_wire_kind {
            let state = compact_transcript_history_metadata_for_snapshot(&mut session.metadata)
                .map_err(<D::Error as serde::de::Error>::custom)?
                .ok_or_else(|| {
                    <D::Error as serde::de::Error>::custom(
                        "transcript-history graph disappeared during ingress",
                    )
                })?;
            let exact_live_prefix = state
                .derive_live_row_lineage_after_final_semantic_replay(session.messages())
                .map_err(<D::Error as serde::de::Error>::custom)?
                .ok_or_else(|| {
                    <D::Error as serde::de::Error>::custom(
                        "live transcript does not preserve the graph-proved audited endpoint",
                    )
                })?;
            let endpoint_prefix = state
                .final_endpoint_witness()
                .ok_or_else(|| {
                    <D::Error as serde::de::Error>::custom(
                        "compact transcript graph has no final endpoint witness",
                    )
                })?
                .row_prefix()
                .clone();
            if !session.install_exact_message_row_lineage(endpoint_prefix, exact_live_prefix) {
                return Err(<D::Error as serde::de::Error>::custom(
                    "failed to install exact live message-row authority",
                ));
            }
            session.transcript_history_metadata_validation =
                TranscriptHistoryMetadataValidation::Validated;
            session
                .history_caches
                .shared_state
                .set(std::sync::Arc::clone(&state));
        }
        Ok(session)
    }
}

/// Serde helper for the metadata-only partial decode of a persisted session
/// envelope.
///
/// LOCKSTEP with [`SessionSerde`]: this struct must decode exactly the field
/// names and serde shapes that `SessionSerde` persists for `version`, `id`,
/// and `metadata` (`rename_all = "snake_case"`, `#[serde(default)]` on
/// `metadata`). The `session_metadata_document_lockstep_with_full_envelope`
/// pin test fails if the two drift.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct SessionMetadataDocumentSerde {
    version: u32,
    id: SessionId,
    #[serde(default)]
    metadata: serde_json::Map<String, serde_json::Value>,
}

/// Metadata-only projection of a persisted session envelope.
///
/// Produced by [`session_metadata_document_from_slice`] without materializing
/// the transcript. Exposes ONLY the two session-authority facts the metadata
/// read seam is allowed to observe ([`SESSION_METADATA_KEY`] and
/// [`SESSION_LIFECYCLE_TERMINAL_KEY`]) — deliberately no raw metadata-map
/// accessor, so the partial decode can never grow into an untyped side
/// channel around [`Session`]'s authority-gated reads.
#[derive(Debug, Clone)]
pub struct SessionMetadataDocument {
    session_id: SessionId,
    metadata: serde_json::Map<String, serde_json::Value>,
}

impl SessionMetadataDocument {
    /// Session identity carried by the envelope.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Raw projected [`SESSION_METADATA_KEY`] value, for divergence
    /// comparison against another projection of the same fact.
    pub fn session_metadata_value(&self) -> Option<&serde_json::Value> {
        self.metadata.get(SESSION_METADATA_KEY)
    }

    /// Raw projected [`SESSION_LIFECYCLE_TERMINAL_KEY`] value, for divergence
    /// comparison against another projection of the same fact.
    pub fn lifecycle_terminal_value(&self) -> Option<&serde_json::Value> {
        self.metadata.get(SESSION_LIFECYCLE_TERMINAL_KEY)
    }

    /// Decode the typed metadata view through the canonical map-level
    /// decoders, failing closed on corrupt values.
    pub fn try_into_view(self) -> Result<PersistedSessionMetadataView, serde_json::Error> {
        PersistedSessionMetadataView::try_from_metadata_map(self.session_id, &self.metadata)
    }
}

/// Partially decode a persisted session envelope into its metadata-only
/// document, without materializing the transcript.
///
/// Fail-closed on the envelope format version through the generated
/// persistence version authority — exactly like the full [`Session`]
/// deserializer.
pub fn session_metadata_document_from_slice(
    bytes: &[u8],
) -> Result<SessionMetadataDocument, serde_json::Error> {
    let serde_repr: SessionMetadataDocumentSerde = serde_json::from_slice(bytes)?;
    session_persistence_version_authority::restore_session_envelope_version(serde_repr.version)
        .map_err(<serde_json::Error as serde::de::Error>::custom)?;
    if import_0810::contains_released_checkpoint_metadata(&serde_repr.metadata) {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "released 0.8.10 proof metadata requires the explicit one-time importer",
        ));
    }
    let history_wire_kind = transcript_history_wire_kind(&serde_repr.metadata)
        .map_err(<serde_json::Error as serde::de::Error>::custom)?;
    if matches!(
        history_wire_kind,
        Some(TranscriptHistoryWireKind::Released0810)
    ) {
        return Err(<serde_json::Error as serde::de::Error>::custom(
            "released 0.8.10 transcript history requires the explicit one-time importer",
        ));
    }
    Ok(SessionMetadataDocument {
        session_id: serde_repr.id,
        metadata: serde_repr.metadata,
    })
}

/// One exact serialized session document plus its single-pass physical digest.
///
/// Typed producers construct this through [`Session::to_persisted_artifact`].
/// The streaming JSON writer feeds the output buffer and SHA-256 in the same
/// pass, so consumers can reuse `row_sha256_token` without scanning the full
/// document again.
#[derive(Debug, Clone)]
pub struct SerializedSessionArtifact {
    // `Arc<Vec<u8>>`, rather than `Arc<[u8]>`, is intentional: promoting a
    // completed Vec into an Arc slice may allocate and copy the full document.
    // Sharing the immutable Vec owner preserves the streaming writer's exact
    // allocation without a second O(document) memory pass.
    bytes: Arc<Vec<u8>>,
    raw_sha256: [u8; 32],
    row_sha256_token: Arc<str>,
}

/// One decoded WholeBlob document paired with its observed physical identity.
///
/// This is an observation, not persistence authority: the owning store must
/// compare [`Self::row_sha256_token`] with its transaction-issued row token
/// before exposing [`Self::session`].  Decoding through this seam also installs
/// the exact serialized-message lineage proven by those same bytes, so a
/// subsequent transcript rewrite does not depend on a self-authenticating
/// `Session` field.
#[derive(Debug)]
pub struct DecodedWholeBlobSessionDocument {
    session: Session,
    row_sha256_token: String,
}

impl DecodedWholeBlobSessionDocument {
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    #[must_use]
    pub fn row_sha256_token(&self) -> &str {
        &self.row_sha256_token
    }

    #[must_use]
    pub fn into_session(self) -> Session {
        self.session
    }
}

impl SerializedSessionArtifact {
    fn from_parts(bytes: Vec<u8>, raw_sha256: [u8; 32]) -> Self {
        Self {
            bytes: Arc::new(bytes),
            raw_sha256,
            row_sha256_token: Arc::from(row_sha256_token(raw_sha256)),
        }
    }

    pub(crate) fn from_raw_bytes(bytes: Vec<u8>) -> Self {
        let raw_sha256 = sha256_key(&bytes);
        Self::from_parts(bytes, raw_sha256)
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }

    #[must_use]
    pub fn bytes_arc(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.bytes)
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        Arc::try_unwrap(self.bytes).unwrap_or_else(|shared| shared.as_ref().clone())
    }

    #[must_use]
    pub const fn raw_sha256(&self) -> &[u8; 32] {
        &self.raw_sha256
    }

    #[must_use]
    pub fn row_sha256_token(&self) -> &str {
        &self.row_sha256_token
    }
}

struct SessionArtifactWriter {
    bytes: Vec<u8>,
    hasher: Sha256,
}

impl SessionArtifactWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            hasher: Sha256::new(),
        }
    }

    fn finish(self) -> SerializedSessionArtifact {
        crate::digest_observability::record_session_encode_bytes(self.bytes.len() as u64);
        let digest = self.hasher.finalize();
        let mut raw_sha256 = [0u8; 32];
        raw_sha256.copy_from_slice(&digest);
        SerializedSessionArtifact::from_parts(self.bytes, raw_sha256)
    }
}

impl std::io::Write for SessionArtifactWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        self.hasher.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn sha256_key(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

fn row_sha256_token(raw_sha256: [u8; 32]) -> String {
    use std::fmt::Write as _;

    let mut token = String::with_capacity("row-sha256:".len() + 64);
    token.push_str("row-sha256:");
    for byte in raw_sha256 {
        let _ = write!(token, "{byte:02x}");
    }
    token
}

fn persisted_session_decode_error(message: impl Into<String>) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    ))
}

impl Session {
    /// Install one current compact transcript graph after out-of-line domain
    /// projections have been materialized.
    ///
    /// Released 0.8.10 graphs are admitted only by the explicit one-time
    /// importer; normal current materialization never interprets them.
    pub(crate) fn normalize_persisted_transcript_history_ingress(
        &mut self,
    ) -> Result<(), TranscriptEditError> {
        let history_wire_kind = transcript_history_wire_kind(&self.metadata)
            .map_err(TranscriptEditError::HistoryStateMalformed)?;
        let Some(history_wire_kind) = history_wire_kind else {
            return Ok(());
        };
        if matches!(history_wire_kind, TranscriptHistoryWireKind::Released0810) {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "released 0.8.10 transcript history requires the explicit one-time importer"
                    .to_string(),
            ));
        }
        let state = compact_transcript_history_metadata_for_snapshot(&mut self.metadata)
            .map_err(TranscriptEditError::HistoryStateMalformed)?
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "transcript-history graph disappeared during ingress".to_string(),
                )
            })?;
        let exact_live_prefix = state
            .derive_live_row_lineage_after_final_semantic_replay(self.messages())
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "live transcript does not preserve the graph-proved audited endpoint"
                        .to_string(),
                )
            })?;
        let endpoint_prefix = state
            .final_endpoint_witness()
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "compact transcript graph has no final endpoint witness".to_string(),
                )
            })?
            .row_prefix()
            .clone();
        if !self.install_exact_message_row_lineage(endpoint_prefix, exact_live_prefix) {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "failed to install exact live message-row authority".to_string(),
            ));
        }
        self.transcript_history_metadata_validation =
            TranscriptHistoryMetadataValidation::Validated;
        self.history_caches
            .shared_state
            .set(std::sync::Arc::clone(&state));
        Ok(())
    }

    /// Rebuild a slim `Session` from persisted head-row parts.
    ///
    /// Used by [`crate::session_store::SessionHead::into_session`] to
    /// materialize a session from an incremental store's head row plus its
    /// strand messages. The envelope version is restored fail-closed through
    /// the generated persistence version authority, exactly like
    /// [`Session::deserialize`].
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_head_parts(
        version: u32,
        id: SessionId,
        messages: Vec<Message>,
        exact_row_prefix: Option<crate::SessionMessageRowPrefixAccumulator>,
        created_at: SystemTime,
        updated_at: SystemTime,
        metadata: serde_json::Map<String, serde_json::Value>,
        usage: Usage,
        head_canonical_metadata: Option<Arc<SessionHeadMetadataProjection>>,
    ) -> Result<Self, String> {
        let version =
            session_persistence_version_authority::restore_session_envelope_version(version)
                .map_err(|err| err.to_string())?;
        if import_0810::contains_released_checkpoint_metadata(&metadata) {
            return Err(
                "embedded released checkpoint metadata requires the explicit one-time 0.8.10 importer"
                    .to_string(),
            );
        }
        let transcript = TranscriptMessages::from_vec(messages);
        if let Some(prefix) = exact_row_prefix
            && !transcript.install_exact_row_prefix(prefix)
        {
            return Err(
                "exact message-row prefix count differs from materialized messages".to_string(),
            );
        }
        let realtime_transcript = Box::new(SessionRealtimeTranscriptProjection::empty(&id));
        let history_caches = Box::<SessionHistoryCaches>::default();
        let mut session = Self {
            version,
            id,
            messages: transcript,
            created_at,
            updated_at,
            transcript_history_metadata_validation: if metadata
                .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            {
                TranscriptHistoryMetadataValidation::RequiresValidation
            } else {
                TranscriptHistoryMetadataValidation::Validated
            },
            metadata,
            realtime_transcript,
            history_caches,
            usage,
        };
        if let Some(projection) = head_canonical_metadata {
            session
                .install_head_canonical_metadata_projection(&projection)
                .map_err(|error| {
                    format!(
                        "failed to install HeadCanonical metadata baseline for session {}: {error}",
                        session.id
                    )
                })?;
        }
        Ok(session)
    }

    /// Serialize this current Session envelope to persisted JSON bytes.
    ///
    /// This does not populate process-global identity or digest caches.
    /// Callers that also need the exact physical blob digest should use
    /// [`Self::to_persisted_artifact`] so bytes and SHA-256 are produced in one
    /// pass.
    pub fn to_persisted_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        Ok(self.to_persisted_artifact()?.into_bytes())
    }

    /// Stream this exact Session into a sealed WholeBlob artifact.
    ///
    /// JSON bytes and their physical SHA-256 are produced in one pass. No
    /// process-global Session cache is populated: durable store authority, not
    /// process memory, owns exact byte identity.
    pub fn to_persisted_artifact(&self) -> Result<SerializedSessionArtifact, serde_json::Error> {
        let mut writer = SessionArtifactWriter::new();
        serde_json::to_writer(&mut writer, self)?;
        Ok(writer.finish())
    }

    /// Decode one current Session envelope without a full-byte pre-hash or a
    /// process-global memo lookup.
    pub fn from_persisted_bytes(serialized: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(serialized)
    }

    /// Decode an observed WholeBlob row and derive its exact physical identity.
    ///
    /// The returned token is deliberately not trusted here. `RuntimeStore`
    /// owns the transaction-issued authority and must compare it before the
    /// decoded session is usable. Once that comparison succeeds, the exact
    /// serialized message vector establishes the row-lineage origin required
    /// by later rewrite commits.
    #[doc(hidden)]
    pub fn decode_whole_blob_document(
        serialized: &[u8],
    ) -> Result<DecodedWholeBlobSessionDocument, serde_json::Error> {
        let session = Self::from_persisted_bytes(serialized)?;
        let message_count = u64::try_from(session.messages().len()).map_err(|_| {
            <serde_json::Error as serde::de::Error>::custom(
                "WholeBlob transcript row count exceeds u64",
            )
        })?;
        if session.exact_message_row_prefix_at(message_count).is_none() {
            session.messages.mark_lazy_whole_blob_row_lineage();
        }
        Ok(DecodedWholeBlobSessionDocument {
            session,
            row_sha256_token: row_sha256_token(sha256_key(serialized)),
        })
    }

    /// Exact durable-row lineage at one prefix count, when this Session was
    /// materialized from that authority and has changed only by appends.
    pub(crate) fn exact_message_row_prefix_at(
        &self,
        row_count: u64,
    ) -> Option<crate::SessionMessageRowPrefixAccumulator> {
        self.messages.exact_row_prefix_at(row_count)
    }

    /// Adopt an exact row prefix after a prepared head-canonical boundary has
    /// been acknowledged as durable.
    pub(crate) fn install_exact_message_row_prefix(
        &self,
        prefix: crate::SessionMessageRowPrefixAccumulator,
    ) -> bool {
        self.messages.install_exact_row_prefix(prefix)
    }

    pub(crate) fn install_exact_message_row_lineage(
        &self,
        anchor: crate::SessionMessageRowPrefixAccumulator,
        current: crate::SessionMessageRowPrefixAccumulator,
    ) -> bool {
        self.messages.install_exact_row_lineage(anchor, current)
    }

    pub(crate) fn exact_message_row_lineage_extends(
        &self,
        anchor: &crate::SessionMessageRowPrefixAccumulator,
        current_count: u64,
    ) -> bool {
        self.messages
            .exact_row_lineage_extends(anchor, current_count)
    }
}

/// Metadata key used to store deferred-turn control state.
pub const SESSION_DEFERRED_TURN_STATE_KEY: &str = "session_deferred_turn_state";

/// Metadata key for a mixed local/external callback batch whose completed
/// sibling outcomes must remain hidden until the external callback result can
/// complete the provider-adjacent `ToolResults` set.
pub(crate) const SESSION_PENDING_CALLBACK_BATCH_KEY: &str = "session_pending_callback_batch_v1";

/// Metadata key used to store recoverable build-only session state.
pub const SESSION_BUILD_STATE_KEY: &str = "session_build_state";

/// Metadata key used to store durable session-local tool visibility intent.
pub const SESSION_TOOL_VISIBILITY_STATE_KEY: &str = "session_tool_visibility_state_v1";

/// Metadata key used to store the typed session lifecycle-terminal fact.
pub const SESSION_LIFECYCLE_TERMINAL_KEY: &str = "session_lifecycle_terminal";

/// Canonical tool name gated by `image_tool_results` capability.
pub const VIEW_IMAGE_TOOL_NAME: &str = "view_image";

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
    // Single reserved-key authority: the typed classifier owns the
    // session-authority key set (the `session_*` state constants).
    key == SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY
        || crate::surface_metadata::ReservedMetadataKey::is_session_authority(key)
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

/// Request-only context coordinator for one live actor.
///
/// This handle owns no Session state and has no persistence or idempotency
/// semantics. It only coordinates publication of exact runtime-owned context
/// at one named model boundary.
#[derive(Clone)]
pub struct TransientTurnContextStateHandle {
    boundary: Arc<TransientTurnContextBoundaryCoordinator>,
}

struct TransientTurnContextBoundaryCoordinator {
    incarnation_id: uuid::Uuid,
    lifecycle: std::sync::Mutex<TransientTurnContextBoundaryLifecycle>,
    notify: tokio::sync::Notify,
}

struct TransientTurnContextBoundaryLifecycle {
    actor_live: bool,
    next_generation: u64,
    next_request_id: u64,
    window: TransientTurnContextBoundaryWindow,
}

enum TransientTurnContextBoundaryWindow {
    Closed,
    Open {
        run_id: RunId,
        generation: u64,
        request: Option<RegisteredTransientTurnContextBoundaryRequest>,
    },
    Parked {
        run_id: RunId,
        generation: u64,
        request_id: u64,
        contexts: Vec<TurnRequestContext>,
    },
    Resolved {
        run_id: RunId,
        request_id: u64,
        contexts: Vec<TurnRequestContext>,
        resolution: TransientTurnContextBoundaryResolution,
    },
}

struct RegisteredTransientTurnContextBoundaryRequest {
    request_id: u64,
    contexts: Vec<TurnRequestContext>,
}

#[derive(Clone)]
enum TransientTurnContextBoundaryResolution {
    Committed,
    Aborted,
}

impl Default for TransientTurnContextBoundaryCoordinator {
    fn default() -> Self {
        Self {
            incarnation_id: uuid::Uuid::new_v4(),
            lifecycle: std::sync::Mutex::new(TransientTurnContextBoundaryLifecycle {
                actor_live: true,
                next_generation: 0,
                next_request_id: 0,
                window: TransientTurnContextBoundaryWindow::Closed,
            }),
            notify: tokio::sync::Notify::new(),
        }
    }
}

impl TransientTurnContextBoundaryCoordinator {
    fn lock(&self) -> std::sync::MutexGuard<'_, TransientTurnContextBoundaryLifecycle> {
        self.lifecycle.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(
                "transient turn-context boundary lock poisoned; retaining exact actor authority"
            );
            poisoned.into_inner()
        })
    }

    fn abort_request(&self, request_id: u64) -> Result<(), CoreBoundaryStageError> {
        let mut lifecycle = self.lock();
        let parked_owner = match &lifecycle.window {
            TransientTurnContextBoundaryWindow::Parked {
                run_id,
                request_id: current,
                contexts,
                ..
            } if *current == request_id => Some((run_id.clone(), contexts.clone())),
            _ => None,
        };
        if let Some((run_id, contexts)) = parked_owner {
            lifecycle.window = TransientTurnContextBoundaryWindow::Resolved {
                run_id,
                request_id,
                contexts,
                resolution: TransientTurnContextBoundaryResolution::Aborted,
            };
            drop(lifecycle);
            self.notify.notify_waiters();
            return Ok(());
        }
        match &mut lifecycle.window {
            TransientTurnContextBoundaryWindow::Open { request, .. }
                if request
                    .as_ref()
                    .is_some_and(|request| request.request_id == request_id) =>
            {
                *request = None;
            }
            TransientTurnContextBoundaryWindow::Resolved {
                request_id: current,
                ..
            } if *current == request_id => return Ok(()),
            _ => {
                return Err(CoreBoundaryStageError::stale(format!(
                    "transient boundary request {request_id} no longer owns its actor window"
                )));
            }
        }
        drop(lifecycle);
        self.notify.notify_waiters();
        Ok(())
    }

    fn close_run(&self, run_id: &RunId) {
        let mut lifecycle = self.lock();
        let owns_window = match &lifecycle.window {
            TransientTurnContextBoundaryWindow::Open {
                run_id: current, ..
            }
            | TransientTurnContextBoundaryWindow::Parked {
                run_id: current, ..
            }
            | TransientTurnContextBoundaryWindow::Resolved {
                run_id: current, ..
            } => current == run_id,
            TransientTurnContextBoundaryWindow::Closed => false,
        };
        if owns_window {
            lifecycle.window = TransientTurnContextBoundaryWindow::Closed;
            drop(lifecycle);
            self.notify.notify_waiters();
        }
    }

    fn revoke_actor(&self) {
        let mut lifecycle = self.lock();
        lifecycle.actor_live = false;
        lifecycle.window = TransientTurnContextBoundaryWindow::Closed;
        drop(lifecycle);
        self.notify.notify_waiters();
    }
}

/// Run-scoped guard closing every unresolved transient-context preparation.
#[must_use]
pub(crate) struct TransientTurnContextBoundaryRunGuard {
    boundary: Arc<TransientTurnContextBoundaryCoordinator>,
    run_id: RunId,
}

impl Drop for TransientTurnContextBoundaryRunGuard {
    fn drop(&mut self) {
        self.boundary.close_run(&self.run_id);
    }
}

struct PendingTransientTurnContextBoundaryPreparation {
    boundary: Arc<TransientTurnContextBoundaryCoordinator>,
    request_id: u64,
    armed: bool,
}

impl Drop for PendingTransientTurnContextBoundaryPreparation {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.boundary.abort_request(self.request_id);
        }
    }
}

/// Unique publication authority for one exact parked request boundary.
#[must_use = "prepared transient turn context must be committed or aborted"]
pub struct PreparedTransientTurnContextBoundary {
    state: TransientTurnContextStateHandle,
    expected_run_id: RunId,
    generation: u64,
    request_id: u64,
    armed: bool,
    _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl std::fmt::Debug for PreparedTransientTurnContextBoundary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedTransientTurnContextBoundary")
            .field("actor_incarnation", &self.state.boundary.incarnation_id)
            .field("expected_run_id", &self.expected_run_id)
            .field("generation", &self.generation)
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}

impl PreparedTransientTurnContextBoundary {
    #[must_use]
    pub fn expected_run_id(&self) -> &RunId {
        &self.expected_run_id
    }

    #[must_use]
    pub fn boundary_generation(&self) -> u64 {
        self.generation
    }

    /// Bind this request-only preparation to the generic commit/abort carrier.
    ///
    /// Transient context never has a Session snapshot; callers pass None.
    pub fn into_stage_output(
        self,
        session_snapshot: Option<Vec<u8>>,
    ) -> crate::lifecycle::CoreBoundaryStageOutput {
        debug_assert!(
            session_snapshot.is_none(),
            "transient turn context cannot carry a durable Session snapshot"
        );
        crate::lifecycle::CoreBoundaryStageOutput::prepared(None, Box::new(self))
    }

    fn resolve(
        &mut self,
        resolution: TransientTurnContextBoundaryResolution,
    ) -> Result<(), CoreBoundaryStageError> {
        if !self.armed {
            return Err(CoreBoundaryStageError::stale(
                "prepared transient boundary authority was already resolved",
            ));
        }
        let mut lifecycle = self.state.boundary.lock();
        if !lifecycle.actor_live {
            self.armed = false;
            return Err(CoreBoundaryStageError::stale(format!(
                "actor incarnation {} was revoked",
                self.state.boundary.incarnation_id
            )));
        }
        let matches_exact = matches!(
            &lifecycle.window,
            TransientTurnContextBoundaryWindow::Parked {
                run_id,
                generation,
                request_id,
                ..
            } if run_id == &self.expected_run_id
                && *generation == self.generation
                && *request_id == self.request_id
        );
        if !matches_exact {
            self.armed = false;
            return Err(CoreBoundaryStageError::stale(
                "prepared transient boundary no longer owns the exact parked generation",
            ));
        }
        let contexts = match std::mem::replace(
            &mut lifecycle.window,
            TransientTurnContextBoundaryWindow::Closed,
        ) {
            TransientTurnContextBoundaryWindow::Parked { contexts, .. } => contexts,
            _ => {
                self.armed = false;
                return Err(CoreBoundaryStageError::stale(
                    "prepared transient boundary lost its parked context",
                ));
            }
        };
        lifecycle.window = TransientTurnContextBoundaryWindow::Resolved {
            run_id: self.expected_run_id.clone(),
            request_id: self.request_id,
            contexts,
            resolution,
        };
        self.armed = false;
        drop(lifecycle);
        self.state.boundary.notify.notify_waiters();
        Ok(())
    }
}

impl crate::lifecycle::core_executor::CoreBoundaryStageCommitAuthority
    for PreparedTransientTurnContextBoundary
{
    fn commit(&mut self) -> Result<(), CoreBoundaryStageError> {
        self.resolve(TransientTurnContextBoundaryResolution::Committed)
    }

    fn abort(&mut self) -> Result<(), CoreBoundaryStageError> {
        self.resolve(TransientTurnContextBoundaryResolution::Aborted)
    }
}

impl Drop for PreparedTransientTurnContextBoundary {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.resolve(TransientTurnContextBoundaryResolution::Aborted);
        }
    }
}

impl Default for TransientTurnContextStateHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TransientTurnContextStateHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TransientTurnContextStateHandle")
            .field("actor_incarnation", &self.boundary.incarnation_id)
            .finish_non_exhaustive()
    }
}

impl TransientTurnContextStateHandle {
    #[must_use]
    pub fn new() -> Self {
        Self {
            boundary: Arc::new(TransientTurnContextBoundaryCoordinator::default()),
        }
    }

    pub(crate) fn begin_boundary_run(
        &self,
        run_id: RunId,
    ) -> Result<TransientTurnContextBoundaryRunGuard, CoreBoundaryStageError> {
        self.open_next_boundary(&run_id)?;
        Ok(TransientTurnContextBoundaryRunGuard {
            boundary: Arc::clone(&self.boundary),
            run_id,
        })
    }

    pub(crate) fn open_next_boundary(&self, run_id: &RunId) -> Result<u64, CoreBoundaryStageError> {
        let mut lifecycle = self.boundary.lock();
        if !lifecycle.actor_live {
            return Err(CoreBoundaryStageError::stale(format!(
                "actor incarnation {} was revoked",
                self.boundary.incarnation_id
            )));
        }
        match &lifecycle.window {
            TransientTurnContextBoundaryWindow::Open {
                run_id: current,
                generation,
                ..
            } if current == run_id => return Ok(*generation),
            TransientTurnContextBoundaryWindow::Parked { .. }
            | TransientTurnContextBoundaryWindow::Resolved { .. } => {
                return Err(CoreBoundaryStageError::fault(
                    "runner attempted to open a boundary while its predecessor was unresolved",
                ));
            }
            TransientTurnContextBoundaryWindow::Open {
                run_id: current, ..
            } => {
                return Err(CoreBoundaryStageError::stale(format!(
                    "run {run_id} cannot replace boundary owned by {current}"
                )));
            }
            TransientTurnContextBoundaryWindow::Closed => {}
        }
        lifecycle.next_generation = lifecycle
            .next_generation
            .checked_add(1)
            .ok_or_else(|| CoreBoundaryStageError::fault("boundary generation overflow"))?;
        let generation = lifecycle.next_generation;
        lifecycle.window = TransientTurnContextBoundaryWindow::Open {
            run_id: run_id.clone(),
            generation,
            request: None,
        };
        drop(lifecycle);
        self.boundary.notify.notify_waiters();
        Ok(generation)
    }

    pub async fn prepare_active_turn_boundary(
        &self,
        expected_run_id: &RunId,
        contexts: Vec<TurnRequestContext>,
    ) -> Result<PreparedTransientTurnContextBoundary, CoreBoundaryStageError> {
        if contexts.is_empty() {
            return Err(CoreBoundaryStageError::fault(
                "transient boundary preparation requires at least one context value",
            ));
        }

        let request_id = {
            let mut lifecycle = self.boundary.lock();
            if !lifecycle.actor_live {
                return Err(CoreBoundaryStageError::stale(format!(
                    "actor incarnation {} was revoked",
                    self.boundary.incarnation_id
                )));
            }
            let (run_id, request) = match &mut lifecycle.window {
                TransientTurnContextBoundaryWindow::Open {
                    run_id, request, ..
                } => (run_id, request),
                TransientTurnContextBoundaryWindow::Closed => {
                    return Err(CoreBoundaryStageError::unavailable(format!(
                        "run {expected_run_id} has no open cooperative model boundary"
                    )));
                }
                TransientTurnContextBoundaryWindow::Parked { .. }
                | TransientTurnContextBoundaryWindow::Resolved { .. } => {
                    return Err(CoreBoundaryStageError::unavailable(format!(
                        "the next boundary for run {expected_run_id} was already claimed"
                    )));
                }
            };
            if run_id != expected_run_id {
                return Err(CoreBoundaryStageError::stale(format!(
                    "open boundary belongs to run {run_id}, not {expected_run_id}"
                )));
            }
            if request.is_some() {
                return Err(CoreBoundaryStageError::unavailable(format!(
                    "the next boundary for run {expected_run_id} already has a preparation"
                )));
            }
            lifecycle.next_request_id = lifecycle
                .next_request_id
                .checked_add(1)
                .ok_or_else(|| CoreBoundaryStageError::fault("boundary request id overflow"))?;
            let request_id = lifecycle.next_request_id;
            let TransientTurnContextBoundaryWindow::Open { request, .. } = &mut lifecycle.window
            else {
                return Err(CoreBoundaryStageError::fault(
                    "boundary window changed while registering preparation",
                ));
            };
            *request = Some(RegisteredTransientTurnContextBoundaryRequest {
                request_id,
                contexts,
            });
            request_id
        };

        let mut pending = PendingTransientTurnContextBoundaryPreparation {
            boundary: Arc::clone(&self.boundary),
            request_id,
            armed: true,
        };
        self.boundary.notify.notify_waiters();

        loop {
            let notified = self.boundary.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let poll = {
                let lifecycle = self.boundary.lock();
                if lifecycle.actor_live {
                    match &lifecycle.window {
                        TransientTurnContextBoundaryWindow::Parked {
                            run_id,
                            generation,
                            request_id: parked_request_id,
                            ..
                        } if *parked_request_id == request_id => {
                            Ok(Some(PreparedTransientTurnContextBoundary {
                                state: self.clone(),
                                expected_run_id: run_id.clone(),
                                generation: *generation,
                                request_id,
                                armed: true,
                                _not_sync: std::marker::PhantomData,
                            }))
                        }
                        TransientTurnContextBoundaryWindow::Open { request, .. }
                            if request
                                .as_ref()
                                .is_some_and(|request| request.request_id == request_id) =>
                        {
                            Ok(None)
                        }
                        _ => Err(CoreBoundaryStageError::unavailable(format!(
                            "run {expected_run_id} ended before transient boundary request {request_id} parked"
                        ))),
                    }
                } else {
                    Err(CoreBoundaryStageError::stale(format!(
                        "actor incarnation {} was revoked while preparing boundary",
                        self.boundary.incarnation_id
                    )))
                }
            };
            match poll {
                Ok(Some(prepared)) => {
                    pending.armed = false;
                    return Ok(prepared);
                }
                Ok(None) => notified.as_mut().await,
                Err(error) => return Err(error),
            }
        }
    }

    /// Consume context published for this exact boundary.
    ///
    /// Runner-first closes the window with an empty result. Prepare-first parks
    /// until the unique external authority commits or aborts.
    pub(crate) async fn take_pending_at_exact_boundary(
        &self,
        run_id: &RunId,
    ) -> Result<Vec<TurnRequestContext>, CoreBoundaryStageError> {
        let request_id = {
            let mut lifecycle = self.boundary.lock();
            if !lifecycle.actor_live {
                return Err(CoreBoundaryStageError::stale(format!(
                    "actor incarnation {} was revoked",
                    self.boundary.incarnation_id
                )));
            }
            let (generation, request) = match &mut lifecycle.window {
                TransientTurnContextBoundaryWindow::Open {
                    run_id: current,
                    generation,
                    request,
                } if current == run_id => (*generation, request.take()),
                TransientTurnContextBoundaryWindow::Open {
                    run_id: current, ..
                } => {
                    return Err(CoreBoundaryStageError::stale(format!(
                        "runner {run_id} reached boundary owned by {current}"
                    )));
                }
                TransientTurnContextBoundaryWindow::Closed => {
                    return Err(CoreBoundaryStageError::unavailable(format!(
                        "run {run_id} reached a boundary with no open generation"
                    )));
                }
                TransientTurnContextBoundaryWindow::Parked { .. }
                | TransientTurnContextBoundaryWindow::Resolved { .. } => {
                    return Err(CoreBoundaryStageError::fault(
                        "runner re-entered an unresolved transient model boundary",
                    ));
                }
            };
            let Some(request) = request else {
                lifecycle.window = TransientTurnContextBoundaryWindow::Closed;
                return Ok(Vec::new());
            };
            let request_id = request.request_id;
            lifecycle.window = TransientTurnContextBoundaryWindow::Parked {
                run_id: run_id.clone(),
                generation,
                request_id,
                contexts: request.contexts,
            };
            request_id
        };
        self.boundary.notify.notify_waiters();

        struct RunnerParkGuard {
            boundary: Arc<TransientTurnContextBoundaryCoordinator>,
            request_id: u64,
            armed: bool,
        }
        impl Drop for RunnerParkGuard {
            fn drop(&mut self) {
                if self.armed {
                    let _ = self.boundary.abort_request(self.request_id);
                }
            }
        }
        let mut park_guard = RunnerParkGuard {
            boundary: Arc::clone(&self.boundary),
            request_id,
            armed: true,
        };

        loop {
            let notified = self.boundary.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let poll = {
                let mut lifecycle = self.boundary.lock();
                if lifecycle.actor_live {
                    match &lifecycle.window {
                        TransientTurnContextBoundaryWindow::Parked {
                            request_id: parked_request_id,
                            ..
                        } if *parked_request_id == request_id => Ok(None),
                        TransientTurnContextBoundaryWindow::Resolved {
                            run_id: resolved_run_id,
                            request_id: resolved_request_id,
                            ..
                        } if resolved_run_id == run_id && *resolved_request_id == request_id => {
                            let (resolution, contexts) = match std::mem::replace(
                                &mut lifecycle.window,
                                TransientTurnContextBoundaryWindow::Closed,
                            ) {
                                TransientTurnContextBoundaryWindow::Resolved {
                                    contexts,
                                    resolution,
                                    ..
                                } => (resolution, contexts),
                                _ => unreachable!("matched resolved transient boundary"),
                            };
                            let contexts = if matches!(
                                resolution,
                                TransientTurnContextBoundaryResolution::Committed
                            ) {
                                contexts
                            } else {
                                Vec::new()
                            };
                            Ok(Some(contexts))
                        }
                        _ => Err(CoreBoundaryStageError::stale(format!(
                            "parked transient request {request_id} lost exact authority"
                        ))),
                    }
                } else {
                    Err(CoreBoundaryStageError::stale(format!(
                        "actor incarnation {} was revoked while parked",
                        self.boundary.incarnation_id
                    )))
                }
            };
            match poll {
                Ok(Some(contexts)) => {
                    park_guard.armed = false;
                    return Ok(contexts);
                }
                Ok(None) => notified.as_mut().await,
                Err(error) => {
                    park_guard.armed = false;
                    return Err(error);
                }
            }
        }
    }

    #[doc(hidden)]
    pub fn revoke_boundary_actor(&self) {
        self.boundary.revoke_actor();
    }
}
/// Typed terminal-lifecycle projection of the canonical
/// [`session_document::SessionDocumentMachine`] `session_lifecycle_terminal`
/// fact.
///
/// The machine owns archive lifecycle truth for ALL profiles (LUC-524 R004
/// fold): both the runtime-backed and the store-only archive paths drive the
/// machine's `ArchiveSessionDocument` input, and this reserved-key field is
/// the machine-realized durable projection of the emitted verdict — the shell
/// realizes it, it never decides it. `RuntimeState::Retired` is the runtime
/// realization of the SAME verdict; the fail-closed realization order (durable
/// document commit first, runtime retire second) keeps the two projections
/// convergent. A two-variant enum (rather than a bare bool) keeps future
/// terminal classes — e.g. `Destroyed` — extending the type rather than the
/// call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycleTerminal {
    /// The session is live / resumable.
    Active,
    /// The session has been archived and is terminal.
    Archived,
}

impl SessionLifecycleTerminal {
    /// Whether this terminal fact marks the session as archived.
    #[must_use]
    pub fn is_archived(self) -> bool {
        matches!(self, Self::Archived)
    }
}

impl From<SessionLifecycleTerminal> for session_document::SessionDocumentLifecycle {
    fn from(value: SessionLifecycleTerminal) -> Self {
        match value {
            SessionLifecycleTerminal::Active => Self::Active,
            SessionLifecycleTerminal::Archived => Self::Archived,
        }
    }
}

impl From<session_document::SessionDocumentLifecycle> for SessionLifecycleTerminal {
    fn from(value: session_document::SessionDocumentLifecycle) -> Self {
        match value {
            session_document::SessionDocumentLifecycle::Active => Self::Active,
            session_document::SessionDocumentLifecycle::Archived => Self::Archived,
        }
    }
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
///
/// `last_seen_provenance` is the single typed identity owner. The formatted
/// `stable_owner_key` string is a read-only projection derived on demand via
/// [`crate::tool_catalog::stable_owner_key_from_provenance`], never stored
/// beside the owner.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ToolVisibilityWitness {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_provenance: Option<ToolProvenance>,
}

impl ToolVisibilityWitness {
    pub fn has_identity_witness(&self) -> bool {
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
    pub name: ToolName,
    pub witness: ToolVisibilityWitness,
}

impl DeferredToolLoadAuthority {
    pub fn new(name: impl Into<ToolName>, witness: ToolVisibilityWitness) -> Self {
        Self {
            name: name.into(),
            witness,
        }
    }

    pub fn into_parts(self) -> (ToolName, ToolVisibilityWitness) {
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
    pub witnesses: BTreeMap<ToolName, ToolVisibilityWitness>,
}

impl WitnessedToolFilter {
    pub fn new(filter: ToolFilter, witnesses: BTreeMap<ToolName, ToolVisibilityWitness>) -> Self {
        Self { filter, witnesses }
    }

    pub fn into_parts(self) -> (ToolFilter, BTreeMap<ToolName, ToolVisibilityWitness>) {
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
    witnesses: BTreeMap<ToolName, ToolVisibilityWitness>,
}

impl InheritedToolVisibilityAuthority {
    pub(crate) fn from_generated_composition_authority(
        filter: ToolFilter,
        witnesses: BTreeMap<ToolName, ToolVisibilityWitness>,
    ) -> Self {
        Self { filter, witnesses }
    }

    pub fn filter(&self) -> &ToolFilter {
        &self.filter
    }

    pub fn witnesses(&self) -> &BTreeMap<ToolName, ToolVisibilityWitness> {
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
    pub active_requested_deferred_names: BTreeSet<ToolName>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub staged_requested_deferred_names: BTreeSet<ToolName>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub active_revision: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub staged_revision: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub requested_witnesses: BTreeMap<ToolName, ToolVisibilityWitness>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub filter_witnesses: BTreeMap<ToolName, ToolVisibilityWitness>,
}

impl SessionToolVisibilityState {
    /// Deterministic projection of the generated CallingLlm visibility
    /// boundary. This is a comparison witness only: semantic promotion still
    /// belongs to the generated visibility owner.
    #[cfg(test)]
    pub(crate) fn projected_boundary_applied(&self) -> Self {
        let mut projected = self.clone();
        projected.active_filter = self.staged_filter.clone();
        projected.active_requested_deferred_names = self.staged_requested_deferred_names.clone();
        projected.active_revision = self.staged_revision;
        projected
    }
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

/// Typed refusal at the deferred callback-result ingress seam.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeferredToolResultsIngressError {
    #[error("callback result ingress contains duplicate tool id '{0}'")]
    DuplicateToolUseId(String),
    #[error("callback result for tool id '{0}' conflicts with its staged payload")]
    ConflictingRedelivery(String),
    #[error("callback result tool id '{0}' is outside the staged pending set")]
    WrongToolUseId(String),
}

/// Durable staging record for one assistant tool-use batch that contains one
/// or more external callbacks and optional locally completed siblings.
///
/// Nothing in this record is provider-visible until the callback result is
/// admitted. The completed results and transcript-producing effects are
/// published together as one complete adjacent batch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) struct PendingCallbackToolBatch {
    pub run_id: RunId,
    pub tool_use_order: Vec<String>,
    pub pending_tool_use_ids: Vec<String>,
    pub completed_results: Vec<ToolResult>,
    pub session_effects: Vec<crate::ops::SessionEffect>,
    pub async_ops: Vec<crate::ops::AsyncOpRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
enum CallbackToolBatchState {
    Pending {
        batch: PendingCallbackToolBatch,
    },
    Applied {
        tool_use_order: Vec<String>,
        results: Vec<ToolResult>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        async_ops: Vec<crate::ops::AsyncOpRef>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        post_tool_messages: Vec<Message>,
        #[serde(default)]
        post_tool_messages_applied: bool,
    },
}

pub(crate) enum ResolvedPendingCallbackToolResults {
    NoState,
    Pending {
        batch: PendingCallbackToolBatch,
        ordered_results: Vec<ToolResult>,
    },
    AlreadyApplied {
        async_ops: Vec<crate::ops::AsyncOpRef>,
    },
}

/// Admission verdict for callback results presented at a session-service
/// boundary before they enter deferred-turn state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub enum CallbackResultIngress {
    /// The session has no durable callback batch; legacy callers may use their
    /// ordinary deferred-input policy.
    NoPendingBatch,
    /// The exact result set belongs to the pending batch.
    Pending { pending_tool_use_ids: Vec<String> },
    /// The identical callback payload was already committed.
    AlreadyApplied,
}

/// Typed failures at the durable callback-batch staging/apply seam.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PendingCallbackBatchError {
    #[error("a pending callback batch is already staged")]
    AlreadyStaged,
    #[error("no pending callback batch is staged")]
    Missing,
    #[error("pending callback batch is malformed: {0}")]
    Malformed(String),
    #[error("callback results contain duplicate tool id '{0}'")]
    DuplicateResult(String),
    #[error("mob authority replacement cannot cross a durable callback staging boundary")]
    NonDurableAuthorityEffect,
    #[error("callback result ids {actual:?} do not match pending ids {expected:?}")]
    ResultSetMismatch {
        expected: BTreeSet<String>,
        actual: BTreeSet<String>,
    },
    #[error("callback result redelivery conflicts with the already applied payload")]
    ConflictingRedelivery,
}

fn unique_tool_results(
    results: Vec<ToolResult>,
) -> Result<BTreeMap<String, ToolResult>, PendingCallbackBatchError> {
    let mut by_id = BTreeMap::new();
    for result in results {
        let id = result.tool_use_id.clone();
        if by_id.insert(id.clone(), result).is_some() {
            return Err(PendingCallbackBatchError::DuplicateResult(id));
        }
    }
    Ok(by_id)
}

fn validate_pending_callback_batch(
    messages: &[Message],
    batch: &PendingCallbackToolBatch,
) -> Result<(), PendingCallbackBatchError> {
    let Some(assistant) = messages.last() else {
        return Err(PendingCallbackBatchError::Malformed(
            "staged callback batch has no assistant transcript tail".to_string(),
        ));
    };
    let assistant_order = assistant_tool_use_ids(assistant)
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if assistant_order != batch.tool_use_order {
        return Err(PendingCallbackBatchError::Malformed(format!(
            "assistant tool ids {assistant_order:?} do not match staged order {:?}",
            batch.tool_use_order
        )));
    }
    let assistant_set = assistant_order.iter().cloned().collect::<BTreeSet<_>>();
    if assistant_set.len() != assistant_order.len() {
        return Err(PendingCallbackBatchError::Malformed(
            "assistant tool-use batch contains duplicate ids".to_string(),
        ));
    }
    let pending_set = batch
        .pending_tool_use_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if pending_set.len() != batch.pending_tool_use_ids.len() || pending_set.is_empty() {
        return Err(PendingCallbackBatchError::Malformed(
            "staged callback batch must contain at least one unique pending tool id".to_string(),
        ));
    }
    let completed = unique_tool_results(batch.completed_results.clone())?;
    let completed_set = completed.keys().cloned().collect::<BTreeSet<_>>();
    if !pending_set.is_disjoint(&completed_set)
        || pending_set
            .union(&completed_set)
            .cloned()
            .collect::<BTreeSet<_>>()
            != assistant_set
    {
        return Err(PendingCallbackBatchError::Malformed(format!(
            "pending ids {pending_set:?} plus completed ids {completed_set:?} do not partition assistant ids {assistant_set:?}"
        )));
    }
    if batch.session_effects.iter().any(|effect| {
        matches!(
            effect,
            crate::ops::SessionEffect::ReplaceMobToolAuthorityContext { .. }
        )
    }) {
        return Err(PendingCallbackBatchError::NonDurableAuthorityEffect);
    }
    Ok(())
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
    pub fn try_stage_tool_results(
        &mut self,
        results: Vec<ToolResult>,
        accepted_at: SystemTime,
    ) -> Result<usize, DeferredToolResultsIngressError> {
        let mut incoming_by_id = BTreeMap::new();
        for result in &results {
            if incoming_by_id
                .insert(result.tool_use_id.clone(), result)
                .is_some()
            {
                return Err(DeferredToolResultsIngressError::DuplicateToolUseId(
                    result.tool_use_id.clone(),
                ));
            }
        }

        let mut staged_by_id = BTreeMap::new();
        for pending in &self.pending_tool_results {
            for result in &pending.results {
                match staged_by_id.insert(result.tool_use_id.clone(), result) {
                    Some(previous) if previous != result => {
                        return Err(DeferredToolResultsIngressError::ConflictingRedelivery(
                            result.tool_use_id.clone(),
                        ));
                    }
                    _ => {}
                }
            }
        }
        if !staged_by_id.is_empty() {
            for (id, incoming) in &incoming_by_id {
                match staged_by_id.get(id) {
                    Some(staged) if *staged == *incoming => {}
                    Some(_) => {
                        return Err(DeferredToolResultsIngressError::ConflictingRedelivery(
                            id.clone(),
                        ));
                    }
                    None => {
                        return Err(DeferredToolResultsIngressError::WrongToolUseId(id.clone()));
                    }
                }
            }
            return Ok(0);
        }

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
                return Ok(0);
            }
        };
        let Some(accepted) = accepted else {
            tracing::warn!(
                "generated session document authority returned no tool-results decision"
            );
            return Ok(0);
        };
        if accepted == 0 {
            return Ok(0);
        }
        let accepted = usize::try_from(accepted).unwrap_or(usize::MAX);
        self.pending_tool_results.push(PendingToolResultsMessage {
            results,
            accepted_at,
        });
        Ok(accepted)
    }

    /// Compatibility projection for callers that cannot surface a typed
    /// ingress refusal. Public session-service ingress uses
    /// [`Self::try_stage_tool_results`] and preserves the error.
    pub fn stage_tool_results(
        &mut self,
        results: Vec<ToolResult>,
        accepted_at: SystemTime,
    ) -> usize {
        match self.try_stage_tool_results(results, accepted_at) {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "deferred callback-result ingress was rejected");
                0
            }
        }
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

/// Failure when appending an identity-bearing System message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemMessageAppendError {
    Conflict {
        key: String,
        existing_text: String,
        existing_source: Option<String>,
    },
}

impl std::fmt::Display for SystemMessageAppendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { key, .. } => {
                write!(
                    f,
                    "System-message append conflict for idempotency key `{key}`"
                )
            }
        }
    }
}

impl std::error::Error for SystemMessageAppendError {}

impl Session {
    /// Validate callback-result ingress against the exact durable callback
    /// batch without mutating transcript or deferred-turn state.
    #[doc(hidden)]
    pub fn classify_callback_result_ingress(
        &self,
        incoming: &[ToolResult],
    ) -> Result<CallbackResultIngress, crate::error::AgentError> {
        match self
            .resolve_pending_callback_tool_results(incoming.to_vec())
            .map_err(|error| {
                crate::error::AgentError::ConfigError(format!(
                    "callback result ingress was rejected: {error}"
                ))
            })? {
            ResolvedPendingCallbackToolResults::NoState => {
                Ok(CallbackResultIngress::NoPendingBatch)
            }
            ResolvedPendingCallbackToolResults::AlreadyApplied { .. } => {
                Ok(CallbackResultIngress::AlreadyApplied)
            }
            ResolvedPendingCallbackToolResults::Pending { batch, .. } => {
                Ok(CallbackResultIngress::Pending {
                    pending_tool_use_ids: batch.pending_tool_use_ids,
                })
            }
        }
    }

    /// Create a new empty session
    pub fn new() -> Self {
        let now = SystemTime::now();
        let id = SessionId::new();
        Self {
            version: session_version(),
            realtime_transcript: Box::new(SessionRealtimeTranscriptProjection::empty(&id)),
            id,
            messages: TranscriptMessages::default(),
            created_at: now,
            updated_at: now,
            metadata: serde_json::Map::new(),
            history_caches: Box::default(),
            transcript_history_metadata_validation: TranscriptHistoryMetadataValidation::Validated,
            usage: Usage::default(),
        }
    }

    /// Create a session with a specific ID (for loading)
    pub fn with_id(id: SessionId) -> Self {
        let mut session = Self::new();
        session.realtime_transcript = Box::new(SessionRealtimeTranscriptProjection::empty(&id));
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

    /// Format-2 content digest of the live transcript.
    ///
    /// Byte-identical to `transcript_messages_digest(session.messages())` —
    /// same canonicalization, same bytes, same string — but served from the
    /// session's retained SHA-256 midstate when one covers the current buffer,
    /// so an ordinary append costs O(delta) instead of O(document). Prefer
    /// this over the free function anywhere a `Session` is in hand; the free
    /// function stays for slices that no session owns (revision bodies,
    /// candidate vectors).
    pub fn transcript_content_digest(&self) -> Result<String, serde_json::Error> {
        self.messages.digest()
    }

    /// Format-2 content digest of the first `count` live messages.
    ///
    /// Served from the boundary ring when a previous full digest was taken at
    /// exactly that count — which is the save-guard prefix question — and by
    /// full recompute otherwise.
    pub fn transcript_prefix_digest(&self, count: usize) -> Result<String, serde_json::Error> {
        if count > self.messages.len() {
            // Fail closed rather than silently digesting a shorter prefix: a
            // caller asking past the end has lost track of which row it is
            // comparing against, and answering with a different prefix's
            // digest would launder that into a continuity verdict.
            return Err(<serde_json::Error as serde::ser::Error>::custom(format!(
                "transcript prefix digest requested for {count} messages but the transcript has {}",
                self.messages.len()
            )));
        }
        if let Some(witness) = self.messages.prefix_digest_witness(count) {
            return Ok(witness);
        }
        transcript_messages_digest(&self.messages[..count])
    }

    /// Number of non-append transcript mutations this in-memory session has
    /// applied. Diagnostics and regression tests only.
    #[doc(hidden)]
    #[must_use]
    pub fn transcript_mutation_epoch(&self) -> u64 {
        self.messages.mutation_epoch()
    }

    /// Replace the message buffer for core-owned internal transcript rewrites.
    ///
    /// Intentionally `pub(crate)`: cross-crate consumers must route same-session
    /// rewrites through transcript-edit APIs so the revision graph remains the
    /// semantic owner of message history.
    #[allow(dead_code)] // Kept for core-owned optional rewrite paths and focused invariants.
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

    /// Replace the full transcript under the opaque authority minted by the
    /// validated compaction rebuild path.
    pub(crate) fn replace_messages_for_compaction_internal(
        &mut self,
        messages: Vec<Message>,
        authority: &crate::agent::compact::ValidatedCompactionRewrite,
    ) -> Result<Option<TranscriptRewriteCommit>, TranscriptEditError> {
        // Authority first. The parent side binds against the session
        // accumulator (O(delta), byte-identical to
        // `transcript_messages_digest(self.messages())`); the rebuilt side
        // binds inside the commit below, where its one required digest is
        // computed and compared against the token's revision. The no-op
        // answer then falls out of the token's own two digests instead of
        // two more whole-document hashes.
        let parent_revision = self
            .transcript_content_digest()
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        if !authority.authorizes_parent_digest(
            &parent_revision,
            self.messages.len(),
            messages.len(),
        ) {
            return Err(TranscriptEditError::InvalidTranscriptShape(
                "validated compaction witness does not authorize this exact transcript rebuild"
                    .to_string(),
            ));
        }
        if authority.is_no_op() {
            return Ok(None);
        }
        let summary_count = messages
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
            })
            .count();
        if messages.len() >= self.messages.len() || summary_count != 1 {
            return Err(TranscriptEditError::InvalidTranscriptShape(
                "validated compaction rewrite must shrink the transcript and carry exactly one CompactionSummary"
                    .to_string(),
            ));
        }
        let selection =
            TranscriptRewriteSelection::validated_compaction(0, self.messages.len(), authority);
        let commit = self.commit_transcript_rewrite_bound(
            selection,
            messages,
            TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(authority.parent_revision().to_string()),
            Some(authority.revision()),
        )?;
        Ok(Some(commit))
    }

    /// Construct one legitimate typed compaction and its paired projection
    /// intent for downstream persistence-contract tests.
    ///
    /// This seam deliberately remains behind `test-support`: tests may ask the
    /// core compaction owner to mint the opaque witness, but may not reproduce
    /// its selection tag or projection fingerprint outside this crate.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn stage_validated_compaction_for_test(
        &mut self,
        replacement: Vec<Message>,
        summary_tokens: u64,
    ) -> Result<
        (
            TranscriptRewriteCommit,
            crate::memory::CompactionProjectionIntent,
        ),
        String,
    > {
        let messages_before = self.messages.len();
        let authority = crate::agent::compact::ValidatedCompactionRewrite::for_test(
            self.messages(),
            &replacement,
        )
        .map_err(|error| error.to_string())?;
        let commit = self
            .replace_messages_for_compaction_internal(replacement, &authority)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "test compaction rewrite was a no-op".to_string())?;
        let projection = crate::memory::CompactionProjectionId::from_validated_transcript_rewrite(
            self.id().clone(),
            &commit,
            &authority,
        )
        .ok_or_else(|| {
            "core-owned test compaction did not mint a projection identity".to_string()
        })?;
        let intent = crate::memory::CompactionProjectionIntent {
            projection,
            summary_tokens,
            messages_before,
            messages_after: self.messages.len(),
        };
        self.add_compaction_projection_intent(intent.clone())
            .map_err(|error| error.to_string())?;
        Ok((commit, intent))
    }

    /// Atomically refresh the synthetic runtime notices of one kind.
    ///
    /// This is the ONE transcript authority operation for synthetic-notice
    /// refresh: it strips every synthetic `SystemNotice` projection of `kind`
    /// while preserving durable notices that share the kind, then appends
    /// `replacements` (possibly empty, meaning "no current synthetic notice")
    /// as one mechanical projection update. It deliberately does not mint an
    /// audited transcript rewrite commit. On a strip fault nothing is pushed
    /// and the typed [`TranscriptEditError`] propagates — callers must not
    /// re-implement the strip-then-push pair (the swallowed-strip variant
    /// leaves a stale notice beside a fresh one: a divergence window).
    pub fn replace_synthetic_notices(
        &mut self,
        kind: crate::types::SystemNoticeKind,
        replacements: Vec<Message>,
    ) -> Result<(), TranscriptEditError> {
        if !kind.is_synthetic_refresh_projection() {
            return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                "system notice kind {kind:?} is durable transcript content, not a synthetic refresh projection"
            )));
        }
        for (index, message) in replacements.iter().enumerate() {
            let matches_kind = matches!(
                message,
                Message::SystemNotice(notice)
                    if notice.kind == kind && notice.is_synthetic_refresh_projection()
            );
            if !matches_kind {
                return Err(TranscriptEditError::InvalidTranscriptShape(format!(
                    "replacement {index} for synthetic notice kind {kind:?} is not a system notice of that kind"
                )));
            }
        }

        // No-op detection without hashing the document. The refresh removes
        // every synthetic notice of `kind` and appends `replacements` at the
        // tail, and every retained message is a clone of the live one, so
        // the refreshed vector is canonically identical to the current one
        // IFF the existing notices already sit contiguously at the tail and
        // each is canonical-equal to its replacement (canonical equality
        // erases construction bookkeeping such as `created_at`, exactly like
        // the digest comparison this replaces — which re-canonicalized and
        // re-hashed the WHOLE document once per pre-LLM boundary). A
        // non-notice message can never be canonical-equal to a notice, so
        // the positional argument is exact, not heuristic.
        let is_refresh_notice = |message: &Message| {
            matches!(
                message,
                Message::SystemNotice(notice)
                    if notice.kind == kind && notice.is_synthetic_refresh_projection()
            )
        };
        let existing_count = self
            .messages
            .iter()
            .filter(|message| is_refresh_notice(message))
            .count();
        let tail_start = self.messages.len().saturating_sub(existing_count);
        let existing_are_contiguous_tail =
            self.messages[tail_start..].iter().all(&is_refresh_notice);
        if existing_count == replacements.len()
            && existing_are_contiguous_tail
            && self.messages[tail_start..]
                .iter()
                .zip(replacements.iter())
                .all(|(existing, replacement)| {
                    canonicalize_message_for_digest(existing)
                        == canonicalize_message_for_digest(replacement)
                })
        {
            return Ok(());
        }

        let lowest_mutated_index = self
            .messages
            .iter()
            .position(&is_refresh_notice)
            .unwrap_or(self.messages.len());
        let mut refreshed = self
            .messages
            .iter()
            .filter(|message| !is_refresh_notice(message))
            .cloned()
            .collect::<Vec<_>>();
        refreshed.extend(replacements);
        let realtime_rebase = self.prepare_realtime_transcript_rebase_after_rewrite(
            &refreshed,
            RealtimeTranscriptSnapshotReasonV1::TranscriptRewrite,
        )?;
        if let Some(history) = self.validated_transcript_history_state()? {
            let head_len = history
                .final_endpoint_witness()
                .ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "compact graph has no final endpoint witness".to_string(),
                    )
                })?
                .message_count();
            // This transformation removes only matching notices and appends
            // every replacement at the tail. Therefore an existing matching
            // notice inside the audited prefix is exactly the shape that
            // would alter it; no whole-prefix hash or message copy is needed
            // to prove the negative.
            if self.messages.len() < head_len
                || self.messages[..head_len].iter().any(&is_refresh_notice)
            {
                return Err(TranscriptEditError::InvalidTranscriptShape(
                    "synthetic notice refresh would rewrite the audited transcript prefix; route it through a typed transcript rewrite"
                        .to_string(),
                ));
            }
        }
        let updated_at = SystemTime::now();
        if lowest_mutated_index == self.messages.len() {
            // No prior notice existed: this is an exact append, so preserve
            // the live digest and row-lineage accumulators as an append.
            let appended = refreshed.split_off(lowest_mutated_index);
            self.messages.extend_batch(appended);
        } else {
            // SEAM 1 (known-index replacement): synthetic notices are stripped
            // from `lowest_mutated_index` onward and replacements are appended.
            // Park the accumulator so it can retain an exact durable-row anchor
            // only when the first changed row is at or beyond that anchor. A
            // refresh that reaches into committed history therefore still drops
            // the witness and must cross the typed rewrite boundary before the
            // next HeadCanonical persist.
            *self.messages.begin_in_place_scan() = refreshed;
            self.messages
                .finish_in_place_scan(Some(lowest_mutated_index));
        }
        self.mark_content_mutated(updated_at);
        self.realtime_transcript
            .apply_prepared_rebase(realtime_rebase);
        Ok(())
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
        // SEAM 2 (append): the accumulator folds only the appended bytes.
        // Retained rewrite history is intentionally untouched: its head is the
        // latest AUDITED endpoint, while this live append is owned by
        // `messages` plus the digest accumulator.
        self.messages.push(message);
        self.mark_content_mutated(SystemTime::now());
    }

    /// Add multiple messages in one operation (single timestamp update)
    ///
    /// More efficient than multiple `push` calls when adding many messages.
    pub fn push_batch(&mut self, messages: Vec<Message>) {
        if messages.is_empty() {
            return;
        }
        // SEAM 3 (append): the accumulator folds only the appended batch.
        // See `push`: ordinary appends never materialize or rewrite the
        // transcript-history compatibility projection.
        self.messages.extend_batch(messages);
        self.mark_content_mutated(SystemTime::now());
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
        // SEAM 4 (in-place media scan): the scan reports the lowest mutated
        // index. `None` means the buffer is byte-identical, so the retained
        // midstate stays valid. Either way audited graph metadata is
        // independent and remains untouched.
        let buffer = self.messages.begin_in_place_scan();
        let lowest_mutated = match crate::image_content::externalize_messages_from_reporting_lowest(
            blob_store, buffer, start,
        )
        .await
        {
            Ok(lowest_mutated) => lowest_mutated,
            Err(error) => {
                // The scan may have externalized part of the buffer before
                // failing; fail safe by discarding the parked midstate.
                self.messages.finish_in_place_scan(Some(start));
                return Err(error);
            }
        };
        self.messages.finish_in_place_scan(lowest_mutated);
        Ok(())
    }

    /// Hydrate user-message images in-place for a realtime provider replay,
    /// under an explicit cumulative decoded-byte budget.
    ///
    /// Realtime reconnect/open is an execution seam, not a historical display
    /// read: missing or malformed blobs fail closed, repeated references count
    /// independently, and image-bearing tool/system content that the realtime
    /// history projector does not consume remains blob-backed.
    pub async fn hydrate_realtime_user_images(
        &mut self,
        blob_store: &dyn crate::BlobStore,
        max_decoded_bytes: usize,
    ) -> Result<(), crate::image_content::RealtimeUserImageHydrationError> {
        self.hydrate_realtime_user_images_with_usage(blob_store, max_decoded_bytes)
            .await
            .map(|_| ())
    }

    /// Hydrate realtime user-message images and return the full canonical
    /// decoded-byte usage for seed-independent future-image admission.
    pub async fn hydrate_realtime_user_images_with_usage(
        &mut self,
        blob_store: &dyn crate::BlobStore,
        max_decoded_bytes: usize,
    ) -> Result<usize, crate::image_content::RealtimeUserImageHydrationError> {
        // SEAM 5 (in-place media scan): same contract as `externalize_media`.
        let buffer = self.messages.begin_in_place_scan();
        let (decoded_total, lowest_mutated) =
            match crate::image_content::hydrate_user_images_for_realtime_projection_reporting_lowest(
                blob_store,
                buffer,
                max_decoded_bytes,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.messages.finish_in_place_scan(Some(0));
                    return Err(error);
                }
            };
        self.messages.finish_in_place_scan(lowest_mutated);
        // This typed hydrator mutates User messages only. It cannot change a
        // SystemNotice semantic identity, so the terminal index remains exact.
        Ok(decoded_total)
    }

    /// Advance the durable content timestamp.
    fn mark_content_mutated(&mut self, at: SystemTime) {
        self.updated_at = at;
    }

    /// Explicitly update the timestamp
    ///
    /// Call this after bulk operations that don't update timestamps automatically.
    pub fn touch(&mut self) {
        self.mark_content_mutated(SystemTime::now());
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
        self.mark_content_mutated(SystemTime::now());
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
    /// turns. Provider item ids, predecessor links, and content segment ids
    /// reduce into the in-memory projection while the exact typed event is
    /// appended to the authenticated HeadCanonical component sidecar.
    /// WholeBlob serialization alone materializes the accumulated projection.
    pub fn append_realtime_transcript_event(
        &mut self,
        event: RealtimeTranscriptEvent,
    ) -> RealtimeTranscriptApplyOutcome {
        let (commit, recorded) =
            self.realtime_transcript
                .apply_event(event)
                .unwrap_or_else(|err| {
                    fail_closed_generated_restore(
                        "realtime-transcript",
                        <serde_json::Error as serde::de::Error>::custom(err),
                    )
                });
        if recorded {
            self.mark_content_mutated(SystemTime::now());
        }
        self.push_batch(commit.messages);
        if commit.usage != Usage::default() {
            self.record_usage(commit.usage);
        }
        commit.outcome
    }

    /// Preview replay/rejection for non-text realtime user content without
    /// mutating session state. Used by persistence before blob writes.
    #[must_use]
    pub fn preflight_realtime_user_content_event(
        &self,
        event: &RealtimeTranscriptEvent,
    ) -> Option<crate::RealtimeUserContentApplyOutcome> {
        realtime_transcript_revision::preflight_realtime_user_content_event(
            self.realtime_transcript.state(),
            event,
        )
        .unwrap_or_else(|err| {
            fail_closed_generated_restore(
                "realtime-user-content-preflight",
                <serde_json::Error as serde::de::Error>::custom(err),
            )
        })
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
        realtime_transcript_revision::in_flight_realtime_assistant_response_ids(
            self.realtime_transcript.state(),
        )
    }

    /// Durable session-scoped bindings used to make live non-text input retry
    /// safe across provider reconnects and lost public receipts.
    #[must_use]
    pub fn realtime_user_content_identities(&self) -> Vec<RealtimeUserContentIdentity> {
        realtime_transcript_revision::realtime_user_content_identities(
            self.realtime_transcript.state(),
        )
    }

    /// Return the bounded metadata-only image-blob recovery anchor, if one is
    /// durably staged ahead of reducer finalization.
    #[must_use]
    pub fn pending_realtime_user_content_blob(
        &self,
    ) -> Option<crate::PendingRealtimeUserContentBlob> {
        realtime_transcript_revision::pending_realtime_user_content_blob(
            self.realtime_transcript.state(),
        )
    }

    /// Stage or exactly reuse the one-slot durable image-blob recovery anchor
    /// through generated SessionDocument authority.
    pub fn stage_pending_realtime_user_content_blob(
        &mut self,
        pending: crate::PendingRealtimeUserContentBlob,
    ) -> Result<
        crate::generated::session_document::RealtimeUserContentBlobStageDisposition,
        realtime_transcript_revision::RealtimeTranscriptShellError,
    > {
        match self
            .realtime_transcript
            .stage_pending_user_content_blob(pending)
        {
            Ok(disposition) => {
                if disposition
                    == crate::generated::session_document::RealtimeUserContentBlobStageDisposition::StageNew
                {
                    self.mark_content_mutated(SystemTime::now());
                }
                Ok(disposition)
            }
            Err(RealtimeTranscriptSidecarError::Reducer(error)) => Err(error),
            Err(error) => fail_closed_generated_restore(
                "realtime-user-content-stage",
                <serde_json::Error as serde::de::Error>::custom(error),
            ),
        }
    }

    pub fn resolve_pending_realtime_user_content_blob_recovery(
        &self,
        request: Option<&crate::PendingRealtimeUserContentBlob>,
        pending_blob_valid: bool,
    ) -> Result<
        crate::generated::session_document::RealtimeUserContentBlobRecoveryDisposition,
        realtime_transcript_revision::RealtimeTranscriptShellError,
    > {
        realtime_transcript_revision::resolve_pending_realtime_user_content_blob_recovery(
            self.realtime_transcript.state(),
            request,
            pending_blob_valid,
        )
    }

    /// Clear a missing/corrupt occupied anchor only after generated recovery
    /// authority classifies a different request as `ClearInvalidBeforeCurrent`.
    pub fn clear_invalid_pending_realtime_user_content_blob(
        &mut self,
        request: Option<&crate::PendingRealtimeUserContentBlob>,
    ) -> Result<(), realtime_transcript_revision::RealtimeTranscriptShellError> {
        match self
            .realtime_transcript
            .clear_invalid_pending_user_content_blob(request)
        {
            Ok(()) => {
                self.mark_content_mutated(SystemTime::now());
                Ok(())
            }
            Err(RealtimeTranscriptSidecarError::Reducer(error)) => Err(error),
            Err(error) => fail_closed_generated_restore(
                "realtime-user-content-clear",
                <serde_json::Error as serde::de::Error>::custom(error),
            ),
        }
    }

    /// Durable caller keys whose canonical realtime image was removed by a
    /// same-session transcript rewrite. Provider adapters consume these as a
    /// pre-send conflict registry on open and refresh.
    #[must_use]
    pub fn realtime_user_content_tombstones(
        &self,
    ) -> Vec<crate::realtime_transcript::RealtimeUserContentTombstone> {
        realtime_transcript_revision::realtime_user_content_tombstones(
            self.realtime_transcript.state(),
        )
    }

    fn prepare_realtime_transcript_rebase_after_rewrite(
        &self,
        messages: &[Message],
        reason: RealtimeTranscriptSnapshotReasonV1,
    ) -> Result<PreparedRealtimeTranscriptRebase, TranscriptEditError> {
        let state =
            realtime_transcript_revision::reconcile_realtime_transcript_state_after_rewrite(
                self.realtime_transcript.state().clone(),
                messages,
            )
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        self.realtime_transcript
            .prepare_rebase_snapshot(state, reason)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))
    }

    /// Append an ordinary System message at the current transcript boundary.
    pub fn append_system_message(&mut self, content: impl Into<String>) {
        use crate::types::SystemMessage;

        self.push(Message::System(SystemMessage::new(content)));
    }

    /// Append an ordinary System message with optional control-ingress
    /// identity.
    ///
    /// The ordered transcript is the singular durable owner. Idempotency is
    /// checked only for this explicit control operation; ordinary turn and
    /// resume paths never scan the transcript.
    pub fn append_system_message_idempotent(
        &mut self,
        content: impl Into<String>,
        source: Option<String>,
        idempotency_key: Option<String>,
        created_at: crate::types::MessageTimestamp,
    ) -> Result<crate::service::AppendSystemContextStatus, SystemMessageAppendError> {
        use crate::types::{SystemMessage, SystemMessageIdentity};

        let content = content.into();
        if let Some(key) = idempotency_key.as_deref() {
            for message in self.messages() {
                let Message::System(existing) = message else {
                    continue;
                };
                let Some(identity) = existing.identity.as_ref() else {
                    continue;
                };
                if identity.idempotency_key.as_deref() != Some(key) {
                    continue;
                }
                if existing.content == content && identity.source == source {
                    return Ok(crate::service::AppendSystemContextStatus::Duplicate);
                }
                return Err(SystemMessageAppendError::Conflict {
                    key: key.to_string(),
                    existing_text: existing.content.clone(),
                    existing_source: identity.source.clone(),
                });
            }
        }

        let identity =
            (source.is_some() || idempotency_key.is_some()).then_some(SystemMessageIdentity {
                source,
                idempotency_key,
            });
        self.push(Message::System(SystemMessage {
            content,
            created_at,
            identity,
        }));
        Ok(crate::service::AppendSystemContextStatus::Applied)
    }

    /// Clone the active ordered transcript for a model request.
    ///
    /// System messages are ordinary durable rows. No request-local System
    /// message is synthesized or repositioned at this boundary.
    pub fn messages_for_model_boundary(&self) -> Vec<Message> {
        self.messages().to_vec()
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
                _ => None,
            })
            .sum()
    }

    /// Get non-component session metadata.
    ///
    /// Typed realtime/system-context reducer projections are intentionally not
    /// exposed through this raw map. WholeBlob serialization materializes its
    /// compatibility projection separately; HeadCanonical binds typed
    /// component roots.
    pub fn metadata(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.metadata
    }

    /// Borrow the accumulated projection for explicit WholeBlob compatibility
    /// encoding. HeadCanonical code must use the compact component prefix.
    pub(crate) fn whole_blob_realtime_transcript_state(
        &self,
    ) -> Option<&SessionRealtimeTranscriptState> {
        self.realtime_transcript.whole_blob_projection()
    }

    /// Inject the accumulated realtime projection into a WholeBlob metadata
    /// map. HeadCanonical digest/head builders must not call this: they bind
    /// the compact event prefix directly and never materialize this value.
    pub(crate) fn inject_realtime_whole_blob_projection(
        &self,
        metadata: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), serde_json::Error> {
        if let Some(projection) = self.whole_blob_realtime_transcript_state() {
            metadata.insert(
                SESSION_REALTIME_TRANSCRIPT_STATE_KEY.to_string(),
                serde_json::to_value(projection)?,
            );
        }
        Ok(())
    }

    /// Current authenticated realtime component-event prefix, including every
    /// event staged since the last durable acknowledgement.
    pub(crate) fn realtime_component_event_prefix(
        &self,
    ) -> Result<crate::ComponentEventPrefixAuthority, RealtimeTranscriptSidecarError> {
        self.realtime_transcript.successor_prefix()
    }

    /// Durable predecessor from which the pending realtime suffix extends.
    pub(crate) fn realtime_component_event_acknowledged_prefix(
        &self,
    ) -> &crate::ComponentEventPrefixAuthority {
        self.realtime_transcript.acknowledged_prefix()
    }

    /// Convert the inline WholeBlob realtime projection into one parked
    /// HeadCanonical snapshot event.
    ///
    /// This is an explicit store-activation seam, not an ordinary mutation
    /// fallback. Callers must persist the resulting component suffix and
    /// rebound schema-v4 head in one transaction.
    #[doc(hidden)]
    pub fn activate_realtime_component_sidecar(
        &mut self,
    ) -> Result<(), RealtimeTranscriptSidecarError> {
        let Some(value) = self.metadata.get(SESSION_REALTIME_TRANSCRIPT_STATE_KEY) else {
            // WholeBlob deserialization already parks the activation snapshot,
            // while new sessions legitimately begin with an empty prefix.
            return Ok(());
        };
        if !self.realtime_transcript.is_pristine() {
            return Err(RealtimeTranscriptSidecarError::Incoherent(
                "inline realtime projection cannot replace an active component sidecar".to_string(),
            ));
        }
        let state = serde_json::from_value(value.clone())?;
        let projection =
            SessionRealtimeTranscriptProjection::from_inline_snapshot(&self.id, state)?;
        self.metadata.remove(SESSION_REALTIME_TRANSCRIPT_STATE_KEY);
        *self.realtime_transcript = projection;
        Ok(())
    }

    /// Seal the exact realtime event suffix pending at this boundary.
    /// Seal the parked realtime activation/ordinary suffix for an atomic
    /// HeadCanonical store transaction.
    #[doc(hidden)]
    pub fn prepare_realtime_component_event_suffix(
        &self,
    ) -> Result<Option<crate::PreparedComponentEventSuffix>, RealtimeTranscriptSidecarError> {
        self.realtime_transcript.prepare_suffix()
    }

    /// Install a store-verified complete realtime sidecar projection.
    pub(crate) fn install_verified_realtime_component_sequence(
        &mut self,
        sequence: &crate::VerifiedComponentEventSequence,
    ) -> Result<(), RealtimeTranscriptSidecarError> {
        *self.realtime_transcript =
            SessionRealtimeTranscriptProjection::from_verified_sequence(&self.id, sequence)?;
        Ok(())
    }

    /// Adopt the exact prepared realtime prefix after the writing transaction
    /// acknowledges that same successor.
    pub(crate) fn acknowledge_realtime_component_event_suffix(
        &mut self,
        prepared: &crate::PreparedComponentEventSuffix,
        committed: &crate::ComponentEventPrefixAuthority,
    ) -> Result<(), RealtimeTranscriptSidecarError> {
        self.realtime_transcript
            .acknowledge_suffix(prepared, committed)
    }

    pub(crate) fn head_canonical_metadata_projection(
        &self,
    ) -> Result<Arc<SessionHeadMetadataProjection>, serde_json::Error> {
        self.history_caches
            .head_canonical_metadata
            .projection(&self.metadata)
            .map_err(<serde_json::Error as serde::ser::Error>::custom)
    }

    pub(crate) fn install_head_canonical_metadata_projection(
        &mut self,
        projection: &Arc<SessionHeadMetadataProjection>,
    ) -> Result<(), String> {
        self.history_caches
            .head_canonical_metadata
            .install_snapshot(projection)
    }

    pub(crate) fn acknowledge_head_canonical_metadata_projection(
        &mut self,
        projection: &Arc<SessionHeadMetadataProjection>,
    ) -> Result<(), String> {
        self.history_caches
            .head_canonical_metadata
            .acknowledge(projection, &self.metadata)
    }

    pub(crate) fn validate_head_canonical_metadata_acknowledgement(
        &self,
        projection: &Arc<SessionHeadMetadataProjection>,
    ) -> Result<(), String> {
        self.history_caches
            .head_canonical_metadata
            .validate_acknowledgement(projection, &self.metadata)
    }

    fn mark_head_canonical_metadata_key_mutated(&mut self, key: &str) {
        self.history_caches
            .head_canonical_metadata
            .mark_key_mutated(key);
    }

    fn adopt_head_canonical_metadata_baseline_from(&mut self, source: &Session) {
        self.history_caches.head_canonical_metadata =
            source.history_caches.head_canonical_metadata.clone();
    }

    fn set_metadata_unchecked(&mut self, key: &str, value: serde_json::Value) {
        // Reapplying an identical durable projection is not a session-content
        // mutation. In particular, cold materialization restores
        // SessionMetadata and SessionBuildState before it knows whether the
        // values changed; advancing `updated_at` for an exact no-op would
        // manufacture a content change even though the committed document is
        // unchanged.
        if self.metadata.get(key) == Some(&value) {
            return;
        }
        self.mark_head_canonical_metadata_key_mutated(key);
        self.metadata.insert(key.to_string(), value);
        if key == SESSION_TRANSCRIPT_HISTORY_STATE_KEY {
            self.metadata
                .remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
            self.history_caches.shared_state.clear();
            self.transcript_history_metadata_validation =
                TranscriptHistoryMetadataValidation::RequiresValidation;
        }
        self.mark_content_mutated(SystemTime::now());
    }

    /// Install a graph a typed path already validated as the singular
    /// in-memory authority.
    ///
    /// The serialized graph and rewrite-prefix projection are absent from
    /// ordinary metadata. WholeBlob encoding synthesizes them at the explicit
    /// wire boundary; HeadCanonical consumes this shared typed state directly.
    fn install_validated_transcript_history_state(
        &mut self,
        state: TranscriptHistoryState,
    ) -> Result<(), serde_json::Error> {
        let state = std::sync::Arc::new(state);
        let unchanged = self
            .history_caches
            .shared_state
            .get()
            .is_some_and(|current| {
                current.graph_prefix() == state.graph_prefix()
                    && current.rewrite_prefix() == state.rewrite_prefix()
                    && current.head() == state.head()
            });
        self.metadata.remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
        self.metadata
            .remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
        if unchanged {
            self.history_caches.shared_state.set(state);
            self.transcript_history_metadata_validation =
                TranscriptHistoryMetadataValidation::Validated;
            return Ok(());
        }
        self.history_caches.shared_state.set(state);
        self.transcript_history_metadata_validation =
            TranscriptHistoryMetadataValidation::Validated;
        self.mark_content_mutated(SystemTime::now());
        Ok(())
    }

    /// Small rewrite-prefix fact for receipt comparison.
    #[must_use]
    pub fn transcript_rewrite_prefix_authority(
        &self,
    ) -> Option<TranscriptRewritePrefixAccumulator> {
        if let Some(state) = self.history_caches.shared_state.get() {
            return Some(state.rewrite_prefix().clone());
        }
        serde_json::from_value(
            self.metadata
                .get(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY)?
                .clone(),
        )
        .ok()
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
        let removed = self.metadata.remove(key).is_some();
        let mut changed = removed;
        if key == SESSION_TRANSCRIPT_HISTORY_STATE_KEY {
            changed |= self.history_caches.shared_state.get().is_some();
            changed |= self
                .metadata
                .remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY)
                .is_some();
            self.history_caches.shared_state.clear();
            self.transcript_history_metadata_validation =
                TranscriptHistoryMetadataValidation::Validated;
        }
        if changed {
            self.mark_head_canonical_metadata_key_mutated(key);
            self.mark_content_mutated(SystemTime::now());
        }
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
            self.mark_head_canonical_metadata_key_mutated(key);
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
        if self.metadata.remove(key).is_some() {
            self.mark_head_canonical_metadata_key_mutated(key);
            self.mark_content_mutated(SystemTime::now());
        }
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
        try_session_metadata_from_map(&self.metadata)
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

    /// Stage an external-callback batch without publishing any
    /// provider-visible tool results or sibling transcript effects.
    pub(crate) fn stage_pending_callback_tool_batch(
        &mut self,
        batch: PendingCallbackToolBatch,
    ) -> Result<(), PendingCallbackBatchError> {
        if matches!(
            self.callback_tool_batch_state()?,
            Some(CallbackToolBatchState::Pending { .. })
        ) {
            return Err(PendingCallbackBatchError::AlreadyStaged);
        }
        validate_pending_callback_batch(self.messages(), &batch)?;
        let value = serde_json::to_value(CallbackToolBatchState::Pending { batch })
            .map_err(|error| PendingCallbackBatchError::Malformed(error.to_string()))?;
        self.set_metadata_unchecked(SESSION_PENDING_CALLBACK_BATCH_KEY, value);
        Ok(())
    }

    fn callback_tool_batch_state(
        &self,
    ) -> Result<Option<CallbackToolBatchState>, PendingCallbackBatchError> {
        self.metadata
            .get(SESSION_PENDING_CALLBACK_BATCH_KEY)
            .map(|value| {
                serde_json::from_value(value.clone())
                    .map_err(|error| PendingCallbackBatchError::Malformed(error.to_string()))
            })
            .transpose()
    }

    /// Restore the typed callback batch. A corrupt durable record is a typed
    /// refusal, never "no pending callback".
    pub(crate) fn pending_callback_tool_batch(
        &self,
    ) -> Result<Option<PendingCallbackToolBatch>, PendingCallbackBatchError> {
        match self.callback_tool_batch_state()? {
            Some(CallbackToolBatchState::Pending { batch }) => {
                validate_pending_callback_batch(self.messages(), &batch)?;
                Ok(Some(batch))
            }
            Some(CallbackToolBatchState::Applied { .. }) | None => Ok(None),
        }
    }

    /// Validate external callback results and combine them with staged sibling
    /// results in the original assistant tool-use order, without mutation.
    pub(crate) fn resolve_pending_callback_tool_results(
        &self,
        incoming: Vec<ToolResult>,
    ) -> Result<ResolvedPendingCallbackToolResults, PendingCallbackBatchError> {
        let Some(state) = self.callback_tool_batch_state()? else {
            return Ok(ResolvedPendingCallbackToolResults::NoState);
        };
        let batch = match state {
            CallbackToolBatchState::Pending { batch } => batch,
            CallbackToolBatchState::Applied {
                tool_use_order,
                results,
                async_ops,
                ..
            } => {
                let incoming_by_id = unique_tool_results(incoming)?;
                let expected = tool_use_order.iter().cloned().collect::<BTreeSet<_>>();
                let actual = incoming_by_id.keys().cloned().collect::<BTreeSet<_>>();
                if actual != expected {
                    return Err(PendingCallbackBatchError::ResultSetMismatch { expected, actual });
                }
                let delivered = tool_use_order
                    .iter()
                    .map(|id| incoming_by_id.get(id).cloned())
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| {
                        PendingCallbackBatchError::Malformed(
                            "applied callback receipt is missing an ordered result".to_string(),
                        )
                    })?;
                return if delivered == results {
                    Ok(ResolvedPendingCallbackToolResults::AlreadyApplied { async_ops })
                } else {
                    Err(PendingCallbackBatchError::ConflictingRedelivery)
                };
            }
        };
        validate_pending_callback_batch(self.messages(), &batch)?;
        let incoming_by_id = unique_tool_results(incoming)?;
        let expected = batch
            .pending_tool_use_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let actual = incoming_by_id.keys().cloned().collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(PendingCallbackBatchError::ResultSetMismatch { expected, actual });
        }
        let mut all_by_id = unique_tool_results(batch.completed_results.clone())?;
        all_by_id.extend(incoming_by_id);
        let ordered = batch
            .tool_use_order
            .iter()
            .map(|id| {
                all_by_id.remove(id).ok_or_else(|| {
                    PendingCallbackBatchError::Malformed(format!(
                        "no result is available for assistant tool id '{id}'"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !all_by_id.is_empty() {
            return Err(PendingCallbackBatchError::Malformed(format!(
                "results contain ids absent from assistant tool-use order: {:?}",
                all_by_id.keys().collect::<Vec<_>>()
            )));
        }
        Ok(ResolvedPendingCallbackToolResults::Pending {
            batch,
            ordered_results: ordered,
        })
    }

    /// Publish the already-resolved full `ToolResults` set and any
    /// transcript-producing sibling effects as one adjacent message batch,
    /// then replace the durable staging record with an idempotency receipt.
    pub(crate) fn commit_pending_callback_tool_results(
        &mut self,
        batch: &PendingCallbackToolBatch,
        ordered_results: Vec<ToolResult>,
        post_tool_messages: Vec<Message>,
    ) -> Result<(), PendingCallbackBatchError> {
        let current = self
            .pending_callback_tool_batch()?
            .ok_or(PendingCallbackBatchError::Missing)?;
        if &current != batch {
            return Err(PendingCallbackBatchError::Malformed(
                "pending callback batch changed between prepare and commit".to_string(),
            ));
        }
        let actual_order = ordered_results
            .iter()
            .map(|result| result.tool_use_id.clone())
            .collect::<Vec<_>>();
        if actual_order != batch.tool_use_order {
            return Err(PendingCallbackBatchError::Malformed(format!(
                "resolved result order {actual_order:?} does not match assistant order {:?}",
                batch.tool_use_order
            )));
        }
        self.push(Message::tool_results(ordered_results.clone()));
        let pending_ids = batch
            .pending_tool_use_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let applied_callback_results = ordered_results
            .into_iter()
            .filter(|result| pending_ids.contains(&result.tool_use_id))
            .collect();
        let value = serde_json::to_value(CallbackToolBatchState::Applied {
            tool_use_order: batch.pending_tool_use_ids.clone(),
            results: applied_callback_results,
            async_ops: batch.async_ops.clone(),
            post_tool_messages,
            post_tool_messages_applied: false,
        })
        .map_err(|error| PendingCallbackBatchError::Malformed(error.to_string()))?;
        self.set_metadata_unchecked(SESSION_PENDING_CALLBACK_BATCH_KEY, value);
        Ok(())
    }

    /// Apply callback-staged post-tool transcript effects only after the
    /// ToolResults tail has been admitted as a pending continuation. This
    /// preserves provider adjacency and prevents the effects from hiding the
    /// continuation boundary from session admission.
    pub(crate) fn apply_pending_callback_resume_effects(
        &mut self,
    ) -> Result<Vec<crate::event::AssistantImageEvent>, PendingCallbackBatchError> {
        let Some(CallbackToolBatchState::Applied {
            tool_use_order,
            results,
            async_ops,
            post_tool_messages,
            post_tool_messages_applied,
        }) = self.callback_tool_batch_state()?
        else {
            return Ok(Vec::new());
        };
        if post_tool_messages_applied {
            return Ok(Vec::new());
        }
        let image_events = post_tool_messages
            .iter()
            .filter_map(|message| match message {
                Message::BlockAssistant(assistant) => Some(assistant.blocks.as_slice()),
                _ => None,
            })
            .flatten()
            .filter_map(crate::event::AssistantImageEvent::from_assistant_block)
            .collect::<Vec<_>>();
        let applied_state = CallbackToolBatchState::Applied {
            tool_use_order,
            results,
            async_ops,
            post_tool_messages: post_tool_messages.clone(),
            post_tool_messages_applied: true,
        };
        let value = serde_json::to_value(applied_state)
            .map_err(|error| PendingCallbackBatchError::Malformed(error.to_string()))?;
        self.push_batch(post_tool_messages);
        self.set_metadata_unchecked(SESSION_PENDING_CALLBACK_BATCH_KEY, value);
        Ok(image_events)
    }

    /// Realize the typed session lifecycle-terminal projection in the session
    /// metadata map.
    ///
    /// The lifecycle-terminal fact is owned by the canonical
    /// [`session_document::SessionDocumentMachine`]; production archive paths
    /// call this only to realize a machine-emitted `SessionArchiveResolved`
    /// verdict (the value written mirrors the machine's decision — the shell
    /// decides nothing here).
    pub fn set_lifecycle_terminal(
        &mut self,
        terminal: SessionLifecycleTerminal,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(terminal)?;
        self.set_metadata_unchecked(SESSION_LIFECYCLE_TERMINAL_KEY, value);
        Ok(())
    }

    /// Try to load the typed session lifecycle-terminal fact.
    ///
    /// Reads the typed [`SESSION_LIFECYCLE_TERMINAL_KEY`]; an absent key means
    /// no terminal fact.
    pub fn try_lifecycle_terminal(
        &self,
    ) -> Result<Option<SessionLifecycleTerminal>, serde_json::Error> {
        try_lifecycle_terminal_from_map(&self.metadata)
    }

    /// Load the typed session lifecycle-terminal fact, failing closed on a
    /// corrupt typed value.
    ///
    /// Callers that need the typed rejection must use
    /// [`Self::try_lifecycle_terminal`].
    pub fn lifecycle_terminal(&self) -> Option<SessionLifecycleTerminal> {
        match self.try_lifecycle_terminal() {
            Ok(state) => state,
            Err(err) => fail_closed_generated_restore("session-lifecycle-terminal", err),
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
        if let Some(state) = self.history_caches.shared_state.get() {
            return Ok(Some(state.as_ref().clone()));
        }
        self.metadata
            .get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
    }

    /// [`Self::transcript_history_state`] served from the per-instance
    /// shared cache: one parse per graph value, shared by `Arc` thereafter.
    /// Every write to the history key clears the cache.
    ///
    /// Public for graph-walk consumers (the session-service rewrite-chain
    /// persistence loop materializes 1-2 projections PER COMMIT; a fresh
    /// owned parse per call re-materializes every retained body each time —
    /// the 2026-07-29 per-turn latency incident's dominant clone).
    pub fn transcript_history_state_shared(
        &self,
    ) -> Result<Option<std::sync::Arc<TranscriptHistoryState>>, serde_json::Error> {
        if let Some(state) = self.history_caches.shared_state.get() {
            return Ok(Some(state));
        }
        let Some(state) = self.transcript_history_state()? else {
            return Ok(None);
        };
        let state = std::sync::Arc::new(state);
        self.history_caches
            .shared_state
            .set(std::sync::Arc::clone(&state));
        Ok(Some(state))
    }

    /// This session's transcript graph together with the proof that it
    /// validates.
    ///
    /// Prefer this over pairing [`Self::validate_transcript_history_state`]
    /// with a separate parse. That pairing establishes the fact and then drops
    /// it on the floor: the parsed value carries no evidence, so every guard it
    /// is handed to re-derives the same whole-graph proof at O(document) cost.
    /// A session whose in-memory marker already records the validation returns
    /// the sealed graph without re-verifying; anything else pays exactly one
    /// full verification here, and no consumer pays again.
    pub fn validated_transcript_history_state(
        &self,
    ) -> Result<Option<ValidatedTranscriptHistory>, TranscriptEditError> {
        let Some(state) = self
            .transcript_history_state_shared()
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?
        else {
            return Ok(None);
        };
        if self.transcript_history_metadata_validation
            == TranscriptHistoryMetadataValidation::Validated
        {
            return Ok(Some(ValidatedTranscriptHistory::adopt_session_validated(
                state,
            )));
        }
        Err(TranscriptEditError::HistoryStateMalformed(
            "transcript-history graph has structural bytes but no verified materialization or construction authority"
                .to_string(),
        ))
    }

    /// This session's transcript graph, but ONLY when the session's own marker
    /// already proves it.
    ///
    /// [`Self::validated_transcript_history_state`] SEALS an unmarked graph,
    /// which costs a whole-graph hash. A caller that wants the proof as an
    /// OPTIMIZATION — evidence that lets it skip work it would otherwise do —
    /// has already spent the saving by the time that hash finishes, so this
    /// reports absence rather than paying for evidence. Absence here is never
    /// a verdict about the graph; it only means no proof is on hand for free.
    pub fn already_validated_transcript_history_state(
        &self,
    ) -> Result<Option<ValidatedTranscriptHistory>, serde_json::Error> {
        if self.transcript_history_metadata_validation
            != TranscriptHistoryMetadataValidation::Validated
        {
            return Ok(None);
        }
        Ok(self
            .transcript_history_state_shared()?
            .map(ValidatedTranscriptHistory::adopt_session_validated))
    }

    /// Prove that `state.head` is either the exact live transcript revision or
    /// a content-addressed prefix ancestor of the live transcript.
    ///
    /// `state` must already have crossed graph validation: that proof binds the
    /// retained head body's messages to `state.head`. The remaining relation is
    /// therefore one prefix digest over the live buffer. Warm append paths serve
    /// it from the retained boundary witness; cold/replay callers may pay one
    /// fail-closed prefix derivation.
    pub(crate) fn live_transcript_extends_history_head(
        &self,
        state: &TranscriptHistoryState,
        _live_revision: &str,
    ) -> Result<bool, TranscriptEditError> {
        let current_count = u64::try_from(self.messages.len()).map_err(|_| {
            TranscriptEditError::HistoryStateMalformed(
                "live transcript row count exceeds u64".to_string(),
            )
        })?;
        let endpoint = state.final_endpoint_witness().ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "compact transcript graph has no final endpoint witness".to_string(),
            )
        })?;
        Ok(self.exact_message_row_lineage_extends(endpoint.row_prefix(), current_count))
    }

    /// Load exact compaction projection intents carried to the runtime's
    /// atomic-apply outbox by this session snapshot.
    pub fn compaction_projection_intents(
        &self,
    ) -> Result<Vec<crate::memory::CompactionProjectionIntent>, serde_json::Error> {
        self.metadata
            .get(crate::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// Load persisted compaction intents only after proving that every
    /// already-carried projection ID is backed by this session's validated
    /// transcript graph.
    ///
    /// This is deliberately a validation boundary, not an ID constructor:
    /// durable typed rewrite tags and legacy records can confirm an existing
    /// identity during recovery but cannot mint a new identity.
    pub fn validated_compaction_projection_intents(
        &self,
    ) -> Result<Vec<crate::memory::CompactionProjectionIntent>, serde_json::Error> {
        let intents = self.compaction_projection_intents()?;
        if intents.is_empty() {
            return Ok(intents);
        }
        let history = self
            .validated_transcript_history_state()
            .map_err(|error| <serde_json::Error as serde::ser::Error>::custom(error.to_string()))?;
        let mut unique = std::collections::HashSet::new();
        for intent in &intents {
            if intent.projection.session_id() != self.id() {
                return Err(<serde_json::Error as serde::ser::Error>::custom(
                    "compaction projection outbox intent has a foreign session id",
                ));
            }
            if !unique.insert(intent.projection.clone()) {
                return Err(<serde_json::Error as serde::ser::Error>::custom(
                    "compaction projection outbox contains a duplicate rewrite identity",
                ));
            }
            let backed = history.as_ref().is_some_and(|history| {
                history.commits().any(|commit| {
                    intent
                        .projection
                        .matches_transcript_rewrite(self.id(), commit)
                })
            });
            if !backed {
                return Err(<serde_json::Error as serde::ser::Error>::custom(format!(
                    "compaction projection outbox intent {} has no matching TranscriptRewriteCommit",
                    intent.projection.revision()
                )));
            }
        }
        Ok(intents)
    }

    /// Record one invisible staged-memory intent only after its exact
    /// TranscriptRewriteCommit is present in the session graph.
    pub fn add_compaction_projection_intent(
        &mut self,
        intent: crate::memory::CompactionProjectionIntent,
    ) -> Result<(), serde_json::Error> {
        if intent.projection.session_id() != self.id() {
            return Err(<serde_json::Error as serde::ser::Error>::custom(
                "compaction projection intent session does not match snapshot session",
            ));
        }
        let history = self
            .validated_transcript_history_state()
            .map_err(|error| <serde_json::Error as serde::ser::Error>::custom(error.to_string()))?
            .ok_or_else(|| {
                <serde_json::Error as serde::ser::Error>::custom(
                    "compaction projection intent requires transcript history state",
                )
            })?;
        let owns_commit = history.commits().any(|commit| {
            commit.parent_revision == intent.projection.parent_revision()
                && commit.revision == intent.projection.revision()
                && intent
                    .projection
                    .matches_transcript_rewrite(self.id(), commit)
        });
        if !owns_commit {
            return Err(<serde_json::Error as serde::ser::Error>::custom(
                "compaction projection intent is not backed by the session transcript graph",
            ));
        }
        let mut intents = self.validated_compaction_projection_intents()?;
        if let Some(existing) = intents
            .iter()
            .find(|existing| existing.projection == intent.projection)
        {
            if existing == &intent {
                return Ok(());
            }
            return Err(<serde_json::Error as serde::ser::Error>::custom(
                "compaction projection intent conflicts with an existing rewrite identity",
            ));
        }
        intents.push(intent);
        self.set_metadata_unchecked(
            crate::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY,
            serde_json::to_value(intents)?,
        );
        Ok(())
    }

    /// Remove an intent after the runtime outbox has finalized its staged
    /// memory batch. Idempotent for repeated recovery finalization.
    pub fn complete_compaction_projection_intent(
        &mut self,
        projection: &crate::memory::CompactionProjectionId,
    ) -> Result<Option<crate::memory::CompactionProjectionIntent>, serde_json::Error> {
        let mut intents = self.compaction_projection_intents()?;
        let Some(position) = intents
            .iter()
            .position(|intent| &intent.projection == projection)
        else {
            return Ok(None);
        };
        let completed = intents.remove(position);
        if intents.is_empty() {
            self.remove_metadata_unchecked(
                crate::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY,
            );
        } else {
            self.set_metadata_unchecked(
                crate::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY,
                serde_json::to_value(intents)?,
            );
        }
        Ok(Some(completed))
    }

    /// Validate the retained transcript revision graph, when present.
    pub fn validate_transcript_history_state(&self) -> Result<(), TranscriptEditError> {
        if self.transcript_history_metadata_validation
            == TranscriptHistoryMetadataValidation::Validated
        {
            return Ok(());
        }
        if self.history_caches.shared_state.get().is_some()
            || self
                .metadata
                .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
        {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "transcript-history graph has not crossed verified materialization or construction authority"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Clear retained transcript revision metadata after a caller has
    /// materialized the desired message projection.
    pub fn clear_transcript_history_state(&mut self) {
        self.remove_metadata_unchecked(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    }

    /// Adopt a durable head's non-transcript persisted state onto a recovery
    /// document whose transcript was rebuilt through the typed mutation seam.
    ///
    /// This is a mechanical document seam, not target-store write authority. A
    /// persistence implementation must still atomically validate its own
    /// observation and fencing preconditions before committing the resulting
    /// bytes.
    ///
    /// Exhaustive over the persisted envelope (`SessionSerde` and its
    /// borrowed encode view `SessionSerdeRef`):
    /// - `version`, `id`, `created_at` — identity fields the digest-verified
    ///   prefix relation already proved equal; untouched.
    /// - `messages` (and the inline transcript-history graph under
    ///   [`SESSION_TRANSCRIPT_HISTORY_STATE_KEY`]) — rebuilt by recovery
    ///   through the mutation seam; owned by the target.
    /// - the lifecycle terminal — merged through generated
    ///   `SessionDocumentMachine` authority because Archived is absorbing; it
    ///   never rides the generic metadata overwrite.
    /// - `updated_at`, `usage`, and EVERY other metadata key (compaction
    ///   projection intents, visibility state, deferred context, ...) — the
    ///   head's values are the newer durable truth and are adopted verbatim,
    ///   including deletions.
    ///
    /// Anyone adding a persisted field must classify it here: the head is read
    /// through an exhaustive `SessionSerdeRef` destructure with no `..` rest
    /// pattern, so an unclassified addition is a compile error at this
    /// adoption site rather than a field that silently reverts to the stale
    /// snapshot value on every recovery.
    pub fn adopt_recovered_head_state(&mut self, head: &Session) -> Result<(), String> {
        const RECOVERY_OWNED_KEYS: [&str; 3] = [
            SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY,
            SESSION_LIFECYCLE_TERMINAL_KEY,
        ];
        let recovered_archived = self
            .try_lifecycle_terminal()
            .map_err(|error| format!("recovered lifecycle-terminal is malformed: {error}"))?
            == Some(SessionLifecycleTerminal::Archived);
        let head_terminal = head
            .try_lifecycle_terminal()
            .map_err(|error| format!("durable-head lifecycle-terminal is malformed: {error}"))?;
        let head_archived = head_terminal == Some(SessionLifecycleTerminal::Archived);
        let mut lifecycle_authority = session_document::SessionDocumentMachineAuthority::new();
        let lifecycle_merge = lifecycle_authority
            .resolve_session_document_lifecycle_merge(
                session_document::SessionDocumentKey::new(self.id.to_string()),
                recovered_archived,
                head_archived,
            )
            .map_err(|error| {
                format!("session document authority rejected recovered lifecycle merge: {error}")
            })?
            .into_iter()
            .find_map(|effect| {
                match effect {
                session_document::SessionDocumentEffect::SessionDocumentLifecycleMergeResolved {
                    merge,
                } => Some(merge),
                _ => None,
            }
            })
            .ok_or_else(|| {
                "session document authority emitted no recovered lifecycle merge".to_string()
            })?;
        // The bindings below ARE the classification: identity-invariant and
        // recovery-owned fields are bound to `_`-prefixed names precisely
        // because reading them from the head would be wrong.
        let SessionSerdeRef {
            version: _identity_version,
            id: _identity_id,
            messages: _recovery_owned_messages,
            created_at: _identity_created_at,
            updated_at: head_updated_at,
            metadata: head_metadata,
            usage: head_usage,
        } = persisted_envelope_ref(head, None);
        self.usage = head_usage.clone();
        // `head` was materialized from the exact authenticated metadata state
        // that owns these general values. Adopt that baseline together with
        // the values instead of diffing or re-hashing the complete map.
        self.adopt_head_canonical_metadata_baseline_from(head);
        self.metadata.retain(|key, _| {
            RECOVERY_OWNED_KEYS.contains(&key.as_str()) || head_metadata.contains_key(key)
        });
        for (key, value) in head_metadata {
            if RECOVERY_OWNED_KEYS.contains(&key.as_str()) {
                continue;
            }
            self.metadata.insert(key.clone(), value.clone());
        }
        match lifecycle_merge {
            session_document::SessionDocumentLifecycleMerge::CarryArchived => self
                .set_lifecycle_terminal(SessionLifecycleTerminal::Archived)
                .map_err(|error| {
                    format!("failed to realize absorbing Archived terminal: {error}")
                })?,
            session_document::SessionDocumentLifecycleMerge::CarryAuthority => {
                match head_terminal {
                    Some(terminal) => self.set_lifecycle_terminal(terminal).map_err(|error| {
                        format!("failed to realize durable-head lifecycle terminal: {error}")
                    })?,
                    None => {
                        self.remove_metadata_unchecked(SESSION_LIFECYCLE_TERMINAL_KEY);
                    }
                }
            }
        }
        self.mark_content_mutated(*head_updated_at);
        Ok(())
    }

    /// Return the retained immutable body for a transcript revision.
    pub fn transcript_revision_body(
        &self,
        revision: &str,
    ) -> Result<Option<TranscriptRevisionBody>, serde_json::Error> {
        let Some(history) = self
            .validated_transcript_history_state()
            .map_err(|error| <serde_json::Error as serde::ser::Error>::custom(error.to_string()))?
        else {
            return Ok(None);
        };
        if !history.state().contains_revision(revision) {
            return Ok(None);
        }
        history
            .materialize_revision(revision)
            .map(Some)
            .map_err(|error| <serde_json::Error as serde::ser::Error>::custom(error.to_string()))
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
        mut state: TranscriptHistoryState,
    ) -> Result<(), TranscriptEditError> {
        state.compact_mechanical_revision_bodies()?;
        self.apply_proved_transcript_history_state(state)
    }

    /// Materialize this session from a proof-bearing transcript-history
    /// projection without re-validating every retained body.
    ///
    /// The capability can only be minted by a full validator or by
    /// proof-preserving graph transformations such as
    /// [`ValidatedTranscriptHistory::project_at_revision`]. This keeps the
    /// write seam fail-closed while letting a rewrite-chain persistence walk
    /// project each already-proved prefix without turning `N` commits into
    /// `N` full-graph verification passes.
    pub fn apply_validated_transcript_history_state(
        &mut self,
        validated: ValidatedTranscriptHistory,
    ) -> Result<(), TranscriptEditError> {
        let mut state = validated.into_state();
        // The graph is already proved, so pruning is the construction-safe
        // half only. Calling `compact_mechanical_revision_bodies()` here
        // would discard the capability and re-run FullVerify.
        state.prune_mechanical_revision_bodies();
        self.apply_proved_transcript_history_state(state)
    }

    /// Install a proof-bearing AUDITED graph while preserving an extending
    /// live transcript.
    ///
    /// Replay consumers reconstruct rewrite history independently from the
    /// current strand tail. Replacing `messages` with the audited endpoint
    /// would discard that tail; manufacturing a mechanical graph head would
    /// copy it into retained history. This seam does neither. It canonicalizes
    /// the proved graph to its latest audited endpoint, proves that endpoint is
    /// the exact live revision or a content-addressed live prefix, and installs
    /// only the graph metadata.
    pub fn install_validated_audited_transcript_history_preserving_live(
        &mut self,
        validated: ValidatedTranscriptHistory,
    ) -> Result<(), TranscriptEditError> {
        let mut state = validated.into_state();
        state.canonicalize_to_latest_audited_head();
        let live_revision = self
            .transcript_content_digest()
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        if !self.live_transcript_extends_history_head(&state, &live_revision)? {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "audited transcript head {} is not a prefix ancestor of live revision {live_revision}",
                state.head()
            )));
        }
        self.install_validated_transcript_history_state(state)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))
    }

    /// Clone the persisted envelope around a proof-bearing transcript
    /// projection without cloning the source graph's metadata value.
    ///
    /// `Session::clone()` is cheap for live messages but deep-clones every
    /// metadata value. Once transcript history is present, that includes the
    /// complete retained graph, so a rewrite-chain walk that immediately
    /// replaces the graph still copied all retained bodies once per commit.
    /// This constructor copies every other persisted field exactly, omits only
    /// the graph the sealed projection replaces, and then installs that
    /// projection through the proof-preserving apply seam.
    pub fn with_validated_transcript_history_projection(
        &self,
        validated: ValidatedTranscriptHistory,
    ) -> Result<Self, TranscriptEditError> {
        let metadata = self
            .metadata
            .iter()
            .filter(|(key, _)| key.as_str() != SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut projected = Self {
            version: self.version,
            id: self.id.clone(),
            messages: self.messages.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            metadata,
            realtime_transcript: self.realtime_transcript.clone(),
            history_caches: Box::default(),
            transcript_history_metadata_validation: TranscriptHistoryMetadataValidation::Validated,
            usage: self.usage.clone(),
        };
        projected.apply_validated_transcript_history_state(validated)?;
        Ok(projected)
    }

    fn apply_proved_transcript_history_state(
        &mut self,
        state: TranscriptHistoryState,
    ) -> Result<(), TranscriptEditError> {
        let head_body = state.materialize_revision(state.head())?;
        let realtime_rebase = self.prepare_realtime_transcript_rebase_after_rewrite(
            &head_body.messages,
            RealtimeTranscriptSnapshotReasonV1::RecoveryRebase,
        )?;
        let mut updated_at = head_body.created_at;
        for commit in state.commits() {
            if commit.committed_at > updated_at {
                updated_at = commit.committed_at;
            }
        }
        self.install_validated_transcript_history_state(state)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        self.realtime_transcript
            .apply_prepared_rebase(realtime_rebase);
        // SEAM 7 (non-append): the projection adopts a graph head body.
        self.messages.replace(head_body.messages);
        self.mark_content_mutated(updated_at);
        Ok(())
    }

    /// Current LIVE transcript revision.
    ///
    /// The retained graph's `head` is the latest audited rewrite endpoint and
    /// may be a prefix ancestor after ordinary appends. Live identity always
    /// comes from the message buffer and its incremental digest accumulator.
    pub fn transcript_revision(&self) -> Result<String, serde_json::Error> {
        self.transcript_content_digest()
    }

    /// Monotonic durable generation for same-session transcript rewrites.
    /// Ordinary message appends advance the content revision but do not change
    /// this value, allowing live config refresh after normal turns while still
    /// forcing reopen after a rewrite.
    pub fn transcript_rewrite_generation(&self) -> Result<u64, serde_json::Error> {
        Ok(self
            .transcript_history_state_shared()?
            .and_then(|state| state.last_commit().map(|commit| commit.rewrite_generation))
            .unwrap_or(0))
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
        let selection = selection.into_current_edit_semantic();
        if selection.semantic() == TranscriptRewriteSemantic::Compaction {
            return Err(TranscriptEditError::InvalidTranscriptShape(
                "typed compaction rewrites require a core-validated compaction witness".to_string(),
            ));
        }
        self.commit_transcript_rewrite_authorized(
            selection,
            replacement,
            reason,
            actor,
            expected_parent_revision,
        )
    }

    fn commit_transcript_rewrite_authorized(
        &mut self,
        selection: TranscriptRewriteSelection,
        replacement: Vec<Message>,
        reason: TranscriptRewriteReason,
        actor: Option<String>,
        expected_parent_revision: Option<String>,
    ) -> Result<TranscriptRewriteCommit, TranscriptEditError> {
        self.commit_transcript_rewrite_bound(
            selection,
            replacement,
            reason,
            actor,
            expected_parent_revision,
            None,
        )
    }

    /// [`Self::commit_transcript_rewrite_authorized`] with an additional
    /// expected digest for the FULL rewritten transcript. The compaction
    /// authority passes its minted revision here, so the rebuilt side of the
    /// token binds against the one digest this commit computes anyway
    /// instead of a second whole-document hash at the authorization seam. A
    /// mismatch fails closed before any state is touched.
    fn commit_transcript_rewrite_bound(
        &mut self,
        selection: TranscriptRewriteSelection,
        replacement: Vec<Message>,
        reason: TranscriptRewriteReason,
        actor: Option<String>,
        expected_parent_revision: Option<String>,
        expected_revision: Option<&str>,
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
        rewritten.extend(replacement.iter().cloned());
        rewritten.extend_from_slice(&self.messages[end..]);
        validate_transcript_tool_result_shape(&rewritten)?;
        // One required hash of the genuinely new content, computed FIRST so
        // the whole-span digests below reuse it instead of re-hashing the
        // same bytes. The reuse conditions are slice-identity arithmetic,
        // never rewrite semantics: a partial-span edit keeps paying O(span).
        let revision = transcript_messages_digest(&rewritten)
            .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?;
        if let Some(expected) = expected_revision
            && expected != revision
        {
            return Err(TranscriptEditError::InvalidTranscriptShape(
                "validated compaction witness does not authorize this exact transcript rebuild"
                    .to_string(),
            ));
        }
        if revision == parent_revision {
            return Err(TranscriptEditError::NoOpRewrite { revision });
        }
        let original_span_digest = if start == 0 && end == message_count {
            // The span IS the whole live transcript; the accumulator serves
            // its digest in O(delta), byte-identical to the free function.
            self.transcript_content_digest()
                .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?
        } else {
            transcript_messages_digest(&self.messages[start..end])
                .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?
        };
        let replacement_digest = if start == 0 && start + replacement_len == rewritten.len() {
            // The replacement span IS the whole rewritten transcript.
            revision.clone()
        } else {
            transcript_messages_digest(&rewritten[start..start + replacement_len])
                .map_err(|err| TranscriptEditError::HistoryStateMalformed(err.to_string()))?
        };
        let realtime_rebase = self.prepare_realtime_transcript_rebase_after_rewrite(
            &rewritten,
            RealtimeTranscriptSnapshotReasonV1::TranscriptRewrite,
        )?;
        let prior_history = self.validated_transcript_history_state()?;
        let rewrite_generation = prior_history
            .as_ref()
            .and_then(|history| history.last_commit())
            .map_or(Some(1), |commit| commit.rewrite_generation.checked_add(1))
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "transcript rewrite generation exhausted u64".to_string(),
                )
            })?;
        let committed_at = SystemTime::now();
        let commit = TranscriptRewriteCommit {
            rewrite_generation,
            parent_revision,
            revision,
            selection,
            original_span_digest,
            replacement_digest,
            messages_before: message_count,
            messages_after: rewritten.len(),
            reason,
            actor,
            committed_at,
        };
        self.finish_compact_transcript_rewrite(
            prior_history,
            commit,
            replacement,
            rewritten,
            realtime_rebase,
        )
    }

    fn finish_compact_transcript_rewrite(
        &mut self,
        prior_history: Option<ValidatedTranscriptHistory>,
        commit: TranscriptRewriteCommit,
        replacement: Vec<Message>,
        rewritten: Vec<Message>,
        realtime_rebase: PreparedRealtimeTranscriptRebase,
    ) -> Result<TranscriptRewriteCommit, TranscriptEditError> {
        let parent_row_prefix = self
            .exact_message_row_prefix_at(u64::try_from(self.messages.len()).map_err(|_| {
                TranscriptEditError::HistoryStateMalformed(
                    "live transcript row count exceeds u64".to_string(),
                )
            })?)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "live transcript has no exact row-lineage authority".to_string(),
                )
            })?;
        let (start, end) = commit.selection.bounds();
        let serialized_replacement = replacement
            .iter()
            .map(serde_json::to_vec)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let start = u64::try_from(start).map_err(|_| {
            TranscriptEditError::HistoryStateMalformed(
                "rewrite start exceeds durable row coordinates".to_string(),
            )
        })?;
        let end = u64::try_from(end).map_err(|_| {
            TranscriptEditError::HistoryStateMalformed(
                "rewrite end exceeds durable row coordinates".to_string(),
            )
        })?;
        let result_row_prefix = parent_row_prefix
            .replace_serialized_range(start, end, &serialized_replacement)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let result_witness = TranscriptEndpointWitness::from_messages_with_row_prefix(
            &rewritten,
            result_row_prefix.clone(),
        )
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;

        let state = match prior_history {
            None => {
                let parent = TranscriptRevisionBody {
                    revision: commit.parent_revision.clone(),
                    parent_revision: None,
                    messages: self.messages.to_vec(),
                    created_at: self.updated_at,
                };
                TranscriptHistoryState::from_authorized_first_rewrite(
                    parent,
                    parent_row_prefix,
                    &commit.revision,
                    &rewritten,
                    commit.committed_at,
                    result_row_prefix.clone(),
                    replacement,
                    commit.clone(),
                )?
            }
            Some(history) => {
                let mut state = history.state().clone();
                let endpoint = state.final_endpoint_witness().ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "compact transcript graph has no final endpoint witness".to_string(),
                    )
                })?;
                if self.messages.len() < endpoint.message_count() {
                    return Err(TranscriptEditError::HistoryStateMalformed(
                        "live rewrite parent is shorter than the audited endpoint".to_string(),
                    ));
                }
                let appended = self.messages[endpoint.message_count()..].to_vec();
                let serialized_appended = appended
                    .iter()
                    .map(serde_json::to_vec)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| {
                        TranscriptEditError::HistoryStateMalformed(error.to_string())
                    })?;
                let exact_append_prefix = endpoint
                    .row_prefix()
                    .extend_serialized_rows(&serialized_appended)
                    .map_err(|error| {
                        TranscriptEditError::HistoryStateMalformed(error.to_string())
                    })?;
                let parent_advance = if exact_append_prefix == parent_row_prefix {
                    TranscriptParentAdvance::ExactAppend { appended }
                } else {
                    return Err(TranscriptEditError::HistoryStateMalformed(
                        "live rewrite parent is not an exact audited append".to_string(),
                    ));
                };
                let messages_before_base = endpoint.message_count();
                state.append_authorized_rewrite(
                    commit.clone(),
                    messages_before_base,
                    parent_advance,
                    parent_row_prefix,
                    replacement,
                    result_witness,
                    self.updated_at,
                    commit.committed_at,
                )?;
                state
            }
        };
        self.install_validated_transcript_history_state(state)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        self.realtime_transcript
            .apply_prepared_rebase(realtime_rebase);
        self.messages.replace(rewritten);
        self.mark_content_mutated(commit.committed_at);
        if !self.install_exact_message_row_prefix(result_row_prefix) {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "failed to install rewrite result row-lineage authority".to_string(),
            ));
        }
        Ok(commit)
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
        let id = SessionId::new();
        Self {
            version: session_version(),
            realtime_transcript: Box::new(SessionRealtimeTranscriptProjection::empty(&id)),
            id,
            messages: TranscriptMessages::from_fresh_branch(truncated),
            created_at: now,
            updated_at: now,
            metadata: self.fork_metadata_projection(),
            history_caches: Box::default(),
            transcript_history_metadata_validation: TranscriptHistoryMetadataValidation::Validated,
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
        let id = SessionId::new();
        Self {
            version: session_version(),
            realtime_transcript: Box::new(SessionRealtimeTranscriptProjection::empty(&id)),
            id,
            messages: self.messages.clone(),
            created_at: now,
            updated_at: now,
            metadata: self.fork_metadata_projection(),
            history_caches: Box::default(),
            transcript_history_metadata_validation: TranscriptHistoryMetadataValidation::Validated,
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
    /// Mandatory on read: a persisted row missing the byte (or carrying a
    /// non-current value) fails closed through the generated persistence
    /// version authority instead of silently defaulting. Stamped with the
    /// current `SESSION_METADATA_SCHEMA_VERSION` on every persist.
    pub schema_version: u32,
    pub model: String,
    pub max_tokens: u32,
    #[serde(default = "crate::config::default_structured_output_retries")]
    pub structured_output_retries: u32,
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_hosted_server_id: Option<String>,
    /// Typed provider parameter overrides persisted with the session.
    /// Parsed fail-closed at the serde boundary — no JSON bag survives here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
    pub tooling: SessionTooling,
    #[serde(default)]
    pub keep_alive: bool,
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (populated when comms is enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_meta: Option<PeerMeta>,
    /// Realm identity for cross-surface storage sharing/isolation.
    ///
    /// Typed [`crate::RealmId`]; the realm slug is validated at the serde
    /// boundary. `RealmId` serializes transparently as its slug string, so the
    /// durable JSON shape is identical to the prior `Option<String>` form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<crate::RealmId>,
    /// Optional process/agent instance identifier within a realm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// Backend pinned by the realm manifest (e.g. "sqlite", "jsonl", "memory").
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
    /// Typed durable identity of a mob member, when this session was created by
    /// the mob runtime.
    ///
    /// This is the canonical owner of the `(mob_id, role, member)` identity
    /// fact used by mob ownership routing on resume/restart. It replaces the
    /// prior recovery-by-string-split of `comms_name` plus a realm
    /// format-string check. `comms_name`/`realm_id`/`peer_meta` remain as the
    /// transport routing name and discovery metadata.
    ///
    /// Older persisted sessions without the field deserialize as `None`
    /// (backward compatible via `#[serde(default)]`), so old rows read as
    /// "no typed binding" rather than failing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_member_binding: Option<crate::MobMemberBinding>,
}

/// Canonical durable LLM identity for a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SessionLlmIdentity {
    pub model: String,
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_hosted_server_id: Option<String>,
    /// Typed provider parameter overrides carried on the durable identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
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
///
/// `provider_params` and `auth_binding` carry the canonical Inherit/Set/Clear
/// tri-state via [`TurnMetadataOverride`]: `None` preserves the durable value,
/// `Some(Set)` overrides it for this turn, and `Some(Clear)` removes it. The
/// illegal "set and clear" fourth state is structurally unrepresentable, so the
/// resolver needs no reject branch for it.
pub struct SessionLlmIdentityOverride<'a> {
    pub model: Option<&'a str>,
    pub provider: Option<Provider>,
    /// Exact configured route for a self-hosted model. This cannot be inferred
    /// from provider/model when multiple local servers expose the same model
    /// identifier.
    pub self_hosted_server_id: Option<&'a str>,
    pub provider_params:
        Option<TurnMetadataOverride<&'a crate::lifecycle::run_primitive::ProviderParamsOverride>>,
    pub auth_binding: Option<TurnMetadataOverride<&'a crate::AuthBindingRef>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionLlmIdentityOverrideError {
    #[error("provider override requires model on an existing session")]
    ProviderRequiresModel,
    #[error("{0}")]
    ProviderModelMismatch(String),
    #[error("self-hosted provider requires a registered model alias; '{model}' is not configured")]
    MissingSelfHostedAlias { model: String },
    #[error("self_hosted_server_id requires provider 'self_hosted'")]
    SelfHostedServerRequiresSelfHostedProvider,
    #[error("self_hosted_server_id must not be empty")]
    EmptySelfHostedServerId,
    #[error(
        "self-hosted model '{model}' is configured on server '{configured}', not requested server '{requested}'"
    )]
    SelfHostedServerMismatch {
        model: String,
        requested: String,
        configured: String,
    },
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

    let provider_params = match overrides.provider_params {
        Some(TurnMetadataOverride::Clear) => None,
        Some(TurnMetadataOverride::Set(value)) => Some(value.clone()),
        None => current.provider_params.clone(),
    };
    if overrides.self_hosted_server_id.is_some() && provider != Provider::SelfHosted {
        return Err(SessionLlmIdentityOverrideError::SelfHostedServerRequiresSelfHostedProvider);
    }
    let self_hosted_server_id = if provider == Provider::SelfHosted {
        if let Some(requested_server_id) = overrides.self_hosted_server_id {
            if requested_server_id.trim().is_empty() {
                return Err(SessionLlmIdentityOverrideError::EmptySelfHostedServerId);
            }
            let entry = registry
                .entry_for_provider(Provider::SelfHosted, &model)
                .ok_or_else(|| SessionLlmIdentityOverrideError::MissingSelfHostedAlias {
                    model: model.clone(),
                })?;
            let configured_server_id = entry
                .self_hosted
                .as_ref()
                .map(|server| server.server_id.as_str())
                .ok_or_else(|| SessionLlmIdentityOverrideError::MissingSelfHostedAlias {
                    model: model.clone(),
                })?;
            if configured_server_id != requested_server_id {
                return Err(SessionLlmIdentityOverrideError::SelfHostedServerMismatch {
                    model,
                    requested: requested_server_id.to_string(),
                    configured: configured_server_id.to_string(),
                });
            }
            Some(requested_server_id.to_string())
        } else if overrides.model.is_none() {
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

    let auth_binding = match overrides.auth_binding {
        Some(TurnMetadataOverride::Clear) => None,
        Some(TurnMetadataOverride::Set(value)) => Some(value.clone()),
        // Inherit: a provider change without an explicit binding drops the
        // stale binding; otherwise the durable binding is retained.
        None if provider != current.provider => None,
        None => current.auth_binding.clone(),
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
/// including provider params and provider-native request defaults resolved for
/// the same target model/provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SessionLlmRequestPolicy {
    pub model: String,
    /// Typed explicit provider parameter overrides for the next LLM call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
    /// Typed provider-native request defaults resolved for the swapped target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_tool_defaults: Option<crate::lifecycle::run_primitive::ProviderTag>,
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

/// Tooling intent captured at session creation time.
///
/// Fields use [`ToolCategoryOverride`] to distinguish "no opinion" from
/// explicit enable/disable (Dogma §10). On resume, `Inherit` falls through
/// to the factory's current runtime default, allowing new tool categories
/// to become available without re-creating the session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionTooling {
    #[serde(default)]
    pub builtins: ToolCategoryOverride,
    #[serde(default)]
    pub shell: ToolCategoryOverride,
    #[serde(default)]
    pub comms: ToolCategoryOverride,
    /// Mob (multi-agent orchestration) tools.
    #[serde(default)]
    pub mob: ToolCategoryOverride,
    /// Semantic memory.
    #[serde(default)]
    pub memory: ToolCategoryOverride,
    /// Scheduler tools.
    #[serde(default)]
    pub schedule: ToolCategoryOverride,
    /// WorkGraph durable work tools.
    #[serde(default)]
    pub workgraph: ToolCategoryOverride,
    /// Assistant image generation.
    #[serde(default)]
    pub image_generation: ToolCategoryOverride,
    /// Meerkat-owned fallback web search.
    #[serde(default)]
    pub web_search: ToolCategoryOverride,
    /// Effective call-level tool execution policy for this session's builds.
    ///
    /// Persisted RESOLVED (never `Inherit`): the factory fails the build
    /// closed on an unresolved `Inherit` before metadata is written, so this
    /// field only ever holds `AllowList`/`DenyList`. Absent means
    /// unrestricted. Spawn/fork resolution reads this field as the parent's
    /// effective policy when a child requests `Inherit` (transitive
    /// containment — a restricted parent cannot mint an unrestricted child
    /// by spawning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<crate::ops::ToolAccessPolicy>,
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

/// Decode the typed [`SESSION_METADATA_KEY`] fact from a session metadata map
/// through the generated restore authority.
///
/// Canonical single decoder: [`Session::try_session_metadata`] and every
/// metadata-only read seam ([`PersistedSessionMetadataView`]) delegate here so
/// the full-session and metadata-only decode paths can never drift.
///
/// Fail-closed: a present-but-corrupt value is an error, never "absent".
pub fn try_session_metadata_from_map(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<SessionMetadata>, serde_json::Error> {
    let Some(value) = metadata.get(SESSION_METADATA_KEY) else {
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

/// Decode the typed [`SESSION_LIFECYCLE_TERMINAL_KEY`] fact from a session
/// metadata map.
///
/// Canonical single decoder: [`Session::try_lifecycle_terminal`] and every
/// metadata-only read seam delegate here. An absent key means no terminal
/// fact; a present-but-corrupt value fails closed.
pub fn try_lifecycle_terminal_from_map(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<SessionLifecycleTerminal>, serde_json::Error> {
    match metadata.get(SESSION_LIFECYCLE_TERMINAL_KEY) {
        Some(value) => serde_json::from_value(value.clone()).map(Some),
        None => Ok(None),
    }
}

/// Typed metadata-only view of a persisted session row or snapshot.
///
/// The metadata read seam's currency (mobkit ask-24 clause 3): carries the
/// session identity plus the two typed session-authority metadata facts,
/// decoded fail-closed through the canonical map-level decoders. Consumers
/// that only need ownership/policy/lifecycle facts read this view instead of
/// materializing the full session document.
#[derive(Debug, Clone)]
pub struct PersistedSessionMetadataView {
    pub session_id: SessionId,
    pub session_metadata: Option<SessionMetadata>,
    pub lifecycle_terminal: Option<SessionLifecycleTerminal>,
}

impl PersistedSessionMetadataView {
    /// Build the view from a persisted metadata map (e.g. a
    /// [`SessionMeta`] row projection).
    ///
    /// Fail-closed: corrupt values under either reserved key are an error,
    /// never treated as absent.
    pub fn try_from_metadata_map(
        session_id: SessionId,
        metadata: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            session_id,
            session_metadata: try_session_metadata_from_map(metadata)?,
            lifecycle_terminal: try_lifecycle_terminal_from_map(metadata)?,
        })
    }

    /// Project the view from a fully materialized session document.
    pub fn try_from_session(session: &Session) -> Result<Self, serde_json::Error> {
        Ok(Self {
            session_id: session.id().clone(),
            session_metadata: session.try_session_metadata()?,
            lifecycle_terminal: session.try_lifecycle_terminal()?,
        })
    }

    /// Typed durable mob member identity carried on the session metadata,
    /// if any.
    pub fn mob_member_binding(&self) -> Option<&crate::MobMemberBinding> {
        self.session_metadata.as_ref()?.mob_member_binding.as_ref()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {

    /// Ordinary append does not consult or rewrite transcript-history
    /// metadata, regardless of whether the session's parsed-graph cache is
    /// warm or requires validation.
    ///
    /// Control session takes the slow path (its history-validation flag is
    /// flipped back to `RequiresValidation` before every append, which is the
    /// state an unchecked metadata write leaves behind); the subject takes the
    /// Both sessions must retain the same audited head, commits, and bodies;
    /// only their live transcript digest advances.
    #[test]
    fn ordinary_append_graph_is_independent_of_validation_cache_state()
    -> Result<(), Box<dyn std::error::Error>> {
        fn seeded() -> Result<Session, Box<dyn std::error::Error>> {
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("A".to_string())));
            session.push(Message::User(UserMessage::text("B".to_string())));
            session.commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::User(UserMessage::text("B2".to_string()))],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )?;
            Ok(session)
        }

        let mut subject = seeded()?;
        let mut control = subject.clone();

        for index in 0..4 {
            let message = Message::User(UserMessage::text(format!("append {index}")));
            subject.push(message.clone());

            // Force the control down the full validating path.
            let value = serde_json::to_value(
                control
                    .transcript_history_state()?
                    .ok_or_else(|| std::io::Error::other("control history missing"))?,
            )?;
            control.set_metadata_unchecked_for_test(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, value);
            control.transcript_history_metadata_validation =
                TranscriptHistoryMetadataValidation::RequiresValidation;
            control.push(message);

            let subject_state = subject
                .transcript_history_state()?
                .ok_or_else(|| std::io::Error::other("subject history missing"))?;
            let control_state = control
                .transcript_history_state()?
                .ok_or_else(|| std::io::Error::other("control history missing"))?;
            assert_eq!(
                subject_state.head(),
                control_state.head(),
                "head at {index}"
            );
            assert_eq!(
                subject_state.commits().collect::<Vec<_>>(),
                control_state.commits().collect::<Vec<_>>(),
                "commits at {index}"
            );
            let project = |state: &TranscriptHistoryState| {
                let mut bodies = state
                    .materialize_revision_bodies()
                    .expect("audited bodies should materialize")
                    .into_iter()
                    .map(|body| (body.revision, body.messages))
                    .collect::<Vec<_>>();
                bodies.sort_by(|left, right| left.0.cmp(&right.0));
                bodies
            };
            assert_eq!(
                project(&subject_state),
                project(&control_state),
                "retained bodies at {index}"
            );
            subject.validate_transcript_history_state()?;
        }
        Ok(())
    }
    use super::*;
    use crate::realtime_transcript::RealtimeTranscriptRole;
    use crate::types::{
        AssistantBlock, BlockAssistantMessage, ContentBlock, StopReason, SystemMessage, Usage,
        UserMessage,
    };
    use std::sync::Arc;

    fn rewrite_record_at(
        state: &TranscriptHistoryState,
        edge_index: usize,
    ) -> TranscriptRewriteRecord {
        let commit = state
            .commit(edge_index)
            .unwrap_or_else(|| panic!("rewrite occurrence {edge_index} should exist"))
            .clone();
        let parent_body = state
            .materialize_occurrence_parent(edge_index)
            .unwrap_or_else(|error| {
                panic!("rewrite occurrence {edge_index} parent should materialize: {error}")
            });
        let revision_body = state
            .materialize_occurrence_child(edge_index)
            .unwrap_or_else(|error| {
                panic!("rewrite occurrence {edge_index} child should materialize: {error}")
            });
        TranscriptRewriteRecord::new(commit, parent_body, revision_body).unwrap_or_else(|error| {
            panic!("rewrite occurrence {edge_index} should validate: {error}")
        })
    }

    fn released_0810_document(
        session: &Session,
        head: String,
        revisions: Vec<TranscriptRevisionBody>,
    ) -> serde_json::Value {
        let state = session
            .transcript_history_state()
            .expect("current history should decode")
            .expect("current history should exist");
        let mut commits = serde_json::to_value(state.commits().cloned().collect::<Vec<_>>())
            .expect("commits should serialize");
        for commit in commits.as_array_mut().expect("commit vector") {
            let fields = commit.as_object_mut().expect("commit object");
            fields.remove("rewrite_generation");
            let selection = fields
                .get_mut("selection")
                .and_then(serde_json::Value::as_object_mut)
                .expect("selection object");
            if matches!(
                selection.get("type").and_then(serde_json::Value::as_str),
                Some("edit_message_range" | "compaction_message_range")
            ) {
                let range = selection
                    .remove("range")
                    .and_then(|value| value.as_object().cloned())
                    .expect("typed range object");
                *selection = serde_json::Map::from_iter([
                    (
                        "type".to_string(),
                        serde_json::Value::String("message_range".to_string()),
                    ),
                    (
                        "start".to_string(),
                        range.get("start").cloned().expect("range start"),
                    ),
                    (
                        "end".to_string(),
                        range.get("end").cloned().expect("range end"),
                    ),
                ]);
            }
        }

        let mut document = serde_json::to_value(session).expect("current session should serialize");
        document["version"] = serde_json::json!(2);
        let metadata = document["metadata"]
            .as_object_mut()
            .expect("metadata object");
        metadata.remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
        metadata.insert(
            SESSION_TRANSCRIPT_HISTORY_STATE_KEY.to_string(),
            serde_json::json!({
                "head": head,
                "commits": commits,
                "revisions": revisions,
                "digest_format": TRANSCRIPT_DIGEST_FORMAT_CURRENT,
            }),
        );
        document
    }

    fn transient_context(text: &str) -> TurnRequestContext {
        TurnRequestContext::new(text.to_string()).expect("non-empty transient context")
    }
    async fn wait_for_transient_boundary_request(handle: &TransientTurnContextStateHandle) {
        for _ in 0..1_000 {
            let registered = matches!(
                &handle.boundary.lock().window,
                TransientTurnContextBoundaryWindow::Open {
                    request: Some(_),
                    ..
                }
            );
            if registered {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("transient boundary request did not register");
    }
    #[test]
    fn prepared_transient_boundary_authority_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<PreparedTransientTurnContextBoundary>();
        assert_send::<crate::lifecycle::CoreBoundaryStageOutput>();
    }
    #[tokio::test]
    async fn transient_boundary_runner_first_consumes_no_context() {
        let state = TransientTurnContextStateHandle::new();
        let run_id = RunId::new();
        let _guard = state
            .begin_boundary_run(run_id.clone())
            .expect("open boundary");
        let contexts = state
            .take_pending_at_exact_boundary(&run_id)
            .await
            .expect("consume empty boundary");
        assert!(contexts.is_empty());
        let error = state
            .prepare_active_turn_boundary(&run_id, vec![transient_context("late")])
            .await
            .expect_err("runner-first boundary is closed");
        assert!(error.is_unavailable());
    }
    #[tokio::test]
    async fn transient_boundary_prepare_commit_publishes_exact_order_once() {
        let state = TransientTurnContextStateHandle::new();
        let run_id = RunId::new();
        let _guard = state
            .begin_boundary_run(run_id.clone())
            .expect("open boundary");
        let prepare_state = state.clone();
        let prepare_run_id = run_id.clone();
        let prepare = tokio::spawn(async move {
            prepare_state
                .prepare_active_turn_boundary(
                    &prepare_run_id,
                    vec![transient_context(" first "), transient_context("second")],
                )
                .await
        });
        wait_for_transient_boundary_request(&state).await;
        let runner_state = state.clone();
        let runner_run_id = run_id.clone();
        let runner = tokio::spawn(async move {
            runner_state
                .take_pending_at_exact_boundary(&runner_run_id)
                .await
        });
        let prepared = prepare
            .await
            .expect("prepare task")
            .expect("parked preparation");
        prepared
            .into_stage_output(None)
            .commit()
            .expect("publish transient context");
        let contexts = runner.await.expect("runner task").expect("runner consume");
        assert_eq!(
            contexts
                .iter()
                .map(TurnRequestContext::as_str)
                .collect::<Vec<_>>(),
            vec![" first ", "second"]
        );
    }

    #[tokio::test]
    async fn transient_boundary_prepare_abort_releases_runner_without_context() {
        let state = TransientTurnContextStateHandle::new();
        let run_id = RunId::new();
        let _guard = state
            .begin_boundary_run(run_id.clone())
            .expect("open boundary");
        let prepare_state = state.clone();
        let prepare_run_id = run_id.clone();
        let prepare = tokio::spawn(async move {
            prepare_state
                .prepare_active_turn_boundary(
                    &prepare_run_id,
                    vec![transient_context("must not publish")],
                )
                .await
        });
        wait_for_transient_boundary_request(&state).await;
        let runner_state = state.clone();
        let runner_run_id = run_id.clone();
        let runner = tokio::spawn(async move {
            runner_state
                .take_pending_at_exact_boundary(&runner_run_id)
                .await
        });
        let prepared = prepare
            .await
            .expect("prepare task")
            .expect("parked preparation");
        prepared
            .into_stage_output(None)
            .abort()
            .expect("abort transient context");
        assert!(
            runner
                .await
                .expect("runner task")
                .expect("runner released")
                .is_empty()
        );
    }

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

    /// Reducer tests enter through the same proof shape as persistent
    /// ingestion: a metadata-only anchor is staged first, then a canonical
    /// blob-backed event is applied. Blob bytes are verified in
    /// PersistentSessionService tests; this helper tests only reducer ownership.
    fn append_staged_user_image(
        session: &mut Session,
        event: &RealtimeTranscriptEvent,
    ) -> RealtimeTranscriptApplyOutcome {
        let RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key,
            item_id,
            previous_item_id,
            content_index,
            content,
        } = event
        else {
            panic!("test helper requires user content final")
        };
        let [ContentBlock::Image { media_type, data }] = content.as_slice() else {
            panic!("test helper requires exactly one image")
        };
        let media_type = crate::image_generation::MediaType::canonical_str(media_type);
        let blob_id = match data {
            crate::types::ImageData::Inline { data } => {
                crate::blob::content_blob_id(&media_type, data)
            }
            crate::types::ImageData::Blob { blob_id } => blob_id.clone(),
        };
        let pending = crate::PendingRealtimeUserContentBlob {
            idempotency_key: idempotency_key.clone(),
            item_id: item_id.clone(),
            previous_item_id: previous_item_id.clone(),
            content_index: *content_index,
            blob_id,
            media_type,
        };
        assert_eq!(
            session
                .stage_pending_realtime_user_content_blob(pending.clone())
                .expect("test pending anchor should stage"),
            crate::generated::session_document::RealtimeUserContentBlobStageDisposition::StageNew
        );
        session.append_realtime_transcript_event(pending.canonical_event())
    }

    #[test]
    fn transcript_digest_is_content_addressed() {
        let base_time = crate::types::message_timestamp_now();
        let stamped = vec![
            Message::User(UserMessage::text("turn one".to_string())),
            Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "answer one".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: crate::types::TranscriptMessageIdentity {
                    interaction_id: None,
                    run_id: Some(crate::lifecycle::RunId::new()),
                    objective_id: None,
                },
                created_at: base_time,
            }),
        ];
        let mut restamped = stamped.clone();
        for message in &mut restamped {
            match message {
                Message::User(user) => {
                    user.created_at = base_time + chrono::Duration::hours(2);
                }
                Message::BlockAssistant(assistant) => {
                    assistant.identity = crate::types::TranscriptMessageIdentity {
                        interaction_id: None,
                        run_id: Some(crate::lifecycle::RunId::new()),
                        objective_id: None,
                    };
                    assistant.created_at = base_time + chrono::Duration::hours(2);
                }
                _ => {}
            }
        }
        assert_eq!(
            transcript_messages_digest(&stamped).expect("digest"),
            transcript_messages_digest(&restamped).expect("digest"),
            "bookkeeping variance must not fork the transcript revision"
        );

        let mut content_changed = stamped.clone();
        if let Message::User(user) = &mut content_changed[0] {
            user.content = vec![ContentBlock::Text {
                text: "a different turn".to_string(),
            }];
        }
        assert_ne!(
            transcript_messages_digest(&stamped).expect("digest"),
            transcript_messages_digest(&content_changed).expect("digest"),
            "content changes must fork the transcript revision"
        );
    }

    #[test]
    fn public_generic_rewrite_api_rejects_typed_compaction_semantic() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("old context")));
        let error = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::typed_compaction_for_test(0, 1),
                vec![Message::User(UserMessage::compaction_summary("summary"))],
                TranscriptRewriteReason::new("anything"),
                None,
                None,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TranscriptEditError::InvalidTranscriptShape(_)
        ));
        assert_eq!(session.messages().len(), 1);
    }

    #[test]
    fn compaction_witness_authorizes_only_the_exact_validated_rebuild() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("old context one")));
        session.push(Message::User(UserMessage::text("old context two")));
        let validated = vec![Message::User(UserMessage::compaction_summary(
            "validated summary",
        ))];
        let authority = crate::agent::compact::ValidatedCompactionRewrite::for_test(
            session.messages(),
            &validated,
        )
        .unwrap();
        let error = session
            .replace_messages_for_compaction_internal(
                vec![Message::User(UserMessage::compaction_summary(
                    "substituted summary",
                ))],
                &authority,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            TranscriptEditError::InvalidTranscriptShape(_)
        ));
        assert_eq!(session.messages().len(), 2);
    }

    /// Whole-span digest reuse pin: the commit seam may substitute the
    /// already-held whole-document digests ONLY when the selection is the
    /// entire transcript. A partial-span rewrite must keep recording genuine
    /// O(span) digests — flip the reuse condition to `start == 0` alone and
    /// this fails (the recorded span digest would wrongly be the whole
    /// transcript's).
    #[test]
    fn partial_span_rewrite_records_span_digests_not_whole_document_digests() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("m-0")));
        session.push(Message::User(UserMessage::text("m-1")));
        session.push(Message::User(UserMessage::text("m-2")));
        let original = session.messages().to_vec();
        let whole_before = session.transcript_content_digest().unwrap();
        let replacement = vec![Message::User(UserMessage::text("m-1-rewritten"))];
        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                replacement.clone(),
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .unwrap();
        assert_eq!(
            commit.original_span_digest,
            transcript_messages_digest(&original[1..2]).unwrap(),
            "partial-span original digest must cover exactly the selected span"
        );
        assert_ne!(commit.original_span_digest, whole_before);
        assert_eq!(
            commit.replacement_digest,
            transcript_messages_digest(&replacement).unwrap(),
            "partial-span replacement digest must cover exactly the replacement"
        );
        assert_ne!(commit.replacement_digest, commit.revision);
        // The graph the commit installed must survive the full validator —
        // in particular `validate_transcript_rewrite_record`'s span/prefix/
        // suffix relations over these exact digests.
        let state = session.transcript_history_state().unwrap().unwrap();
        validate_transcript_history_state(&state).unwrap();
    }

    /// A cosmetic synthetic-notice refresh may never rewrite bytes inside an
    /// audited endpoint. Refuse it atomically at the mechanical mutation seam
    /// instead of allowing a graph/live divergence that only the next save or
    /// rewrite discovers.
    #[test]
    fn synthetic_notice_refresh_inside_audited_prefix_fails_before_mutation() {
        use crate::types::{SystemNoticeKind, SystemNoticeMessage};

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("u-0")));
        session.push(Message::User(UserMessage::text("u-1")));
        // A synthetic refresh notice INSIDE the window later mutations retain.
        session
            .replace_synthetic_notices(
                SystemNoticeKind::McpPending,
                vec![Message::SystemNotice(SystemNoticeMessage::new(
                    SystemNoticeKind::McpPending,
                    "pending v1",
                ))],
            )
            .expect("initial notice install");
        // First audited rewrite creates the graph; its endpoint body retains
        // the notice at index 2.
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("u-0-rewritten"))],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("first rewrite commits");
        // Ordinary append, then an attempted refresh that would move the
        // audited notice from inside the prefix to the tail.
        session.push(Message::User(UserMessage::text("u-2")));
        let before = session.messages().to_vec();
        let error = session
            .replace_synthetic_notices(
                SystemNoticeKind::McpPending,
                vec![Message::SystemNotice(SystemNoticeMessage::new(
                    SystemNoticeKind::McpPending,
                    "pending v2",
                ))],
            )
            .expect_err("refresh must not rewrite an audited prefix");
        assert!(
            matches!(error, TranscriptEditError::InvalidTranscriptShape(_)),
            "expected the atomic audited-prefix refusal, got: {error:?}"
        );
        assert_eq!(session.messages(), before.as_slice());

        // Fail-closed means untouched and durable.
        let bytes = serde_json::to_vec(&session).expect("session serializes");
        let decoded: Session = serde_json::from_slice(&bytes)
            .expect("the failed rewrite must leave a graph every cold reader accepts");
        assert_eq!(decoded.messages().len(), session.messages().len());
    }

    /// Decode validates the graph internally but the save/rewrite boundary
    /// owns its audited-prefix relation to top-level live messages. A current
    /// envelope whose live rows diverge inside the graph-proved
    /// endpoint must fail at ingress. Historical exact parent splices remain
    /// materializable only when already encoded as imported graph edges;
    /// current top-level rows cannot manufacture that relationship around the
    /// graph.
    #[test]
    fn current_envelope_rejects_non_append_live_rows_at_ingress() {
        let mut session = Session::new();
        session.append_system_message("original system");
        session.push(Message::User(UserMessage::text("m-1")));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::User(UserMessage::text("m-1-rewritten"))],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("seed rewrite commits");
        session.push(Message::User(UserMessage::text("m-2")));

        // Tamper only the first top-level row. The compact graph
        // remains internally valid, but the live vector is no longer its
        // exact endpoint plus an append-only suffix.
        let mut document = serde_json::to_value(&session).expect("session serializes");
        let divergent_message =
            serde_json::to_value(Message::System(SystemMessage::new("replacement system")))
                .expect("message serializes");
        let messages = document
            .get_mut("messages")
            .and_then(serde_json::Value::as_array_mut)
            .expect("messages array");
        messages[0] = divergent_message;
        let error = serde_json::from_value::<Session>(document)
            .expect_err("a current non-append live tail must fail closed at ingress");
        assert!(
            error
                .to_string()
                .contains("live transcript does not preserve the graph-proved audited endpoint"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn current_rewrite_refuses_non_append_parent_divergence() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("original row")));
        session.push(Message::User(UserMessage::text("question")));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::User(UserMessage::text("edited question"))],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("seed audited endpoint");
        let generation_before = session
            .transcript_rewrite_generation()
            .expect("rewrite generation");

        let mut divergent = session.messages().to_vec();
        divergent[0] = Message::User(UserMessage::text("replacement row"));
        session.messages.replace(divergent);
        let current_prefix =
            crate::SessionMessageRowPrefixAccumulator::from_messages(session.messages())
                .expect("current row prefix");
        assert!(session.install_exact_message_row_prefix(current_prefix));

        let error = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::User(UserMessage::text("edited again"))],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect_err("current writer must not infer a non-append parent splice");
        assert!(
            matches!(error, TranscriptEditError::HistoryStateMalformed(ref message)
                if message.contains("not an exact audited append")),
            "unexpected error: {error}"
        );
        assert_eq!(
            session
                .transcript_rewrite_generation()
                .expect("rewrite generation"),
            generation_before,
            "failed current write must not append a graph edge"
        );
    }

    #[test]
    fn semantic_marker_prevents_new_generic_compaction_forgery_and_heals_prior_data() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("old context one")));
        session.push(Message::User(UserMessage::text("old context two")));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![Message::User(UserMessage::compaction_summary("summary"))],
                TranscriptRewriteReason::new("compaction"),
                None,
                None,
            )
            .unwrap();
        let session: Session =
            serde_json::from_value(serde_json::to_value(&session).unwrap()).unwrap();
        let history = session.transcript_history_state().unwrap().unwrap();
        let current_commit = history.commit(0).expect("current rewrite commit");
        assert_eq!(
            current_commit.selection.semantic(),
            TranscriptRewriteSemantic::Edit,
            "new generic rewrites retain an explicit typed edit marker after roundtrip"
        );
        assert_eq!(current_commit.reason.kind, "compaction");

        let mut legacy_value =
            serde_json::to_value(rewrite_record_at(&history, 0)).expect("record wire");
        let legacy = legacy_value.as_object_mut().expect("record object");
        legacy.remove("digest_format");
        legacy.get_mut("commit").expect("legacy commit")["selection"] = serde_json::json!({
            "type": "message_range",
            "start": 0,
            "end": 2,
        });
        let legacy: TranscriptRewriteRecord =
            serde_json::from_value(legacy_value).expect("legacy record should heal");
        assert_eq!(
            legacy.commit.selection.semantic(),
            TranscriptRewriteSemantic::Compaction,
            "marker-absent prior data derives compaction from typed transcript evidence"
        );

        let mut ordinary = Session::new();
        ordinary.push(Message::User(UserMessage::text("ordinary old one")));
        ordinary.push(Message::User(UserMessage::text("ordinary old two")));
        ordinary
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![Message::User(UserMessage::text("ordinary replacement"))],
                TranscriptRewriteReason::new("compaction"),
                None,
                None,
            )
            .unwrap();
        let history = ordinary.transcript_history_state().unwrap().unwrap();
        assert_eq!(
            history
                .commit(0)
                .expect("ordinary rewrite commit")
                .selection
                .semantic(),
            TranscriptRewriteSemantic::Edit,
            "free-form reason must not upgrade an ordinary edit"
        );
    }

    /// Sealed-capability seam: the snapshot compaction returns the proof of
    /// exactly the graph value it installed into the metadata map.
    #[test]
    fn snapshot_compaction_returns_the_proof_of_the_installed_graph() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("seam proof before")));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("seam proof after"))],
                TranscriptRewriteReason::new("edit"),
                None,
                None,
            )
            .unwrap();
        let document = serde_json::to_value(&session).unwrap();
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            SESSION_TRANSCRIPT_HISTORY_STATE_KEY.to_string(),
            document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY].clone(),
        );
        let graph_wire = metadata[SESSION_TRANSCRIPT_HISTORY_STATE_KEY].clone();

        let sealed = compact_transcript_history_metadata_for_snapshot(&mut metadata)
            .expect("valid graph compacts")
            .expect("graph value present");
        assert_eq!(
            serde_json::to_value(sealed.as_ref()).unwrap(),
            graph_wire,
            "the returned proof must cover exactly the consumed graph value"
        );
        assert!(
            !metadata.contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            "the transient wire graph must not remain beside the typed authority"
        );

        // No graph, no proof: the seam must not manufacture evidence.
        let mut empty = serde_json::Map::new();
        assert!(
            compact_transcript_history_metadata_for_snapshot(&mut empty)
                .expect("empty metadata compacts")
                .is_none()
        );
    }

    /// Decode threads the proven parse into the per-instance shared cache and
    /// removes the transient wire projection from ordinary metadata. The first
    /// consumer after a decode must not re-parse the graph value serialized
    /// from that exact state one statement earlier.
    #[test]
    fn decode_seeds_shared_transcript_graph_with_the_proven_parse() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("seed shared parse")));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("seed shared parse two"))],
                TranscriptRewriteReason::new("edit"),
                None,
                None,
            )
            .unwrap();
        let document = serde_json::to_value(&session).unwrap();
        let serialized_graph = document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY].clone();

        let decoded: Session = serde_json::from_value(document).unwrap();
        let seeded = decoded
            .history_caches
            .shared_state
            .get()
            .expect("decode must seed the shared graph parse");
        assert_eq!(
            serde_json::to_value(&*seeded).unwrap(),
            serialized_graph,
            "the seeded graph must be the value the wire carried"
        );
        assert!(
            !decoded
                .metadata
                .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            "the typed graph cache is the singular in-memory authority"
        );
        // The sealed accessor serves the seeded allocation, not a re-parse.
        let sealed = decoded
            .validated_transcript_history_state()
            .expect("validated read")
            .expect("graph present");
        assert!(
            std::sync::Arc::ptr_eq(&seeded, &sealed.shared()),
            "validated_transcript_history_state must serve the decode-seeded parse"
        );

        // A graph-free document must not seed anything.
        let bare: Session =
            serde_json::from_value(serde_json::to_value(Session::new()).unwrap()).unwrap();
        assert!(bare.history_caches.shared_state.get().is_none());
    }

    /// A `ValidatedTranscriptHistory` is the evidence its consumers stopped
    /// re-deriving, so the one place that mints it must never hand one out for
    /// a graph this process has not actually verified. Metadata written through
    /// an unchecked seam clears the validation marker; the accessor owes that
    /// session a full verification, and a digest-inconsistent body must fail it.
    #[test]
    fn sealed_transcript_history_refuses_unverified_corrupt_graph() {
        let mut source = Session::new();
        source.push(Message::User(UserMessage::text("hello".to_string())));
        source
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("hello again".to_string()))],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("consistent rewrite should commit");
        let mut source_document = serde_json::to_value(&source).expect("source serializes");
        source_document["metadata"][SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["anchor"]["messages"]
            [0] = serde_json::to_value(Message::User(UserMessage::text("tampered".to_string())))
            .expect("tampered message");
        assert!(
            serde_json::from_value::<Session>(source_document).is_err(),
            "decode must not mint a proof for a digest-inconsistent transcript graph"
        );
    }

    /// The other half of the same contract: verification is what the accessor
    /// owes, not refusal. A consistent graph installed through the same
    /// unchecked seam seals, so downstream guards keep working without each
    /// re-running the whole-graph validator.
    #[test]
    fn sealed_transcript_history_verifies_and_seals_consistent_graph() {
        let mut source = Session::new();
        source.push(Message::User(UserMessage::text("hello".to_string())));
        let commit = source
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("hello again".to_string()))],
                TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("consistent rewrite should commit");
        let source_document = serde_json::to_value(&source).expect("source serializes");
        let session: Session =
            serde_json::from_value(source_document).expect("consistent graph decodes");
        let sealed = session
            .validated_transcript_history_state()
            .expect("a consistent graph must seal")
            .expect("history metadata is present");
        assert_eq!(sealed.state().head(), commit.revision);
    }

    /// K4 invariant: synthetic-notice refresh is ONE atomic transcript edit —
    /// after a refresh, at most the replacement notices of that kind exist
    /// (no stale notice survives beside a fresh one).
    #[test]
    fn replace_synthetic_notices_leaves_only_replacements_of_kind() {
        use crate::types::{SystemNoticeKind, SystemNoticeMessage};

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));
        session.push(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::McpPending,
            "stale one",
        )));
        session.push(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::McpPending,
            "stale two",
        )));
        // A notice of another kind must be untouched.
        session.push(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::BackgroundJob,
            "other-kind",
        )));

        session
            .replace_synthetic_notices(
                SystemNoticeKind::McpPending,
                vec![Message::SystemNotice(SystemNoticeMessage::new(
                    SystemNoticeKind::McpPending,
                    "fresh",
                ))],
            )
            .expect("notice refresh succeeds");

        let mcp_pending: Vec<&SystemNoticeMessage> = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::McpPending => {
                    Some(notice)
                }
                _ => None,
            })
            .collect();
        assert_eq!(mcp_pending.len(), 1, "exactly one notice of the kind");
        assert_eq!(mcp_pending[0].body.as_deref(), Some("fresh"));
        assert!(
            session.messages().iter().any(|message| matches!(
                message,
                Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::BackgroundJob
            )),
            "other-kind notices are untouched"
        );

        // Empty replacements = pure strip.
        session
            .replace_synthetic_notices(SystemNoticeKind::McpPending, Vec::new())
            .expect("pure strip succeeds");
        assert!(
            !session.messages().iter().any(|message| matches!(
                message,
                Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::McpPending
            )),
            "empty replacement clears the kind"
        );
    }

    #[test]
    fn ordinary_appends_after_rewrite_leave_audited_graph_untouched() {
        let mut session = Session::new();
        for message in 0..133 {
            session.push(Message::User(UserMessage::text(format!(
                "seed message {message}"
            ))));
        }
        let parent = session.transcript_revision().expect("parent revision");
        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange {
                    start: 132,
                    end: 133,
                },
                vec![Message::User(UserMessage::text("edited question"))],
                TranscriptRewriteReason::new("unit-test-edit"),
                Some("unit-test".to_string()),
                Some(parent),
            )
            .expect("rewrite should commit");
        let graph_before = session
            .validated_transcript_history_state()
            .expect("history validation")
            .expect("rewrite graph");

        for turn in 0..762 {
            session.push(Message::User(UserMessage::text(format!("turn {turn}"))));
        }

        let graph_after = session
            .validated_transcript_history_state()
            .expect("history validation")
            .expect("rewrite graph");
        assert!(
            graph_before.shares_exact_state_with(&graph_after),
            "ordinary appends must preserve the exact audited graph authority"
        );
        let state = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("rewrite should create history state");
        assert_eq!(session.messages().len(), 895);
        assert_eq!(state.commit_count(), 1, "ordinary appends are not rewrites");
        let retained_bodies = state
            .materialize_revision_bodies()
            .expect("audited bodies should materialize");
        assert_eq!(
            retained_bodies.len(),
            2,
            "one real rewrite retains only its two audited endpoints"
        );
        assert_eq!(state.head(), commit.revision);
        assert_ne!(
            session.transcript_revision().expect("live revision"),
            state.head(),
            "the live append tail is Session authority, not a mechanical graph head"
        );
        let retained_message_entries = retained_bodies
            .iter()
            .map(|body| body.messages.len())
            .sum::<usize>();
        assert!(retained_message_entries <= 2 * session.messages().len());

        let live_bytes = serde_json::to_vec(session.messages())
            .expect("live transcript should serialize")
            .len();
        let snapshot_bytes = serde_json::to_vec(&session)
            .expect("session snapshot should serialize")
            .len();
        assert!(
            snapshot_bytes <= live_bytes.saturating_mul(5).saturating_add(64 * 1024),
            "snapshot must remain linear in the live transcript: {snapshot_bytes} bytes for {live_bytes} live bytes"
        );
    }

    #[test]
    fn repeated_synthetic_notice_refreshes_do_not_mint_rewrite_commits() {
        use crate::types::{SystemNoticeKind, SystemNoticeMessage};

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("before".to_string())));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("after".to_string()))],
                TranscriptRewriteReason::new("unit-test-edit"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("seed rewrite");
        let graph_before = session
            .validated_transcript_history_state()
            .expect("history validation")
            .expect("audited graph");

        for refresh in 0..64 {
            session
                .replace_synthetic_notices(
                    SystemNoticeKind::McpPending,
                    vec![Message::SystemNotice(SystemNoticeMessage::new(
                        SystemNoticeKind::McpPending,
                        format!("refresh {refresh}"),
                    ))],
                )
                .expect("mechanical refresh");
        }
        let graph_after = session
            .validated_transcript_history_state()
            .expect("history validation")
            .expect("audited graph");
        assert!(
            graph_before.shares_exact_state_with(&graph_after),
            "tail-only synthetic refreshes must preserve the exact audited graph authority"
        );

        let state = session
            .transcript_history_state()
            .expect("history state")
            .expect("seed rewrite history");
        assert_eq!(state.commit_count(), 1);
        assert_eq!(session.transcript_rewrite_generation().unwrap(), 1);
        assert_eq!(
            state
                .materialize_revision_bodies()
                .expect("audited bodies should materialize")
                .len(),
            2,
            "mechanical refreshes do not mint retained live-head bodies"
        );
    }

    #[test]
    fn snapshot_compaction_does_not_launder_corrupt_old_body() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("seed".to_string())));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("rewritten".to_string()))],
                TranscriptRewriteReason::new("unit-test-edit"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("seed rewrite");
        let state = session
            .transcript_history_state()
            .expect("state")
            .expect("history");
        let mut state = serde_json::to_value(&state).expect("current history value");
        state["anchor"]["messages"][0] =
            serde_json::to_value(Message::User(UserMessage::text("tampered".to_string())))
                .expect("tampered message");
        session.set_metadata_unchecked_for_test(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, state);

        assert!(
            serde_json::to_vec(&session).is_err(),
            "serialization must fail before pruning a corrupt old body"
        );
    }

    /// The compatibility floor is 0.8.10. Its current-digest graph could still
    /// carry full mechanical append bodies; the explicit one-time importer
    /// validates that exact released shape before canonicalizing to audited
    /// endpoints.
    #[test]
    fn released_0_8_10_mechanical_history_is_compacted_at_import_boundary() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("seed".to_string())));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("rewritten".to_string()))],
                TranscriptRewriteReason::new("unit-test-edit"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("seed rewrite");
        let state = session
            .transcript_history_state()
            .expect("state")
            .expect("history");
        let mut revisions = state
            .materialize_revision_bodies()
            .expect("released audit bodies should materialize");
        let mut parent = state.head().to_string();
        for index in 0..8 {
            session.push(Message::User(UserMessage::text(format!(
                "0.8.10 ordinary append {index}"
            ))));
            let revision = transcript_messages_digest(session.messages()).expect("revision digest");
            revisions.push(TranscriptRevisionBody {
                revision: revision.clone(),
                parent_revision: Some(parent),
                messages: session.messages().to_vec(),
                created_at: SystemTime::now(),
            });
            parent = revision;
        }
        let released = released_0810_document(&session, parent, revisions);
        let released = serde_json::to_vec(&released).expect("released document bytes");
        let imported =
            import_released_0810_session(&released).expect("released document should import");
        assert_eq!(
            imported.receipt().evidence(),
            Released0810ImportEvidence::StoreAuthorizationRequired
        );
        let compact = imported
            .session()
            .transcript_history_state()
            .expect("imported history")
            .expect("imported graph");

        assert_eq!(
            compact
                .materialize_revision_bodies()
                .expect("compact bodies should materialize")
                .len(),
            2,
            "import boundary should retain only the two audited endpoints"
        );
        assert_eq!(
            compact.head(),
            compact.last_commit().expect("rewrite commit").revision
        );
        validate_transcript_history_state(&compact).expect("compacted history remains valid");
    }

    #[test]
    fn transcript_history_rejects_stale_branch_after_digest_recurrence() {
        let mut restored = Session::new();
        restored.push(Message::User(UserMessage::text("A".to_string())));
        restored
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("B".to_string()))],
                TranscriptRewriteReason::new("to-b"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("A to B");
        let mut stale_branch = restored.clone();
        restored
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("A".to_string()))],
                TranscriptRewriteReason::new("restore-a"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("B back to A");
        stale_branch
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("C".to_string()))],
                TranscriptRewriteReason::new("stale-b-to-c"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("stale B to C is locally valid");

        let stale_state = stale_branch
            .transcript_history_state()
            .expect("stale state")
            .expect("stale history");
        let restored_state = restored
            .transcript_history_state()
            .expect("restored state")
            .expect("restored history");
        let mut records = (0..restored_state.commit_count())
            .map(|index| rewrite_record_at(&restored_state, index))
            .collect::<Vec<_>>();
        let mut stale_record = rewrite_record_at(&stale_state, 1);
        stale_record.commit.rewrite_generation = 3;
        records.push(stale_record);

        assert!(
            TranscriptHistoryState::from_rewrite_records(records).is_err(),
            "an old B<-A body edge cannot authorize stale B->C after B->A restored A"
        );
    }

    #[test]
    fn zero_generation_0_8_10_cycle_normalizes_from_proved_vector_order() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("A".to_string())));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("B".to_string()))],
                TranscriptRewriteReason::new("to-b"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("A to B");
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("A".to_string()))],
                TranscriptRewriteReason::new("restore-a"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("B back to A");

        let current = session
            .transcript_history_state()
            .expect("current history decodes")
            .expect("current history exists");
        let released = released_0810_document(
            &session,
            current.head().to_string(),
            current
                .materialize_revision_bodies()
                .expect("released bodies should materialize"),
        );
        let released = serde_json::to_vec(&released).expect("released document bytes");
        let imported =
            import_released_0810_session(&released).expect("0.8.10 cyclic graph remains supported");
        assert_eq!(
            imported.receipt().evidence(),
            Released0810ImportEvidence::StoreAuthorizationRequired
        );
        let state = imported
            .session()
            .transcript_history_state()
            .expect("history decodes")
            .expect("history exists");
        assert_eq!(
            state
                .commits()
                .map(|commit| commit.rewrite_generation)
                .collect::<Vec<_>>(),
            vec![1, 2],
            "content recurrence must not rotate or refuse the proved commit-vector order"
        );
        validate_transcript_history_state(&state).expect("normalized cycle remains valid");
    }

    #[test]
    fn transcript_history_rejects_cyclic_edge_base() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("P".to_string())));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("Q".to_string()))],
                TranscriptRewriteReason::new("valid"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("valid seed rewrite");
        let state = session
            .transcript_history_state()
            .expect("state")
            .expect("history");
        let mut state = serde_json::to_value(&state).expect("current history value");
        let child_revision = state["edges"][0]["commit"]["revision"]
            .as_str()
            .expect("edge child revision")
            .to_string();
        state["edges"][0]["base_revision"] = serde_json::Value::String(child_revision);
        session.set_metadata_unchecked_for_test(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, state);

        assert!(
            serde_json::to_vec(&session).is_err(),
            "a cyclic compact-edge base must fail instead of looping"
        );
    }

    #[test]
    fn live_append_can_recur_to_an_audited_digest_without_moving_audited_head() {
        let a = Message::User(UserMessage::text("A".to_string()));
        let b = Message::User(UserMessage::text("B".to_string()));
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("X".to_string())));
        let first = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![a.clone(), b.clone()],
                TranscriptRewriteReason::new("to-a-b"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("X to [A,B]");
        let h_parent = session
            .transcript_revision_body(&first.revision)
            .expect("H body")
            .expect("H retained")
            .parent_revision;
        let second = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
                vec![a],
                TranscriptRewriteReason::new("to-a"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("[A,B] to [A]");

        session.push(b);

        let state = session
            .transcript_history_state()
            .expect("state")
            .expect("history");
        assert_eq!(
            state.head(),
            second.revision,
            "graph head remains the latest audited endpoint"
        );
        assert_eq!(session.transcript_revision().unwrap(), first.revision);
        let recurred_body = state
            .materialize_revision(&first.revision)
            .expect("recurred H body");
        assert_eq!(
            recurred_body.parent_revision, h_parent,
            "reusing an audited digest must not rewrite its occurrence metadata"
        );
        validate_transcript_history_state(&state).expect("audited graph remains valid");
    }

    /// K4 invariant (fail-closed): an invalid replacement is rejected with a
    /// typed fault BEFORE any strip happens — the transcript is unchanged, so
    /// a fault can never strand a half-refreshed notice state.
    #[test]
    fn replace_synthetic_notices_rejects_mismatched_kind_without_mutation() {
        use crate::types::{SystemNoticeKind, SystemNoticeMessage};

        let mut session = Session::new();
        session.push(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::McpPending,
            "stale",
        )));
        let before = session.messages().to_vec();

        let err = session
            .replace_synthetic_notices(
                SystemNoticeKind::McpPending,
                vec![Message::User(UserMessage::text("not a notice".to_string()))],
            )
            .expect_err("mismatched replacement must fail typed");
        assert!(
            matches!(err, TranscriptEditError::InvalidTranscriptShape(_)),
            "expected InvalidTranscriptShape, got {err:?}"
        );
        assert_eq!(
            session.messages(),
            before.as_slice(),
            "fault must leave the transcript unchanged (no partial strip)"
        );
    }

    #[test]
    fn replace_synthetic_notices_rejects_malformed_history_atomically() {
        use crate::types::{SystemNoticeKind, SystemNoticeMessage};

        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("before".to_string())));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("after".to_string()))],
                TranscriptRewriteReason::new("unit-test-edit"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("seed rewrite");
        session.push(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::McpPending,
            "stale",
        )));
        let state = session
            .transcript_history_state()
            .expect("state")
            .expect("history");
        let mut state = serde_json::to_value(&state).expect("current history value");
        state["anchor"]["messages"][0] =
            serde_json::to_value(Message::User(UserMessage::text("tampered".to_string())))
                .expect("tampered message");
        session.set_metadata_unchecked_for_test(SESSION_TRANSCRIPT_HISTORY_STATE_KEY, state);
        let before_messages = session.messages.clone();
        let before_metadata = session.metadata.clone();
        let before_updated_at = session.updated_at;

        assert!(
            session
                .replace_synthetic_notices(SystemNoticeKind::McpPending, Vec::new())
                .is_err()
        );
        assert_eq!(session.messages(), before_messages.as_slice());
        assert_eq!(session.metadata, before_metadata);
        assert_eq!(session.updated_at, before_updated_at);
    }

    #[test]
    fn replace_synthetic_notices_rejects_durable_notice_kinds() {
        use crate::types::SystemNoticeKind;

        let mut session = Session::new();
        let before = session.messages().to_vec();
        assert!(
            session
                .replace_synthetic_notices(SystemNoticeKind::Comms, Vec::new())
                .is_err()
        );
        assert_eq!(session.messages(), before);
    }

    #[test]
    fn replace_synthetic_notices_preserves_persisted_mcp_pending_notice() {
        use crate::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};

        let mut session = Session::new();
        session.push(Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::McpPending,
            Some("persisted pending fact".to_string()),
            SystemNoticeBlock::Mcp {
                server_id: Some("server".to_string()),
                operation: None,
                phase: None,
                persisted: true,
                detail: None,
                pending_sources: Vec::new(),
            },
        )));
        let before = session.messages().to_vec();

        session
            .replace_synthetic_notices(SystemNoticeKind::McpPending, Vec::new())
            .expect("synthetic refresh must coexist with a durable notice of the same kind");
        assert_eq!(session.messages(), before);
    }

    #[test]
    fn replace_synthetic_notices_replaces_projection_beside_persisted_mcp_fact() {
        use crate::types::{SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};

        let durable = Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::McpPending,
            Some("persisted pending fact".to_string()),
            SystemNoticeBlock::Mcp {
                server_id: Some("server".to_string()),
                operation: None,
                phase: None,
                persisted: true,
                detail: None,
                pending_sources: Vec::new(),
            },
        ));
        let stale = Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::McpPending,
            "stale synthetic projection",
        ));
        let fresh = Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::McpPending,
            "fresh synthetic projection",
        ));
        let mut session = Session::new();
        session.push(durable.clone());
        session.push(stale);

        session
            .replace_synthetic_notices(SystemNoticeKind::McpPending, vec![fresh.clone()])
            .expect("synthetic refresh beside durable fact");

        assert_eq!(session.messages(), &[durable, fresh]);
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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "unchanged".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
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
        original.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent_revision = original.transcript_revision().expect("parent revision");
        let mut incoming = original.clone();
        incoming
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent_revision),
            )
            .expect("rewrite should commit");
        incoming.push(Message::User(UserMessage::text("follow-up".to_string())));
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "follow-up answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
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
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "no tool after all".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = session.transcript_revision().expect("parent revision");

        let err = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "lookup".to_string(),
                        args: serde_json::value::RawValue::from_string("{}".to_string())
                            .expect("valid args"),
                        meta: None,
                    }],
                    stop_reason: StopReason::ToolUse,
                    identity: crate::types::TranscriptMessageIdentity::default(),
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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        session.push(Message::User(UserMessage::text("keep suffix".to_string())));

        let parent_revision = session.transcript_revision().expect("parent revision");
        let commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
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
        let record = rewrite_record_at(&state, 0);
        let parent_body = record.parent_body;
        let revision_body = record.revision_body;

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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose first answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let first_parent = session.transcript_revision().expect("first parent");
        let first_commit = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact first answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(first_parent),
            )
            .expect("first rewrite");

        session.push(Message::User(UserMessage::text("normal turn".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose second answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
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
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact second answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
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
        let records =
            (0..state.commit_count()).map(|edge_index| rewrite_record_at(&state, edge_index));

        let replayed = TranscriptHistoryState::from_rewrite_records(records)
            .expect("rewrite replay should accept normal-turn bridge revisions")
            .expect("rewrite records should exist");
        assert_eq!(replayed.head(), second_commit.revision);
        assert!(replayed.contains_revision(&bridge_parent));
    }

    #[test]
    fn transcript_rewrite_replay_rejects_branched_rewrite_records() {
        let mut base = Session::new();
        base.push(Message::User(UserMessage::text("question".to_string())));
        base.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent = base.transcript_revision().expect("parent revision");

        let mut first = base.clone();
        first
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "first compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
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
        second
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "second compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
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

        let err = TranscriptHistoryState::from_rewrite_records(vec![
            rewrite_record_at(&first_state, 0),
            rewrite_record_at(&second_state, 0),
        ])
        .expect_err("branched rewrite records must not replay as a linear source history");
        assert!(
            err.to_string()
                .contains("not expected contiguous generation"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn internal_message_rewrites_refresh_transcript_history_head() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent = session.transcript_revision().expect("parent revision");
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
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
        let retained = session
            .messages()
            .iter()
            .filter(|message| {
                !matches!(
                    message,
                    Message::User(user)
                        if user.content.iter().any(|block| matches!(
                            block,
                            ContentBlock::Text { text } if text.contains("notice-bearing")
                        ))
                )
            })
            .cloned()
            .collect();
        session
            .replace_messages_internal(
                retained,
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
                    Message::BlockAssistant(BlockAssistantMessage {
                        blocks: vec![AssistantBlock::Text {
                            text: "compacted answer".to_string(),
                            meta: None,
                        }],
                        stop_reason: StopReason::EndTurn,
                        identity: crate::types::TranscriptMessageIdentity::default(),
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
        assert!(state.contains_revision(&replaced_digest));
        validate_transcript_history_state(&state).expect("history state remains valid");
    }

    #[test]
    fn append_system_message_preserves_exact_prefix_without_rewriting_history() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent = session.transcript_revision().expect("parent revision");
        let rewrite = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent),
            )
            .expect("rewrite should commit");
        let graph_before = session
            .validated_transcript_history_state()
            .expect("history validation")
            .expect("audited graph");
        let messages_before = session.messages().to_vec();

        session.append_system_message("durable system prompt".to_string());

        assert_eq!(
            &session.messages()[..messages_before.len()],
            messages_before.as_slice(),
            "setting a System prompt must preserve every existing message as an exact prefix"
        );
        assert!(matches!(
            session.messages().last(),
            Some(Message::System(system)) if system.content == "durable system prompt"
        ));
        let head = session
            .transcript_revision()
            .expect("live system prompt digest");
        assert_ne!(head, rewrite.revision);
        assert_eq!(
            head,
            transcript_messages_digest(session.messages()).expect("current digest")
        );
        let graph_after = session
            .validated_transcript_history_state()
            .expect("history validation")
            .expect("audited graph");
        assert!(
            graph_before.shares_exact_state_with(&graph_after),
            "mechanical prompt mutation must preserve the exact audited graph authority"
        );
        assert!(
            session
                .transcript_revision_messages(&head)
                .expect("history state should decode")
                .is_none(),
            "live message digests are not retained graph revisions"
        );
        let state = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        assert_eq!(state.head(), rewrite.revision);
        validate_transcript_history_state(&state).expect("audited graph remains valid");
    }

    #[test]
    fn apply_transcript_history_state_uses_latest_commit_time_for_restored_head() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let original_messages = session.messages().to_vec();
        let parent = session.transcript_revision().expect("parent revision");
        let compact = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(parent.clone()),
            )
            .expect("rewrite should commit");

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
            .materialize_revision(&restore.revision)
            .expect("restored body should be retained")
            .created_at;
        assert_eq!(
            restored_body_created_at, restore.committed_at,
            "restoring a repeated revision selects its latest occurrence timestamp"
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
    fn validated_bridge_parent_materialization_preserves_its_selected_head() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("question".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let first_parent = session.transcript_revision().expect("first parent");
        let _first = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact answer".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(first_parent),
            )
            .expect("first rewrite should commit");

        session.push(Message::User(UserMessage::text("follow up".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose follow-up".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let bridge_messages = session.messages().to_vec();
        let bridge_revision = session.transcript_revision().expect("bridge revision");

        let second = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 3, end: 4 },
                vec![Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "compact follow-up".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    identity: crate::types::TranscriptMessageIdentity::default(),
                    created_at: crate::types::message_timestamp_now(),
                })],
                TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                Some(bridge_revision.clone()),
            )
            .expect("second rewrite should commit");
        assert_ne!(second.revision, bridge_revision);

        let full = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        assert_eq!(full.head(), second.revision);
        let bridge_body = ValidatedTranscriptHistory::seal_owned(full)
            .expect("full graph should seal")
            .materialize_rewrite_parent(&second)
            .expect("the exact rewrite occurrence must materialize its bridge parent");
        assert_eq!(
            bridge_body.revision, bridge_revision,
            "explicit parent materialization must preserve the selected bridge revision"
        );
        assert_eq!(
            serde_json::to_value(&bridge_body.messages).expect("projection serializes"),
            serde_json::to_value(&bridge_messages).expect("bridge serializes")
        );
    }

    #[test]
    fn exact_rewrite_occurrence_projection_orders_digest_recurrence() {
        let message_a = Message::User(UserMessage::text("A".to_string()));
        let message_b = Message::User(UserMessage::text("B".to_string()));
        let mut session = Session::new();
        session.push(message_a.clone());
        let revision_a = session.transcript_revision().expect("A revision");

        let first_b = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![message_b.clone()],
                TranscriptRewriteReason::new("A-to-B"),
                Some("unit-test".to_string()),
                Some(revision_a.clone()),
            )
            .expect("first B occurrence should commit");
        let back_to_a = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![message_a.clone()],
                TranscriptRewriteReason::new("B-to-A"),
                Some("unit-test".to_string()),
                Some(first_b.revision.clone()),
            )
            .expect("second A occurrence should commit");
        let second_b = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![message_b.clone()],
                TranscriptRewriteReason::new("A-to-B-again"),
                Some("unit-test".to_string()),
                Some(back_to_a.revision.clone()),
            )
            .expect("second B occurrence should commit");

        assert_eq!(back_to_a.revision, revision_a);
        assert_eq!(second_b.revision, first_b.revision);
        let graph = session
            .transcript_history_state()
            .expect("history state should decode")
            .expect("history state should exist");
        let sealed =
            ValidatedTranscriptHistory::seal_owned(graph).expect("recurrence graph should seal");

        for (generation, commit, parent_message, revision_message) in [
            (1_u64, &first_b, &message_a, &message_b),
            (2_u64, &back_to_a, &message_b, &message_a),
            (3_u64, &second_b, &message_a, &message_b),
        ] {
            assert_eq!(commit.rewrite_generation, generation);

            let before = sealed
                .materialize_rewrite_parent(commit)
                .expect("exact parent occurrence should materialize");
            assert_eq!(before.messages, std::slice::from_ref(parent_message));

            let mut after = Session::new();
            after
                .apply_validated_transcript_history_state(
                    sealed
                        .project_at_rewrite_commit(commit)
                        .expect("exact rewrite occurrence should project"),
                )
                .expect("exact rewrite occurrence should materialize");
            assert_eq!(after.messages(), std::slice::from_ref(revision_message));
            let after_graph = after
                .transcript_history_state()
                .expect("rewrite graph should decode")
                .expect("rewrite graph should exist");
            assert_eq!(
                after_graph.commit_count(),
                usize::try_from(generation).expect("test generation fits usize")
            );
            assert_eq!(
                after_graph.last_commit(),
                Some(commit),
                "the projection must end at this occurrence, not a later equal digest"
            );
        }

        let latest_b = sealed
            .project_at_revision(&first_b.revision)
            .expect("content lookup should remain available");
        assert_eq!(
            latest_b.commit_count(),
            3,
            "digest-only lookup intentionally selects the latest matching occurrence"
        );
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
        let registry = crate::ModelRegistry::from_config(
            &crate::Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .unwrap();
        let current = SessionLlmIdentity {
            model: "test-anthropic-default".to_string(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(crate::AuthBindingRef {
                realm: crate::RealmId::parse("tenant_a").unwrap(),
                binding: crate::BindingId::parse("anthropic_default").unwrap(),
                profile: None,
                origin: crate::BindingOrigin::Configured,
            }),
        };

        let resolved = resolve_session_llm_identity_override(
            &current,
            &registry,
            SessionLlmIdentityOverride {
                model: Some("test-openai-default"),
                provider: None,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        )
        .unwrap();

        assert_eq!(resolved.model, "test-openai-default");
        assert_eq!(resolved.provider, Provider::OpenAI);
        assert!(
            resolved.auth_binding.is_none(),
            "provider switches must not inherit a binding from the previous provider"
        );
    }

    #[test]
    fn llm_identity_model_override_keeps_uncatalogued_model_on_current_provider() {
        let registry = crate::ModelRegistry::from_config(
            &crate::Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .unwrap();
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
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        )
        .unwrap();

        assert_eq!(resolved.model, "uncatalogued-custom-model");
        assert_eq!(resolved.provider, Provider::Anthropic);
    }

    fn self_hosted_registry_with_shared_remote_model() -> crate::ModelRegistry {
        use crate::config::{
            SelfHostedApiStyle, SelfHostedModelConfig, SelfHostedServerConfig, SelfHostedTransport,
        };
        use crate::model_profile::catalog::ModelTier;

        let mut config = crate::Config::default();
        for server_id in ["local-a", "local-b"] {
            config.self_hosted.servers.insert(
                server_id.to_string(),
                SelfHostedServerConfig {
                    transport: SelfHostedTransport::OpenAiCompatible,
                    base_url: format!("http://{server_id}.test"),
                    api_style: SelfHostedApiStyle::Responses,
                },
            );
            config.self_hosted.models.insert(
                format!("shared-local-{server_id}"),
                SelfHostedModelConfig {
                    server: server_id.to_string(),
                    remote_model: "shared-local-model".to_string(),
                    display_name: "Shared local model".to_string(),
                    family: "shared-local".to_string(),
                    tier: ModelTier::Supported,
                    ..Default::default()
                },
            );
        }
        config.self_hosted.default_model = Some("shared-local-local-a".to_string());
        crate::ModelRegistry::from_config(
            &config,
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .expect("shared local registry")
    }

    #[test]
    fn llm_identity_override_preserves_exact_self_hosted_server_route() {
        let registry = self_hosted_registry_with_shared_remote_model();
        let current = SessionLlmIdentity {
            model: "shared-local-local-a".to_string(),
            provider: Provider::SelfHosted,
            self_hosted_server_id: Some("local-a".to_string()),
            provider_params: None,
            auth_binding: None,
        };

        let resolved = resolve_session_llm_identity_override(
            &current,
            &registry,
            SessionLlmIdentityOverride {
                model: Some("shared-local-local-b"),
                provider: Some(Provider::SelfHosted),
                self_hosted_server_id: Some("local-b"),
                provider_params: None,
                auth_binding: None,
            },
        )
        .expect("exact configured local route should resolve");

        assert_eq!(resolved.model, "shared-local-local-b");
        assert_eq!(resolved.provider, Provider::SelfHosted);
        assert_eq!(resolved.self_hosted_server_id.as_deref(), Some("local-b"));
    }

    #[test]
    fn llm_identity_override_rejects_self_hosted_server_model_mismatch() {
        let registry = self_hosted_registry_with_shared_remote_model();
        let current = SessionLlmIdentity {
            model: "shared-local-local-a".to_string(),
            provider: Provider::SelfHosted,
            self_hosted_server_id: Some("local-a".to_string()),
            provider_params: None,
            auth_binding: None,
        };

        let error = resolve_session_llm_identity_override(
            &current,
            &registry,
            SessionLlmIdentityOverride {
                model: Some("shared-local-local-b"),
                provider: Some(Provider::SelfHosted),
                self_hosted_server_id: Some("local-a"),
                provider_params: None,
                auth_binding: None,
            },
        )
        .expect_err("server id must match the requested model alias route");

        assert!(matches!(
            error,
            SessionLlmIdentityOverrideError::SelfHostedServerMismatch {
                requested,
                configured,
                ..
            } if requested == "local-a" && configured == "local-b"
        ));
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

    #[test]
    fn realtime_legacy_inline_activation_is_failure_atomic_and_preserves_whole_blob() {
        let mut malformed = Session::new();
        malformed.set_metadata_unchecked_for_test(
            SESSION_REALTIME_TRANSCRIPT_STATE_KEY,
            serde_json::json!("not-a-realtime-state"),
        );
        let pristine_prefix = malformed
            .realtime_component_event_prefix()
            .expect("pristine realtime prefix");
        assert!(matches!(
            malformed.activate_realtime_component_sidecar(),
            Err(RealtimeTranscriptSidecarError::Serialization(_))
        ));
        assert_eq!(
            malformed
                .metadata()
                .get(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
            Some(&serde_json::json!("not-a-realtime-state")),
            "failed activation must leave the exact legacy value in place"
        );
        assert_eq!(
            malformed
                .realtime_component_event_prefix()
                .expect("unchanged realtime prefix"),
            pristine_prefix,
            "failed activation must not advance component authority"
        );

        let mut session = Session::new();
        let state = SessionRealtimeTranscriptState::default();
        let inline = serde_json::to_value(&state).expect("inline projection");
        session
            .set_metadata_unchecked_for_test(SESSION_REALTIME_TRANSCRIPT_STATE_KEY, inline.clone());
        session
            .activate_realtime_component_sidecar()
            .expect("supported inline activation");
        assert!(
            !session
                .metadata()
                .contains_key(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
            "successful activation removes raw shadow authority"
        );
        let suffix = session
            .prepare_realtime_component_event_suffix()
            .expect("prepare activation suffix")
            .expect("SnapshotV1 suffix");
        assert_eq!(suffix.events().len(), 1);
        assert!(matches!(
            suffix.events()[0]
                .decode_payload::<crate::RealtimeTranscriptSidecarRecord>(
                    crate::REALTIME_TRANSCRIPT_SIDECAR_EVENT_SCHEMA_V1
                )
                .expect("decode activation record"),
            crate::RealtimeTranscriptSidecarRecord::SnapshotV1 { .. }
        ));

        let whole_blob =
            serde_json::to_value(&session).expect("WholeBlob projection after activation");
        assert_eq!(
            whole_blob
                .get("metadata")
                .and_then(serde_json::Value::as_object)
                .and_then(|metadata| metadata.get(SESSION_REALTIME_TRANSCRIPT_STATE_KEY)),
            Some(&inline),
            "activation changes storage authority, not the WholeBlob projection"
        );
    }

    #[test]
    fn realtime_user_image_materializes_once_and_unblocks_causal_assistant() {
        let mut session = Session::new();
        let image_data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".to_string();
        let image = RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key: "image-request-1".to_string(),
            item_id: "item_image".to_string(),
            previous_item_id: None,
            content_index: 0,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: crate::types::ImageData::Inline {
                    data: image_data.clone(),
                },
            }],
        };

        assert!(
            !append_staged_user_image(&mut session, &image).is_inert(),
            "first image final must materialize canonical user content"
        );
        let replay = session
            .preflight_realtime_user_content_event(&image)
            .expect("exact retry should preflight as committed");
        assert!(matches!(
            replay,
            crate::RealtimeUserContentApplyOutcome::AlreadyCommitted(_)
        ));

        assert!(
            !session
                .metadata()
                .contains_key(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
            "ordinary operation must keep the full realtime projection out of raw metadata"
        );
        let whole_blob =
            serde_json::to_value(&session).expect("WholeBlob projection should serialize");
        let staged_state = whole_blob
            .get("metadata")
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get(SESSION_REALTIME_TRANSCRIPT_STATE_KEY))
            .expect("WholeBlob compatibility projection must include realtime state");
        assert!(
            !staged_state.to_string().contains(&image_data),
            "materialized image bytes must not remain duplicated in transcript metadata"
        );

        assert!(
            session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTextDelta {
                    response_id: "resp_image".to_string(),
                    delta_id: "delta_image".to_string(),
                    item_id: "item_assistant".to_string(),
                    previous_item_id: Some("item_image".to_string()),
                    content_index: 0,
                    delta: "I see red.".to_string(),
                })
                .is_inert()
        );
        assert!(
            !session
                .append_realtime_transcript_event(RealtimeTranscriptEvent::AssistantTurnCompleted {
                    response_id: "resp_image".to_string(),
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                },)
                .is_inert(),
            "materialized image predecessor must unblock the assistant response"
        );

        assert_eq!(session.messages().len(), 2);
        assert!(matches!(
            &session.messages()[0],
            Message::User(user)
                if matches!(
                    user.content.as_slice(),
                    [ContentBlock::Image {
                        media_type,
                        data: crate::types::ImageData::Blob { blob_id },
                    }] if media_type == "image/png"
                        && blob_id == &crate::blob::content_blob_id("image/png", &image_data)
                )
        ));
        assert!(matches!(
            &session.messages()[1],
            Message::BlockAssistant(assistant) if block_assistant_text(assistant) == "I see red."
        ));
    }

    #[test]
    fn realtime_user_image_identity_is_durable_canonical_and_conflict_safe() {
        let mut session = Session::new();
        let data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".to_string();
        let initial = RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key: "stable-image-key".to_string(),
            item_id: "canonical-image-item".to_string(),
            previous_item_id: None,
            content_index: 0,
            content: vec![ContentBlock::Image {
                media_type: " image/PNG; charset=binary ".to_string(),
                data: crate::types::ImageData::Inline { data: data.clone() },
            }],
        };
        let committed = append_staged_user_image(&mut session, &initial);
        let Some(crate::RealtimeUserContentApplyOutcome::Committed(identity)) =
            committed.user_content
        else {
            panic!("first image must commit its durable identity");
        };
        assert_eq!(identity.item_id, "canonical-image-item");
        assert_eq!(identity.media_type, "image/png");

        let encoded = serde_json::to_string(&session).expect("session should serialize");
        let restored: Session =
            serde_json::from_str(&encoded).expect("committed identity should restore");

        let replay_event = RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key: "stable-image-key".to_string(),
            item_id: "ignored-retry-item".to_string(),
            previous_item_id: None,
            content_index: 0,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: crate::types::ImageData::Inline { data: data.clone() },
            }],
        };
        let replay = restored
            .preflight_realtime_user_content_event(&replay_event)
            .expect("exact retry should preflight");
        assert!(matches!(
            replay,
            crate::RealtimeUserContentApplyOutcome::AlreadyCommitted(
                crate::RealtimeUserContentIdentity { ref item_id, .. }
            ) if item_id == "canonical-image-item"
        ));

        let conflict = restored
            .preflight_realtime_user_content_event(&RealtimeTranscriptEvent::UserContentFinal {
                idempotency_key: "stable-image-key".to_string(),
                item_id: "conflicting-item".to_string(),
                previous_item_id: None,
                content_index: 0,
                content: vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: crate::types::ImageData::Inline {
                        data: "different-payload".to_string(),
                    },
                }],
            })
            .expect("conflicting retry should preflight");
        assert!(matches!(
            conflict,
            crate::RealtimeUserContentApplyOutcome::RejectedConflict { .. }
        ));

        let item_collision = restored
            .preflight_realtime_user_content_event(&RealtimeTranscriptEvent::UserContentFinal {
                idempotency_key: "another-key".to_string(),
                item_id: "canonical-image-item".to_string(),
                previous_item_id: None,
                content_index: 0,
                content: vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: crate::types::ImageData::Inline { data },
                }],
            })
            .expect("item collision should preflight");
        assert!(matches!(
            item_collision,
            crate::RealtimeUserContentApplyOutcome::RejectedConflict { .. }
        ));
        assert_eq!(restored.messages().len(), 1);
        serde_json::to_string(&restored).expect("rejections must not corrupt durable state");
    }

    #[test]
    fn realtime_user_image_reducer_never_receipts_without_pending_blob_proof() {
        for data in [
            crate::types::ImageData::Inline {
                data: "iVBORw0KGgo=".to_string(),
            },
            crate::types::ImageData::Blob {
                blob_id: crate::blob::content_blob_id("image/png", "iVBORw0KGgo="),
            },
        ] {
            let mut session = Session::new();
            let outcome = session.append_realtime_transcript_event(
                RealtimeTranscriptEvent::UserContentFinal {
                    idempotency_key: "unstaged-image-key".to_string(),
                    item_id: "unstaged-image-item".to_string(),
                    previous_item_id: None,
                    content_index: 0,
                    content: vec![ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data,
                    }],
                },
            );
            assert!(matches!(
                outcome.user_content,
                Some(crate::RealtimeUserContentApplyOutcome::RejectedInvalidIdentity { .. })
            ));
            assert!(session.messages().is_empty());
            assert!(session.realtime_user_content_identities().is_empty());
        }
    }

    #[test]
    fn realtime_user_image_pending_slot_is_generated_bounded_and_recovery_typed() {
        use crate::generated::session_document::{
            RealtimeUserContentBlobRecoveryDisposition, RealtimeUserContentBlobStageDisposition,
        };
        let mut session = Session::new();
        let pending = crate::PendingRealtimeUserContentBlob {
            idempotency_key: "pending-key-a".to_string(),
            item_id: "pending-item-a".to_string(),
            previous_item_id: None,
            content_index: 0,
            blob_id: crate::blob::content_blob_id("image/png", "iVBORw0KGgo="),
            media_type: "image/png".to_string(),
        };
        let different = crate::PendingRealtimeUserContentBlob {
            idempotency_key: "pending-key-b".to_string(),
            item_id: "pending-item-b".to_string(),
            previous_item_id: None,
            content_index: 0,
            blob_id: crate::blob::content_blob_id("image/png", "iVBORw0KGgoB"),
            media_type: "image/png".to_string(),
        };
        assert_eq!(
            session
                .stage_pending_realtime_user_content_blob(pending.clone())
                .expect("empty slot stages"),
            RealtimeUserContentBlobStageDisposition::StageNew
        );
        assert_eq!(
            session
                .stage_pending_realtime_user_content_blob(pending.clone())
                .expect("exact stage retry is idempotent"),
            RealtimeUserContentBlobStageDisposition::ReuseExact
        );
        assert_eq!(
            session
                .stage_pending_realtime_user_content_blob(different.clone())
                .expect("occupied decision is typed"),
            RealtimeUserContentBlobStageDisposition::RejectOccupied
        );
        assert_eq!(
            session.pending_realtime_user_content_blob(),
            Some(pending.clone())
        );
        assert_eq!(
            session
                .resolve_pending_realtime_user_content_blob_recovery(Some(&pending), false)
                .expect("exact recovery decision"),
            RealtimeUserContentBlobRecoveryDisposition::RetryExact
        );
        assert_eq!(
            session
                .resolve_pending_realtime_user_content_blob_recovery(Some(&different), true)
                .expect("verified older recovery decision"),
            RealtimeUserContentBlobRecoveryDisposition::CommitVerifiedBeforeCurrent
        );
        assert_eq!(
            session
                .resolve_pending_realtime_user_content_blob_recovery(Some(&different), false)
                .expect("invalid older recovery decision"),
            RealtimeUserContentBlobRecoveryDisposition::ClearInvalidBeforeCurrent
        );
        session
            .clear_invalid_pending_realtime_user_content_blob(Some(&different))
            .expect("generated clear-invalid disposition authorizes clear");
        assert!(session.pending_realtime_user_content_blob().is_none());
    }

    #[test]
    fn transcript_rewrite_tombstones_removed_image_key_and_accepts_new_key() {
        let mut session = Session::new();
        let data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".to_string();
        let original = RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key: "removed-image-key".to_string(),
            item_id: "removed-image-item".to_string(),
            previous_item_id: None,
            content_index: 0,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: crate::types::ImageData::Inline { data: data.clone() },
            }],
        };
        assert!(matches!(
            append_staged_user_image(&mut session, &original).user_content,
            Some(crate::RealtimeUserContentApplyOutcome::Committed(_))
        ));

        let parent = session.transcript_revision().expect("parent revision");
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("image removed"))],
                TranscriptRewriteReason::new("remove-image"),
                None,
                Some(parent),
            )
            .expect("rewrite should tombstone removed image identity");

        assert!(session.realtime_user_content_identities().is_empty());
        assert_eq!(
            session.realtime_user_content_tombstones(),
            vec![crate::RealtimeUserContentTombstone {
                idempotency_key: "removed-image-key".to_string(),
            }]
        );
        assert!(matches!(
            session.preflight_realtime_user_content_event(&original),
            Some(crate::RealtimeUserContentApplyOutcome::RejectedConflict { .. })
        ));
        assert!(matches!(
            session
                .append_realtime_transcript_event(original)
                .user_content,
            Some(crate::RealtimeUserContentApplyOutcome::RejectedConflict { .. })
        ));
        assert_eq!(
            session.messages().len(),
            1,
            "stale retry emits no receipt content"
        );

        let new_image = RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key: "new-image-key".to_string(),
            item_id: "new-image-item".to_string(),
            previous_item_id: None,
            content_index: 0,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: crate::types::ImageData::Inline { data },
            }],
        };
        assert!(matches!(
            append_staged_user_image(&mut session, &new_image).user_content,
            Some(crate::RealtimeUserContentApplyOutcome::Committed(_))
        ));
        assert_eq!(session.messages().len(), 2);

        let restored: Session = serde_json::from_str(
            &serde_json::to_string(&session).expect("serialize rewritten session"),
        )
        .expect("cold restore rewritten session");
        assert_eq!(restored.realtime_user_content_identities().len(), 1);
        assert_eq!(restored.realtime_user_content_tombstones().len(), 1);
    }

    #[test]
    fn transcript_rewrite_retains_only_canonical_image_occurrence_for_exact_replay() {
        let mut session = Session::new();
        let data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".to_string();
        let original = RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key: "retained-image-key".to_string(),
            item_id: "retained-image-item".to_string(),
            previous_item_id: None,
            content_index: 0,
            content: vec![ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: crate::types::ImageData::Inline { data },
            }],
        };
        assert!(matches!(
            append_staged_user_image(&mut session, &original).user_content,
            Some(crate::RealtimeUserContentApplyOutcome::Committed(_))
        ));
        let retained_message = session.messages()[0].clone();
        let parent = session.transcript_revision().expect("parent revision");
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![
                    retained_message,
                    Message::User(UserMessage::text("new canonical neighbor")),
                ],
                TranscriptRewriteReason::new("retain-image"),
                None,
                Some(parent),
            )
            .expect("rewrite retaining exact inline image should reconcile");

        assert!(session.realtime_user_content_tombstones().is_empty());
        let replay = session
            .preflight_realtime_user_content_event(&original)
            .expect("retained image should preflight as exact replay");
        assert!(matches!(
            replay,
            crate::RealtimeUserContentApplyOutcome::AlreadyCommitted(_)
        ));
        assert_eq!(session.messages().len(), 2);
    }

    #[test]
    fn transcript_rewrite_rejects_atomically_while_image_blob_anchor_is_pending() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("before rewrite")));
        let pending = crate::PendingRealtimeUserContentBlob {
            idempotency_key: "pending-rewrite-key".to_string(),
            item_id: "pending-rewrite-item".to_string(),
            previous_item_id: None,
            content_index: 0,
            blob_id: crate::blob::content_blob_id("image/png", "pending-bytes"),
            media_type: "image/png".to_string(),
        };
        session
            .stage_pending_realtime_user_content_blob(pending.clone())
            .expect("stage durable pending anchor");
        let parent = session.transcript_revision().expect("parent revision");
        let error = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("after rewrite"))],
                TranscriptRewriteReason::new("blocked-pending-image"),
                None,
                Some(parent),
            )
            .expect_err("rewrite must not cross an unresolved image anchor");
        assert!(
            error
                .to_string()
                .contains("history_rewrite_pending_user_content_blob")
        );
        assert!(matches!(
            &session.messages()[0],
            Message::User(user) if user.text_content() == "before rewrite"
        ));
        assert_eq!(session.pending_realtime_user_content_blob(), Some(pending));
    }

    #[test]
    fn realtime_user_image_rejects_noncanonical_blob_and_multiblock_shape() {
        let mut session = Session::new();
        for (key, content) in [
            (
                "invalid-blob",
                vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: crate::types::ImageData::Blob {
                        blob_id: crate::BlobId::new("sha256:not-a-digest"),
                    },
                }],
            ),
            (
                "multi-block",
                vec![
                    ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: crate::types::ImageData::Inline {
                            data: "payload".to_string(),
                        },
                    },
                    ContentBlock::Text {
                        text: "smuggled".to_string(),
                    },
                ],
            ),
        ] {
            let outcome = session.append_realtime_transcript_event(
                RealtimeTranscriptEvent::UserContentFinal {
                    idempotency_key: key.to_string(),
                    item_id: format!("item-{key}"),
                    previous_item_id: None,
                    content_index: 0,
                    content,
                },
            );
            assert!(matches!(
                outcome.user_content,
                Some(crate::RealtimeUserContentApplyOutcome::RejectedInvalidIdentity { .. })
            ));
        }
        assert!(session.messages().is_empty());
        let encoded = serde_json::to_string(&session).expect("session should serialize");
        serde_json::from_str::<Session>(&encoded).expect("rejections must leave restorable state");
    }

    #[test]
    fn realtime_restore_rejects_malformed_causal_graphs_and_accepts_waiting_dag() {
        fn restore(
            items: serde_json::Value,
            first_seen_order: Vec<&str>,
        ) -> Result<
            crate::realtime_transcript_revision::SessionRealtimeTranscriptState,
            crate::realtime_transcript_revision::RealtimeTranscriptShellError,
        > {
            let state = serde_json::from_value(serde_json::json!({
                "items": items,
                "first_seen_order": first_seen_order,
            }))
            .expect("test state shape should deserialize");
            crate::realtime_transcript_revision::restore_realtime_transcript_state(state)
        }

        assert!(
            restore(
                serde_json::json!({
                    "child": { "role": "user", "previous_item_id": "missing" }
                }),
                vec!["child"],
            )
            .is_ok(),
            "an unmaterialized out-of-order item must survive cold restore until its predecessor arrives"
        );
        assert!(
            restore(
                serde_json::json!({
                    "child": {
                        "role": "user",
                        "previous_item_id": "missing",
                        "ready": true,
                        "materialized": true
                    }
                }),
                vec!["child"],
            )
            .is_err(),
            "a materialized item cannot reference a missing predecessor"
        );
        assert!(
            restore(
                serde_json::json!({
                    "self": { "role": "user", "previous_item_id": "self" }
                }),
                vec!["self"],
            )
            .is_err(),
            "self edge must fail cold restore"
        );
        assert!(
            restore(
                serde_json::json!({
                    "a": { "role": "user", "previous_item_id": "b" },
                    "b": { "role": "user", "previous_item_id": "a" }
                }),
                vec!["a", "b"],
            )
            .is_err(),
            "cycle must fail cold restore"
        );
        assert!(
            restore(
                serde_json::json!({
                    "root": { "role": "user" },
                    "materialized_child": {
                        "role": "user",
                        "previous_item_id": "root",
                        "ready": true,
                        "materialized": true
                    }
                }),
                vec!["root", "materialized_child"],
            )
            .is_err(),
            "materialized child cannot have unmaterialized ancestry"
        );
        assert!(
            restore(
                serde_json::json!({
                    "root": { "role": "user" },
                    "waiting_child": { "role": "user", "previous_item_id": "root" }
                }),
                vec!["waiting_child", "root"],
            )
            .is_ok(),
            "valid acyclic waiting graph should restore even when first-seen order is child-first"
        );
    }

    #[test]
    fn realtime_restore_handles_long_waiting_chain_with_bounded_graph_walk() {
        const ITEM_COUNT: usize = 4_096;
        let mut items = serde_json::Map::new();
        let mut order = Vec::with_capacity(ITEM_COUNT);
        for index in 0..ITEM_COUNT {
            let item_id = format!("item-{index:04}");
            let value = if index == 0 {
                serde_json::json!({ "role": "user" })
            } else {
                serde_json::json!({
                    "role": "user",
                    "previous_item_id": format!("item-{:04}", index - 1),
                })
            };
            order.push(item_id.clone());
            items.insert(item_id, value);
        }
        let state = serde_json::from_value(serde_json::json!({
            "items": items,
            "first_seen_order": order,
        }))
        .expect("long-chain fixture should deserialize");
        crate::realtime_transcript_revision::restore_realtime_transcript_state(state)
            .expect("long valid waiting DAG should restore in one bounded graph walk");
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
        assert!(Arc::ptr_eq(session.messages.arc(), forked.messages.arc()));
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
        assert!(Arc::ptr_eq(session.messages.arc(), forked.messages.arc()));

        // Push on original triggers CoW - original gets new Arc
        session.push(Message::User(UserMessage::text("Second".to_string())));

        // Now they should have different Arcs
        assert!(!Arc::ptr_eq(session.messages.arc(), forked.messages.arc()));
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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Hi!".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
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
        session.metadata.insert(
            crate::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY.to_string(),
            serde_json::json!([{"sealed_projection": "must-not-fork"}]),
        );
        assert!(
            !session
                .metadata()
                .contains_key(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
            "typed realtime authority must not leak into the raw metadata map"
        );
        assert_eq!(
            session
                .realtime_component_event_prefix()
                .expect("realtime component prefix")
                .event_count(),
            1,
            "test setup should park one typed realtime event"
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
            assert_eq!(
                forked
                    .realtime_component_event_prefix()
                    .expect("fork realtime component prefix")
                    .event_count(),
                0,
                "forked sessions must start a new empty realtime component lineage"
            );
            assert!(
                !forked
                    .metadata()
                    .contains_key(crate::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY),
                "forked sessions must not raw-copy compaction outbox authority"
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
    fn identical_metadata_projection_is_wire_idempotent() {
        let mut session = Session::new();
        session.set_metadata("key", serde_json::json!({ "value": 1 }));
        let updated_at = session.updated_at;
        let bytes = session
            .to_persisted_bytes()
            .expect("session bytes before identical projection");

        session.set_metadata("key", serde_json::json!({ "value": 1 }));
        session.remove_metadata("already_absent");

        assert_eq!(
            session.updated_at, updated_at,
            "an identical durable projection must not manufacture a content mutation"
        );
        assert_eq!(
            session
                .to_persisted_bytes()
                .expect("session bytes after identical projection"),
            bytes,
            "an identical durable projection must not rotate current Session bytes"
        );
    }

    #[test]
    fn session_metadata_realm_id_is_back_read_compatible_string() {
        // A typed realm_id serializes as a bare JSON string (byte-identical to
        // the prior Option<String> durable shape).
        let metadata = SessionMetadata {
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
            realm_id: Some(crate::RealmId::parse("env_default").unwrap()),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        };
        let value = serde_json::to_value(&metadata).unwrap();
        assert_eq!(
            value.get("realm_id"),
            Some(&serde_json::json!("env_default")),
            "typed realm_id must serialize as a bare slug string"
        );

        // A legacy persisted row stored realm_id as a JSON string; it must
        // deserialize into the typed RealmId (durable back-read).
        let legacy = serde_json::json!({
            "schema_version": SESSION_METADATA_SCHEMA_VERSION,
            "model": "test-model",
            "max_tokens": 1024,
            "structured_output_retries": 2,
            "provider": "other",
            "tooling": SessionTooling::default(),
            "keep_alive": false,
            "comms_name": null,
            "realm_id": "legacy_realm",
        });
        let restored: SessionMetadata = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            restored.realm_id.as_ref().map(crate::RealmId::as_str),
            Some("legacy_realm")
        );
    }

    /// Ask 6: `SessionTooling.tool_access_policy` is additive — a persisted
    /// row without the field back-reads as `None` (unrestricted), `None` is
    /// omitted on write (durable shape unchanged for ungated sessions), and a
    /// resolved policy round-trips intact.
    #[test]
    fn session_tooling_tool_access_policy_round_trip_and_absent_default() {
        // Absent field back-reads as None.
        let legacy = serde_json::json!({});
        let restored: SessionTooling = serde_json::from_value(legacy).unwrap();
        assert_eq!(restored.tool_access_policy, None);

        // None is omitted on write — ungated sessions keep their prior shape.
        let value = serde_json::to_value(SessionTooling::default()).unwrap();
        assert!(
            value.get("tool_access_policy").is_none(),
            "None policy must not serialize"
        );

        // A resolved policy round-trips intact.
        let tooling = SessionTooling {
            tool_access_policy: Some(crate::ops::ToolAccessPolicy::AllowList(
                ["read_file", "send_message"].into_iter().collect(),
            )),
            ..SessionTooling::default()
        };
        let value = serde_json::to_value(&tooling).unwrap();
        let restored: SessionTooling = serde_json::from_value(value).unwrap();
        assert_eq!(restored.tool_access_policy, tooling.tool_access_policy);
    }

    #[test]
    fn lifecycle_terminal_typed_round_trip() {
        let mut session = Session::new();
        assert_eq!(session.lifecycle_terminal(), None);

        session
            .set_lifecycle_terminal(SessionLifecycleTerminal::Archived)
            .expect("typed terminal write should serialize");
        assert_eq!(
            session.lifecycle_terminal(),
            Some(SessionLifecycleTerminal::Archived)
        );
        assert!(
            session
                .lifecycle_terminal()
                .is_some_and(SessionLifecycleTerminal::is_archived)
        );
        // Persisted JSON for the typed key is the snake_case variant string.
        assert_eq!(
            session
                .metadata()
                .get(SESSION_LIFECYCLE_TERMINAL_KEY)
                .unwrap(),
            &serde_json::json!("archived")
        );
    }

    #[test]
    fn recovered_head_adoption_keeps_archived_absorbing_from_either_copy() {
        let mut archived_recovery = Session::new();
        archived_recovery
            .set_lifecycle_terminal(SessionLifecycleTerminal::Archived)
            .expect("archive terminal serializes");
        let mut active_head = archived_recovery.clone();
        active_head
            .set_lifecycle_terminal(SessionLifecycleTerminal::Active)
            .expect("active terminal serializes");
        archived_recovery
            .adopt_recovered_head_state(&active_head)
            .expect("generated lifecycle merge resolves");
        assert_eq!(
            archived_recovery.lifecycle_terminal(),
            Some(SessionLifecycleTerminal::Archived),
            "a newer Active projection must not resurrect an Archived recovery base"
        );

        let mut active_recovery = Session::new();
        active_recovery
            .set_lifecycle_terminal(SessionLifecycleTerminal::Active)
            .expect("active terminal serializes");
        let mut archived_head = active_recovery.clone();
        archived_head
            .set_lifecycle_terminal(SessionLifecycleTerminal::Archived)
            .expect("archive terminal serializes");
        active_recovery
            .adopt_recovered_head_state(&archived_head)
            .expect("generated lifecycle merge resolves");
        assert_eq!(
            active_recovery.lifecycle_terminal(),
            Some(SessionLifecycleTerminal::Archived),
            "an Archived durable head must remain terminal after recovery adoption"
        );
    }

    #[test]
    fn lifecycle_terminal_key_rejects_raw_mutation() {
        let mut session = Session::new();
        assert!(
            session
                .try_set_metadata(
                    SESSION_LIFECYCLE_TERMINAL_KEY,
                    serde_json::json!("archived")
                )
                .is_err(),
            "the typed lifecycle-terminal key is reserved for session authority"
        );
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
                .try_set_metadata(SESSION_METADATA_KEY, serde_json::json!({}))
                .is_err()
        );
        assert!(
            session
                .try_set_metadata(SESSION_BUILD_STATE_KEY, serde_json::json!({}))
                .is_err()
        );
        assert!(
            session
                .try_set_metadata(
                    SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY,
                    serde_json::json!({
                        "occurrence_count": 0,
                        "digest": "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                    })
                )
                .is_err(),
            "raw metadata must not forge rewrite-prefix authority"
        );
        let compaction_intents_key = crate::memory::SESSION_COMPACTION_PROJECTION_INTENTS_KEY;
        let sealed_compaction_intents =
            serde_json::json!([{"sealed_projection": "typed-owner-only"}]);
        session.metadata.insert(
            compaction_intents_key.to_string(),
            sealed_compaction_intents.clone(),
        );
        assert!(
            session
                .try_set_metadata(compaction_intents_key, serde_json::json!([]))
                .is_err(),
            "raw metadata must not overwrite compaction outbox authority"
        );
        session.remove_metadata(compaction_intents_key);
        assert_eq!(
            session.metadata().get(compaction_intents_key),
            Some(&sealed_compaction_intents),
            "raw metadata removal must not erase compaction outbox authority"
        );
        let mut absent = Session::new();
        assert!(
            !absent.backfill_metadata_if_absent(
                compaction_intents_key,
                serde_json::json!([{"forged_projection": true}])
            ),
            "compatibility backfill must not fabricate compaction outbox authority"
        );
        assert!(!absent.metadata().contains_key(compaction_intents_key));
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
                mob_member_binding: None,
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
            !session
                .metadata()
                .contains_key(SESSION_REALTIME_TRANSCRIPT_STATE_KEY),
            "typed realtime transcript append must not recreate raw shadow authority"
        );
        assert_eq!(
            session
                .realtime_component_event_prefix()
                .expect("typed realtime prefix")
                .event_count(),
            1,
            "typed append must advance the authenticated component prefix"
        );
        session.metadata.insert(
            SESSION_REALTIME_TRANSCRIPT_STATE_KEY.to_string(),
            serde_json::json!("not-a-state"),
        );
        let whole_blob =
            serde_json::to_value(&session).expect("typed projection must override a raw shadow");
        let projected = whole_blob
            .get("metadata")
            .and_then(serde_json::Value::as_object)
            .and_then(|metadata| metadata.get(SESSION_REALTIME_TRANSCRIPT_STATE_KEY))
            .expect("WholeBlob projection");
        assert!(
            serde_json::from_value::<SessionRealtimeTranscriptState>(projected.clone()).is_ok(),
            "WholeBlob encoding must derive from typed authority, never a raw metadata shadow"
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
        // The build-state-persist admission decision now lives in the canonical
        // SessionDocumentMachine durable-config region (LUC-524); the rejection
        // surfaces with that machine's authority wording.
        assert!(
            err.to_string()
                .contains("generated session document authority rejected"),
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
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Hi!".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
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
    fn deferred_tool_result_redelivery_is_idempotent_per_exact_payload() {
        let mut state = SessionDeferredTurnState::default();
        let results = vec![
            ToolResult::new("callback-a".to_string(), "a".to_string(), false),
            ToolResult::new("callback-b".to_string(), "b".to_string(), false),
        ];
        assert_eq!(
            state.stage_tool_results(results.clone(), SystemTime::UNIX_EPOCH),
            2
        );
        let before = serde_json::to_value(&state).expect("serialize staged state");

        assert_eq!(
            state.stage_tool_results(
                results,
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1),
            ),
            0,
            "identical redelivery must coalesce without restaging"
        );
        assert_eq!(
            serde_json::to_value(&state).expect("serialize redelivered state"),
            before,
            "duplicate ingress must be a byte-identical no-op"
        );
    }

    #[test]
    fn deferred_tool_result_conflict_and_wrong_id_fail_without_poison_after_replay() {
        let mut state = SessionDeferredTurnState::default();
        state
            .try_stage_tool_results(
                vec![ToolResult::new(
                    "callback-a".to_string(),
                    "approved".to_string(),
                    false,
                )],
                SystemTime::UNIX_EPOCH,
            )
            .expect("first callback payload should stage");
        let mut replayed: SessionDeferredTurnState = serde_json::from_value(
            serde_json::to_value(&state).expect("serialize deferred callback state"),
        )
        .expect("restore deferred callback state");
        let before = serde_json::to_value(&replayed).expect("serialize replayed state");

        assert!(matches!(
            replayed.try_stage_tool_results(
                vec![ToolResult::new(
                    "callback-a".to_string(),
                    "denied".to_string(),
                    false,
                )],
                SystemTime::UNIX_EPOCH,
            ),
            Err(DeferredToolResultsIngressError::ConflictingRedelivery(id))
                if id == "callback-a"
        ));
        assert_eq!(serde_json::to_value(&replayed).unwrap(), before);

        assert!(matches!(
            replayed.try_stage_tool_results(
                vec![ToolResult::new(
                    "callback-b".to_string(),
                    "wrong".to_string(),
                    false,
                )],
                SystemTime::UNIX_EPOCH,
            ),
            Err(DeferredToolResultsIngressError::WrongToolUseId(id))
                if id == "callback-b"
        ));
        assert_eq!(
            serde_json::to_value(&replayed).unwrap(),
            before,
            "typed ingress refusals must leave the valid pending continuation intact"
        );
    }

    #[test]
    fn persisted_round_trip_preserves_multiple_systems_anywhere_exactly() {
        let mut session = Session::new();
        session.append_system_message("first");
        session.push(Message::User(UserMessage::text("hello")));
        session.append_system_message(" second ");
        session.append_system_message("");
        session.append_system_message(" second ");
        let expected = session.messages().to_vec();
        let bytes = serde_json::to_vec(&session).expect("serialize session");
        let resumed: Session = serde_json::from_slice(&bytes).expect("deserialize session");
        assert_eq!(resumed.messages(), expected.as_slice());
        assert_eq!(resumed.messages_for_model_boundary(), expected);
    }
    #[test]
    fn system_control_idempotency_is_explicit_and_does_not_coalesce_keyless_rows() {
        let mut session = Session::new();
        let timestamp = crate::types::message_timestamp_now();
        let first = session
            .append_system_message_idempotent(
                " exact ",
                Some("host".to_string()),
                Some("key".to_string()),
                timestamp,
            )
            .expect("first append");
        assert_eq!(first, crate::service::AppendSystemContextStatus::Applied);
        let duplicate = session
            .append_system_message_idempotent(
                " exact ",
                Some("host".to_string()),
                Some("key".to_string()),
                timestamp,
            )
            .expect("exact retry");
        assert_eq!(
            duplicate,
            crate::service::AppendSystemContextStatus::Duplicate
        );
        session
            .append_system_message_idempotent("", None, None, timestamp)
            .expect("empty keyless System");
        session
            .append_system_message_idempotent("", None, None, timestamp)
            .expect("duplicate keyless System");
        assert_eq!(
            session
                .messages()
                .iter()
                .filter(|message| matches!(message, Message::System(system) if system.content.is_empty()))
                .count(),
            2
        );
        assert!(matches!(
            session.append_system_message_idempotent(
                "different",
                Some("host".to_string()),
                Some("key".to_string()),
                timestamp,
            ),
            Err(SystemMessageAppendError::Conflict { .. })
        ));
    }
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

    fn metadata_seam_session_metadata() -> SessionMetadata {
        SessionMetadata {
            schema_version: SESSION_METADATA_SCHEMA_VERSION,
            model: "test-model".to_string(),
            max_tokens: 1024,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: SessionTooling::default(),
            keep_alive: false,
            comms_name: Some("team/reviewer/alice".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: Some(crate::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "reviewer".to_string(),
                member: "alice".to_string(),
            }),
        }
    }

    /// Lockstep pin: the metadata-only partial decode must read the exact
    /// envelope that `SessionSerde` writes. If a field rename or serde-shape
    /// change lands on the full envelope without the partial decoder
    /// following, this test fails.
    #[test]
    fn session_metadata_document_lockstep_with_full_envelope() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));
        session
            .set_session_metadata(metadata_seam_session_metadata())
            .expect("session metadata should persist");
        session
            .set_lifecycle_terminal(SessionLifecycleTerminal::Archived)
            .expect("lifecycle terminal should persist");

        let bytes = serde_json::to_vec(&session).expect("session should serialize");
        let document = session_metadata_document_from_slice(&bytes)
            .expect("partial decode must accept the canonical envelope");

        assert_eq!(document.session_id(), session.id());
        assert_eq!(
            document.session_metadata_value(),
            session.metadata().get(SESSION_METADATA_KEY),
            "partial decode must project the identical raw session-metadata value"
        );
        assert_eq!(
            document.lifecycle_terminal_value(),
            session.metadata().get(SESSION_LIFECYCLE_TERMINAL_KEY),
            "partial decode must project the identical raw lifecycle-terminal value"
        );

        let view = document
            .try_into_view()
            .expect("typed view must decode from the partial document");
        let full_view =
            PersistedSessionMetadataView::try_from_session(&session).expect("full-session view");
        assert_eq!(view.session_id, full_view.session_id);
        assert_eq!(
            view.session_metadata.as_ref().map(|m| m.model.clone()),
            full_view.session_metadata.as_ref().map(|m| m.model.clone())
        );
        assert_eq!(
            view.mob_member_binding(),
            full_view.mob_member_binding(),
            "typed binding must be identical across the two decode paths"
        );
        assert_eq!(
            view.lifecycle_terminal,
            Some(SessionLifecycleTerminal::Archived)
        );
        assert_eq!(
            full_view.lifecycle_terminal,
            Some(SessionLifecycleTerminal::Archived)
        );
    }

    /// The metadata-only partial decode fails closed on an unsupported
    /// envelope version — same contract as the full deserializer.
    #[test]
    fn session_metadata_document_fails_closed_on_envelope_version() {
        let session = Session::new();
        let mut value = serde_json::to_value(&session).expect("session should serialize");
        value["version"] = serde_json::json!(SESSION_VERSION + 999);
        let bytes = serde_json::to_vec(&value).expect("mangled envelope should serialize");

        session_metadata_document_from_slice(&bytes)
            .expect_err("an unsupported envelope version must fail the partial decode closed");
    }

    #[test]
    fn current_session_deserializer_rejects_released_envelope_version() {
        let session = Session::new();
        let mut value = serde_json::to_value(&session).expect("session should serialize");
        value["version"] = serde_json::json!(2);
        let bytes = serde_json::to_vec(&value).expect("released envelope should serialize");

        Session::from_persisted_bytes(&bytes)
            .expect_err("ordinary Session decode must accept only current envelope v3");
    }

    /// Corrupt values under either reserved key are a read FAULT for the
    /// metadata view — never coalesced into "absent".
    #[test]
    fn persisted_session_metadata_view_fails_closed_on_corrupt_values() {
        let session_id = SessionId::new();

        let mut corrupt_metadata = serde_json::Map::new();
        corrupt_metadata.insert(SESSION_METADATA_KEY.to_string(), serde_json::json!(42));
        PersistedSessionMetadataView::try_from_metadata_map(session_id.clone(), &corrupt_metadata)
            .expect_err("corrupt session_metadata must fail the view decode closed");

        let mut corrupt_terminal = serde_json::Map::new();
        corrupt_terminal.insert(
            SESSION_LIFECYCLE_TERMINAL_KEY.to_string(),
            serde_json::json!("definitely-not-a-terminal"),
        );
        PersistedSessionMetadataView::try_from_metadata_map(session_id, &corrupt_terminal)
            .expect_err("corrupt lifecycle terminal must fail the view decode closed");
    }

    /// Absent reserved keys decode as typed absence through the view.
    #[test]
    fn persisted_session_metadata_view_reads_absent_facts_as_none() {
        let view = PersistedSessionMetadataView::try_from_metadata_map(
            SessionId::new(),
            &serde_json::Map::new(),
        )
        .expect("empty metadata map must decode");
        assert!(view.session_metadata.is_none());
        assert!(view.lifecycle_terminal.is_none());
        assert!(view.mob_member_binding().is_none());
    }
}

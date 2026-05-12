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
use crate::peer_meta::PeerMeta;
use crate::realtime_transcript::{
    RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent, RealtimeTranscriptMaterializedMessage,
    RealtimeTranscriptRole, SESSION_REALTIME_TRANSCRIPT_STATE_KEY, TranscriptLane,
};
use crate::service::{AppendSystemContextRequest, MobToolAuthorityContext};
use crate::time_compat::SystemTime;
use crate::tool_scope::ToolFilter;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ContentInput, Message, SessionId,
    StopReason, ToolDef, ToolProvenance, ToolResult, Usage, UserMessage,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
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
pub const SESSION_VERSION: u32 = 2;

/// Current `SessionMetadata` schema version. Distinct from `SESSION_VERSION`
/// so `SessionMetadata` can evolve independently of the Session envelope.
///
/// - v1 — pre-wave-c. Default on read for rows written before the byte
///   was introduced.
/// - v2 — wave-c C-3. Typed `AuthBindingRef` inner fields; any future
///   `SessionMetadata`-local shape change bumps this without moving
///   `SESSION_VERSION`.
pub const SESSION_METADATA_SCHEMA_VERSION: u32 = 2;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct SessionRealtimeTranscriptState {
    #[serde(default)]
    items: BTreeMap<String, RealtimeTranscriptItemState>,
    #[serde(default)]
    first_seen_order: Vec<String>,
    #[serde(default)]
    seen_delta_ids: BTreeSet<String>,
    #[serde(default)]
    assistant_completions: BTreeMap<String, RealtimeAssistantCompletion>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    discarded_assistant_response_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RealtimeTranscriptItemState {
    role: RealtimeTranscriptRole,
    #[serde(default)]
    previous_item_id: Option<String>,
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    content_segments: BTreeMap<u32, String>,
    #[serde(default)]
    skipped: bool,
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    materialized: bool,
    /// T9/T10: output lane this assistant item carries. `Display` is the
    /// default (matches all pre-T10 sessions on disk); promoted to `Spoken`
    /// the first time an [`RealtimeTranscriptEvent::AssistantTranscriptDelta`]
    /// fragment arrives for the item. User-role items always carry
    /// `Display` (the field is unused for user transcripts).
    #[serde(default)]
    lane: TranscriptLane,
}

impl RealtimeTranscriptItemState {
    fn new(
        role: RealtimeTranscriptRole,
        previous_item_id: Option<String>,
        response_id: Option<String>,
    ) -> Self {
        Self {
            role,
            previous_item_id,
            response_id,
            content_segments: BTreeMap::new(),
            skipped: false,
            ready: false,
            materialized: false,
            lane: TranscriptLane::Display,
        }
    }

    fn skipped(previous_item_id: Option<String>) -> Self {
        Self {
            role: RealtimeTranscriptRole::Assistant,
            previous_item_id,
            response_id: None,
            content_segments: BTreeMap::new(),
            skipped: true,
            ready: true,
            materialized: false,
            lane: TranscriptLane::Display,
        }
    }

    fn text(&self) -> String {
        self.content_segments.values().cloned().collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct RealtimeAssistantCompletion {
    stop_reason: StopReason,
    usage: Usage,
    usage_consumed: bool,
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
        Ok(Session {
            version: serde_repr.version,
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
    SESSION_VERSION
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

/// Durable control state for runtime system-context append requests.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SessionSystemContextState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PendingSystemContextAppend>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied: Vec<PendingSystemContextAppend>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub seen: std::collections::BTreeMap<String, SeenSystemContextKey>,
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
    pub accepted_at: SystemTime,
}

/// Durable control state for deferred first-turn prompt and staged callback tool results.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct SessionDeferredTurnState {
    #[serde(default, skip_serializing_if = "DeferredFirstTurnPhase::is_inactive")]
    pub first_turn_phase: DeferredFirstTurnPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_initial_prompt: Option<PendingDeferredPrompt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_tool_results: Vec<PendingToolResultsMessage>,
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

/// Seen idempotency-key entry for system-context append requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SeenSystemContextKey {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
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
    /// Stage an append request, enforcing per-session idempotency.
    pub fn stage_append(
        &mut self,
        req: &AppendSystemContextRequest,
        accepted_at: SystemTime,
    ) -> Result<crate::service::AppendSystemContextStatus, SystemContextStageError> {
        let text = req.text.trim();
        if text.is_empty() {
            return Err(SystemContextStageError::InvalidRequest(
                "system context text must not be empty".to_string(),
            ));
        }

        if let Some(key) = req.idempotency_key.as_ref() {
            match self.seen.get(key) {
                Some(existing)
                    if existing.text == text
                        && existing.source.as_deref() == req.source.as_deref() =>
                {
                    return Ok(crate::service::AppendSystemContextStatus::Duplicate);
                }
                Some(existing) => {
                    return Err(SystemContextStageError::Conflict {
                        key: key.clone(),
                        existing_text: existing.text.clone(),
                        existing_source: existing.source.clone(),
                    });
                }
                None => {}
            }
        }

        let append = PendingSystemContextAppend {
            text: text.to_string(),
            source: req.source.clone(),
            idempotency_key: req.idempotency_key.clone(),
            accepted_at,
        };
        if let Some(key) = req.idempotency_key.as_ref() {
            self.seen.insert(
                key.clone(),
                SeenSystemContextKey {
                    text: append.text.clone(),
                    source: append.source.clone(),
                    state: SeenSystemContextState::Pending,
                },
            );
        }
        self.pending.push(append);
        Ok(crate::service::AppendSystemContextStatus::Staged)
    }

    /// Mark all currently-pending appends as applied and clear the pending queue.
    pub fn mark_pending_applied(&mut self) {
        for pending in &self.pending {
            if !self.applied.contains(pending) {
                self.applied.push(pending.clone());
            }
        }
        for pending in &self.pending {
            if let Some(key) = pending.idempotency_key.as_ref()
                && let Some(seen) = self.seen.get_mut(key)
            {
                seen.state = SeenSystemContextState::Applied;
            }
        }
        self.pending.clear();
    }
}

impl SessionDeferredTurnState {
    /// Mark that this session has a deferred first turn waiting to start.
    pub fn mark_initial_turn_pending(&mut self) {
        self.first_turn_phase = DeferredFirstTurnPhase::Pending;
    }

    /// Mark the deferred first turn as started.
    ///
    /// Returns true when the phase transitioned from `Pending`.
    pub fn mark_initial_turn_started(&mut self) -> bool {
        let was_pending = matches!(self.first_turn_phase, DeferredFirstTurnPhase::Pending);
        if was_pending {
            self.first_turn_phase = DeferredFirstTurnPhase::Consumed;
        }
        was_pending
    }

    /// Restore the deferred first-turn pending phase after a failed pre-run setup.
    pub fn restore_initial_turn_pending(&mut self) {
        self.first_turn_phase = DeferredFirstTurnPhase::Pending;
    }

    /// Whether build-only first-turn overrides are still legal for this session.
    pub fn allows_initial_turn_overrides(&self) -> bool {
        matches!(self.first_turn_phase, DeferredFirstTurnPhase::Pending)
    }

    /// Stage the create-time prompt for a later first turn.
    pub fn stage_initial_prompt(&mut self, prompt: ContentInput, accepted_at: SystemTime) {
        if !prompt.has_images() && prompt.text_content().trim().is_empty() {
            self.pending_initial_prompt = None;
            return;
        }

        self.pending_initial_prompt = Some(PendingDeferredPrompt {
            prompt,
            accepted_at,
        });
    }

    /// Stage one callback tool-results message for the next turn.
    pub fn stage_tool_results(
        &mut self,
        results: Vec<ToolResult>,
        accepted_at: SystemTime,
    ) -> usize {
        if results.is_empty() {
            return 0;
        }

        let accepted = results.len();
        self.pending_tool_results.push(PendingToolResultsMessage {
            results,
            accepted_at,
        });
        accepted
    }

    /// Consume the staged initial prompt, if any.
    pub fn take_initial_prompt(&mut self) -> Option<ContentInput> {
        self.pending_initial_prompt
            .take()
            .map(|pending| pending.prompt)
    }

    /// Consume all staged callback tool-results messages.
    pub fn take_tool_results(&mut self) -> Vec<PendingToolResultsMessage> {
        std::mem::take(&mut self.pending_tool_results)
    }

    /// Whether any callback tool results are currently staged.
    pub fn has_pending_tool_results(&self) -> bool {
        !self.pending_tool_results.is_empty()
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

fn render_system_context_block(append: &PendingSystemContextAppend) -> String {
    let mut rendered = String::from("[Runtime System Context]");
    if let Some(source) = &append.source {
        rendered.push_str("\nsource: ");
        rendered.push_str(source);
    }
    rendered.push_str("\n\n");
    rendered.push_str(&append.text);
    rendered
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

impl Session {
    /// Create a new empty session
    pub fn new() -> Self {
        let now = SystemTime::now();
        Self {
            version: SESSION_VERSION,
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

    /// Mutable access to the message buffer — *append-only witness escape hatch*.
    ///
    /// Intentionally `pub(crate)`: the only legitimate mutations of the
    /// message history within a single `SessionId` are `push` / `push_batch`
    /// (extend) and the two in-crate rewrite operations used by the agent
    /// loop (compaction-summary replacement, synthetic-notice stripping).
    /// Cross-crate consumers must route in-place content rewrites through
    /// the typed proxy [`Session::externalize_media`]; shrink or replace
    /// operations must go through [`Session::fork_at`] which rotates
    /// `SessionId` (F1/F7 closure from the state-scope audit).
    pub(crate) fn messages_mut_internal(&mut self) -> &mut Vec<Message> {
        Arc::make_mut(&mut self.messages)
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
        let messages = Arc::make_mut(&mut self.messages);
        crate::image_content::externalize_messages_from(blob_store, messages, start).await
    }

    /// Explicitly update the timestamp
    ///
    /// Call this after bulk operations that don't update timestamps automatically.
    pub fn touch(&mut self) {
        self.updated_at = SystemTime::now();
    }

    /// Whether the conversation has a pending turn boundary.
    ///
    /// Returns `true` if the last message is `User` or `ToolResults`, meaning
    /// the conversation is waiting for an assistant turn and `run_pending` can
    /// resume without a new user message.
    pub fn has_pending_boundary(&self) -> bool {
        self.messages
            .last()
            .is_some_and(|m| matches!(m, Message::User(_) | Message::ToolResults { .. }))
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
        match event {
            RealtimeTranscriptEvent::ItemObserved {
                item_id,
                previous_item_id,
                role,
                response_id,
            } => {
                let response_id = normalize_realtime_optional_response_id(response_id);
                if role == RealtimeTranscriptRole::Assistant
                    && response_id
                        .as_ref()
                        .is_some_and(|id| state.discarded_assistant_response_ids.contains(id))
                {
                    observe_realtime_skipped_item(&mut state, item_id, previous_item_id);
                } else {
                    observe_realtime_item(&mut state, item_id, previous_item_id, role, response_id);
                }
            }
            RealtimeTranscriptEvent::ItemSkipped {
                item_id,
                previous_item_id,
            } => {
                observe_realtime_skipped_item(&mut state, item_id, previous_item_id);
            }
            RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id,
                previous_item_id,
                content_index,
                text,
            } => {
                if let Some(item) = observe_realtime_item(
                    &mut state,
                    item_id,
                    previous_item_id,
                    RealtimeTranscriptRole::User,
                    None,
                ) {
                    let segment = item.content_segments.entry(content_index).or_default();
                    if segment.is_empty() && !text.is_empty() {
                        *segment = text;
                    } else if !text.is_empty() && segment.as_str() != text {
                        tracing::warn!(
                            content_index,
                            "ignoring conflicting realtime user transcript segment replay"
                        );
                    }
                    item.ready = true;
                }
            }
            RealtimeTranscriptEvent::AssistantTextDelta {
                response_id,
                delta_id,
                item_id,
                previous_item_id,
                content_index,
                delta,
            } => {
                let Some(response_id) = normalize_realtime_response_id(response_id) else {
                    return RealtimeTranscriptApplyOutcome::default();
                };
                if state
                    .discarded_assistant_response_ids
                    .contains(&response_id)
                {
                    observe_realtime_skipped_item(&mut state, item_id, previous_item_id);
                    let outcome = self.materialize_realtime_transcript_ready_items(&mut state);
                    self.store_realtime_transcript_state(&state);
                    return outcome;
                }
                if !delta_id.trim().is_empty() && !state.seen_delta_ids.insert(delta_id) {
                    return RealtimeTranscriptApplyOutcome::default();
                }
                let response_completed = state.assistant_completions.contains_key(&response_id);
                if let Some(item) = observe_realtime_item(
                    &mut state,
                    item_id,
                    previous_item_id,
                    RealtimeTranscriptRole::Assistant,
                    Some(response_id),
                ) {
                    if promote_item_lane(item, TranscriptLane::Display) {
                        item.content_segments
                            .entry(content_index)
                            .or_default()
                            .push_str(&delta);
                        if response_completed && !item.text().is_empty() {
                            item.ready = true;
                        }
                    } else {
                        // R5-6 sibling: this delta was routed at a
                        // Spoken-classified item (e.g. truncation arrived
                        // first and locked the lane). `promote_item_lane`
                        // already warned about the lane conflict; drop the
                        // delta to preserve the lane invariant rather than
                        // clobbering Spoken content with Display text.
                        tracing::warn!(
                            "AssistantTextDelta routed to a Spoken-lane item; dropping delta to preserve lane invariant — this indicates a provider lane-classification bug"
                        );
                    }
                }
            }
            RealtimeTranscriptEvent::AssistantTranscriptDelta {
                response_id,
                delta_id,
                item_id,
                previous_item_id,
                content_index,
                delta,
            } => {
                // T9/T10: spoken-transcript lane. Identical staging shape to
                // `AssistantTextDelta` (same idempotency / ordering / dedup
                // logic owns both lanes); the only difference is that the
                // owning item is tagged `Spoken` so the materializer flushes
                // `AssistantBlock::Transcript` instead of `AssistantBlock::Text`.
                let Some(response_id) = normalize_realtime_response_id(response_id) else {
                    return RealtimeTranscriptApplyOutcome::default();
                };
                if state
                    .discarded_assistant_response_ids
                    .contains(&response_id)
                {
                    observe_realtime_skipped_item(&mut state, item_id, previous_item_id);
                    let outcome = self.materialize_realtime_transcript_ready_items(&mut state);
                    self.store_realtime_transcript_state(&state);
                    return outcome;
                }
                if !delta_id.trim().is_empty() && !state.seen_delta_ids.insert(delta_id) {
                    return RealtimeTranscriptApplyOutcome::default();
                }
                let response_completed = state.assistant_completions.contains_key(&response_id);
                if let Some(item) = observe_realtime_item(
                    &mut state,
                    item_id,
                    previous_item_id,
                    RealtimeTranscriptRole::Assistant,
                    Some(response_id),
                ) {
                    if promote_item_lane(item, TranscriptLane::Spoken) {
                        item.content_segments
                            .entry(content_index)
                            .or_default()
                            .push_str(&delta);
                        if response_completed && !item.text().is_empty() {
                            item.ready = true;
                        }
                    } else {
                        // R5-6 sibling: this transcript delta arrived for a
                        // Display-classified item (a Display delta or other
                        // Display-lane event landed first). `promote_item_lane`
                        // already warned; drop the delta rather than appending
                        // Spoken text into a Display-locked content_segment.
                        tracing::warn!(
                            "AssistantTranscriptDelta routed to a Display-lane item; dropping delta to preserve lane invariant — this indicates a provider lane-classification bug"
                        );
                    }
                }
            }
            RealtimeTranscriptEvent::AssistantTranscriptTruncated {
                response_id,
                item_id,
                content_index,
                text,
            } => {
                let Some(response_id) = normalize_realtime_response_id(response_id) else {
                    return RealtimeTranscriptApplyOutcome::default();
                };
                if state
                    .discarded_assistant_response_ids
                    .contains(&response_id)
                {
                    observe_realtime_skipped_item(&mut state, item_id, None);
                    let outcome = self.materialize_realtime_transcript_ready_items(&mut state);
                    self.store_realtime_transcript_state(&state);
                    return outcome;
                }
                let response_completed = state.assistant_completions.contains_key(&response_id);
                let item_id_for_log = item_id.clone();
                let response_id_for_log = response_id.clone();
                if let Some(item) = observe_realtime_item(
                    &mut state,
                    item_id,
                    None,
                    RealtimeTranscriptRole::Assistant,
                    Some(response_id),
                ) {
                    // R5-7-sibling: a late truncation arriving after the
                    // canonical message has already committed cannot mutate
                    // history (append-only invariant). Same shape as the
                    // late-FinalText guard above — warn and skip.
                    if item.materialized {
                        tracing::warn!(
                            target: "meerkat::session",
                            item_id = %item_id_for_log,
                            response_id = %response_id_for_log,
                            "AssistantTranscriptTruncated arrived after item already materialized; canonical message is locked, late truncation dropped",
                        );
                    } else if promote_item_lane(item, TranscriptLane::Spoken) {
                        // R5-6: truncation is a Spoken-lane-only semantic —
                        // it describes the audio output that was actually
                        // heard before barge-in cut it short. Promote to
                        // Spoken so the materializer commits as
                        // `AssistantBlock::Transcript`. If a Display delta
                        // arrived first, `promote_item_lane` keeps the
                        // existing lane and returns `false` — that's a
                        // provider bug; warn already emitted inside
                        // `promote_item_lane`.
                        item.content_segments.insert(content_index, text);
                        if response_completed && !item.text().is_empty() {
                            item.ready = true;
                        }
                    }
                }
            }
            RealtimeTranscriptEvent::AssistantTranscriptFinalText {
                response_id,
                item_id,
                content_index,
                text,
            } => {
                // R5-7: authoritative final text overrides any incomplete
                // delta accumulation for this `(response_id, item_id,
                // content_index)`. If no item is staged yet (final-only
                // provider, or all deltas dropped), create one on the
                // Spoken lane so the materializer flushes it as
                // `AssistantBlock::Transcript`. Flush gating is unchanged:
                // `AssistantTurnCompleted` still drives readiness.
                let Some(response_id) = normalize_realtime_response_id(response_id) else {
                    return RealtimeTranscriptApplyOutcome::default();
                };
                if state
                    .discarded_assistant_response_ids
                    .contains(&response_id)
                {
                    observe_realtime_skipped_item(&mut state, item_id, None);
                    let outcome = self.materialize_realtime_transcript_ready_items(&mut state);
                    self.store_realtime_transcript_state(&state);
                    return outcome;
                }
                let response_completed = state.assistant_completions.contains_key(&response_id);
                if let Some(item) = observe_realtime_item(
                    &mut state,
                    item_id,
                    None,
                    RealtimeTranscriptRole::Assistant,
                    Some(response_id),
                ) {
                    // R5-7: late `AssistantTranscriptFinalText` after the
                    // item has already been committed to canonical history.
                    // The staged item is `materialized = true` and the
                    // session's `Message::BlockAssistant` already carries
                    // the delta-accumulated text. Append-only history is a
                    // stronger invariant than typed text repair: rewriting
                    // the canonical message would violate it (consumers may
                    // already have observed the prior text via the event
                    // stream / projector). Warn-and-skip; the SDK or human
                    // can decide whether the provider-side ordering bug
                    // matters.
                    if item.materialized {
                        tracing::warn!(
                            "AssistantTranscriptFinalText arrived after item already materialized; canonical message is locked, late repair dropped"
                        );
                        self.store_realtime_transcript_state(&state);
                        return RealtimeTranscriptApplyOutcome::default();
                    }
                    // Spoken lane: this variant is the authoritative
                    // transcript-final text path. Display-text finals come
                    // through a different seam. If the item is locked to
                    // Display (e.g. an earlier `AssistantTextDelta` staged
                    // it), drop the final to preserve the lane invariant —
                    // append-only history would otherwise silently switch
                    // block type at materialize time.
                    if promote_item_lane(item, TranscriptLane::Spoken) {
                        // Replace, not append: the final's text is
                        // authoritative and supersedes any (possibly
                        // partial) accumulated segment text.
                        item.content_segments.insert(content_index, text);
                        if response_completed && !item.text().is_empty() {
                            item.ready = true;
                        }
                    } else {
                        tracing::warn!(
                            "AssistantTranscriptFinalText routed to a Display-lane item; dropping authoritative final to preserve lane invariant — this indicates a provider lane-classification bug"
                        );
                    }
                }
            }
            RealtimeTranscriptEvent::AssistantTurnCompleted {
                response_id,
                stop_reason,
                usage,
            } => {
                let Some(response_id) = normalize_realtime_response_id(response_id) else {
                    return RealtimeTranscriptApplyOutcome::default();
                };
                if state
                    .discarded_assistant_response_ids
                    .contains(&response_id)
                {
                    discard_realtime_assistant_response(&mut state, &response_id);
                    let outcome = self.materialize_realtime_transcript_ready_items(&mut state);
                    self.store_realtime_transcript_state(&state);
                    return outcome;
                }
                match stop_reason {
                    StopReason::Cancelled => {
                        discard_realtime_assistant_response(&mut state, &response_id);
                    }
                    StopReason::ToolUse => {
                        state.assistant_completions.remove(&response_id);
                    }
                    _ => {
                        state
                            .assistant_completions
                            .entry(response_id.clone())
                            .or_insert(RealtimeAssistantCompletion {
                                stop_reason,
                                usage,
                                usage_consumed: false,
                            });
                        mark_realtime_assistant_response_ready(&mut state, &response_id);
                    }
                }
            }
            RealtimeTranscriptEvent::AssistantTurnInterrupted { response_id } => {
                let Some(response_id) = normalize_realtime_response_id(response_id) else {
                    return RealtimeTranscriptApplyOutcome::default();
                };
                // R5-5: barge-in invalidates the spoken/audio lane (the user
                // is speaking over what they heard) but preserves the
                // Display lane (sideband display text from the same response
                // is not "spoken over"). The interrupt is also terminal for
                // the response on the realtime-staging path: any later
                // `AssistantTurnCompleted` arrives with `StopReason::Cancelled`
                // and short-circuits via the discarded-set guard. Therefore
                // we must materialize retained Display items immediately here
                // by inserting a synthetic `assistant_completions` entry —
                // otherwise they would never commit.
                discard_realtime_assistant_response_by_lane(&mut state, &response_id);
                state
                    .assistant_completions
                    .entry(response_id.clone())
                    .or_insert(RealtimeAssistantCompletion {
                        stop_reason: StopReason::Cancelled,
                        usage: Usage::default(),
                        usage_consumed: false,
                    });
                mark_realtime_assistant_response_ready(&mut state, &response_id);
            }
        }

        let outcome = self.materialize_realtime_transcript_ready_items(&mut state);
        self.store_realtime_transcript_state(&state);
        outcome
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
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut out: Vec<String> = Vec::new();
        for item_id in &state.first_seen_order {
            let Some(item) = state.items.get(item_id) else {
                continue;
            };
            if item.role != RealtimeTranscriptRole::Assistant {
                continue;
            }
            if item.materialized || item.skipped {
                continue;
            }
            let Some(response_id) = item.response_id.as_ref() else {
                continue;
            };
            if state.discarded_assistant_response_ids.contains(response_id) {
                continue;
            }
            if seen.insert(response_id.clone()) {
                out.push(response_id.clone());
            }
        }
        out
    }

    fn realtime_transcript_state(&self) -> SessionRealtimeTranscriptState {
        self.metadata
            .get(SESSION_REALTIME_TRANSCRIPT_STATE_KEY)
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    fn store_realtime_transcript_state(&mut self, state: &SessionRealtimeTranscriptState) {
        match serde_json::to_value(state) {
            Ok(value) => self.set_metadata(SESSION_REALTIME_TRANSCRIPT_STATE_KEY, value),
            Err(error) => {
                tracing::warn!(error = %error, "failed to serialize realtime transcript state");
            }
        }
    }

    fn materialize_realtime_transcript_ready_items(
        &mut self,
        state: &mut SessionRealtimeTranscriptState,
    ) -> RealtimeTranscriptApplyOutcome {
        let mut materialized = Vec::new();

        // Round-4 CC7: when a single response_id produces both display-text
        // and spoken-transcript items (mixed-modality response), we emit a
        // SINGLE `Message::BlockAssistant` whose `blocks` interleave
        // `AssistantBlock::Text` and `AssistantBlock::Transcript` in
        // arrival order — not multiple messages. Pending state lives across
        // outer-loop batches: when chained items (`previous_item_id`) force
        // serial materialization, batch N may emit the display item and
        // batch N+1 may emit the spoken item under the same response_id —
        // we still want one combined message.
        //
        // The pending group is flushed when:
        //   - a User item lands (canonical-history ordering boundary)
        //   - an assistant item with a different response_id lands
        //   - the outer materialization loop terminates
        let mut pending_blocks: Vec<AssistantBlock> = Vec::new();
        let mut pending_response_id: Option<String> = None;
        let mut pending_stop_reason: StopReason = StopReason::EndTurn;
        let mut pending_usage: Usage = Usage::default();

        loop {
            let order = realtime_transcript_order(state);
            let mut skipped_batch = Vec::new();
            let mut batch = Vec::new();
            for item_id in order {
                let Some(item) = state.items.get(&item_id) else {
                    continue;
                };
                if item.materialized {
                    continue;
                }
                if !realtime_predecessor_materialized(state, item.previous_item_id.as_deref()) {
                    continue;
                }
                if item.skipped {
                    skipped_batch.push(item_id.clone());
                    continue;
                }
                if !item.ready {
                    continue;
                }
                let text = item.text();
                if text.is_empty() {
                    continue;
                }
                match item.role {
                    RealtimeTranscriptRole::User => {
                        batch.push(RealtimeTranscriptMaterializedMessage::User {
                            item_id: item_id.clone(),
                            text,
                        });
                    }
                    RealtimeTranscriptRole::Assistant => {
                        let Some(response_id) = item.response_id.as_ref() else {
                            continue;
                        };
                        let Some(completion) = state.assistant_completions.get(response_id) else {
                            continue;
                        };
                        let usage = if completion.usage_consumed {
                            Usage::default()
                        } else {
                            completion.usage.clone()
                        };
                        batch.push(RealtimeTranscriptMaterializedMessage::Assistant {
                            item_id: item_id.clone(),
                            response_id: response_id.clone(),
                            text,
                            stop_reason: completion.stop_reason,
                            usage,
                            lane: item.lane,
                        });
                    }
                }
            }
            if skipped_batch.is_empty() && batch.is_empty() {
                break;
            }
            for item_id in skipped_batch {
                if let Some(item) = state.items.get_mut(&item_id) {
                    item.materialized = true;
                }
            }

            for message in batch {
                match &message {
                    RealtimeTranscriptMaterializedMessage::User { item_id, text } => {
                        if !pending_blocks.is_empty() {
                            let drained = std::mem::take(&mut pending_blocks);
                            self.append_external_assistant_blocks(
                                drained,
                                pending_stop_reason,
                                std::mem::take(&mut pending_usage),
                            );
                            pending_response_id = None;
                        }
                        if let Some(item) = state.items.get_mut(item_id) {
                            item.materialized = true;
                        }
                        self.append_external_user_content(ContentInput::Text(text.clone()));
                    }
                    RealtimeTranscriptMaterializedMessage::Assistant {
                        item_id,
                        response_id,
                        text,
                        stop_reason,
                        usage,
                        lane,
                    } => {
                        // Flush if this assistant item belongs to a different
                        // response than the pending group. (Same response_id
                        // → accumulate; different → emit prior message.)
                        if pending_response_id
                            .as_ref()
                            .is_some_and(|existing| existing != response_id)
                            && !pending_blocks.is_empty()
                        {
                            let drained = std::mem::take(&mut pending_blocks);
                            self.append_external_assistant_blocks(
                                drained,
                                pending_stop_reason,
                                std::mem::take(&mut pending_usage),
                            );
                            pending_response_id = None;
                        }
                        if let Some(item) = state.items.get_mut(item_id) {
                            item.materialized = true;
                        }
                        if let Some(completion) = state.assistant_completions.get_mut(response_id) {
                            completion.usage_consumed = true;
                        }
                        // T9/T10: route to the correct canonical block by
                        // lane. `Display` keeps the legacy `AssistantBlock::Text`
                        // shape; `Spoken` flushes
                        // `AssistantBlock::Transcript { source: Spoken }` so
                        // OpenAI realtime audio transcripts stop being
                        // persisted as authored display text.
                        let block = match lane {
                            TranscriptLane::Display => AssistantBlock::Text {
                                text: text.clone(),
                                meta: None,
                            },
                            TranscriptLane::Spoken => AssistantBlock::Transcript {
                                text: text.clone(),
                                source: crate::types::TranscriptSource::Spoken,
                                meta: None,
                            },
                        };
                        // First item in a group seeds the response_id /
                        // stop_reason / usage. `usage_consumed` is flipped
                        // to true above as the first item is processed, so
                        // every subsequent item sees `Usage::default()` from
                        // the materializer's per-item builder — accumulating
                        // across items in one group is naturally
                        // single-counted.
                        if pending_response_id.is_none() {
                            pending_response_id = Some(response_id.clone());
                            pending_stop_reason = *stop_reason;
                            pending_usage = usage.clone();
                        }
                        pending_blocks.push(block);
                    }
                }
                materialized.push(message);
            }
        }

        // Final flush after the outer materialization loop has drained.
        if !pending_blocks.is_empty() {
            self.append_external_assistant_blocks(
                pending_blocks,
                pending_stop_reason,
                pending_usage,
            );
        }

        RealtimeTranscriptApplyOutcome {
            materialized_messages: materialized,
        }
    }

    /// Set a system prompt (adds or replaces System message at start)
    pub fn set_system_prompt(&mut self, prompt: String) {
        use crate::types::SystemMessage;

        let inner = Arc::make_mut(&mut self.messages);
        // Check if first message is system
        if let Some(Message::System(_)) = inner.first() {
            inner[0] = Message::System(SystemMessage::new(prompt));
        } else {
            inner.insert(0, Message::System(SystemMessage::new(prompt)));
        }
        self.updated_at = SystemTime::now();
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
        let mut state = self.system_context_state().unwrap_or_default();
        let mut state_dirty = false;
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
                    if !state
                        .applied
                        .iter()
                        .any(|applied| applied.idempotency_key.as_ref() == Some(key))
                    {
                        state.applied.push(append.clone());
                        state_dirty = true;
                    }
                    if state
                        .seen
                        .get(key)
                        .is_none_or(|seen| seen.state != SeenSystemContextState::Applied)
                    {
                        state.seen.insert(
                            key.clone(),
                            SeenSystemContextKey {
                                text: append.text.clone(),
                                source: append.source.clone(),
                                state: SeenSystemContextState::Applied,
                            },
                        );
                        state_dirty = true;
                    }
                    continue;
                }
            } else if new_appends.contains(append) || current_system_prompt.contains(&rendered) {
                continue;
            }
            new_appends.push(append.clone());
        }
        if new_appends.is_empty() {
            if state_dirty && let Err(err) = self.set_system_context_state(state) {
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
        self.set_system_prompt(next);

        for append in new_appends {
            if let Some(key) = append.idempotency_key.as_ref() {
                state.seen.insert(
                    key.clone(),
                    SeenSystemContextKey {
                        text: append.text.clone(),
                        source: append.source.clone(),
                        state: SeenSystemContextState::Applied,
                    },
                );
                if state
                    .applied
                    .iter()
                    .any(|applied| applied.idempotency_key.as_ref() == Some(key))
                {
                    continue;
                }
            } else if state.applied.contains(&append) {
                continue;
            }
            state.applied.push(append);
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

    /// Set a metadata value
    pub fn set_metadata(&mut self, key: &str, value: serde_json::Value) {
        self.metadata.insert(key.to_string(), value);
        self.updated_at = SystemTime::now();
    }

    /// Backfill a missing metadata value without changing `updated_at`.
    ///
    /// This is only for compatibility reads that need to hydrate metadata from
    /// an older projection. Semantic metadata mutations must use
    /// [`Session::set_metadata`] so the session timestamp advances.
    pub fn backfill_metadata_if_absent(&mut self, key: &str, value: serde_json::Value) -> bool {
        if self.metadata.contains_key(key) {
            false
        } else {
            self.metadata.insert(key.to_string(), value);
            true
        }
    }

    /// Remove a metadata value.
    pub fn remove_metadata(&mut self, key: &str) {
        self.metadata.remove(key);
        self.updated_at = SystemTime::now();
    }

    /// Store SessionMetadata in the session metadata map.
    pub fn set_session_metadata(
        &mut self,
        metadata: SessionMetadata,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(metadata)?;
        self.set_metadata(SESSION_METADATA_KEY, value);
        Ok(())
    }

    /// Load SessionMetadata from the session metadata map.
    pub fn session_metadata(&self) -> Option<SessionMetadata> {
        self.metadata
            .get(SESSION_METADATA_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    /// Store durable system-context control state in the session metadata map.
    pub fn set_system_context_state(
        &mut self,
        state: SessionSystemContextState,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(state)?;
        self.set_metadata(SESSION_SYSTEM_CONTEXT_STATE_KEY, value);
        Ok(())
    }

    /// Load durable system-context control state from the session metadata map.
    pub fn system_context_state(&self) -> Option<SessionSystemContextState> {
        self.metadata
            .get(SESSION_SYSTEM_CONTEXT_STATE_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    /// Store durable deferred-turn control state in the session metadata map.
    pub fn set_deferred_turn_state(
        &mut self,
        state: SessionDeferredTurnState,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(state)?;
        self.set_metadata(SESSION_DEFERRED_TURN_STATE_KEY, value);
        Ok(())
    }

    /// Load durable deferred-turn control state from the session metadata map.
    pub fn deferred_turn_state(&self) -> Option<SessionDeferredTurnState> {
        self.metadata
            .get(SESSION_DEFERRED_TURN_STATE_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    /// Store recoverable build-only session state in the session metadata map.
    pub fn set_build_state(&mut self, state: SessionBuildState) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(state)?;
        self.set_metadata(SESSION_BUILD_STATE_KEY, value);
        Ok(())
    }

    /// Load recoverable build-only session state from the session metadata map.
    pub fn build_state(&self) -> Option<SessionBuildState> {
        self.metadata
            .get(SESSION_BUILD_STATE_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    /// Store durable tool-visibility control state in the session metadata map.
    pub fn set_tool_visibility_state(
        &mut self,
        state: SessionToolVisibilityState,
    ) -> Result<(), serde_json::Error> {
        let value = serde_json::to_value(state)?;
        self.set_metadata(SESSION_TOOL_VISIBILITY_STATE_KEY, value);
        Ok(())
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

    /// Store typed mob operator authority inside canonical build-state metadata.
    pub fn set_mob_tool_authority_context(
        &mut self,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), serde_json::Error> {
        let mut build_state = self.build_state().unwrap_or_default();
        build_state.mob_tool_authority_context = authority_context;
        self.set_build_state(build_state)
    }

    /// Load typed mob operator authority from canonical build-state metadata.
    pub fn mob_tool_authority_context(&self) -> Option<MobToolAuthorityContext> {
        self.build_state()
            .and_then(|state| state.mob_tool_authority_context)
    }

    /// Fork the session at a specific message index
    ///
    /// Creates a new session with a subset of messages. The messages are copied
    /// (not shared) since the new session has a different prefix.
    pub fn fork_at(&self, index: usize) -> Self {
        let now = SystemTime::now();
        let truncated = self.messages[..index.min(self.messages.len())].to_vec();
        Self {
            version: SESSION_VERSION,
            id: SessionId::new(),
            messages: Arc::new(truncated),
            created_at: now,
            updated_at: now,
            metadata: self.metadata.clone(),
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
            version: SESSION_VERSION,
            id: SessionId::new(),
            messages: Arc::clone(&self.messages),
            created_at: now,
            updated_at: now,
            metadata: self.metadata.clone(),
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
    1
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

fn normalize_realtime_item_id(item_id: String) -> Option<String> {
    let trimmed = item_id.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalize_realtime_previous_item_id(previous_item_id: Option<String>) -> Option<String> {
    previous_item_id.and_then(normalize_realtime_item_id)
}

fn normalize_realtime_response_id(response_id: String) -> Option<String> {
    normalize_realtime_item_id(response_id)
}

fn normalize_realtime_optional_response_id(response_id: Option<String>) -> Option<String> {
    response_id.and_then(normalize_realtime_response_id)
}

/// T9/T10: tag the assistant item with its output lane (Display vs Spoken).
///
/// First lane wins: if a later delta tries to promote an item already
/// classified as the other lane, we keep the existing lane and emit a
/// `tracing::warn!`. The materializer can only flush one block-type per
/// item; mixed-lane content on the same `item_id` is not expected from
/// any provider and would be a provider-side bug. Items with empty
/// content (no deltas yet observed) accept any lane.
///
/// Returns `true` when the item now carries the requested `lane` (either
/// the lane already matched, or the item was empty so the lane was
/// promoted). Returns `false` when the item already carried the *other*
/// lane with staged content — in that case the caller MUST skip any
/// content insert it was about to perform on `lane`, otherwise it would
/// clobber the locked-in lane content (R5-6 sibling: silent-clobber bug).
#[must_use]
fn promote_item_lane(item: &mut RealtimeTranscriptItemState, lane: TranscriptLane) -> bool {
    if item.lane == lane {
        return true;
    }
    let has_content = item.content_segments.values().any(|s| !s.is_empty());
    if !has_content {
        // No content has been staged on the original lane yet; safe to
        // re-classify.
        item.lane = lane;
        return true;
    }
    tracing::warn!(
        existing_lane = ?item.lane,
        observed_lane = ?lane,
        "ignoring realtime transcript lane conflict on item with staged content"
    );
    false
}

fn observe_realtime_item(
    state: &mut SessionRealtimeTranscriptState,
    item_id: String,
    previous_item_id: Option<String>,
    role: RealtimeTranscriptRole,
    response_id: Option<String>,
) -> Option<&mut RealtimeTranscriptItemState> {
    let item_id = normalize_realtime_item_id(item_id)?;
    let previous_item_id = normalize_realtime_previous_item_id(previous_item_id);
    let response_id = normalize_realtime_optional_response_id(response_id);
    if !state
        .first_seen_order
        .iter()
        .any(|existing| existing == &item_id)
    {
        state.first_seen_order.push(item_id.clone());
    }
    let item = state.items.entry(item_id.clone()).or_insert_with(|| {
        RealtimeTranscriptItemState::new(role, previous_item_id.clone(), response_id.clone())
    });
    if item.skipped {
        if item.previous_item_id.is_none() && previous_item_id.is_some() {
            item.previous_item_id = previous_item_id;
        }
        tracing::warn!(
            item_id = %item_id,
            observed_role = ?role,
            "ignoring realtime transcript content for item already marked as a contentless causal anchor"
        );
        return None;
    }
    if item.role != role {
        tracing::warn!(
            item_id = %item_id,
            existing_role = ?item.role,
            observed_role = ?role,
            "ignoring realtime transcript item role conflict"
        );
        return None;
    }
    if item.previous_item_id.is_none() && previous_item_id.is_some() {
        item.previous_item_id = previous_item_id;
    }
    if let Some(response_id) = response_id {
        match item.response_id.as_ref() {
            Some(existing) if existing != &response_id => {
                tracing::warn!(
                    item_id = %item_id,
                    existing_response_id = %existing,
                    observed_response_id = %response_id,
                    "ignoring realtime transcript item response conflict"
                );
                return None;
            }
            Some(_) => {}
            None => item.response_id = Some(response_id),
        }
    }
    Some(item)
}

fn observe_realtime_skipped_item(
    state: &mut SessionRealtimeTranscriptState,
    item_id: String,
    previous_item_id: Option<String>,
) {
    let Some(item_id) = normalize_realtime_item_id(item_id) else {
        return;
    };
    let previous_item_id = normalize_realtime_previous_item_id(previous_item_id);
    if !state
        .first_seen_order
        .iter()
        .any(|existing| existing == &item_id)
    {
        state.first_seen_order.push(item_id.clone());
    }
    let item = state
        .items
        .entry(item_id)
        .or_insert_with(|| RealtimeTranscriptItemState::skipped(previous_item_id.clone()));
    if item.previous_item_id.is_none() && previous_item_id.is_some() {
        item.previous_item_id = previous_item_id;
    }
    if item.materialized || item.skipped {
        return;
    }
    if item.role != RealtimeTranscriptRole::Assistant {
        tracing::warn!(
            existing_role = ?item.role,
            "ignoring realtime skipped-item observation for non-assistant item"
        );
        return;
    }
    if !item.content_segments.is_empty() {
        tracing::warn!("ignoring realtime skipped-item observation for content-bearing item");
        return;
    }
    item.skipped = true;
    item.ready = true;
}

fn mark_realtime_assistant_response_ready(
    state: &mut SessionRealtimeTranscriptState,
    response_id: &str,
) {
    for item in state.items.values_mut() {
        if item.role == RealtimeTranscriptRole::Assistant
            && item.response_id.as_deref() == Some(response_id)
            && !item.materialized
            && !item.text().is_empty()
        {
            item.ready = true;
        }
    }
}

fn discard_realtime_assistant_response(
    state: &mut SessionRealtimeTranscriptState,
    response_id: &str,
) {
    state
        .discarded_assistant_response_ids
        .insert(response_id.to_string());
    for item in state.items.values_mut() {
        if item.role == RealtimeTranscriptRole::Assistant
            && item.response_id.as_deref() == Some(response_id)
            && !item.materialized
        {
            item.content_segments.clear();
            item.skipped = true;
            item.ready = true;
        }
    }
    state.assistant_completions.remove(response_id);
}

/// R5-5: lane-scoped discard for barge-in (`AssistantTurnInterrupted`).
///
/// Drops Spoken-lane staged items (the heard audio + transcript the user
/// spoke over) and preserves Display-lane items as committable content.
/// Items with empty content (no deltas observed yet, lane still defaulting
/// to `Display`) are also discarded — they carry no preserveable Display
/// text. The caller is responsible for inserting a synthetic
/// `assistant_completions` entry so the materializer can flush retained
/// Display items immediately.
///
/// Unlike [`discard_realtime_assistant_response`], this helper does NOT
/// remove the response from `assistant_completions`: barge-in must seed a
/// completion entry so retained Display items can materialize before any
/// late `AssistantTurnCompleted` (which would short-circuit on the
/// discarded-set membership) arrives.
fn discard_realtime_assistant_response_by_lane(
    state: &mut SessionRealtimeTranscriptState,
    response_id: &str,
) {
    state
        .discarded_assistant_response_ids
        .insert(response_id.to_string());
    for item in state.items.values_mut() {
        if item.role != RealtimeTranscriptRole::Assistant
            || item.response_id.as_deref() != Some(response_id)
            || item.materialized
        {
            continue;
        }
        let has_content = item.content_segments.values().any(|s| !s.is_empty());
        if item.lane == TranscriptLane::Display && has_content {
            // Retain the Display content; the caller's synthetic
            // completion entry + `mark_realtime_assistant_response_ready`
            // will flag it ready for the next materializer pass.
            continue;
        }
        // Spoken (or empty Display, which carries no preserveable text):
        // drop content and mark as a contentless predecessor so chained
        // items downstream of it can still materialize.
        item.content_segments.clear();
        item.skipped = true;
        item.ready = true;
    }
}

fn realtime_transcript_order(state: &SessionRealtimeTranscriptState) -> Vec<String> {
    let mut roots = Vec::new();
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item_id in &state.first_seen_order {
        let Some(item) = state.items.get(item_id) else {
            continue;
        };
        if let Some(previous) = item.previous_item_id.as_ref()
            && state.items.contains_key(previous)
        {
            children
                .entry(previous.clone())
                .or_default()
                .push(item_id.clone());
        } else {
            roots.push(item_id.clone());
        }
    }
    roots.sort_by_key(|item_id| realtime_first_seen_index(state, item_id));
    for child_ids in children.values_mut() {
        child_ids.sort_by_key(|item_id| realtime_first_seen_index(state, item_id));
    }

    let mut ordered = Vec::new();
    let mut visited = BTreeSet::new();
    for root in roots {
        visit_realtime_transcript_item(&root, &children, &mut visited, &mut ordered);
    }
    for item_id in &state.first_seen_order {
        visit_realtime_transcript_item(item_id, &children, &mut visited, &mut ordered);
    }
    ordered
}

fn realtime_first_seen_index(state: &SessionRealtimeTranscriptState, item_id: &str) -> usize {
    state
        .first_seen_order
        .iter()
        .position(|existing| existing == item_id)
        .unwrap_or(usize::MAX)
}

fn visit_realtime_transcript_item(
    item_id: &str,
    children: &BTreeMap<String, Vec<String>>,
    visited: &mut BTreeSet<String>,
    ordered: &mut Vec<String>,
) {
    if !visited.insert(item_id.to_string()) {
        return;
    }
    ordered.push(item_id.to_string());
    if let Some(child_ids) = children.get(item_id) {
        for child_id in child_ids {
            visit_realtime_transcript_item(child_id, children, visited, ordered);
        }
    }
}

fn realtime_predecessor_materialized(
    state: &SessionRealtimeTranscriptState,
    previous_item_id: Option<&str>,
) -> bool {
    match previous_item_id {
        None => true,
        Some(previous_item_id) => state
            .items
            .get(previous_item_id)
            .is_some_and(|item| item.materialized),
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
    use crate::types::{
        AssistantMessage, BlockAssistantMessage, StopReason, SystemMessage, UserMessage,
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
    fn test_session_new() {
        let session = Session::new();
        assert_eq!(session.version(), SESSION_VERSION);
        assert!(session.messages().is_empty());
        assert!(session.created_at() <= session.updated_at());
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
    fn test_session_mob_tool_authority_context_roundtrip() {
        let mut session = Session::new();
        let authority = MobToolAuthorityContext::new(
            crate::service::OpaquePrincipalToken::new("opaque-principal"),
            false,
        )
        .with_managed_mob_scope(["mob-a"])
        .with_audit_invocation_id("audit-1");

        session
            .set_mob_tool_authority_context(Some(authority.clone()))
            .expect("authority should serialize");
        assert_eq!(session.mob_tool_authority_context(), Some(authority));

        session
            .set_mob_tool_authority_context(None)
            .expect("authority should clear");
        assert!(session.mob_tool_authority_context().is_none());
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
            .set_tool_visibility_state(state.clone())
            .expect("tool visibility state should serialize");
        assert_eq!(session.tool_visibility_state().unwrap(), Some(state));
    }

    #[test]
    fn test_session_tool_visibility_state_malformed_returns_error() {
        let mut session = Session::new();
        session.set_metadata(
            SESSION_TOOL_VISIBILITY_STATE_KEY,
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
    fn has_pending_boundary_empty_session() {
        let session = Session::new();
        assert!(!session.has_pending_boundary());
    }

    #[test]
    fn has_pending_boundary_after_user_message() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello")));
        assert!(session.has_pending_boundary());
    }

    #[test]
    fn has_pending_boundary_after_assistant_message() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello")));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![],
            StopReason::EndTurn,
        )));
        assert!(!session.has_pending_boundary());
    }

    #[test]
    fn has_pending_boundary_after_tool_results() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello")));
        session.push(Message::tool_results(vec![]));
        assert!(session.has_pending_boundary());
    }

    #[test]
    fn has_pending_boundary_after_system() {
        let mut session = Session::new();
        session.push(Message::System(SystemMessage::new("system")));
        assert!(!session.has_pending_boundary());
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
    fn append_system_context_blocks_records_typed_applied_context() {
        let append = PendingSystemContextAppend {
            text: "Authoritative peer token is birch seventeen.".to_string(),
            source: Some(
                "peer_response_terminal:analyst:018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string(),
            ),
            idempotency_key: Some("018f6f79-7a82-7c4e-a552-a3b86f9630f1".to_string()),
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

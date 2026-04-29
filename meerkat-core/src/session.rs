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
use crate::service::{AppendSystemContextRequest, MobToolAuthorityContext};
use crate::time_compat::SystemTime;
use crate::tool_scope::ToolFilter;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, ContentInput, Message, SessionId, StopReason, ToolDef,
    ToolProvenance, ToolResult, Usage, UserMessage,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Current session format version.
///
/// Version history:
/// - v1 — pre-wave-c. `SessionMetadata.connection_ref` inner fields were
///   untyped strings (`realm_id`, `binding_id`, `profile`); no per-entity
///   schema version byte on `SessionMetadata`.
/// - v2 — wave-c C-3. `ConnectionRef` inner fields are typed
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
/// - v2 — wave-c C-3. Typed `ConnectionRef` inner fields; any future
///   `SessionMetadata`-local shape change bumps this without moving
///   `SESSION_VERSION`.
pub const SESSION_METADATA_SCHEMA_VERSION: u32 = 2;

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

        let rendered = appends
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

        let mut state = self.system_context_state().unwrap_or_default();
        for append in appends {
            if !state.applied.contains(append) {
                state.applied.push(append.clone());
            }
        }
        if let Err(err) = self.set_system_context_state(state) {
            tracing::warn!(error = %err, "failed to persist applied system-context state");
        }
    }

    /// Get the last assistant message text content.
    pub fn last_assistant_text(&self) -> Option<String> {
        self.messages.iter().rev().find_map(|m| match m {
            Message::BlockAssistant(a) => {
                let mut buf = String::new();
                for block in &a.blocks {
                    if let crate::types::AssistantBlock::Text { text, .. } = block {
                        buf.push_str(text);
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
    pub fn tool_visibility_state(&self) -> Option<SessionToolVisibilityState> {
        self.metadata
            .get(SESSION_TOOL_VISIBILITY_STATE_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
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
    /// Realm-scoped connection binding (Phase 3 provider-auth redesign).
    ///
    /// Persisted intent for the auth/backend binding this session resolved
    /// through. On resume, `apply_resumed_session_metadata` writes this
    /// back into `AgentBuildConfig.connection_ref` so the same realm
    /// binding is re-resolved. Never carries secret material — leases
    /// are rebuilt from the active realm connection set at resume time.
    /// Older persisted sessions without the field deserialize as `None`
    /// (backward compatible via `#[serde(default)]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<crate::ConnectionRef>,
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
    /// `SessionMetadata.connection_ref`; this field is the
    /// read/write projection used by hot-swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<crate::ConnectionRef>,
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
            connection_ref: self.connection_ref.clone(),
        }
    }

    /// Overwrite the durable LLM identity while preserving unrelated session metadata.
    pub fn apply_llm_identity(&mut self, identity: &SessionLlmIdentity) {
        self.model = identity.model.clone();
        self.provider = identity.provider;
        self.self_hosted_server_id = identity.self_hosted_server_id.clone();
        self.provider_params = identity.provider_params.clone();
        self.connection_ref = identity.connection_ref.clone();
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::{
        AssistantMessage, BlockAssistantMessage, StopReason, SystemMessage, UserMessage,
    };
    use std::sync::Arc;

    #[test]
    fn test_session_new() {
        let session = Session::new();
        assert_eq!(session.version(), SESSION_VERSION);
        assert!(session.messages().is_empty());
        assert!(session.created_at() <= session.updated_at());
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
        assert_eq!(session.tool_visibility_state(), Some(state));
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
                    source: Some("peer_response_terminal:analyst:req-123".to_string()),
                    idempotency_key: Some("req-123".to_string()),
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
            Some("peer_response_terminal:analyst:req-123")
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
            source: Some("peer_response_terminal:analyst:req-123".to_string()),
            idempotency_key: Some("req-123".to_string()),
            accepted_at: SystemTime::UNIX_EPOCH,
        };
        let mut session = Session::new();

        session.append_system_context_blocks(std::slice::from_ref(&append));

        let state = session
            .system_context_state()
            .expect("append should persist typed context state");
        assert_eq!(state.applied, vec![append]);
    }
}

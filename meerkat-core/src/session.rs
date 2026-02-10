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
use crate::types::{Message, SessionId, Usage};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Arc;
use std::time::SystemTime;

/// Current session format version
pub const SESSION_VERSION: u32 = 1;

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

    /// Get all messages
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get mutable access to messages (triggers CoW if Arc is shared)
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
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

    /// Set a system prompt (adds or replaces System message at start)
    pub fn set_system_prompt(&mut self, prompt: String) {
        use crate::types::SystemMessage;

        let inner = Arc::make_mut(&mut self.messages);
        // Check if first message is system
        if let Some(Message::System(_)) = inner.first() {
            inner[0] = Message::System(SystemMessage { content: prompt });
        } else {
            inner.insert(0, Message::System(SystemMessage { content: prompt }));
        }
        self.updated_at = SystemTime::now();
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
    pub model: String,
    pub max_tokens: u32,
    pub provider: Provider,
    pub tooling: SessionTooling,
    pub host_mode: bool,
    pub comms_name: Option<String>,
}

/// Key used to store SessionMetadata in Session metadata map.
pub const SESSION_METADATA_KEY: &str = "session_metadata";

/// Tooling flags captured at session creation time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub struct SessionTooling {
    pub builtins: bool,
    pub shell: bool,
    pub comms: bool,
    pub subagents: bool,
    /// Active skills at session creation time (for deterministic resume).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_skills: Option<Vec<crate::skills::SkillId>>,
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
    use crate::types::{AssistantMessage, StopReason, SystemMessage, UserMessage};
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
            session.push(Message::User(UserMessage {
                content: format!("Message {}", i),
            }));
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
            session.push(Message::User(UserMessage {
                content: format!("Message {}", i),
            }));
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
        session.push(Message::User(UserMessage {
            content: "First".to_string(),
        }));

        // Fork shares the Arc
        let forked = session.fork();
        assert!(Arc::ptr_eq(&session.messages, &forked.messages));

        // Push on original triggers CoW - original gets new Arc
        session.push(Message::User(UserMessage {
            content: "Second".to_string(),
        }));

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
            Message::User(UserMessage {
                content: "First".to_string(),
            }),
            Message::User(UserMessage {
                content: "Second".to_string(),
            }),
            Message::User(UserMessage {
                content: "Third".to_string(),
            }),
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

        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        assert_eq!(session.messages().len(), 1);
        assert!(session.updated_at() > initial_updated);
    }

    #[test]
    fn test_session_fork() {
        let mut session = Session::new();
        session.push(Message::System(SystemMessage {
            content: "System prompt".to_string(),
        }));
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));
        session.push(Message::Assistant(AssistantMessage {
            content: "Hi!".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
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
    fn test_session_serialization() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Test".to_string(),
        }));

        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id(), session.id());
        assert_eq!(parsed.messages().len(), 1);
        assert_eq!(parsed.version(), SESSION_VERSION);
    }

    #[test]
    fn test_session_meta_from_session() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));
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
}

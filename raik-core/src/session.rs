//! Session management for RAIK
//!
//! A session represents a conversation history that can be persisted and resumed.

use crate::types::{Message, SessionId, Usage};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Current session format version
pub const SESSION_VERSION: u32 = 1;

/// A conversation session with full history
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Session {
    /// Format version for migrations
    #[serde(default = "default_version")]
    version: u32,
    /// Unique identifier
    id: SessionId,
    /// All messages in order
    messages: Vec<Message>,
    /// When the session was created
    created_at: SystemTime,
    /// When the session was last updated
    updated_at: SystemTime,
    /// Arbitrary metadata
    #[serde(default)]
    metadata: serde_json::Map<String, serde_json::Value>,
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
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
            metadata: serde_json::Map::new(),
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

    /// Get mutable access to messages
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
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
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
        self.updated_at = SystemTime::now();
    }

    /// Get the last N messages
    pub fn last_n(&self, n: usize) -> &[Message] {
        let start = self.messages.len().saturating_sub(n);
        &self.messages[start..]
    }

    /// Count total tokens used (approximation from stored usage)
    pub fn total_tokens(&self) -> u64 {
        self.messages
            .iter()
            .filter_map(|m| match m {
                Message::Assistant(a) => Some(a.usage.total_tokens()),
                _ => None,
            })
            .sum()
    }

    /// Get total usage statistics for the session
    pub fn total_usage(&self) -> Usage {
        let mut usage = Usage::default();
        for msg in &self.messages {
            if let Message::Assistant(a) = msg {
                usage.add(&a.usage);
            }
        }
        usage
    }

    /// Set a system prompt (adds or replaces System message at start)
    pub fn set_system_prompt(&mut self, prompt: String) {
        use crate::types::SystemMessage;

        // Check if first message is system
        if let Some(Message::System(_)) = self.messages.first() {
            self.messages[0] = Message::System(SystemMessage { content: prompt });
        } else {
            self.messages.insert(0, Message::System(SystemMessage { content: prompt }));
        }
        self.updated_at = SystemTime::now();
    }

    /// Get the last assistant message text content
    pub fn last_assistant_text(&self) -> Option<&str> {
        self.messages.iter().rev().find_map(|m| match m {
            Message::Assistant(a) if !a.content.is_empty() => Some(a.content.as_str()),
            _ => None,
        })
    }

    /// Count tool calls made
    pub fn tool_call_count(&self) -> usize {
        self.messages
            .iter()
            .filter_map(|m| match m {
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

    /// Fork the session at a specific message index
    pub fn fork_at(&self, index: usize) -> Self {
        let mut forked = Self::new();
        forked.messages = self.messages[..index.min(self.messages.len())].to_vec();
        forked.metadata = self.metadata.clone();
        forked
    }

    /// Fork the entire session (full history)
    pub fn fork(&self) -> Self {
        self.fork_at(self.messages.len())
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
mod tests {
    use super::*;
    use crate::types::{AssistantMessage, StopReason, SystemMessage, UserMessage};

    #[test]
    fn test_session_new() {
        let session = Session::new();
        assert_eq!(session.version(), SESSION_VERSION);
        assert!(session.messages().is_empty());
        assert!(session.created_at() <= session.updated_at());
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

        let meta = SessionMeta::from(&session);
        assert_eq!(meta.id, *session.id());
        assert_eq!(meta.message_count, 2);
        assert_eq!(meta.total_tokens, 15);
    }
}

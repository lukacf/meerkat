//! EventStore trait — append-only event log with monotonic sequence numbers.
//!
//! Gated behind the `session-store` feature.

use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// A stored event with sequence metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Monotonically increasing sequence number within a session.
    pub seq: u64,
    /// Schema version for forward compatibility.
    pub schema_version: u32,
    /// When the event was stored.
    pub timestamp: SystemTime,
    /// The event payload.
    pub event: AgentEvent,
}

/// Current schema version for stored events.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// Append-only event log.
#[async_trait]
pub trait EventStore: Send + Sync {
    /// Append events to the log for a session.
    ///
    /// Returns the sequence number of the last appended event.
    async fn append(
        &self,
        session_id: &SessionId,
        events: &[AgentEvent],
    ) -> Result<u64, EventStoreError>;

    /// Read events from a given sequence number onward.
    async fn read_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;

    /// Get the latest sequence number for a session (0 if empty).
    async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError>;
}

/// Errors from event store operations.
#[derive(Debug, thiserror::Error)]
pub enum EventStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Store error: {0}")]
    Store(String),
}

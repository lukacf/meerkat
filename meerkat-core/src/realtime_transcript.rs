//! Typed realtime transcript append seam.
//!
//! Provider adapters translate provider-native realtime events into these
//! identity-bearing events. The session layer owns idempotency, causal ordering,
//! and the decision to materialize canonical transcript messages.

use serde::{Deserialize, Serialize};

use crate::types::{StopReason, Usage};

/// Durable session metadata key for realtime transcript append state.
pub const SESSION_REALTIME_TRANSCRIPT_STATE_KEY: &str = "realtime_transcript_state";

/// Provider-neutral role for a realtime transcript item.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeTranscriptRole {
    User,
    Assistant,
}

/// A typed, identity-bearing realtime transcript event consumed by the session.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RealtimeTranscriptEvent {
    /// Observe a provider item and its causal predecessor without committing
    /// content yet.
    ItemObserved {
        item_id: String,
        previous_item_id: Option<String>,
        role: RealtimeTranscriptRole,
        response_id: Option<String>,
    },
    /// Observe a provider item that participates in provider causal ordering
    /// but must not materialize transcript content.
    ItemSkipped {
        item_id: String,
        previous_item_id: Option<String>,
    },
    /// Provider finalized the transcript for a user input item.
    UserTranscriptFinal {
        item_id: String,
        previous_item_id: Option<String>,
        content_index: u32,
        text: String,
    },
    /// Provider emitted an assistant text delta for an output item.
    AssistantTextDelta {
        response_id: String,
        delta_id: String,
        item_id: String,
        previous_item_id: Option<String>,
        content_index: u32,
        delta: String,
    },
    /// Provider reported the assistant output item was truncated to the heard
    /// transcript prefix.
    AssistantTranscriptTruncated {
        response_id: String,
        item_id: String,
        content_index: u32,
        text: String,
    },
    /// Provider turn reached a terminal boundary. The session decides which
    /// staged assistant items, if any, are now canonical.
    AssistantTurnCompleted {
        response_id: String,
        stop_reason: StopReason,
        usage: Usage,
    },
    /// Provider turn was interrupted before terminal materialization.
    AssistantTurnInterrupted { response_id: String },
}

/// Canonical message materialized by applying a realtime transcript event.
#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeTranscriptMaterializedMessage {
    User {
        item_id: String,
        text: String,
    },
    Assistant {
        item_id: String,
        response_id: String,
        text: String,
        stop_reason: StopReason,
        usage: Usage,
    },
}

/// Result of applying a realtime transcript event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RealtimeTranscriptApplyOutcome {
    pub materialized_messages: Vec<RealtimeTranscriptMaterializedMessage>,
}

impl RealtimeTranscriptApplyOutcome {
    #[must_use]
    pub fn is_inert(&self) -> bool {
        self.materialized_messages.is_empty()
    }
}

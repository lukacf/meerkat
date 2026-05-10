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

/// Output lane carried by an assistant realtime transcript item.
///
/// T9/T10: distinguishes display text (authored output the model writes,
/// e.g. OpenAI realtime `response.output_text.delta`) from spoken transcript
/// (text derived from audio output, e.g. `response.output_audio_transcript.*`).
/// The materializer at [`crate::session::Session::append_realtime_transcript_event`]
/// dispatches on this to flush either [`crate::types::AssistantBlock::Text`]
/// (for `Display`) or [`crate::types::AssistantBlock::Transcript`] with
/// `source: TranscriptSource::Spoken` (for `Spoken`).
///
/// `Display` is the default for items that arrive only via
/// [`RealtimeTranscriptEvent::AssistantTextDelta`]; an item is upgraded to
/// `Spoken` the first time an [`RealtimeTranscriptEvent::AssistantTranscriptDelta`]
/// fragment arrives for it. Mixed-lane content on the same `item_id` is not
/// expected from any provider today; if observed the **first** lane wins
/// (the materializer cannot retroactively re-classify a partially-flushed
/// item) and a `tracing::warn!` is emitted.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TranscriptLane {
    #[default]
    Display,
    Spoken,
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
    /// Provider emitted an assistant **display-text** delta for an output
    /// item — authored text the model writes (e.g. OpenAI realtime
    /// `response.output_text.delta`).
    ///
    /// Materializes as [`crate::types::AssistantBlock::Text`].
    AssistantTextDelta {
        response_id: String,
        delta_id: String,
        item_id: String,
        previous_item_id: Option<String>,
        content_index: u32,
        delta: String,
    },
    /// Provider emitted an assistant **spoken-transcript** delta for an
    /// output item — text derived from audio output (e.g. OpenAI realtime
    /// `response.output_audio_transcript.delta`).
    ///
    /// Identity shape mirrors [`Self::AssistantTextDelta`] so the session's
    /// idempotent ordering / staging logic owns dedup uniformly across
    /// lanes. Materializes as [`crate::types::AssistantBlock::Transcript`]
    /// with `source: TranscriptSource::Spoken` (T9/T10).
    AssistantTranscriptDelta {
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
    /// R5-7: provider supplied authoritative final transcript text for an
    /// assistant output item, overriding any incomplete delta accumulation.
    ///
    /// Necessary for two cases:
    ///   1. Final-only providers that emit a single `AssistantTranscriptFinal`
    ///      observation without prior deltas.
    ///   2. Recovery from delta loss (R5-1: lossy media lane back-pressure
    ///      may drop transcript deltas; the final's text is the authoritative
    ///      reconciliation).
    ///
    /// The materializer locates the staged item by
    /// `(response_id, item_id, content_index)`, replaces its accumulated
    /// content with `text`, and (if no item is staged yet) creates one on
    /// the spoken lane. Flush still happens via `AssistantTurnCompleted`;
    /// this variant only updates the staged content.
    AssistantTranscriptFinalText {
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
        /// T9/T10: which output lane the staged content arrived on.
        /// Drives whether the materializer flushes
        /// [`crate::types::AssistantBlock::Text`] (Display) or
        /// [`crate::types::AssistantBlock::Transcript`] with
        /// `source: TranscriptSource::Spoken` (Spoken).
        lane: TranscriptLane,
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

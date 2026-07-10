//! Typed realtime transcript append seam.
//!
//! Provider adapters translate provider-native realtime events into these
//! identity-bearing events. Generated session realtime transcript authority owns
//! idempotency, causal ordering, and canonical transcript materialization.

use serde::{Deserialize, Serialize};

use crate::blob::BlobId;
use crate::types::{ContentBlock, StopReason, Usage};

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
/// The generated realtime transcript authority dispatches on this to flush
/// either [`crate::types::AssistantBlock::Text`]
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

/// Durable identity binding for one committed non-text user input.
///
/// Live image callers retry with a session-scoped `idempotency_key`. The binding is
/// persisted independently of provider connection state so receipt loss and
/// reconnect cannot duplicate canonical content, while reuse with a different
/// fingerprint can fail closed before provider send.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RealtimeUserContentIdentity {
    pub idempotency_key: String,
    pub item_id: String,
    pub previous_item_id: Option<String>,
    pub content_index: u32,
    pub blob_id: BlobId,
    pub media_type: String,
}

/// One-slot durable recovery anchor for a non-text user input whose blob and
/// reducer commit are not yet known to be jointly durable.
///
/// This record deliberately contains identity only. Inline bytes remain in
/// the caller retry and realm blob store; they never enter session metadata.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRealtimeUserContentBlob {
    pub idempotency_key: String,
    pub item_id: String,
    pub previous_item_id: Option<String>,
    pub content_index: u32,
    pub blob_id: BlobId,
    pub media_type: String,
}

impl PendingRealtimeUserContentBlob {
    #[must_use]
    pub fn identity(&self) -> RealtimeUserContentIdentity {
        RealtimeUserContentIdentity {
            idempotency_key: self.idempotency_key.clone(),
            item_id: self.item_id.clone(),
            previous_item_id: self.previous_item_id.clone(),
            content_index: self.content_index,
            blob_id: self.blob_id.clone(),
            media_type: self.media_type.clone(),
        }
    }

    #[must_use]
    pub fn canonical_event(&self) -> RealtimeTranscriptEvent {
        RealtimeTranscriptEvent::UserContentFinal {
            idempotency_key: self.idempotency_key.clone(),
            item_id: self.item_id.clone(),
            previous_item_id: self.previous_item_id.clone(),
            content_index: self.content_index,
            content: vec![ContentBlock::Image {
                media_type: self.media_type.clone(),
                data: crate::types::ImageData::Blob {
                    blob_id: self.blob_id.clone(),
                },
            }],
        }
    }

    #[must_use]
    pub fn matches_identity(&self, identity: &RealtimeUserContentIdentity) -> bool {
        self.identity() == *identity
    }
}

/// Durable rejection marker for a caller-stable user-content key whose
/// canonical content was removed by a same-session transcript rewrite.
///
/// Tombstones are intentionally retained instead of deleting the old binding:
/// a retry carrying the removed key must fail closed before provider send and
/// must never manufacture an `AlreadyCommitted` receipt for content that is no
/// longer present in the canonical message projection.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RealtimeUserContentTombstone {
    pub idempotency_key: String,
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
    /// Provider accepted a non-text user content segment into its
    /// conversation. Unlike [`Self::ItemObserved`], this event carries the
    /// canonical Meerkat content that must be durably materialized. The
    /// identity-bearing realtime transcript authority owns deduplication and
    /// causal ordering exactly as it does for [`Self::UserTranscriptFinal`].
    ///
    /// Image bytes may be inline at the live adapter seam; persistent session
    /// services externalize them into the realm blob store before reducer
    /// application so causally waiting segments never place bytes in durable
    /// transcript metadata.
    #[cfg_attr(feature = "schema", schemars(skip))]
    UserContentFinal {
        /// Caller-stable, session-scoped idempotency identity.
        idempotency_key: String,
        item_id: String,
        previous_item_id: Option<String>,
        content_index: u32,
        content: Vec<ContentBlock>,
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
    /// Identity shape mirrors [`Self::AssistantTextDelta`] so generated
    /// idempotent ordering / staging authority owns dedup uniformly across
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

/// Typed staged-transcript append seam (#51).
///
/// Provider adapters that stage a transcript item before it is committed —
/// e.g. the OpenAI realtime adapter's explicit-commit text-input path, which
/// accepts a user text item via `send_input` while the turn is open and holds
/// it until `commit_turn_with_modality` — lower the staged turn into this typed
/// input rather than an adapter-local `Vec`. Lowering the staged turn makes it a
/// machine-owned fact (the generated `MeerkatMachine` emits
/// `RealtimeTranscriptAppended` when this is applied), so a committed turn proves
/// a real staged turn and the staged set survives a crash/cancel between stage
/// and commit instead of living only in adapter memory.
///
/// `item_id` is the opaque synthetic provider item id (the same id sent to the
/// provider via `ConversationItemCreate` and reused on
/// [`RealtimeTranscriptEvent`]); `text` is the staged content; `role` and `lane`
/// are the typed classifiers the generated transcript authority dispatches on.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendRealtimeTranscript {
    /// Opaque synthetic provider item id for the staged turn.
    pub item_id: String,
    /// Staged transcript content.
    pub text: String,
    /// Provider-neutral role of the staged item.
    pub role: RealtimeTranscriptRole,
    /// Output lane the staged content belongs to.
    pub lane: TranscriptLane,
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
    /// Persistence-backed user-content receipt authority. Present only after
    /// canonical materialization or exact replay of an already committed key.
    pub user_content: Option<RealtimeUserContentApplyOutcome>,
}

/// Canonical result of applying a caller-stable non-text user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeUserContentApplyOutcome {
    Committed(RealtimeUserContentIdentity),
    AlreadyCommitted(RealtimeUserContentIdentity),
    /// The caller identity is malformed. This is an expected typed rejection,
    /// not durable-state corruption.
    RejectedInvalidIdentity {
        idempotency_key: String,
    },
    /// The provider item depends on a predecessor that is not canonical yet.
    RejectedUnmaterializedPredecessor {
        idempotency_key: String,
        previous_item_id: Option<String>,
    },
    /// The key is already bound to a different canonical payload.
    RejectedConflict {
        idempotency_key: String,
    },
}

impl RealtimeTranscriptApplyOutcome {
    #[must_use]
    pub fn is_inert(&self) -> bool {
        self.materialized_messages.is_empty() && self.user_content.is_none()
    }
}

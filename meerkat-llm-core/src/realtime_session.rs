//! Provider-neutral realtime session traits for product-layer channel clients.
//!
//! These types live in `meerkat-client` because they describe provider transport
//! capabilities and normalized event mapping, not runtime lifecycle truth.

use async_trait::async_trait;
use meerkat_contracts::{
    RealtimeAudioChunk, RealtimeCapabilities, RealtimeInputChunk, RealtimeTurningMode,
    RealtimeVideoChunk,
};
use meerkat_core::{
    Provider, RealtimeTranscriptEvent, RealtimeUserContentIdentity, RealtimeUserContentTombstone,
    ToolResult,
};
use meerkat_core::{
    RealtimeOpenProjectionLease, RealtimeOpenProjectionLeaseSlot, SessionLlmIdentity, StopReason,
    ToolDef, types::Message, types::Usage,
};
use serde_json::Value;

use crate::LlmError;

/// Advanced/internal target for attaching to an existing provider session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeExternalSessionTarget {
    pub provider_session_id: String,
}

impl RealtimeExternalSessionTarget {
    /// Construct a provider session target, rejecting blank identifiers.
    pub fn new(provider_session_id: impl Into<String>) -> Result<Self, LlmError> {
        let provider_session_id = provider_session_id.into();
        if provider_session_id.trim().is_empty() {
            return Err(LlmError::InvalidRequest {
                message: "provider realtime session id must not be empty".to_string(),
            });
        }
        Ok(Self {
            provider_session_id,
        })
    }
}

/// Provider-neutral realtime event stream.
#[derive(Debug, Clone, PartialEq)]
pub enum RealtimeSessionEvent {
    InputTranscriptPartial {
        text: String,
    },
    InputTranscriptFinal {
        text: String,
    },
    InputTranscriptFinalForItem {
        item_id: String,
        previous_item_id: Option<String>,
        content_index: u32,
        text: String,
    },
    TurnStarted,
    TurnCommitted,
    TurnCompleted {
        response_id: String,
        stop_reason: StopReason,
        usage: Usage,
    },
    OutputTextDelta {
        delta: String,
    },
    OutputTextDeltaForItem {
        response_id: String,
        delta_id: String,
        item_id: String,
        previous_item_id: Option<String>,
        content_index: u32,
        delta: String,
    },
    /// Spoken-transcript lane delta for an output item — text derived from
    /// the provider's audio output (OpenAI realtime
    /// `response.output_audio_transcript.delta`).
    ///
    /// T9/T10: distinct from [`Self::OutputTextDeltaForItem`] (display
    /// text). The adapter forwards this to
    /// `LiveAdapterObservation::AssistantTranscriptDelta`, which the
    /// runtime materializes as
    /// [`meerkat_core::types::AssistantBlock::Transcript`] with
    /// `source: TranscriptSource::Spoken` rather than as authored display
    /// text.
    OutputAudioTranscriptDeltaForItem {
        response_id: String,
        delta_id: String,
        item_id: String,
        previous_item_id: Option<String>,
        content_index: u32,
        delta: String,
    },
    /// Streaming audio frame from the provider. R5-4: identity fields
    /// (`response_id`, `item_id`, `content_index`) carry the source server
    /// event's identity so the live-adapter translator can stamp the public
    /// `LiveAdapterObservation::AssistantAudioChunk` and clients can attach a
    /// playback cursor to a provider item without racing on transcript-delta
    /// arrival order. All three are `Option` because not every provider
    /// surfaces a content segment id and degraded paths may drop identity.
    OutputAudioChunk {
        chunk: RealtimeAudioChunk,
        response_id: Option<String>,
        item_id: Option<String>,
        content_index: Option<u32>,
    },
    OutputVideoChunk {
        chunk: RealtimeVideoChunk,
    },
    Interrupted {
        response_id: Option<String>,
    },
    ToolCallRequested {
        call_id: String,
        tool_name: String,
        arguments: Value,
    },
    /// The assistant output identified by `item_id` was truncated at
    /// `audio_played_ms` because the user barged in. `truncated_text` is the
    /// heard prefix, or `None` if the provider has not yet re-projected it.
    AssistantTranscriptTruncated {
        response_id: Option<String>,
        item_id: String,
        /// Content segment index that was truncated. Some providers (OpenAI
        /// realtime) carry this on the truncation client command and echo it
        /// implicitly through the server `conversation.item.truncated` ack.
        /// `None` means the provider did not surface a content segment id and
        /// downstream projectors should treat the truncation as covering the
        /// item's primary content segment.
        content_index: Option<u32>,
        audio_played_ms: u64,
        truncated_text: Option<String>,
    },
    /// Identity-bearing transcript event for providers that need to expose an
    /// ordering/append fact without an otherwise public channel event.
    RealtimeTranscript {
        event: RealtimeTranscriptEvent,
    },
    /// Provider finalized the assistant transcript for an output item.
    ///
    /// Emitted by providers that surface a single terminal "transcript done"
    /// fact (OpenAI: `response.output_audio_transcript.done`). The adapter
    /// forwards this 1:1 to `LiveAdapterObservation::AssistantTranscriptFinal`
    /// so the runtime's projection layer has an authoritative end-of-item
    /// signal carrying the full transcript text. `stop_reason`/`usage` are
    /// best-effort: providers that do not deliver them atomically with the
    /// transcript-done event use sentinel defaults (the runtime layer will
    /// reconcile against a subsequent `TurnCompleted` if it carries the
    /// authoritative values).
    AssistantTranscriptFinal {
        item_id: String,
        previous_item_id: Option<String>,
        content_index: Option<u32>,
        response_id: Option<String>,
        text: String,
        stop_reason: StopReason,
        usage: Usage,
    },
}

/// Provider-neutral realtime session surface.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RealtimeSession: Send {
    /// Report the product-facing capability set the provider can honor.
    fn capabilities(&self) -> &RealtimeCapabilities;

    /// Report the turning mode selected when the session was opened.
    fn turning_mode(&self) -> RealtimeTurningMode;

    /// Refresh the provider's projection of canonical Meerkat session state.
    ///
    /// This is projection-only: canonical Meerkat history, visible tools, and
    /// related policy remain the semantic owner. Providers update their local
    /// session view from the latest canonical open config before the next user
    /// turn, rather than becoming a second owner of conversation truth.
    async fn refresh_projection(
        &mut self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<(), LlmError>;

    /// Stream one modality-neutral input chunk into the provider session.
    async fn send_input(&mut self, chunk: RealtimeInputChunk) -> Result<(), LlmError>;

    /// Commit the staged turn when the session is using explicit commit.
    async fn commit_turn(&mut self) -> Result<(), LlmError>;

    /// Interrupt the currently active provider response, if any.
    async fn interrupt(&mut self) -> Result<(), LlmError>;

    /// Truncate the assistant output for `item_id` to `audio_played_ms` so the
    /// canonical session transcript reflects what the user actually heard
    /// before barging in. The adapter is expected to eventually emit
    /// [`RealtimeSessionEvent::AssistantTranscriptTruncated`] with the
    /// re-projected prefix (or a best-effort approximation if the provider
    /// cannot supply exact text).
    async fn truncate_assistant_output(
        &mut self,
        item_id: String,
        content_index: u32,
        audio_played_ms: u64,
    ) -> Result<(), LlmError>;

    /// Submit a completed tool result back into the provider session so its
    /// response can continue.
    async fn submit_tool_result(&mut self, result: ToolResult) -> Result<(), LlmError>;

    /// Submit a tool-dispatch error back into the provider session.
    async fn submit_tool_error(&mut self, call_id: String, error: String) -> Result<(), LlmError>;

    /// Read the next normalized realtime session event.
    async fn next_event(&mut self) -> Result<Option<RealtimeSessionEvent>, LlmError>;

    /// Close the provider session and release any local transport state.
    async fn close(&mut self) -> Result<(), LlmError>;
}

/// Canonical live session projection used to open a provider-backed realtime session.
///
/// This is the product-session equivalent of a build seam: the provider session
/// must be opened from the currently-owned Meerkat session identity, visible
/// tools, and committed transcript instead of inventing a parallel provider-only
/// conversation.
#[derive(Debug, Clone)]
pub struct RealtimeSessionOpenConfig {
    pub turning_mode: RealtimeTurningMode,
    pub llm_identity: SessionLlmIdentity,
    pub visible_tools: Vec<ToolDef>,
    seed_messages: Vec<Message>,
    /// Take-once process memory custody spanning canonical image hydration
    /// through provider seed acknowledgement.
    ///
    /// Cloned configs share this slot. A factory consumes the lease locally;
    /// reusing or concurrently opening from another clone must acquire fresh
    /// custody instead of reusing the original reservation.
    open_projection_lease: RealtimeOpenProjectionLeaseSlot,
    /// Provider-lowered ordered system-channel instructions for this realtime session.
    ///
    /// The canonical owners remain the ordered `Message::System` and
    /// `Message::SystemNotice` transcript rows. Projection deterministically
    /// lifts every such row, in transcript order, into this singular field for
    /// realtime providers whose instruction channel cannot represent
    /// interleaved system-channel messages.
    ///
    /// Provider adapters and snapshot builders MUST consume this typed field
    /// when they need one provider instruction value (e.g. the OpenAI Refresh
    /// path rebuilding the realtime `session.update` instructions field). They
    /// MUST NOT re-derive it by inspecting `seed_messages[0]`: System
    /// messages have no privileged position, and the
    /// history-event projector drops `Message::System` / `Message::SystemNotice`
    /// entries. `None` means the session has no ordered system-channel rows;
    /// `Some("")` means it has an authored empty System row.
    ordered_system_instructions: Option<String>,
    /// Durable caller-id bindings for committed non-text user inputs. Provider
    /// adapters rebuild this registry on reconnect before accepting retries.
    pub user_content_identities: Vec<RealtimeUserContentIdentity>,
    /// Durable conflict markers for caller keys whose canonical image was
    /// removed by a same-session transcript rewrite.
    pub user_content_tombstones: Vec<RealtimeUserContentTombstone>,
    /// Full canonical decoded-image usage at projection time.
    ///
    /// This is deliberately independent of `seed_messages`: a caller-selected
    /// seed window may omit old image messages, but the live adapter must still
    /// enforce future image admission against the complete canonical history.
    /// `None` is reserved for direct/legacy Rust callers that did not obtain the
    /// projection from a session service; adapters then derive usage from the
    /// supplied seed for backward compatibility.
    pub canonical_user_image_decoded_bytes: Option<usize>,
    /// Canonical transcript revision used to detect same-session history
    /// rewrites that cannot be applied to an already-seeded provider
    /// conversation in place.
    pub transcript_rewrite_generation: u64,
    /// Per-channel override for the "nudge the provider" timeout the OpenAI
    /// adapter uses while waiting for the first real delta after a turn is
    /// admitted. `None` inherits the adapter's compile-time default.
    pub response_nudge_timeout_ms: Option<u64>,
    /// Per-channel override for the maximum number of nudge attempts before
    /// the adapter gives up. `None` inherits the adapter default.
    pub response_nudge_max_attempts: Option<u8>,
}

impl RealtimeSessionOpenConfig {
    /// Deterministically lower every ordered System/SystemNotice row into the
    /// singular instruction representation used by realtime providers.
    #[must_use]
    pub fn lower_ordered_system_messages(messages: &[Message]) -> Option<String> {
        let mut systems = messages.iter().filter_map(|message| match message {
            Message::System(system) => Some(system.content.clone()),
            Message::SystemNotice(notice) => Some(notice.model_projection_text()),
            _ => None,
        });
        let mut lowered = systems.next()?;
        for system in systems {
            lowered.push_str("\n\n");
            lowered.push_str(&system);
        }
        Some(lowered)
    }

    #[must_use]
    pub fn new(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
    ) -> Self {
        let ordered_system_instructions = Self::lower_ordered_system_messages(&seed_messages);
        Self::new_with_projection(
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            ordered_system_instructions,
        )
    }

    /// Construct an open projection from a caller-selected replay seed while
    /// deriving instructions from the complete active materialized transcript.
    ///
    /// A bounded replay seed may omit old dialogue and System rows. The
    /// provider's top-level instruction projection must nevertheless cover
    /// every System/SystemNotice in the active transcript. `canonical_messages`
    /// is that active materialization only; retained historical rewrite
    /// strands are never instruction input.
    #[must_use]
    pub fn for_open_from_messages(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
        canonical_messages: &[Message],
    ) -> Self {
        let ordered_system_instructions = Self::lower_ordered_system_messages(canonical_messages);
        Self::new_with_projection(
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            ordered_system_instructions,
        )
    }

    fn new_with_projection(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
        ordered_system_instructions: Option<String>,
    ) -> Self {
        Self {
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            open_projection_lease: RealtimeOpenProjectionLeaseSlot::default(),
            ordered_system_instructions,
            user_content_identities: Vec::new(),
            user_content_tombstones: Vec::new(),
            canonical_user_image_decoded_bytes: None,
            transcript_rewrite_generation: 0,
            response_nudge_timeout_ms: None,
            response_nudge_max_attempts: None,
        }
    }

    /// Construct a refresh-only projection. Refresh has no seed replay, but its
    /// provider instructions are still derived here from the ordinary ordered
    /// System rows at the projection boundary.
    #[must_use]
    pub fn for_refresh_from_messages(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        canonical_messages: &[Message],
    ) -> Self {
        let mut config = Self::new(turning_mode, llm_identity, visible_tools, Vec::new());
        config.ordered_system_instructions =
            Self::lower_ordered_system_messages(canonical_messages);
        config
    }

    /// Carry an already-acquired open-projection lease from the runtime's
    /// pre-hydration boundary to the provider seed boundary.
    #[must_use]
    pub fn with_open_projection_lease(mut self, lease: RealtimeOpenProjectionLease) -> Self {
        self.open_projection_lease = RealtimeOpenProjectionLeaseSlot::new(lease);
        self
    }

    /// Transfer the carried lease into one factory invocation.
    ///
    /// The slot is shared across config clones, so exactly one caller can take
    /// it. A caller that receives `None` must acquire a fresh lease before
    /// materializing provider seed events.
    #[must_use]
    pub fn take_open_projection_lease(&self) -> Option<RealtimeOpenProjectionLease> {
        self.open_projection_lease.take()
    }

    /// Provider-lowered ordered System instructions.
    ///
    /// Open configs derive this from every ordered seed System. Refresh-only
    /// configs derive it from the canonical transcript through
    /// [`Self::for_refresh_from_messages`].
    #[must_use]
    pub fn ordered_system_instructions(&self) -> Option<&str> {
        self.ordered_system_instructions.as_deref()
    }

    /// Immutable canonical seed transcript paired with
    /// [`Self::ordered_system_instructions`].
    ///
    /// Mutation is intentionally unavailable: changing the seed after
    /// construction would invalidate the derived ordered-System projection.
    #[must_use]
    pub fn seed_messages(&self) -> &[Message] {
        &self.seed_messages
    }

    /// Replace the canonical seed while atomically re-deriving its ordered
    /// System projection.
    #[must_use]
    pub fn with_seed_messages(mut self, seed_messages: Vec<Message>) -> Self {
        self.ordered_system_instructions = Self::lower_ordered_system_messages(&seed_messages);
        self.seed_messages = seed_messages;
        self
    }

    /// Builder-style durable user-content idempotency registry.
    #[must_use]
    pub fn with_user_content_identities(
        mut self,
        identities: Vec<RealtimeUserContentIdentity>,
    ) -> Self {
        self.user_content_identities = identities;
        self
    }

    /// Builder-style durable removed-key conflict registry.
    #[must_use]
    pub fn with_user_content_tombstones(
        mut self,
        tombstones: Vec<RealtimeUserContentTombstone>,
    ) -> Self {
        self.user_content_tombstones = tombstones;
        self
    }

    /// Carry full canonical image usage separately from the selected seed.
    #[must_use]
    pub fn with_canonical_user_image_decoded_bytes(mut self, decoded_bytes: usize) -> Self {
        self.canonical_user_image_decoded_bytes = Some(decoded_bytes);
        self
    }

    /// Builder-style canonical transcript revision for live rewrite guards.
    #[must_use]
    pub fn with_transcript_rewrite_generation(mut self, generation: u64) -> Self {
        self.transcript_rewrite_generation = generation;
        self
    }

    /// Builder-style override for the per-channel nudge timeout.
    #[must_use]
    pub fn with_response_nudge_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.response_nudge_timeout_ms = timeout_ms;
        self
    }

    /// Builder-style override for the per-channel nudge max attempts.
    #[must_use]
    pub fn with_response_nudge_max_attempts(mut self, max_attempts: Option<u8>) -> Self {
        self.response_nudge_max_attempts = max_attempts;
        self
    }
}

/// Factory for provider-neutral realtime sessions.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RealtimeSessionFactory: Send + Sync {
    /// Report the provider/product capability set exposed by this factory.
    fn capabilities(&self) -> RealtimeCapabilities;

    /// Whether this factory is the typed owner for opening live/realtime
    /// adapters for `provider`.
    ///
    /// Shared runtime code must not infer wired-adapter support from provider
    /// names. The concrete factory that actually mints the adapter owns that
    /// support fact.
    fn supports_provider(&self, _provider: Provider) -> bool {
        false
    }

    /// Open a provider-created realtime session using the selected turning mode.
    async fn open_session(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError>;

    /// Attach to an existing provider-managed realtime session.
    ///
    /// Attach consumes the same canonical open projection as a newly-created
    /// provider session. In particular, committed user-content bindings and
    /// removed-key tombstones must be installed before attached input is
    /// admitted; accepting only an LLM identity here would create a bypass of
    /// rewrite-safe idempotency admission.
    async fn attach_external_session(
        &self,
        target: &RealtimeExternalSessionTarget,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError>;

    /// E25: Open a provider-native `LiveAdapter` directly.
    ///
    /// The default impl returns `Unsupported` so providers that have not
    /// yet implemented the direct seam keep working (their callers continue
    /// to go through the `RealtimeSession` trait via mob/test harnesses).
    /// The OpenAI factory overrides this to construct an `OpenAiLiveAdapter`
    /// without boxing the session as `Box<dyn RealtimeSession>`.
    ///
    /// Returns an `Arc<dyn LiveAdapter>` because the live-adapter host owns
    /// adapters by `Arc` and exposes `&self` methods (concurrent
    /// send/receive without an outer mutex).
    async fn open_live_adapter(
        &self,
        _open_config: &RealtimeSessionOpenConfig,
    ) -> Result<std::sync::Arc<dyn meerkat_core::live_adapter::LiveAdapter>, LlmError> {
        Err(LlmError::InvalidRequest {
            message: "this provider has not implemented direct LiveAdapter; \
                      callers must wrap a RealtimeSession via meerkat-live"
                .to_string(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use meerkat_core::Provider;
    use meerkat_core::types::{SystemMessage, SystemNoticeKind, SystemNoticeMessage, UserMessage};

    fn sample_identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: "gpt-5.4".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    #[test]
    fn external_session_target_rejects_blank_provider_id() {
        let error = match RealtimeExternalSessionTarget::new("   ") {
            Ok(_) => panic!("blank provider id must fail"),
            Err(error) => error,
        };
        assert!(matches!(error, LlmError::InvalidRequest { .. }));
    }

    #[test]
    fn open_config_derives_every_ordered_seed_system_without_row_zero_privilege() {
        let config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            sample_identity(),
            Vec::new(),
            vec![
                Message::User(UserMessage::text("hello")),
                Message::System(SystemMessage::new("first")),
                Message::User(UserMessage::text("continue")),
                Message::System(SystemMessage::new("")),
                Message::SystemNotice(SystemNoticeMessage::new(SystemNoticeKind::Generic, "")),
                Message::SystemNotice(SystemNoticeMessage::new(
                    SystemNoticeKind::Generic,
                    "duplicate",
                )),
                Message::SystemNotice(SystemNoticeMessage::new(
                    SystemNoticeKind::Generic,
                    "duplicate",
                )),
                Message::System(SystemMessage::new(" \t ")),
            ],
        );
        assert_eq!(
            config.ordered_system_instructions(),
            Some("first\n\n\n\n\n\nduplicate\n\nduplicate\n\n \t ")
        );
    }

    #[test]
    fn open_without_system_rows_is_none_and_refresh_derives_without_seed_replay() {
        let open = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            vec![Message::User(UserMessage::text("hello"))],
        );
        assert_eq!(open.ordered_system_instructions(), None);

        let refresh = RealtimeSessionOpenConfig::for_refresh_from_messages(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            &[
                Message::User(UserMessage::text("work")),
                Message::System(SystemMessage::new("authoritative")),
                Message::System(SystemMessage::new("")),
                Message::System(SystemMessage::new(" \t ")),
            ],
        );
        assert!(refresh.seed_messages().is_empty());
        assert_eq!(
            refresh.ordered_system_instructions(),
            Some("authoritative\n\n\n\n \t ")
        );
    }

    #[test]
    fn bounded_open_seed_derives_instructions_from_full_active_materialization() {
        let seed = vec![Message::User(UserMessage::text("recent"))];
        let active_messages = vec![
            Message::System(SystemMessage::new("outside replay window")),
            Message::User(UserMessage::text("old dialogue")),
            Message::System(SystemMessage::new("")),
            Message::User(UserMessage::text("recent")),
        ];

        let config = RealtimeSessionOpenConfig::for_open_from_messages(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            seed.clone(),
            &active_messages,
        );

        assert_eq!(config.seed_messages(), seed);
        assert_eq!(
            config.ordered_system_instructions(),
            Some("outside replay window\n\n")
        );
    }

    #[test]
    fn replacing_seed_atomically_rederives_ordered_system_projection() {
        let config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            vec![Message::System(SystemMessage::new("stale"))],
        )
        .with_seed_messages(vec![
            Message::User(UserMessage::text("work")),
            Message::System(SystemMessage::new("current")),
            Message::System(SystemMessage::new("")),
        ]);

        assert_eq!(config.ordered_system_instructions(), Some("current\n\n"));
    }
}

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
    ToolDef, TurnUsage, types::Message, types::Usage,
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
        usage: TurnUsage,
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
    /// reported playback prefix, or `None` when playback coverage is
    /// unmeasured. It is never a biological-hearing claim.
    AssistantTranscriptTruncated {
        interaction_id: Option<meerkat_core::InteractionId>,
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
    /// canonical session transcript may reflect a reported playback prefix
    /// before barge-in. The adapter is expected to eventually emit
    /// [`RealtimeSessionEvent::AssistantTranscriptTruncated`] with the
    /// exact reported prefix. Providers that cannot supply exact prefix text
    /// must emit `None` rather than an approximation.
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
    /// Canonical committed message cursor at projection time. This remains the
    /// full document count even when `seed_messages` is windowed.
    canonical_message_cursor: u64,
    /// Take-once process memory custody spanning canonical image hydration
    /// through provider seed acknowledgement.
    ///
    /// Cloned configs share this slot. A factory consumes the lease locally;
    /// reusing or concurrently opening from another clone must acquire fresh
    /// custody instead of reusing the original reservation.
    open_projection_lease: RealtimeOpenProjectionLeaseSlot,
    /// Exact provider-visible System payload sequence at projection time.
    ///
    /// This is a refresh drift witness, not a provider instruction field.
    /// Provider adapters replay the actual `Message::System` rows from
    /// `seed_messages` in their authored transcript positions.
    canonical_system_messages: Vec<String>,
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
    /// Collect provider-visible System payloads in authored message order.
    /// Superseded versions of a keyed prompt remain durable but are excluded
    /// from both provider replay and the refresh drift witness.
    #[must_use]
    pub fn canonical_system_messages(messages: &[Message]) -> Vec<String> {
        let superseded = meerkat_core::types::superseded_system_prompt_offsets(messages);
        messages
            .iter()
            .enumerate()
            .filter(|(offset, _)| !superseded.contains(offset))
            .filter_map(|(_, message)| match message {
                Message::System(system) => Some(system.content.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn new(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
    ) -> Result<Self, LlmError> {
        let canonical_message_cursor = seed_messages.len() as u64;
        let seed_messages =
            meerkat_core::types::materialize_latest_system_prompt_versions(&seed_messages);
        let canonical_system_messages = Self::canonical_system_messages(&seed_messages);
        Ok(Self::new_with_projection(
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            canonical_system_messages,
            canonical_message_cursor,
        ))
    }

    /// Construct an open projection from a caller-selected replay seed while
    /// retaining the complete canonical System subsequence as a drift witness.
    ///
    /// The replay seed applies the same ordered window policy to every role.
    /// Any retained System row is replayed natively at its transcript position.
    pub fn for_open_from_messages(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
        canonical_messages: &[Message],
    ) -> Result<Self, LlmError> {
        let seed_messages =
            meerkat_core::types::materialize_latest_system_prompt_versions(&seed_messages);
        let canonical_system_messages = Self::canonical_system_messages(canonical_messages);
        let canonical_message_cursor = canonical_messages.len() as u64;
        Ok(Self::new_with_projection(
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            canonical_system_messages,
            canonical_message_cursor,
        ))
    }

    fn new_with_projection(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
        canonical_system_messages: Vec<String>,
        canonical_message_cursor: u64,
    ) -> Self {
        Self {
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            canonical_message_cursor,
            open_projection_lease: RealtimeOpenProjectionLeaseSlot::default(),
            canonical_system_messages,
            user_content_identities: Vec::new(),
            user_content_tombstones: Vec::new(),
            canonical_user_image_decoded_bytes: None,
            transcript_rewrite_generation: 0,
            response_nudge_timeout_ms: None,
            response_nudge_max_attempts: None,
        }
    }

    /// Construct a refresh-only projection. Refresh has no seed replay; the
    /// exact System sequence is retained only to detect a required reopen.
    pub fn for_refresh_from_messages(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        canonical_messages: &[Message],
    ) -> Result<Self, LlmError> {
        let mut config = Self::new(turning_mode, llm_identity, visible_tools, Vec::new())?;
        config.canonical_system_messages = Self::canonical_system_messages(canonical_messages);
        config.canonical_message_cursor = canonical_messages.len() as u64;
        Ok(config)
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

    /// Exact canonical System payload sequence used for refresh drift checks.
    #[must_use]
    pub fn canonical_system_messages_ref(&self) -> &[String] {
        &self.canonical_system_messages
    }

    /// Immutable canonical seed transcript paired with
    /// [`Self::canonical_system_messages_ref`].
    ///
    /// Mutation is intentionally unavailable: changing the seed after
    /// construction would invalidate the canonical System drift witness.
    #[must_use]
    pub fn seed_messages(&self) -> &[Message] {
        &self.seed_messages
    }

    #[must_use]
    pub const fn canonical_message_cursor(&self) -> u64 {
        self.canonical_message_cursor
    }

    /// Replace the canonical seed while atomically re-deriving its System
    /// drift witness.
    pub fn with_seed_messages(mut self, seed_messages: Vec<Message>) -> Result<Self, LlmError> {
        let seed_messages =
            meerkat_core::types::materialize_latest_system_prompt_versions(&seed_messages);
        self.canonical_system_messages = Self::canonical_system_messages(&seed_messages);
        self.seed_messages = seed_messages;
        Ok(self)
    }

    /// Append a per-open System overlay without changing the canonical
    /// durable-System drift witness.
    ///
    /// This is a narrow compatibility seam for surfaces that historically
    /// accepted ephemeral instructions on live/open. The overlay reaches the
    /// provider seed for this open only; refresh still compares against the
    /// durable session's canonical System sequence.
    #[doc(hidden)]
    pub fn append_ephemeral_system_overlay(&mut self, content: String) {
        self.seed_messages
            .push(Message::System(meerkat_core::types::SystemMessage::new(
                content,
            )));
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
    use meerkat_core::types::{
        SystemMessage, SystemNoticeKind, SystemNoticeMessage, SystemPromptKey, SystemPromptVersion,
        SystemPromptVersionIdentity, UserMessage,
    };

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
    fn open_config_collects_every_system_message_in_authored_order() {
        let config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            sample_identity(),
            Vec::new(),
            vec![
                Message::System(SystemMessage::new("first")),
                Message::System(SystemMessage::new("")),
                Message::System(SystemMessage::new(" \t ")),
                Message::SystemNotice(SystemNoticeMessage::new(SystemNoticeKind::Generic, "")),
                Message::User(UserMessage::text("hello")),
                Message::System(SystemMessage::new("later")),
            ],
        )
        .expect("ordered System messages must be representable");
        assert_eq!(
            config.canonical_system_messages_ref(),
            &["first", "", " \t ", "later"]
        );
    }

    #[test]
    fn realtime_seed_and_drift_witness_select_latest_prompt_version() {
        let key = SystemPromptKey::new("primary").expect("prompt key");
        let mut first = SystemMessage::new("version one");
        first.prompt_version = Some(SystemPromptVersionIdentity {
            key: key.clone(),
            version: SystemPromptVersion::INITIAL,
        });
        let mut second = SystemMessage::new("version two");
        second.prompt_version = Some(SystemPromptVersionIdentity {
            key,
            version: SystemPromptVersion::new(2).expect("version two"),
        });
        let raw = vec![
            Message::System(first),
            Message::System(second.clone()),
            Message::User(UserMessage::text("hello")),
        ];

        let config = RealtimeSessionOpenConfig::for_open_from_messages(
            RealtimeTurningMode::ProviderManaged,
            sample_identity(),
            Vec::new(),
            raw.clone(),
            &raw,
        )
        .expect("versioned prompt projection");
        assert_eq!(config.canonical_system_messages_ref(), &["version two"]);
        assert_eq!(config.seed_messages().len(), 2);
        assert_eq!(config.seed_messages()[0], Message::System(second));
    }

    #[test]
    fn open_without_system_rows_is_none_and_refresh_derives_without_seed_replay() {
        let open = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            vec![Message::User(UserMessage::text("hello"))],
        )
        .expect("ordinary dialogue must be representable");
        assert!(open.canonical_system_messages_ref().is_empty());

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
        )
        .expect("every ordered System row is projected");
        assert_eq!(
            refresh.canonical_system_messages_ref(),
            &["authoritative", "", " \t "]
        );
    }

    #[test]
    fn bounded_open_seed_retains_full_system_drift_witness() {
        let recent = Message::User(UserMessage::text("recent"));
        let seed = vec![recent.clone()];
        let active_messages = vec![
            Message::System(SystemMessage::new("outside replay window")),
            Message::User(UserMessage::text("old dialogue")),
            Message::System(SystemMessage::new("")),
            recent.clone(),
        ];

        let config = RealtimeSessionOpenConfig::for_open_from_messages(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            seed.clone(),
            &active_messages,
        )
        .expect("all System rows in the full materialization are projected");
        assert_eq!(
            config.canonical_system_messages_ref(),
            &["outside replay window", ""]
        );
        assert_eq!(seed, vec![recent]);
    }

    #[test]
    fn replacing_seed_atomically_rederives_system_drift_witness() {
        let config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            vec![Message::System(SystemMessage::new("stale"))],
        )
        .expect("System messages must be representable")
        .with_seed_messages(vec![
            Message::System(SystemMessage::new("current")),
            Message::System(SystemMessage::new("")),
            Message::User(UserMessage::text("work")),
        ])
        .expect("ordered System messages must be representable");

        assert_eq!(config.canonical_system_messages_ref(), &["current", ""]);
    }
}

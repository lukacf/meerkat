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
    PendingSystemContextAppend, Provider, RealtimeTranscriptEvent, RealtimeUserContentIdentity,
    RealtimeUserContentTombstone, ToolResult,
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
    pub seed_messages: Vec<Message>,
    /// Take-once process memory custody spanning canonical image hydration
    /// through provider seed acknowledgement.
    ///
    /// Cloned configs share this slot. A factory consumes the lease locally;
    /// reusing or concurrently opening from another clone must acquire fresh
    /// custody instead of reusing the original reservation.
    open_projection_lease: RealtimeOpenProjectionLeaseSlot,
    /// Authoritative resolved root system prompt for this realtime session.
    ///
    /// This is the canonical owner of the live session's system prompt. It is
    /// populated at projection time by the runtime from the resolved root
    /// system message (`realtime_projection_root_system_message`) — the same
    /// content that, when present, is materialized into `seed_messages[0]`.
    ///
    /// Provider adapters and snapshot builders MUST consume this typed field
    /// when they need the prompt as a distinct value (e.g. the OpenAI Refresh
    /// path rebuilding the realtime `session.update` instructions field). They
    /// MUST NOT re-derive prompt truth by inspecting `seed_messages[0]`: the
    /// history-event projector drops `Message::System` / `Message::SystemNotice`
    /// entries, so inference from seed history silently wipes the prompt on
    /// refresh. `None` means the session has no resolved root system prompt.
    pub system_prompt: Option<String>,
    /// Runtime-authored system context carried as typed provenance.
    ///
    /// Provider adapters must treat this as the only authoritative realtime
    /// reconstruction source for runtime context. Rendered transcript markers
    /// are projections only and must not be parsed back into authority.
    pub runtime_system_context: Vec<PendingSystemContextAppend>,
    /// Durable caller-id bindings for committed non-text user inputs. Provider
    /// adapters rebuild this registry on reconnect before accepting retries.
    pub user_content_identities: Vec<RealtimeUserContentIdentity>,
    /// Durable conflict markers for caller keys whose canonical image was
    /// removed by a same-session transcript rewrite.
    pub user_content_tombstones: Vec<RealtimeUserContentTombstone>,
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
    #[must_use]
    pub fn new(
        turning_mode: RealtimeTurningMode,
        llm_identity: SessionLlmIdentity,
        visible_tools: Vec<ToolDef>,
        seed_messages: Vec<Message>,
    ) -> Self {
        Self {
            turning_mode,
            llm_identity,
            visible_tools,
            seed_messages,
            open_projection_lease: RealtimeOpenProjectionLeaseSlot::default(),
            system_prompt: None,
            runtime_system_context: Vec::new(),
            user_content_identities: Vec::new(),
            user_content_tombstones: Vec::new(),
            transcript_rewrite_generation: 0,
            response_nudge_timeout_ms: None,
            response_nudge_max_attempts: None,
        }
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

    /// Builder-style typed root system prompt for provider reconstruction.
    ///
    /// The runtime populates this from the resolved root system message so the
    /// provider refresh path consumes the authoritative prompt instead of
    /// inferring it from `seed_messages[0]`.
    #[must_use]
    pub fn with_system_prompt(mut self, system_prompt: Option<String>) -> Self {
        self.system_prompt = system_prompt;
        self
    }

    /// Builder-style typed runtime context for provider reconstruction.
    #[must_use]
    pub fn with_runtime_system_context(
        mut self,
        runtime_system_context: Vec<PendingSystemContextAppend>,
    ) -> Self {
        self.runtime_system_context = runtime_system_context;
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
    use meerkat_core::types::{SystemMessage, UserMessage};

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

    /// Row #209 gate: the realtime open config carries the resolved system
    /// prompt as a typed field, and the prompt's authority is independent of
    /// `seed_messages[0]`. A refresh that consumes `config.system_prompt` must
    /// preserve the prompt even when the seed history has been projected to
    /// drop its lead `Message::System` (the history-event projector path), so
    /// no consumer needs to re-infer prompt truth from `seed_messages`.
    #[test]
    fn open_config_system_prompt_is_typed_and_independent_of_seed_history() {
        let resolved_prompt = "You are a careful realtime assistant.".to_string();

        // Seed history WITHOUT a lead `Message::System` — i.e. the
        // history-event projection already dropped the system message, which
        // is exactly the case where `seed_messages[0]` inference returns the
        // wrong answer and silently wipes the prompt on refresh.
        let config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ProviderManaged,
            sample_identity(),
            Vec::new(),
            vec![Message::User(UserMessage::text("hello".to_string()))],
        )
        .with_system_prompt(Some(resolved_prompt.clone()));

        // The typed field is the authoritative source for the refresh path.
        assert_eq!(
            config.system_prompt.as_deref(),
            Some(resolved_prompt.as_str())
        );

        // Authority does NOT come from inspecting the seed history lead: the
        // first seed message is a user turn, not a system message.
        assert!(matches!(
            config.seed_messages.first(),
            Some(Message::User(_))
        ));
    }

    /// Row #209 gate: `new` defaults the typed prompt to `None`, and the
    /// builder is the single populate-point used by the runtime projection.
    #[test]
    fn open_config_system_prompt_defaults_none_and_builder_sets_it() {
        let config = RealtimeSessionOpenConfig::new(
            RealtimeTurningMode::ExplicitCommit,
            sample_identity(),
            Vec::new(),
            vec![Message::System(SystemMessage::new("seed-lead".to_string()))],
        );
        assert_eq!(config.system_prompt, None);

        let config = config.with_system_prompt(Some("authoritative".to_string()));
        assert_eq!(config.system_prompt.as_deref(), Some("authoritative"));
    }
}

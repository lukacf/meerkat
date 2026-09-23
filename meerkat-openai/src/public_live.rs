//! Public OpenAI Live (`gpt-live-1`) broker adapter over `oai_rt_rs::live`.
//!
//! This module owns provider mechanics only. It validates a registry-minted
//! realtime target for the public Live API, creates the WebRTC session with a
//! client delegation, attaches the server-side sideband, and lowers public
//! Live events to the shared [`GptLiveBrokerObservation`] vocabulary consumed
//! by the provider-neutral facade.
//!
//! The public Live API has no authoritative turn identifiers, completed-turn
//! events, or delegation task text. This adapter therefore synthesizes turns
//! from transcript role alternation and joins a client delegation to the most
//! recent user turn. Those joins are provider evidence only; transcript
//! admission, delegation meaning, channel policy, and model selection remain
//! outside this module.

use std::collections::{HashSet, VecDeque};

use meerkat_core::model_profile::catalog::ModelReleaseStage;
use meerkat_core::types::Message;
use meerkat_llm_core::provider_runtime::errors::ProviderClientError;
use meerkat_llm_core::provider_runtime::{NormalizedBackendKind, ResolvedRealtimeTarget};
use oai_rt_rs::live::{
    AudioConfig, AudioOutput, ClientEvent, ClientOptions, Command, CreateRequest, Delegation,
    DelegationConfig, DelegationTarget, DelegationType, Error as LiveError, Field, InitialItem,
    InitialRole, InitialText, InitialTextType, LiveClient, LiveReceiver, LiveSender, MessageType,
    Nullable, ServerEvent, ServerFrame, SessionConfig, Voice, WebRtcTransport,
};
use tokio::sync::Mutex;

use crate::OpenAiBackendKind;
use crate::gpt_live_broker::{
    GptLiveAppendToken, GptLiveBrokerError, GptLiveBrokerObservation, GptLiveBrokerTerminalClass,
    GptLiveDelegationRef, GptLiveDelegationTarget, GptLiveTranscriptItemRef, GptLiveTurnRef,
    GptLiveTurnRole, protocol_error, require_context, summarize_unknown_provider_event,
};

pub use crate::runtime::GPT_LIVE_MODEL_FAMILY;

/// Scoped diagnostic capture for offline fixtures and explicitly opted-in live
/// acceptance tests. No raw frames, credentials, SDP, or provider session IDs.
#[cfg(feature = "test-realtime-fixtures")]
#[doc(hidden)]
pub mod thinking_capture {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum EventKind {
        SessionAttached,
        /// The startup `session.input` and `instructions` shape of the
        /// create request: how many history items rode the documented
        /// history carrier, how many were developer-role (a seeded
        /// summary), and whether the startup instructions carry the
        /// history framing. Host-side only; the browser never sees the
        /// create body.
        SessionInputSeeded {
            input_items: usize,
            developer_items: usize,
            frames_history: bool,
        },
        ThinkingAppendAttempt {
            client_event_id: String,
            text: String,
        },
        ThinkingAppended {
            client_event_id: Option<String>,
            matched_owned: bool,
            accepted: bool,
        },
        InstructionsAppendAttempt {
            client_event_id: String,
            text: String,
        },
        InstructionsAppended {
            client_event_id: Option<String>,
            matched_owned: bool,
            accepted: bool,
        },
    }

    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct Event {
        pub channel_ordinal: u32,
        pub elapsed_ms: u64,
        pub event: EventKind,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Fault {
        Overflow,
        StringLimit,
        Contention,
    }

    struct Inner {
        started: Instant,
        events: Mutex<VecDeque<Event>>,
        fault: AtomicU8,
    }

    #[derive(Clone)]
    pub struct Capture {
        inner: Arc<Inner>,
        channel_ordinal: u32,
    }

    tokio::task_local! {
        static CURRENT: Capture;
    }

    impl Default for Capture {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Capture {
        pub const MAX_EVENTS: usize = 512;
        pub const MAX_TEXT_BYTES: usize = 1024;
        pub const MAX_ID_BYTES: usize = 256;

        pub fn new() -> Self {
            Self {
                inner: Arc::new(Inner {
                    started: Instant::now(),
                    events: Mutex::new(VecDeque::new()),
                    fault: AtomicU8::new(0),
                }),
                channel_ordinal: 0,
            }
        }

        pub fn for_channel(&self, channel_ordinal: u32) -> Self {
            Self {
                inner: self.inner.clone(),
                channel_ordinal,
            }
        }

        pub async fn scope<F: std::future::Future>(&self, future: F) -> F::Output {
            CURRENT.scope(self.clone(), future).await
        }

        pub(super) fn current() -> Option<Self> {
            CURRENT.try_with(Clone::clone).ok()
        }

        pub fn fault(&self) -> Option<Fault> {
            match self.inner.fault.load(Ordering::Acquire) {
                0 => None,
                1 => Some(Fault::Overflow),
                2 => Some(Fault::StringLimit),
                _ => Some(Fault::Contention),
            }
        }

        pub fn drain(&self) -> Result<Vec<Event>, Fault> {
            self.inner
                .events
                .lock()
                .map(|mut events| events.drain(..).collect())
                .map_err(|_| Fault::Contention)
        }

        pub(super) fn string_limit(&self) {
            self.inner.fault.store(2, Ordering::Release);
        }

        pub(super) fn record(&self, event: EventKind) {
            if self.fault().is_some() {
                return;
            }
            let valid = match &event {
                EventKind::SessionAttached | EventKind::SessionInputSeeded { .. } => true,
                EventKind::ThinkingAppendAttempt {
                    client_event_id,
                    text,
                }
                | EventKind::InstructionsAppendAttempt {
                    client_event_id,
                    text,
                } => {
                    client_event_id.len() <= Self::MAX_ID_BYTES
                        && text.len() <= Self::MAX_TEXT_BYTES
                }
                EventKind::ThinkingAppended {
                    client_event_id, ..
                }
                | EventKind::InstructionsAppended {
                    client_event_id, ..
                } => client_event_id
                    .as_ref()
                    .is_none_or(|id| id.len() <= Self::MAX_ID_BYTES),
            };
            if !valid {
                self.inner.fault.store(2, Ordering::Release);
                return;
            }
            let Ok(mut events) = self.inner.events.try_lock() else {
                self.inner.fault.store(3, Ordering::Release);
                return;
            };
            if events.len() == Self::MAX_EVENTS {
                self.inner.fault.store(1, Ordering::Release);
                return;
            }
            events.push_back(Event {
                channel_ordinal: self.channel_ordinal,
                elapsed_ms: u64::try_from(self.inner.started.elapsed().as_millis())
                    .unwrap_or(u64::MAX),
                event,
            });
        }
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn capture_is_opt_in_task_scoped_and_follows_the_selected_session_clone() {
            let capture = Capture::new().for_channel(7);
            assert!(Capture::current().is_none());
            let session_capture = capture
                .scope(async {
                    assert!(
                        tokio::spawn(async { Capture::current().is_none() })
                            .await
                            .unwrap()
                    );
                    Capture::current().unwrap()
                })
                .await;
            assert!(Capture::current().is_none());
            session_capture.record(EventKind::SessionAttached);
            assert_eq!(capture.drain().unwrap()[0].channel_ordinal, 7);
        }

        #[test]
        fn bounds_and_contention_latch_without_blocking_or_success_shaped_loss() {
            let capture = Capture::new();
            for _ in 0..=Capture::MAX_EVENTS {
                capture.record(EventKind::SessionAttached);
            }
            assert_eq!(capture.fault(), Some(Fault::Overflow));
            assert_eq!(capture.drain().unwrap().len(), Capture::MAX_EVENTS);
            let capture = Capture::new();
            capture.record(EventKind::ThinkingAppendAttempt {
                client_event_id: "owned".into(),
                text: "x".repeat(Capture::MAX_TEXT_BYTES + 1),
            });
            assert_eq!(capture.fault(), Some(Fault::StringLimit));
            let capture = Capture::new();
            let _lock = capture.inner.events.lock().unwrap();
            capture.record(EventKind::SessionAttached);
            assert_eq!(capture.fault(), Some(Fault::Contention));
        }
    }
}

/// Provider-owned mechanical configuration for one browser WebRTC bootstrap.
///
/// The public broker always starts the session with a client delegation: the
/// voice model speaks and the channel-bound Meerkat executor performs work.
#[derive(Clone)]
pub struct PublicLiveOpenConfig {
    offer_sdp: String,
    voice: String,
    instructions: Option<String>,
    context_seed: PublicLiveContextSeed,
}

#[derive(Clone)]
enum PublicLiveContextSeed {
    Absent,
    History(Vec<InitialItem>),
    /// A summary as one developer item, plus the most recent canonical turns
    /// it also covers, seeded verbatim within the startup input budget.
    FactualSummary {
        summary: String,
        recent: Vec<InitialItem>,
    },
    HistoricalContextPending,
}

/// Provider limits on startup history: 128 messages and 8,192 rendered
/// tokens. Tokens are estimated conservatively at three bytes each; the
/// provider's own tokenizer verdict stays visible as a startup error.
pub const LIVE_STARTUP_INPUT_MAX_ITEMS: usize = 128;
pub const LIVE_STARTUP_INPUT_TOKEN_BUDGET: usize = 8192;
const LIVE_STARTUP_INPUT_BYTES_PER_TOKEN: usize = 3;

/// What the startup input budget dropped to fit the provider limits. Recent
/// turns are dropped oldest first; the summary is never dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveStartupInputTruncation {
    pub dropped_items: usize,
    pub dropped_bytes: usize,
}

fn estimated_startup_tokens(item: &InitialItem) -> usize {
    let bytes: usize = item.content.iter().map(|part| part.text.len()).sum();
    bytes.div_ceil(LIVE_STARTUP_INPUT_BYTES_PER_TOKEN)
}

/// Compose the startup input from a leading item that is always kept and a
/// tail of recent items, dropping the oldest tail items until both limits
/// hold. The truncation is typed and reported by the caller.
fn budget_startup_input(
    keep: InitialItem,
    recent: &[InitialItem],
) -> (Vec<InitialItem>, LiveStartupInputTruncation) {
    let mut truncation = LiveStartupInputTruncation::default();
    let mut items = Vec::with_capacity(recent.len() + 1);
    let mut tokens = estimated_startup_tokens(&keep);
    // Newest first so the oldest are the ones left out.
    let mut kept_recent = Vec::new();
    for item in recent.iter().rev() {
        let item_tokens = estimated_startup_tokens(item);
        if kept_recent.len() + 1 < LIVE_STARTUP_INPUT_MAX_ITEMS
            && tokens + item_tokens <= LIVE_STARTUP_INPUT_TOKEN_BUDGET
        {
            tokens += item_tokens;
            kept_recent.push(item.clone());
        } else {
            truncation.dropped_items += 1;
            truncation.dropped_bytes += item
                .content
                .iter()
                .map(|part| part.text.len())
                .sum::<usize>();
        }
    }
    items.push(keep);
    items.extend(kept_recent.into_iter().rev());
    (items, truncation)
}

impl PublicLiveContextSeed {
    /// Prefix of the developer-role startup item that carries a summary.
    pub(crate) const SUMMARY_ITEM_PREFIX: &'static str = "Factual summary of the background agent's context at voice-channel open (context data, not a new user request):";

    /// History belongs in the session's startup `input`: real prior dialogue
    /// turns under their own roles, and a factual summary as one
    /// `developer`-role item. The provider documents `input` as the history
    /// carrier (up to 128 text messages) and it is read before the session
    /// starts, so it prompts no speech. A summary is never a user-role item:
    /// a user item at session start made the provider answer it with a fresh
    /// greeting. The pending notice is availability state, not history, and
    /// stays with the instructions.
    fn initial_input(&self) -> Option<Vec<InitialItem>> {
        match self {
            Self::History(items) => (!items.is_empty()).then(|| items.clone()),
            Self::FactualSummary { .. } => Some(self.startup_input_plan().0),
            Self::Absent | Self::HistoricalContextPending => None,
        }
    }

    /// The budgeted startup input for a summary seed and what the budget
    /// dropped. Only meaningful for [`Self::FactualSummary`].
    fn startup_input_plan(&self) -> (Vec<InitialItem>, LiveStartupInputTruncation) {
        match self {
            Self::FactualSummary { summary, recent } => {
                let developer = InitialItem {
                    role: InitialRole::Developer,
                    content: vec![InitialText {
                        text: format!("{}\n{summary}", Self::SUMMARY_ITEM_PREFIX),
                        text_type: Some(InitialTextType::InputText),
                    }],
                    id: Field::Absent,
                    status: Field::Absent,
                    item_type: Some(MessageType::Message),
                };
                let (items, truncation) = budget_startup_input(developer, recent);
                if truncation != LiveStartupInputTruncation::default() {
                    tracing::warn!(
                        dropped_items = truncation.dropped_items,
                        dropped_bytes = truncation.dropped_bytes,
                        max_items = LIVE_STARTUP_INPUT_MAX_ITEMS,
                        token_budget = LIVE_STARTUP_INPUT_TOKEN_BUDGET,
                        "public Live startup input dropped the oldest recent turns to fit the provider limits"
                    );
                }
                (items, truncation)
            }
            _ => (Vec::new(), LiveStartupInputTruncation::default()),
        }
    }

    /// Startup state for the instructions lane, appended after the caller's
    /// own instructions.
    fn instructions_context(&self) -> Option<String> {
        match self {
            Self::Absent | Self::History(_) | Self::FactualSummary { .. } => None,
            Self::HistoricalContextPending => Some(
                "Voice-channel context availability (factual state, not a new user request):\nHistorical session context is being prepared and is not yet available."
                    .to_string(),
            ),
        }
    }
}

impl std::fmt::Debug for PublicLiveContextSeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Absent => formatter.write_str("Absent"),
            Self::History(items) => formatter
                .debug_struct("History")
                .field("messages", &items.len())
                .finish(),
            Self::FactualSummary { recent, .. } => formatter
                .debug_struct("FactualSummary")
                .field("summary", &"<redacted>")
                .field("recent", &recent.len())
                .finish(),
            Self::HistoricalContextPending => formatter.write_str("HistoricalContextPending"),
        }
    }
}

impl PublicLiveOpenConfig {
    /// Construct the minimum verified public session shape.
    ///
    /// # Errors
    ///
    /// Returns a typed local validation error for blank SDP or voice input.
    pub fn new(
        offer_sdp: impl Into<String>,
        voice: impl Into<String>,
    ) -> Result<Self, GptLiveBrokerError> {
        let offer_sdp = offer_sdp.into();
        if offer_sdp.trim().is_empty() {
            return Err(GptLiveBrokerError::MissingOfferSdp);
        }
        let voice = voice.into();
        if voice.trim().is_empty() {
            return Err(GptLiveBrokerError::MissingVoice);
        }
        if !oai_rt_rs::live::VOICES.contains(&voice.as_str()) {
            // Public voice names are extensible and custom voices exist, so
            // this is not a rejection; the provider answers an unknown name
            // with HTTP 403 "Voice session access denied", which this note
            // makes diagnosable without exposing the configured value.
            tracing::warn!(
                known_voices = ?oai_rt_rs::live::VOICES,
                "public Live voice is not one of the released public voice names"
            );
        }
        Ok(Self {
            offer_sdp,
            voice,
            instructions: None,
            context_seed: PublicLiveContextSeed::Absent,
        })
    }

    /// Lower host-catalog guidance into the startup-only `session.instructions`.
    /// Raw `live/open` prose never reaches this seam.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Replay the selected canonical dialogue as startup history, not as a
    /// speech-prompting commentary append. Executor instructions and tool
    /// mechanics do not belong to the tool-less voice model's conversation.
    ///
    /// No history is truncated here. The provider's startup limits are
    /// validated by the protocol client and provider; oversize seeds reject
    /// channel creation rather than silently losing canonical context.
    #[must_use]
    pub fn with_history(mut self, messages: &[Message]) -> Self {
        let input = messages
            .iter()
            .filter_map(|message| {
                let (role, text_type, text) = match message {
                    Message::User(user) => (
                        InitialRole::User,
                        InitialTextType::InputText,
                        user.text_content(),
                    ),
                    Message::BlockAssistant(assistant) => (
                        InitialRole::Assistant,
                        InitialTextType::OutputText,
                        if assistant.blocks.iter().any(|block| {
                            matches!(
                                block,
                                meerkat_core::AssistantBlock::Transcript {
                                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                                    ..
                                }
                            )
                        }) {
                            meerkat_core::types::TranscriptSource::SpokenUnmeasured
                                .text_for_model(&assistant.to_string())
                                .into_owned()
                        } else {
                            assistant.to_string()
                        },
                    ),
                    Message::System(_) | Message::SystemNotice(_) | Message::ToolResults { .. } => {
                        return None;
                    }
                };
                (!text.is_empty()).then_some(InitialItem {
                    role,
                    content: vec![InitialText {
                        text,
                        text_type: Some(text_type),
                    }],
                    id: Field::Absent,
                    status: Field::Absent,
                    item_type: Some(MessageType::Message),
                })
            })
            .collect();
        self.context_seed = PublicLiveContextSeed::History(input);
        self
    }

    /// Lower an owner-generated factual summary as unprivileged startup
    /// history: one `developer`-role item in the session's startup `input`,
    /// followed by any history items already selected with
    /// [`Self::with_history`] (the recent turns the summary also covers),
    /// budgeted to the provider limits with the oldest turns dropped first.
    /// This never changes the catalog-owned behavior instructions and asks
    /// for no speech.
    #[must_use]
    pub fn with_context_summary(mut self, summary: &str) -> Self {
        let recent = match std::mem::replace(&mut self.context_seed, PublicLiveContextSeed::Absent)
        {
            PublicLiveContextSeed::History(items) => items,
            PublicLiveContextSeed::FactualSummary { recent, .. } => recent,
            PublicLiveContextSeed::Absent | PublicLiveContextSeed::HistoricalContextPending => {
                Vec::new()
            }
        };
        self.context_seed = PublicLiveContextSeed::FactualSummary {
            summary: summary.to_owned(),
            recent,
        };
        self
    }

    /// What the startup input budget would drop for this configuration.
    #[must_use]
    pub fn startup_input_truncation(&self) -> LiveStartupInputTruncation {
        self.context_seed.startup_input_plan().1
    }

    /// Declare that historical context is not yet available when media opens.
    ///
    /// The provider-owned availability fact is native startup input, not
    /// instructions, a summary, canonical replay, or speech-prompting
    /// commentary. It requires no summary work or sideband acknowledgement.
    /// Later factual context arrives through [`PublicLiveBrokerSession::append_thinking_context`].
    #[must_use]
    pub fn with_pending_context(mut self) -> Self {
        self.context_seed = PublicLiveContextSeed::HistoricalContextPending;
        self
    }
}

impl std::fmt::Debug for PublicLiveOpenConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicLiveOpenConfig")
            .field("offer_sdp", &"<redacted>")
            .field("voice", &"<redacted>")
            .field(
                "instructions",
                &self.instructions.as_ref().map(|_| "<catalog-bound>"),
            )
            .field("context_seed", &self.context_seed)
            .finish()
    }
}

/// Concrete provider factory admitted from one exact resolved realtime target.
pub struct PublicLiveBrokerFactory {
    model: String,
    client: LiveClient,
    #[cfg(feature = "test-realtime-fixtures")]
    thinking_capture: Option<thinking_capture::Capture>,
}

impl std::fmt::Debug for PublicLiveBrokerFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicLiveBrokerFactory")
            .field("model", &"<registry-admitted>")
            .field("client", &"<public live client>")
            .finish()
    }
}

struct AdmittedPublicLiveTarget {
    model: String,
    api_key: String,
    base_url: Option<String>,
}

impl PublicLiveBrokerFactory {
    /// Build the public broker only from catalog and binding evidence.
    ///
    /// The witness must classify the identity as a released, realtime-capable
    /// `gpt-live` family model, and the resolved connection must be the plain
    /// OpenAI API backend with inline API-key material.
    pub fn try_from_target(target: ResolvedRealtimeTarget) -> Result<Self, ProviderClientError> {
        let admitted = Self::admit_target(target)?;
        let options = match admitted.base_url.as_deref() {
            Some(base_url) => Self::options_for_base_url(base_url)?,
            None => ClientOptions::default(),
        };
        Self::from_admitted(admitted, options)
    }

    fn options_for_base_url(base_url: &str) -> Result<ClientOptions, ProviderClientError> {
        Ok(ClientOptions {
            base_url: base_url
                .parse()
                .map_err(|_| ProviderClientError::InvalidBaseUrl(base_url.to_string()))?,
            ..ClientOptions::default()
        })
    }

    /// Test-only base URL injection after real admission has been consumed
    /// into provider custody. This retains the exact admitted target and
    /// changes only the public HTTP and WebSocket destination.
    #[cfg(feature = "test-realtime-fixtures")]
    #[doc(hidden)]
    pub fn __try_from_target_with_base_url(
        target: ResolvedRealtimeTarget,
        base_url: &str,
    ) -> Result<Self, ProviderClientError> {
        let admitted = Self::admit_target(target)?;
        Self::from_admitted(admitted, Self::options_for_base_url(base_url)?)
    }

    fn from_admitted(
        admitted: AdmittedPublicLiveTarget,
        options: ClientOptions,
    ) -> Result<Self, ProviderClientError> {
        let client = LiveClient::with_options(&admitted.api_key, options).map_err(|_| {
            ProviderClientError::ClientInit("failed to construct public Live client".to_string())
        })?;
        Ok(Self {
            model: admitted.model,
            client,
            #[cfg(feature = "test-realtime-fixtures")]
            thinking_capture: thinking_capture::Capture::current(),
        })
    }

    fn admit_target(
        target: ResolvedRealtimeTarget,
    ) -> Result<AdmittedPublicLiveTarget, ProviderClientError> {
        let profile = target.profile().profile();
        if profile.model_family != GPT_LIVE_MODEL_FAMILY {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-model-family",
            ));
        }
        if profile.release_stage == ModelReleaseStage::Experimental {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-released-model",
            ));
        }
        if !profile.realtime {
            return Err(ProviderClientError::MissingFeature("openai-live-realtime"));
        }
        let (identity, _, connection) = target.into_parts();
        if !matches!(
            connection.backend,
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi)
        ) {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-openai-api-backend",
            ));
        }
        if connection.resolved_authorizer().is_some() {
            return Err(ProviderClientError::MissingFeature(
                "openai-live-authorizer-auth",
            ));
        }
        let api_key = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        Ok(AdmittedPublicLiveTarget {
            model: identity.model,
            api_key,
            base_url: connection.backend_profile.base_url.clone(),
        })
    }

    /// Create the public WebRTC session and attach its sideband before
    /// returning.
    ///
    /// The answer SDP remains opaque browser bootstrap data. The returned
    /// session keeps the provider session identity and raw events inside the
    /// OpenAI adapter boundary.
    pub async fn open(
        &self,
        config: PublicLiveOpenConfig,
    ) -> Result<PublicLiveBootstrap, GptLiveBrokerError> {
        let session = self.session_config(&config);
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(capture) = &self.thinking_capture {
            let items = session.input.as_deref().unwrap_or(&[]);
            capture.record(thinking_capture::EventKind::SessionInputSeeded {
                input_items: items.len(),
                developer_items: items
                    .iter()
                    .filter(|item| item.role == InitialRole::Developer)
                    .count(),
                frames_history: matches!(
                    &session.instructions,
                    Field::Value(text) if text.contains("Conversation history:")
                ),
            });
        }
        let request = CreateRequest {
            session,
            transport: WebRtcTransport::WebRtc {
                sdp: config.offer_sdp,
            },
        };
        let created = self
            .client
            .create_webrtc(&request)
            .await
            .map_err(map_live_error)?;
        let answer_sdp = created.transport.sdp().to_string();
        let sideband = self
            .client
            .attach(&created.session.id)
            .await
            .map_err(map_live_error)?;
        let (sender, receiver) = sideband.split();
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(capture) = &self.thinking_capture {
            capture.record(thinking_capture::EventKind::SessionAttached);
        }
        Ok(PublicLiveBootstrap {
            answer_sdp,
            session: PublicLiveBrokerSession {
                sender,
                receiver: Mutex::new(receiver),
                state: Mutex::new(SessionState::default()),
                #[cfg(feature = "test-realtime-fixtures")]
                thinking_capture: self.thinking_capture.clone(),
            },
        })
    }

    fn session_config(&self, config: &PublicLiveOpenConfig) -> SessionConfig {
        SessionConfig {
            model: self.model.clone(),
            // WebRTC negotiates media; only the voice is selected here.
            audio: Some(AudioConfig {
                format: None,
                output: Some(AudioOutput {
                    voice: Some(Voice::Named(config.voice.clone())),
                }),
            }),
            client: None,
            delegation: Field::Value(DelegationConfig::Client),
            input: config.context_seed.initial_input(),
            instructions: Self::session_instructions(config).map_or(Field::Absent, Field::Value),
            store: None,
        }
    }

    /// Caller instructions first, then any startup context, separated by a
    /// blank line. Absent when neither exists.
    fn session_instructions(config: &PublicLiveOpenConfig) -> Option<String> {
        match (
            config.instructions.as_deref(),
            config.context_seed.instructions_context(),
        ) {
            (None, None) => None,
            (Some(instructions), None) => Some(instructions.to_string()),
            (None, Some(context)) => Some(context),
            (Some(instructions), Some(context)) => Some(format!("{instructions}\n\n{context}")),
        }
    }
}

/// Browser-facing SDP answer paired with a provider-owned opaque broker session.
pub struct PublicLiveBootstrap {
    answer_sdp: String,
    session: PublicLiveBrokerSession,
}

impl std::fmt::Debug for PublicLiveBootstrap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicLiveBootstrap")
            .field("answer_sdp", &"<redacted>")
            .field("session", &self.session)
            .finish()
    }
}

impl PublicLiveBootstrap {
    /// Borrow the answer SDP for the browser peer connection.
    #[must_use]
    pub fn answer_sdp(&self) -> &str {
        &self.answer_sdp
    }

    /// Transfer the opaque broker session to its provider-owned host.
    #[must_use]
    pub fn into_parts(self) -> (String, PublicLiveBrokerSession) {
        (self.answer_sdp, self.session)
    }
}

/// Opaque connected sideband handle for one public Live session.
///
/// Provider session identity and raw events are intentionally not exposed.
/// Events are lowered to sanitized observations inside this crate.
pub struct PublicLiveBrokerSession {
    sender: LiveSender,
    receiver: Mutex<LiveReceiver>,
    state: Mutex<SessionState>,
    #[cfg(feature = "test-realtime-fixtures")]
    thinking_capture: Option<thinking_capture::Capture>,
}

impl std::fmt::Debug for PublicLiveBrokerSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PublicLiveBrokerSession(<connected>)")
    }
}

impl PublicLiveBrokerSession {
    /// Seed one ordered canonical commentary envelope after the answer has
    /// been delivered and the sideband has attached. The exact commentary
    /// acknowledgement is the server-side readiness evidence when a seed
    /// exists. Valid observations that race ahead of the acknowledgement are
    /// preserved in order. Any missing, ambiguous, or mismatched
    /// acknowledgement closes the partial session; this method never retries.
    pub async fn await_ready_and_seed_session_context(
        &self,
        commentary: Option<String>,
    ) -> Result<(), GptLiveBrokerError> {
        let seeded = async {
            let Some(commentary) = commentary else {
                return Ok(());
            };
            let token = self.append_session_context(commentary).await?;
            let mut deferred = VecDeque::new();
            loop {
                match self.next_observation().await? {
                    Some(GptLiveBrokerObservation::SessionContextAppendAcknowledged {
                        token: acknowledged,
                    }) if acknowledged == token => break,
                    Some(GptLiveBrokerObservation::SessionContextAppendAcknowledged { .. }) => {
                        return Err(protocol_error());
                    }
                    Some(GptLiveBrokerObservation::UnsupportedProviderEvent) => {
                        return Err(protocol_error());
                    }
                    Some(observation) => deferred.push_back(observation),
                    None => {
                        return Err(GptLiveBrokerError::Transport {
                            class: GptLiveBrokerTerminalClass::Protocol,
                        });
                    }
                }
            }
            if !deferred.is_empty() {
                let mut state = self.state.lock().await;
                deferred.append(&mut state.queued_observations);
                state.queued_observations = deferred;
            }
            Ok(())
        }
        .await;
        if seeded.is_err() {
            let _ = self.close().await;
        }
        seeded
    }

    /// Append canonical Meerkat context as commentary without granting it
    /// automatic speech.
    ///
    /// Every append carries a `client_event_id`; the provider echoes it on the
    /// acknowledgement, which is how pending appends are correlated. An
    /// acknowledgement without an id is accepted only while exactly one append
    /// is pending; otherwise it is ambiguous and fails closed. A send failure
    /// is classified as ambiguous and fails every later append closed so an
    /// unacknowledged write can never be silently retried or misattributed.
    pub async fn append_session_context(
        &self,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let token = self
            .state
            .lock()
            .await
            .reserve_append(PendingAppendLane::Session)?;
        let event = Self::commentary_event(token, text, Nullable(None));
        self.deliver_append(token, event).await
    }

    /// Append factual background data through the quiet native thinking lane.
    ///
    /// This neither changes instructions nor requests speech or a response.
    /// Content is preserved in UTF-8 fragments of at most 500 bytes, a
    /// conservative payload bound for the provider's 500-token append limit;
    /// actual tokenizer rejection remains provider evidence. At most 64
    /// outstanding fragments are admitted across all append lanes.
    ///
    /// One local token identifies the whole append. Only exact, lane-matching
    /// receipts for every fragment acknowledge it. Any rejection may follow
    /// partial consumption and never authorizes replay. Native errors report
    /// rejection; only confirmed session closure reports interruption by close.
    /// Send failures preserve the same ambiguous-delivery/no-retry contract as
    /// commentary appends.
    pub async fn append_thinking_context(
        &self,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let fragments = context_fragments(&text);
        let token = self
            .state
            .lock()
            .await
            .reserve_thinking_append(fragments.len())?;
        for (index, content) in fragments.into_iter().enumerate() {
            let event = Self::thinking_event(token, index, content.to_owned());
            self.deliver_append(token, event).await?;
        }
        Ok(token)
    }

    /// Append trusted background knowledge through the native instructions
    /// lane. The provider treats instructions as authoritative context the
    /// model may use to answer; unlike the thinking lane it is not limited to
    /// quiet progress notes, and the provider may interrupt speech to apply
    /// it. Fragmenting, receipts, rejection, and close semantics match the
    /// thinking lane.
    pub async fn append_instructions_context(
        &self,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        self.append_instructions_context_sections(vec![text.into()])
            .await
    }

    /// Append trusted knowledge as ordered sections that never share a
    /// fragment: every section starts at a fragment boundary, so a framing
    /// preface is acknowledged as its own fragment(s) and the knowledge it
    /// frames arrives intact from the first byte of the next fragment
    /// (measured against gpt-live-1: a summary sentence cut mid-word across
    /// the framing's fragment boundary was recalled about half the time).
    /// Blank sections are skipped; the total must be non-empty. One token,
    /// exact receipts, and the same rejection and close semantics as
    /// [`Self::append_instructions_context`].
    pub async fn append_instructions_context_sections(
        &self,
        sections: Vec<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let fragments: Vec<String> = sections
            .iter()
            .filter(|section| !section.trim().is_empty())
            .flat_map(|section| context_fragments(section).into_iter().map(str::to_owned))
            .collect();
        if fragments.is_empty() {
            return Err(GptLiveBrokerError::MissingContext);
        }
        let token = self
            .state
            .lock()
            .await
            .reserve_instructions_append(fragments.len())?;
        for (index, content) in fragments.into_iter().enumerate() {
            let event = Self::instructions_event(token, index, content);
            self.deliver_append(token, event).await?;
        }
        Ok(token)
    }

    /// Append executor context to an observed client delegation.
    ///
    /// The provider identifier remains inside the opaque delegation reference.
    /// Results and progress are always appended as commentary; the live model
    /// alone decides whether and how to speak from that context.
    pub async fn append_delegation_context(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let token = self
            .state
            .lock()
            .await
            .reserve_append(PendingAppendLane::Delegation)?;
        let event = Self::commentary_event(token, text, Nullable(Some(delegation.0.clone())));
        self.deliver_append(token, event).await
    }

    fn commentary_event(
        token: GptLiveAppendToken,
        content: String,
        delegation_id: Nullable<String>,
    ) -> ClientEvent {
        ClientEvent {
            event_id: Field::Value(pending_event_id(token)),
            command: Command::CommentaryAppend {
                content,
                delegation_id,
            },
        }
    }

    fn instructions_event(token: GptLiveAppendToken, index: usize, content: String) -> ClientEvent {
        ClientEvent {
            event_id: Field::Value(instructions_event_id(token, index)),
            command: Command::InstructionsAppend {
                content,
                delegation_id: Nullable(None),
            },
        }
    }

    fn thinking_event(token: GptLiveAppendToken, index: usize, content: String) -> ClientEvent {
        ClientEvent {
            event_id: Field::Value(thinking_event_id(token, index)),
            command: Command::ThinkingAppend {
                content,
                delegation_id: Nullable(None),
            },
        }
    }

    async fn deliver_append(
        &self,
        token: GptLiveAppendToken,
        event: ClientEvent,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(capture) = &self.thinking_capture
            && let ClientEvent {
                event_id: Field::Value(client_event_id),
                command: Command::ThinkingAppend { content, .. },
            } = &event
        {
            capture.record(thinking_capture::EventKind::ThinkingAppendAttempt {
                client_event_id: client_event_id.clone(),
                text: content.clone(),
            });
        }
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(capture) = &self.thinking_capture
            && let ClientEvent {
                event_id: Field::Value(client_event_id),
                command: Command::InstructionsAppend { content, .. },
            } = &event
        {
            capture.record(thinking_capture::EventKind::InstructionsAppendAttempt {
                client_event_id: client_event_id.clone(),
                text: content.clone(),
            });
        }
        if self.sender.send(event).await.is_err() {
            self.state.lock().await.append_delivery_ambiguous = true;
            return Err(GptLiveBrokerError::AppendDeliveryAmbiguous { token });
        }
        Ok(token)
    }

    /// Receive and sanitize one sideband event.
    ///
    /// Turn boundaries are synthesized from transcript role alternation inside
    /// this serialized receive owner. A client delegation is joined to the
    /// open or most recent user turn and is the sole terminal observation for
    /// an open user turn. This is provider evidence only and grants no
    /// executor authority.
    pub async fn next_observation(
        &self,
    ) -> Result<Option<GptLiveBrokerObservation>, GptLiveBrokerError> {
        let mut receiver = self.receiver.lock().await;
        loop {
            {
                let mut state = self.state.lock().await;
                if let Some(observation) = state.queued_observations.pop_front() {
                    tracing::debug!(?observation, "public Live lowered a sideband observation");
                    return Ok(Some(observation));
                }
            }
            // Missing transcript events are neither silence nor completion.
            // Keep the same output identity across pauses, including while a
            // delegation result is being injected into the provider context.
            let next = receiver.next_event().await;
            let Some(frame) = next.map_err(map_live_error)? else {
                if !self.state.lock().await.closed_observed {
                    return Err(GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::WebSocket,
                    });
                }
                return Ok(None);
            };
            let mut state = self.state.lock().await;
            #[cfg(feature = "test-realtime-fixtures")]
            let instructions_ack = self.thinking_capture.as_ref().and_then(|capture| {
                if !matches!(&frame.event, ServerEvent::InstructionsAppended { .. }) {
                    return None;
                }
                if frame
                    .client_event_id
                    .as_ref()
                    .is_some_and(|id| id.len() > thinking_capture::Capture::MAX_ID_BYTES)
                {
                    capture.string_limit();
                    return None;
                }
                let matched_owned = frame
                    .client_event_id
                    .as_deref()
                    .and_then(|id| state.find_append_receipt(id))
                    .is_some_and(|(append, _)| {
                        state.pending_appends[append.0].lane == PendingAppendLane::Instructions
                    });
                Some((frame.client_event_id.clone(), matched_owned))
            });
            #[cfg(feature = "test-realtime-fixtures")]
            let thinking_ack = self.thinking_capture.as_ref().and_then(|capture| {
                if !matches!(&frame.event, ServerEvent::ThinkingAppended { .. }) {
                    return None;
                }
                if frame
                    .client_event_id
                    .as_ref()
                    .is_some_and(|id| id.len() > thinking_capture::Capture::MAX_ID_BYTES)
                {
                    capture.string_limit();
                    return None;
                }
                let matched_owned = frame
                    .client_event_id
                    .as_deref()
                    .and_then(|id| state.find_append_receipt(id))
                    .is_some_and(|(append, _)| {
                        state.pending_appends[append.0].lane == PendingAppendLane::Thinking
                    });
                Some((frame.client_event_id.clone(), matched_owned))
            });
            let applied = state.apply_frame(frame);
            #[cfg(feature = "test-realtime-fixtures")]
            if let Some(capture) = &self.thinking_capture
                && let Some((client_event_id, matched_owned)) = instructions_ack
            {
                capture.record(thinking_capture::EventKind::InstructionsAppended {
                    client_event_id,
                    matched_owned,
                    accepted: applied.is_ok(),
                });
            }
            #[cfg(feature = "test-realtime-fixtures")]
            if let Some(capture) = &self.thinking_capture
                && let Some((client_event_id, matched_owned)) = thinking_ack
            {
                capture.record(thinking_capture::EventKind::ThinkingAppended {
                    client_event_id,
                    matched_owned,
                    accepted: applied.is_ok(),
                });
            }
            applied?;
        }
    }

    /// Request `session.close` through the sideband without exposing its
    /// wire identity. Successful repeated requests are idempotent. Continue
    /// draining observations: only `session.closed`, not this send or a bare
    /// transport EOF, confirms physical closure.
    pub async fn close(&self) -> Result<(), GptLiveBrokerError> {
        let mut state = self.state.lock().await;
        if state.close_requested || state.closed_observed {
            return Ok(());
        }
        // Measured against gpt-live-1: a pending quiet (thinking) append is
        // injected and acknowledged only at an input frame stall, and the
        // provider withholds `session.closed` until then. With microphone
        // audio still flowing that stall never comes. Muting input first
        // creates it, so a close issued while an append is in flight can
        // complete instead of waiting on media the client has not stopped.
        self.sender
            .send(ClientEvent::new(Command::InputAudioMute))
            .await
            .map_err(map_live_error)?;
        self.sender
            .send(ClientEvent::new(Command::Close))
            .await
            .map_err(map_live_error)?;
        state.close_requested = true;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingAppendLane {
    Session,
    Delegation,
    Thinking,
    Instructions,
}

impl PendingAppendLane {
    fn acknowledged(self, token: GptLiveAppendToken) -> GptLiveBrokerObservation {
        match self {
            Self::Session => GptLiveBrokerObservation::SessionContextAppendAcknowledged { token },
            Self::Delegation => {
                GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token }
            }
            Self::Thinking => GptLiveBrokerObservation::ThinkingContextAppendAcknowledged { token },
            Self::Instructions => {
                GptLiveBrokerObservation::InstructionsContextAppendAcknowledged { token }
            }
        }
    }

    fn rejected(self, token: GptLiveAppendToken) -> GptLiveBrokerObservation {
        match self {
            Self::Session => GptLiveBrokerObservation::SessionContextAppendRejected { token },
            Self::Delegation => GptLiveBrokerObservation::DelegationContextAppendRejected { token },
            Self::Thinking => GptLiveBrokerObservation::ThinkingContextAppendRejected { token },
            Self::Instructions => {
                GptLiveBrokerObservation::InstructionsContextAppendRejected { token }
            }
        }
    }
}

impl PendingAppendLane {
    /// Lanes delivered as bounded UTF-8 fragments with one receipt each.
    fn is_fragmented(self) -> bool {
        matches!(self, Self::Thinking | Self::Instructions)
    }

    fn interrupted_by_close(self, token: GptLiveAppendToken) -> Option<GptLiveBrokerObservation> {
        match self {
            Self::Thinking => {
                Some(GptLiveBrokerObservation::ThinkingContextAppendInterruptedByClose { token })
            }
            Self::Instructions => Some(
                GptLiveBrokerObservation::InstructionsContextAppendInterruptedByClose { token },
            ),
            Self::Session | Self::Delegation => None,
        }
    }
}

#[derive(Clone, Copy)]
enum AppendReceiptKind {
    Commentary,
    Thinking,
    Instructions,
}

impl AppendReceiptKind {
    fn matches(self, lane: PendingAppendLane) -> bool {
        matches!(
            (self, lane),
            (
                Self::Commentary,
                PendingAppendLane::Session | PendingAppendLane::Delegation
            ) | (Self::Thinking, PendingAppendLane::Thinking)
                | (Self::Instructions, PendingAppendLane::Instructions)
        )
    }
}

struct PendingAppend {
    lane: PendingAppendLane,
    token: GptLiveAppendToken,
    outstanding_receipts: Vec<String>,
    rejected: bool,
}

struct PendingAppendIndex(usize);
struct PendingReceiptIndex(usize);

struct OpenTurn {
    provider_ref: String,
    role: GptLiveTurnRole,
    /// Transcript deltas in arrival order.
    segments: Vec<String>,
    /// Index of the first segment that arrived after the most recent
    /// `session.delegation.created`; earlier segments belong to the
    /// previous delegation window.
    window_start: usize,
}

impl OpenTurn {
    fn window_transcript(&self) -> String {
        join_segments(self.segments.iter().skip(self.window_start))
    }
}

fn join_segments<'a>(segments: impl IntoIterator<Item = &'a String>) -> String {
    segments.into_iter().map(String::as_str).collect()
}

/// Whole-turn transcripts separated by one space; blank turns are skipped.
/// Deltas inside one turn are joined exactly; the separator only marks a
/// synthesized turn boundary inside the window.
fn join_window_chunks<'a>(chunks: impl IntoIterator<Item = &'a str>) -> String {
    let mut joined = String::new();
    for chunk in chunks {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(chunk);
    }
    joined
}

struct FinishedUserTurn {
    transcript: String,
}

/// Transcript received since the previous `session.delegation.created` (or
/// since open), by role, as whole-turn chunks in arrival order. This is the
/// executor-input rule: the provider defines no turn boundary and its
/// backchannels are designed behaviour, so assistant output never ends the
/// user's request. Turn synthesis stays a display concern.
#[derive(Default)]
struct DelegationWindow {
    user_chunks: Vec<String>,
    assistant_chunks: Vec<String>,
}

impl DelegationWindow {
    fn push(&mut self, role: GptLiveTurnRole, chunk: String) {
        if chunk.trim().is_empty() {
            return;
        }
        match role {
            GptLiveTurnRole::User => self.user_chunks.push(chunk),
            GptLiveTurnRole::Assistant | GptLiveTurnRole::Unknown => {
                self.assistant_chunks.push(chunk);
            }
        }
    }
}

struct SessionState {
    next_append_token: u64,
    pending_appends: VecDeque<PendingAppend>,
    append_delivery_ambiguous: bool,
    next_turn: u64,
    next_transcript_item: u64,
    open_turn: Option<OpenTurn>,
    last_user_turn: Option<FinishedUserTurn>,
    /// The previous delegation's executor request, re-presented when a
    /// delegation arrives without any new user transcript in its window.
    last_request: Option<String>,
    window: DelegationWindow,
    seen_delegation_ids: HashSet<String>,
    queued_observations: VecDeque<GptLiveBrokerObservation>,
    reflected_output_audio_frames: u64,
    close_requested: bool,
    closed_observed: bool,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            next_append_token: 1,
            pending_appends: VecDeque::new(),
            append_delivery_ambiguous: false,
            next_turn: 0,
            next_transcript_item: 0,
            open_turn: None,
            last_user_turn: None,
            last_request: None,
            window: DelegationWindow::default(),
            seen_delegation_ids: HashSet::new(),
            queued_observations: VecDeque::new(),
            reflected_output_audio_frames: 0,
            close_requested: false,
            closed_observed: false,
        }
    }
}

impl SessionState {
    const MAX_DELEGATION_IDENTITIES: usize = 4096;
    const MAX_PENDING_APPENDS: usize = 64;

    fn reserve_append(
        &mut self,
        lane: PendingAppendLane,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        self.reserve_append_fragments(lane, 1)
    }

    fn reserve_thinking_append(
        &mut self,
        count: usize,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        self.reserve_append_fragments(PendingAppendLane::Thinking, count)
    }

    fn reserve_instructions_append(
        &mut self,
        count: usize,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        self.reserve_append_fragments(PendingAppendLane::Instructions, count)
    }

    fn outstanding_receipt_count(&self) -> usize {
        self.pending_appends
            .iter()
            .map(|pending| pending.outstanding_receipts.len())
            .sum()
    }

    fn reserve_append_fragments(
        &mut self,
        lane: PendingAppendLane,
        count: usize,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        if self.append_delivery_ambiguous
            || count == 0
            || count > Self::MAX_PENDING_APPENDS.saturating_sub(self.outstanding_receipt_count())
        {
            return Err(GptLiveBrokerError::AppendInFlight);
        }
        let token = GptLiveAppendToken(self.next_append_token);
        self.next_append_token = self.next_append_token.saturating_add(1);
        let outstanding_receipts = match lane {
            PendingAppendLane::Thinking => (0..count)
                .map(|index| thinking_event_id(token, index))
                .collect(),
            PendingAppendLane::Instructions => (0..count)
                .map(|index| instructions_event_id(token, index))
                .collect(),
            PendingAppendLane::Session | PendingAppendLane::Delegation => {
                vec![pending_event_id(token)]
            }
        };
        self.pending_appends.push_back(PendingAppend {
            lane,
            token,
            outstanding_receipts,
            rejected: false,
        });
        Ok(token)
    }

    fn apply_frame(&mut self, frame: ServerFrame) -> Result<(), GptLiveBrokerError> {
        let ServerFrame {
            event,
            client_event_id,
            raw,
        } = frame;
        match event {
            // Readiness is a host-owned fact established by the seed
            // acknowledgement; a sideband `session.started` (or its replay)
            // is not projected as a second readiness observation.
            ServerEvent::Started { .. } | ServerEvent::Updated { .. } => {}
            ServerEvent::Closed { .. } => {
                // The SDK ends the stream after the terminal event; flush the
                // open turn so its final transcript is not lost.
                self.closed_observed = true;
                self.finish_open_turn();
                let observations = &mut self.queued_observations;
                self.pending_appends.retain(|pending| {
                    let Some(interrupted) = pending.lane.interrupted_by_close(pending.token) else {
                        return true;
                    };
                    if !pending.rejected {
                        observations.push_back(interrupted);
                    }
                    false
                });
            }
            ServerEvent::CommentaryAppended { .. } => {
                self.acknowledge_append(AppendReceiptKind::Commentary, client_event_id.as_deref())?;
            }
            ServerEvent::ThinkingAppended { .. } => {
                self.acknowledge_append(AppendReceiptKind::Thinking, client_event_id.as_deref())?;
            }
            ServerEvent::InstructionsAppended { .. } => {
                self.acknowledge_append(
                    AppendReceiptKind::Instructions,
                    client_event_id.as_deref(),
                )?;
            }
            ServerEvent::InputTranscriptDelta {
                delta,
                start_ms,
                end_ms,
                ..
            } => {
                // Timeline span only; the text never reaches the log.
                tracing::debug!(start_ms, end_ms, "public Live input transcript delta span");
                self.record_transcript_delta(GptLiveTurnRole::User, delta);
            }
            ServerEvent::OutputTranscriptDelta { delta, .. } => {
                self.record_transcript_delta(GptLiveTurnRole::Assistant, delta);
            }
            ServerEvent::DelegationCreated {
                delegation,
                offset_ms,
                ..
            } => {
                self.record_delegation(delegation, offset_ms)?;
            }
            // Reflected media, mute state, accounting telemetry, telephony
            // signalling, and informational notices carry no conversational,
            // transcript, delegation, or effect authority.
            ServerEvent::OutputAudioDelta { .. } => {
                // Reflected media is not evidence of transcript or playback
                // completion; frames may also represent silence.
                self.reflected_output_audio_frames =
                    self.reflected_output_audio_frames.saturating_add(1);
                if self.reflected_output_audio_frames.is_multiple_of(50) {
                    tracing::debug!(
                        frames = self.reflected_output_audio_frames,
                        "public Live sideband reflected output audio"
                    );
                }
            }
            ServerEvent::InputAudio { .. }
            | ServerEvent::InputAudioMuted { .. }
            | ServerEvent::InputAudioUnmuted { .. }
            | ServerEvent::UsageUpdated { .. }
            | ServerEvent::Info { .. }
            | ServerEvent::DtmfReceived { .. }
            | ServerEvent::DtmfSend { .. }
            | ServerEvent::Ringing { .. }
            | ServerEvent::Answered { .. }
            | ServerEvent::TransportFailed { .. } => {}
            ServerEvent::Response { .. } => {
                // Client delegation never produces backend Responses events.
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::UnsupportedProviderEvent);
            }
            ServerEvent::Error { error, .. } => {
                if let (Some(inner), Some(outer)) =
                    (error.client_event_id.as_deref(), client_event_id.as_deref())
                    && inner != outer
                {
                    return Err(protocol_error());
                }
                let rejected = error
                    .client_event_id
                    .as_deref()
                    .or(client_event_id.as_deref());
                let summary = summarize_unknown_provider_event("error", &raw);
                tracing::warn!(
                    provider_event_class = "error",
                    error_class = summary.error_class,
                    top_level_field_count = summary.top_level_field_count,
                    normalized_json_bytes = summary.normalized_json_bytes,
                    message_bytes = summary.message_bytes,
                    "public Live reported a provider error on the sideband"
                );
                if let Some((append_index, receipt_index)) =
                    rejected.and_then(|id| self.find_append_receipt(id))
                {
                    let pending = &mut self.pending_appends[append_index.0];
                    pending.outstanding_receipts.remove(receipt_index.0);
                    let report_rejection = self.close_requested || pending.lane.is_fragmented();
                    if report_rejection && !pending.rejected {
                        self.queued_observations
                            .push_back(pending.lane.rejected(pending.token));
                        pending.rejected = true;
                    }
                    // Retain unmatched fragments after partial rejection so
                    // later exact receipts can drain without fabricating ACK.
                    if pending.outstanding_receipts.is_empty() {
                        self.pending_appends.remove(append_index.0);
                    }
                    if report_rejection {
                        return Ok(());
                    }
                }
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::UnsupportedProviderEvent);
            }
            ServerEvent::Unknown => {
                let kind = raw
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let summary = summarize_unknown_provider_event(kind, &raw);
                tracing::warn!(
                    provider_event_class = "unknown",
                    event_kind_sha256 = %summary.event_kind_sha256,
                    error_class = summary.error_class,
                    top_level_field_count = summary.top_level_field_count,
                    normalized_json_bytes = summary.normalized_json_bytes,
                    message_bytes = summary.message_bytes,
                    "public Live received an unsupported sideband event"
                );
                self.queued_observations
                    .push_back(GptLiveBrokerObservation::UnsupportedProviderEvent);
            }
        }
        Ok(())
    }

    fn find_append_receipt(&self, id: &str) -> Option<(PendingAppendIndex, PendingReceiptIndex)> {
        self.pending_appends
            .iter()
            .enumerate()
            .find_map(|(append_index, pending)| {
                pending
                    .outstanding_receipts
                    .iter()
                    .position(|receipt| receipt == id)
                    .map(|receipt_index| {
                        (
                            PendingAppendIndex(append_index),
                            PendingReceiptIndex(receipt_index),
                        )
                    })
            })
    }

    /// Existing single-commentary ID-less receipts remain supported. Quiet
    /// thinking fragments always require the exact echoed ID and native kind.
    fn acknowledge_append(
        &mut self,
        kind: AppendReceiptKind,
        client_event_id: Option<&str>,
    ) -> Result<(), GptLiveBrokerError> {
        let indices = match client_event_id {
            Some(id) => self.find_append_receipt(id),
            None if matches!(kind, AppendReceiptKind::Commentary)
                && self.outstanding_receipt_count() == 1 =>
            {
                Some((PendingAppendIndex(0), PendingReceiptIndex(0)))
            }
            None => None,
        }
        .ok_or_else(protocol_error)?;
        let (append_index, receipt_index) = indices;
        let pending = &mut self.pending_appends[append_index.0];
        if !kind.matches(pending.lane) {
            return Err(protocol_error());
        }
        pending.outstanding_receipts.remove(receipt_index.0);
        if pending.outstanding_receipts.is_empty() {
            if !pending.rejected {
                self.queued_observations
                    .push_back(pending.lane.acknowledged(pending.token));
            }
            self.pending_appends.remove(append_index.0);
        }
        Ok(())
    }

    fn record_transcript_delta(&mut self, role: GptLiveTurnRole, delta: String) {
        let turn = self.ensure_open_turn(role);
        self.queue_transcript_fragment(role, turn, delta.clone());
        if let Some(open) = self.open_turn.as_mut() {
            open.segments.push(delta);
        }
    }

    fn mint_transcript_item(&mut self, role: GptLiveTurnRole) -> GptLiveTranscriptItemRef {
        self.next_transcript_item = self.next_transcript_item.saturating_add(1);
        GptLiveTranscriptItemRef(format!(
            "{}:{}",
            match role {
                GptLiveTurnRole::User => "input",
                GptLiveTurnRole::Assistant | GptLiveTurnRole::Unknown => "output",
            },
            self.next_transcript_item
        ))
    }

    /// Announce one transcript delta as a fragment of `turn`.
    fn queue_transcript_fragment(
        &mut self,
        role: GptLiveTurnRole,
        turn: GptLiveTurnRef,
        delta: String,
    ) {
        let item = self.mint_transcript_item(role);
        self.queued_observations.push_back(match role {
            GptLiveTurnRole::User => GptLiveBrokerObservation::UserTranscriptFragment {
                item,
                text: delta.clone(),
            },
            GptLiveTurnRole::Assistant | GptLiveTurnRole::Unknown => {
                GptLiveBrokerObservation::AssistantTranscriptFragment {
                    item,
                    text: delta.clone(),
                }
            }
        });
        self.queued_observations
            .push_back(GptLiveBrokerObservation::TurnSnapshotDelta { turn, delta });
    }

    /// Return the open turn for `role`, finishing a different-role turn and
    /// starting a new one when the speaker changes.
    fn ensure_open_turn(&mut self, role: GptLiveTurnRole) -> GptLiveTurnRef {
        if let Some(open) = self.open_turn.as_ref()
            && open.role == role
        {
            return GptLiveTurnRef(open.provider_ref.clone());
        }
        self.finish_open_turn();
        self.start_turn(role)
    }

    fn start_turn(&mut self, role: GptLiveTurnRole) -> GptLiveTurnRef {
        let turn = self.mint_turn(role);
        self.open_turn = Some(OpenTurn {
            provider_ref: turn.0.clone(),
            role,
            segments: Vec::new(),
            window_start: 0,
        });
        turn
    }

    /// Announce a fresh synthesized turn without making it the open turn.
    fn mint_turn(&mut self, role: GptLiveTurnRole) -> GptLiveTurnRef {
        self.next_turn = self.next_turn.saturating_add(1);
        let turn = GptLiveTurnRef(format!("public-live-turn:{}", self.next_turn));
        self.queued_observations
            .push_back(GptLiveBrokerObservation::TurnStarted {
                turn: turn.clone(),
                role,
            });
        turn
    }

    fn finish_open_turn(&mut self) {
        let Some(open) = self.open_turn.take() else {
            return;
        };
        let transcript = join_segments(&open.segments);
        self.window.push(open.role, open.window_transcript());
        if open.role == GptLiveTurnRole::User {
            self.last_user_turn = Some(FinishedUserTurn {
                transcript: transcript.clone(),
            });
        }
        self.queued_observations
            .push_back(GptLiveBrokerObservation::TurnFinished {
                turn: GptLiveTurnRef(open.provider_ref),
                role: open.role,
                transcript,
            });
    }

    fn record_delegation(
        &mut self,
        delegation: Delegation,
        offset_ms: f64,
    ) -> Result<(), GptLiveBrokerError> {
        if delegation.item_type != DelegationType::Delegation
            || delegation.id.trim().is_empty()
            || self.seen_delegation_ids.contains(&delegation.id)
            || self.seen_delegation_ids.len() >= Self::MAX_DELEGATION_IDENTITIES
        {
            return Err(protocol_error());
        }
        self.seen_delegation_ids.insert(delegation.id.clone());
        let reference = GptLiveDelegationRef(delegation.id);
        if delegation.target != DelegationTarget::Client {
            // No executor input is produced, so the window stays open: the
            // user's words are not drained by a delegation nobody acts on.
            self.queued_observations.push_back(
                GptLiveBrokerObservation::DelegationActionableInputUnsupported {
                    delegation: reference,
                },
            );
            return Ok(());
        }
        // The executor-input window is anchored on this protocol event: it
        // holds every transcript delta received since the previous client
        // `session.delegation.created` (or since open).
        let (request_transcript, assistant_context) = self.take_delegation_window();
        // The executor request is the whole window's user transcript. The
        // provider's backchannels ("mm-hm", "sure") are designed behaviour
        // and carry no turn boundary, so an assistant delta between two
        // parts of one request does not split it; what the assistant said
        // in the window travels separately as context. A delegation with
        // no new user transcript re-presents the previous request.
        let request_transcript = if request_transcript.is_empty() {
            match self.last_request.as_ref() {
                Some(previous) => previous.clone(),
                None => {
                    // No user input exists to become the task.
                    self.queued_observations.push_back(
                        GptLiveBrokerObservation::DelegationActionableInputUnsupported {
                            delegation: reference,
                        },
                    );
                    return Ok(());
                }
            }
        } else {
            request_transcript
        };
        // Canonical rows are unchanged by the window rule. The delegation
        // terminates the open user turn, whose transcript is its row.
        // `offset_ms` is the model's decision point on the session timeline,
        // not the end of the utterance (measured against gpt-live-1, the
        // final word starts at the offset), so speech at and after the
        // offset stays part of the turn. When no user turn is open (the
        // assistant spoke first) the most recent user transcript is
        // re-presented under a fresh detached turn so the facade's
        // start/finish pairing stays exact and any open assistant turn
        // continues undisturbed.
        //
        // The join is defined by arrival: a transcript delta arriving after
        // it opens a new user turn like any other, so its words always
        // reach the durable transcript and the next delegation's window.
        // The public protocol carries no per-utterance completion, item
        // lifecycle, or speech start/stop event that could say otherwise.
        let (turn_ref, transcript) = match self.open_turn.take() {
            Some(open) if open.role == GptLiveTurnRole::User => {
                let transcript = join_segments(&open.segments);
                (open.provider_ref, transcript)
            }
            other => {
                self.open_turn = other;
                let transcript = self.last_user_turn.as_ref().map_or_else(
                    || request_transcript.clone(),
                    |last| last.transcript.clone(),
                );
                (self.mint_turn(GptLiveTurnRole::User).0, transcript)
            }
        };
        tracing::debug!(
            offset_ms,
            "public Live client delegation joined its user turn"
        );
        self.last_user_turn = Some(FinishedUserTurn {
            transcript: transcript.clone(),
        });
        self.last_request = Some(request_transcript.clone());
        self.queued_observations
            .push_back(GptLiveBrokerObservation::ClientDelegationFinal {
                delegation: reference,
                target: GptLiveDelegationTarget::Client,
                turn: GptLiveTurnRef(turn_ref),
                transcript,
                request_transcript,
                assistant_context,
            });
        Ok(())
    }

    /// Close the delegation window at a `session.delegation.created`: return
    /// its user and assistant transcript and start the next window. The open
    /// turn keeps its identity; only its later segments belong to the next
    /// window.
    fn take_delegation_window(&mut self) -> (String, String) {
        let mut window = std::mem::take(&mut self.window);
        if let Some(open) = self.open_turn.as_mut() {
            window.push(open.role, open.window_transcript());
            open.window_start = open.segments.len();
        }
        (
            join_window_chunks(window.user_chunks.iter().map(String::as_str)),
            join_window_chunks(window.assistant_chunks.iter().map(String::as_str)),
        )
    }
}

fn pending_event_id(token: GptLiveAppendToken) -> String {
    format!("meerkat-append-{}", token.0)
}

fn thinking_event_id(token: GptLiveAppendToken, index: usize) -> String {
    format!("meerkat-thinking-{}-{index}", token.0)
}

fn instructions_event_id(token: GptLiveAppendToken, index: usize) -> String {
    format!("meerkat-instructions-{}-{index}", token.0)
}

/// Largest payload of one fragmented append, a conservative byte bound for
/// the provider's 500-token append limit.
const CONTEXT_FRAGMENT_MAX_BYTES: usize = 500;

/// Split `text` into ordered fragments of at most [`CONTEXT_FRAGMENT_MAX_BYTES`]
/// whose concatenation is exactly `text`. A fragment ends at the end of the
/// last whitespace run inside its window, so no word (or number, or quoted
/// phrase) is cut in two and the next fragment starts on a non-blank
/// character; fragments are therefore often shorter than the bound. A single
/// token longer than the bound has no whitespace to cut at and falls back to
/// a byte split on a UTF-8 character boundary, which the provider still
/// accepts (the bound is what matters on the wire, the seam only matters for
/// recall). The result is bounded by the caller's pending-append limit.
fn context_fragments(mut text: &str) -> Vec<&str> {
    let mut fragments = Vec::new();
    while !text.is_empty() {
        if text.len() <= CONTEXT_FRAGMENT_MAX_BYTES {
            fragments.push(text);
            break;
        }
        let window = text.floor_char_boundary(CONTEXT_FRAGMENT_MAX_BYTES);
        // End of the last whitespace run that finishes inside the window:
        // the cut lands after the blank, before the next non-blank char.
        let cut = text[..window]
            .char_indices()
            .rev()
            .filter(|(_, ch)| ch.is_whitespace())
            .map(|(index, ch)| index + ch.len_utf8())
            .find(|end| text[*end..].starts_with(|ch: char| !ch.is_whitespace()))
            .unwrap_or(window);
        let (fragment, remaining) = text.split_at(cut);
        fragments.push(fragment);
        text = remaining;
    }
    fragments
}

fn map_live_error(error: LiveError) -> GptLiveBrokerError {
    let class = match error {
        LiveError::Invalid(reason) => {
            // Protocol-client validation, for example a startup history seed
            // over the provider's 128-message / 8,192-token limit. The class
            // alone is indistinguishable from a malformed provider event; the
            // reason names no credential or dialogue content.
            tracing::warn!(
                %reason,
                "public Live request rejected by protocol validation before it reached the provider"
            );
            GptLiveBrokerTerminalClass::Protocol
        }
        LiveError::Json(_)
        | LiveError::MalformedEvent { .. }
        | LiveError::Provider(_)
        | LiveError::SessionIdentityMismatch(_)
        | LiveError::ContinuityLost => GptLiveBrokerTerminalClass::Protocol,
        LiveError::Http {
            status, request_id, ..
        } => {
            // Operators otherwise see only a sanitized transport class; the
            // status and request id carry no credential or content material
            // and distinguish entitlement (403) from availability failures.
            tracing::warn!(
                status,
                request_id = request_id.as_deref().unwrap_or("<none>"),
                forbidden_hint = status == 403,
                "public Live HTTP request was rejected by the provider (403: unknown voice name or organization without Live access)"
            );
            GptLiveBrokerTerminalClass::Http
        }
        LiveError::Transport(_)
        | LiveError::Timeout
        | LiveError::AmbiguousWrite
        | LiveError::UnconfirmedClose => GptLiveBrokerTerminalClass::WebSocket,
        LiveError::Closed => GptLiveBrokerTerminalClass::Closed,
    };
    GptLiveBrokerError::Transport { class }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use meerkat_core::{
        AuthMetadata, Config, ModelRegistry, Provider, SessionLlmIdentity,
        connection::BackendProfile,
    };
    use meerkat_llm_core::provider_runtime::{ResolvedConnection, StaticLease};
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn realtime_target(model: &str, backend_kind: OpenAiBackendKind) -> ResolvedRealtimeTarget {
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())
            .expect("canonical model registry");
        let identity = SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let witness = registry
            .profile_witness_for_provider(Provider::OpenAI, &identity.model)
            .expect("registry witness");
        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            backend_profile: Arc::new(BackendProfile {
                id: "test-backend".to_string(),
                provider: Provider::OpenAI,
                backend_kind: backend_kind.as_str().to_string(),
                base_url: None,
                options: Value::Null,
                server: None,
            }),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &meerkat_core::AuthBindingRef {
                    realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
                    binding: meerkat_core::BindingId::parse("openai").expect("valid binding"),
                    profile: None,
                    origin: meerkat_core::BindingOrigin::Configured,
                },
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "credential-secret".to_string(),
                AuthMetadata::default(),
                None,
                "openai:test",
            )),
        };
        ResolvedRealtimeTarget::new(identity, witness, connection).expect("matching target")
    }

    fn frame(value: Value) -> ServerFrame {
        oai_rt_rs::live::Codec::default()
            .decode_server(&value.to_string())
            .expect("fixture event decodes")
    }

    fn drain(state: &mut SessionState) -> Vec<GptLiveBrokerObservation> {
        state.queued_observations.drain(..).collect()
    }

    fn input_delta(text: &str) -> Value {
        input_delta_at(text, 0.0)
    }

    fn input_delta_at(text: &str, start_ms: f64) -> Value {
        input_delta_span(text, start_ms, start_ms + 1.0)
    }

    fn input_delta_span(text: &str, start_ms: f64, end_ms: f64) -> Value {
        json!({"type":"session.input_transcript.delta","event_id":"i","delta":text,"start_ms":start_ms,"end_ms":end_ms})
    }

    fn ack(client_event_id: Option<&str>) -> Value {
        let mut value = json!({"type":"session.commentary.appended","event_id":"a","start_ms":1.0,"end_ms":1.0});
        if let Some(id) = client_event_id {
            value["client_event_id"] = json!(id);
        }
        value
    }

    fn thinking_ack(client_event_id: Option<&str>) -> Value {
        let mut value = ack(client_event_id);
        value["type"] = json!("session.thinking.appended");
        value
    }

    fn append_rejected(client_event_id: Option<&str>) -> Value {
        let mut value = json!({"type":"error","event_id":"e","error":{
            "type":"invalid_request_error","code":"invalid_value","message":"PRIVATE_REJECTION"
        }});
        if let Some(id) = client_event_id {
            value["error"]["client_event_id"] = json!(id);
        }
        value
    }

    fn session_closed() -> Value {
        json!({
            "type":"session.closed","event_id":"c","session":{
                "id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0
            },"reason":"close_requested","usage":{"seconds":1.0}
        })
    }

    /// An assistant response starting where the fixture input span ends
    /// (`input_delta` covers 0.0..1.0), so the joined transcript already
    /// reaches the response start.
    fn output_delta(text: &str) -> Value {
        output_delta_span(text, 1.0, 2.0)
    }

    fn output_delta_span(text: &str, start_ms: f64, end_ms: f64) -> Value {
        json!({"type":"session.output_transcript.delta","event_id":"o","delta":text,"start_ms":start_ms,"end_ms":end_ms})
    }

    /// A delegation decided inside the fixture input span (`input_delta`
    /// covers 0.0..1.0).
    fn delegation_created(id: &str, target: &str) -> Value {
        delegation_created_at(id, target, 0.5)
    }

    fn delegation_created_at(id: &str, target: &str, offset_ms: f64) -> Value {
        json!({"type":"session.delegation.created","event_id":"d","offset_ms":offset_ms,
            "delegation":{"type":"delegation","id":id,"target":target}})
    }

    fn assert_protocol_error(error: GptLiveBrokerError) {
        assert!(matches!(
            error,
            GptLiveBrokerError::Transport {
                class: GptLiveBrokerTerminalClass::Protocol
            }
        ));
    }

    #[test]
    fn factory_admits_only_released_gpt_live_rows_on_the_openai_api_backend() {
        assert!(matches!(
            PublicLiveBrokerFactory::try_from_target(realtime_target(
                "gpt-realtime-2",
                OpenAiBackendKind::OpenAiApi
            )),
            Err(ProviderClientError::MissingFeature(
                "openai-live-model-family"
            ))
        ));
        assert!(matches!(
            PublicLiveBrokerFactory::try_from_target(realtime_target(
                "gpt-live-1-codex",
                OpenAiBackendKind::OpenAiApi
            )),
            Err(ProviderClientError::MissingFeature(
                "openai-live-released-model"
            ))
        ));
        assert!(matches!(
            PublicLiveBrokerFactory::try_from_target(realtime_target(
                "gpt-live-1",
                OpenAiBackendKind::ChatGptBackend
            )),
            Err(ProviderClientError::MissingFeature(
                "openai-live-openai-api-backend"
            ))
        ));
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .expect("released public row admits");
        let rendered = format!("{factory:?}");
        assert!(!rendered.contains("credential-secret"));
        assert!(!rendered.contains("gpt-live-1"));
    }

    #[test]
    fn session_config_selects_client_delegation_voice_and_instructions_only() {
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .expect("admitted factory");
        let config = PublicLiveOpenConfig::new("v=0\r\nOFFER", "marin")
            .expect("valid config")
            .with_instructions("Keep answers short.");
        let session = factory.session_config(&config);
        assert_eq!(session.model, "gpt-live-1");
        let audio = session.audio.expect("voice selection");
        assert!(audio.format.is_none(), "WebRTC negotiates media");
        assert_eq!(
            audio.output.and_then(|output| output.voice),
            Some(Voice::Named("marin".to_string()))
        );
        assert_eq!(session.delegation, Field::Value(DelegationConfig::Client));
        assert_eq!(
            session.instructions,
            Field::Value("Keep answers short.".to_string())
        );
        assert!(session.client.is_none() && session.input.is_none() && session.store.is_none());
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("OFFER") && !rendered.contains("marin"));
        assert!(!rendered.contains("Keep answers short"));
    }

    #[test]
    fn startup_history_preserves_dialogue_roles_without_executor_authority() {
        use meerkat_core::types::{
            AssistantBlock, BlockAssistantMessage, StopReason, SystemMessage, ToolResult,
            UserMessage,
        };

        let history = vec![
            Message::System(SystemMessage::new("private executor instructions")),
            Message::User(UserMessage::text("Remember  the table.\n")),
            Message::tool_results(vec![ToolResult::new(
                "private-call".to_string(),
                "private tool output".to_string(),
                false,
            )]),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "Booked for two.".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            )),
        ];
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&history);
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let encoded = serde_json::to_value(factory.session_config(&config)).unwrap();
        assert_eq!(
            encoded["input"],
            json!([
                {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "Remember  the table.\n"}
                ]},
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "Booked for two."}
                ]}
            ])
        );
        assert!(!encoded.to_string().contains("private"));
        assert!(!format!("{config:?}").contains("Remember"));
    }

    #[test]
    fn startup_history_preserves_unmeasured_assistant_provenance_when_flattened() {
        let history = [Message::BlockAssistant(
            meerkat_core::types::BlockAssistantMessage::new(
                vec![meerkat_core::AssistantBlock::Transcript {
                    text: "Observed voice dialogue.".into(),
                    source: meerkat_core::types::TranscriptSource::SpokenUnmeasured,
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            ),
        )];
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&history);
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let encoded = serde_json::to_value(factory.session_config(&config)).unwrap();
        assert_eq!(encoded["input"][0]["role"], "assistant");
        let text = encoded["input"][0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Observed voice dialogue."));
        assert!(text.contains("UNMEASURED"));
        assert!(text.contains("Not proof"));
        assert!(encoded["input"][0].get("status").is_none());
    }

    #[test]
    fn startup_summary_is_a_developer_input_item_not_a_user_item_or_instructions() {
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_instructions("Speak briefly.")
            .with_context_summary("The agent is comparing two tables.");
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let session = factory.session_config(&config);
        session
            .validate()
            .expect("pinned SDK accepts a developer-role startup input item");
        let encoded = serde_json::to_value(session).unwrap();
        // The summary is history and rides the documented history carrier,
        // the startup `input`, as one developer-role item. It is never a
        // user-role item (a user item at session start made the provider
        // open the call with a greeting) and it never touches the
        // instructions, which stay the caller's own.
        assert_eq!(encoded["instructions"], "Speak briefly.");
        let input = encoded["input"].as_array().unwrap();
        assert_eq!(input.len(), 1, "one developer item: {encoded}");
        assert_eq!(input[0]["role"], "developer");
        assert_eq!(input[0]["type"], "message");
        let content = input[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "input_text");
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.starts_with(PublicLiveContextSeed::SUMMARY_ITEM_PREFIX));
        assert!(text.contains("context data, not a new user request"));
        assert!(text.ends_with("\nThe agent is comparing two tables."));
        assert!(!format!("{config:?}").contains("two tables"));
    }

    #[test]
    fn startup_summary_seeds_the_developer_item_then_the_recent_turns_in_order() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, UserMessage};
        let recent = vec![
            Message::User(UserMessage::text("earlier question")),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "earlier answer".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            )),
        ];
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_instructions("Speak briefly.")
            .with_history(&recent)
            .with_context_summary("The agent compared two tables.");
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let session = factory.session_config(&config);
        session
            .validate()
            .expect("developer item plus recent turns");
        let encoded = serde_json::to_value(session).unwrap();
        let input = encoded["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["role"], "developer");
        assert!(
            input[0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .ends_with("The agent compared two tables.")
        );
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"][0]["text"], "earlier question");
        assert_eq!(input[2]["role"], "assistant");
        assert_eq!(input[2]["content"][0]["text"], "earlier answer");
        assert_eq!(encoded["instructions"], "Speak briefly.");
        assert_eq!(
            config.startup_input_truncation(),
            LiveStartupInputTruncation::default()
        );
    }

    #[test]
    fn startup_input_budget_drops_the_oldest_recent_turns_first_and_reports_it() {
        use meerkat_core::types::UserMessage;
        // Three 9,000-byte turns estimate to 3,000 tokens each; with the
        // summary the budget of 8,192 tokens holds the two newest.
        let recent: Vec<Message> = (1..=3)
            .map(|index| {
                Message::User(UserMessage::text(format!(
                    "turn {index} {}",
                    "x".repeat(9_000)
                )))
            })
            .collect();
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&recent)
            .with_context_summary("summary");
        let truncation = config.startup_input_truncation();
        assert_eq!(truncation.dropped_items, 1, "{truncation:?}");
        assert!(truncation.dropped_bytes >= 9_000);
        let items = config.context_seed.initial_input().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].role, InitialRole::Developer);
        assert!(
            items[1].content[0].text.starts_with("turn 2 "),
            "oldest dropped"
        );
        assert!(items[2].content[0].text.starts_with("turn 3 "));
        // The item cap holds too: 130 tiny turns keep the developer item and
        // the 127 newest turns.
        let many: Vec<Message> = (0..130)
            .map(|index| Message::User(UserMessage::text(format!("t{index}"))))
            .collect();
        let capped = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&many)
            .with_context_summary("summary");
        let items = capped.context_seed.initial_input().unwrap();
        assert_eq!(items.len(), LIVE_STARTUP_INPUT_MAX_ITEMS);
        assert_eq!(capped.startup_input_truncation().dropped_items, 3);
        assert_eq!(
            items[1].content[0].text, "t3",
            "the three oldest were dropped"
        );
        assert_eq!(items[127].content[0].text, "t129");
        // The summary is never dropped, even alone over budget.
        let huge = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&recent)
            .with_context_summary(&"s".repeat(30_000));
        let items = huge.context_seed.initial_input().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(huge.startup_input_truncation().dropped_items, 3);
    }

    #[test]
    fn pending_context_is_instructions_context_distinct_from_the_summary() {
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_instructions("Catalog behavior.")
            .with_context_summary("PRIVATE_PRIOR_SUMMARY")
            .with_pending_context();
        assert!(matches!(
            config.context_seed,
            PublicLiveContextSeed::HistoricalContextPending
        ));
        let session = factory.session_config(&config);
        session
            .validate()
            .expect("pinned SDK accepts instructions-only startup context");
        let encoded = serde_json::to_value(session).unwrap();
        assert!(
            encoded.get("input").is_none(),
            "pending notice is not a user item: {encoded}"
        );
        assert_eq!(
            encoded["instructions"],
            "Catalog behavior.\n\nVoice-channel context availability (factual state, not a new user request):\nHistorical session context is being prepared and is not yet available."
        );
        assert!(!encoded.to_string().contains("PRIVATE_PRIOR_SUMMARY"));
        assert!(format!("{config:?}").contains("HistoricalContextPending"));
        assert!(!format!("{config:?}").contains("PRIVATE_PRIOR_SUMMARY"));
        let summarized = config.with_context_summary("Prepared facts.");
        assert!(matches!(
            summarized.context_seed,
            PublicLiveContextSeed::FactualSummary { .. }
        ));
        let summary = serde_json::to_value(factory.session_config(&summarized)).unwrap();
        assert_eq!(
            summary["instructions"], "Catalog behavior.",
            "a ready summary leaves the instructions alone"
        );
        let text = summary["input"][0]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Factual summary"));
        assert!(!text.contains("not yet available"));
        assert!(text.ends_with("Prepared facts."));
    }

    #[test]
    fn startup_history_does_not_silently_trim_over_limit_dialogue() {
        let messages = (0..129)
            .map(|index| Message::User(meerkat_core::types::UserMessage::text(index.to_string())))
            .collect::<Vec<_>>();
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let allowed = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&messages[..128]);
        assert!(factory.session_config(&allowed).validate().is_ok());
        let oversize = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&messages);
        assert!(factory.session_config(&oversize).validate().is_err());
        assert_eq!(factory.session_config(&oversize).input.unwrap().len(), 129);
    }

    #[test]
    fn open_config_rejects_blank_mechanical_inputs() {
        assert!(matches!(
            PublicLiveOpenConfig::new("  ", "marin"),
            Err(GptLiveBrokerError::MissingOfferSdp)
        ));
        assert!(matches!(
            PublicLiveOpenConfig::new("v=0", " "),
            Err(GptLiveBrokerError::MissingVoice)
        ));
    }

    #[test]
    fn transcript_role_alternation_synthesizes_turns() {
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("hello "))).unwrap();
        state.apply_frame(frame(input_delta("there"))).unwrap();
        state.apply_frame(frame(output_delta("hi"))).unwrap();
        let observations = drain(&mut state);
        let kinds: Vec<_> = observations.iter().map(|o| format!("{o:?}")).collect();
        assert!(matches!(
            &observations[0],
            GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::User,
                ..
            }
        ));
        assert!(matches!(
            &observations[1],
            GptLiveBrokerObservation::UserTranscriptFragment { text, .. } if text == "hello "
        ));
        assert!(matches!(
            &observations[2],
            GptLiveBrokerObservation::TurnSnapshotDelta { delta, .. } if delta == "hello "
        ));
        assert!(matches!(
            &observations[3],
            GptLiveBrokerObservation::UserTranscriptFragment { text, .. } if text == "there"
        ));
        let GptLiveBrokerObservation::TurnFinished {
            turn: finished_user,
            role: GptLiveTurnRole::User,
            transcript,
        } = &observations[5]
        else {
            panic!("user turn must finish when the assistant starts: {kinds:?}");
        };
        assert_eq!(transcript, "hello there");
        let GptLiveBrokerObservation::TurnStarted {
            turn: started_user,
            role: GptLiveTurnRole::User,
        } = &observations[0]
        else {
            unreachable!()
        };
        assert_eq!(started_user, finished_user);
        assert!(matches!(
            &observations[6],
            GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::Assistant,
                ..
            }
        ));
        assert!(matches!(
            &observations[7],
            GptLiveBrokerObservation::AssistantTranscriptFragment { text, .. } if text == "hi"
        ));
        assert_eq!(observations.len(), 9);

        // Stream end flushes the open assistant turn with its full transcript.
        state.apply_frame(frame(output_delta(" there"))).unwrap();
        state.finish_open_turn();
        let tail = drain(&mut state);
        assert!(matches!(
            tail.last(),
            Some(GptLiveBrokerObservation::TurnFinished {
                role: GptLiveTurnRole::Assistant,
                transcript,
                ..
            }) if transcript == "hi there"
        ));
        assert!(
            !format!("{tail:?}").contains("hi there"),
            "Debug must redact text"
        );
    }

    #[test]
    fn client_delegation_terminates_the_open_user_turn_as_an_exact_join() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("book a table")))
            .unwrap();
        let started = drain(&mut state);
        let GptLiveBrokerObservation::TurnStarted {
            turn: user_turn, ..
        } = &started[0]
        else {
            panic!("user turn start");
        };
        state
            .apply_frame(frame(delegation_created("dlg_1", "client")))
            .unwrap();
        let joined = drain(&mut state);
        assert_eq!(joined.len(), 1, "the join is the sole terminal observation");
        assert!(matches!(
            &joined[0],
            GptLiveBrokerObservation::ClientDelegationFinal { delegation, target: GptLiveDelegationTarget::Client, turn, transcript, request_transcript, assistant_context }
                if delegation.__opaque_provider_id() == "dlg_1" && turn == user_turn && transcript == "book a table"
                    && request_transcript == "book a table" && assistant_context.is_empty()
        ));
        // The assistant reply starts a new turn without a second user finish.
        state.apply_frame(frame(output_delta("sure"))).unwrap();
        let next = drain(&mut state);
        assert!(matches!(
            &next[0],
            GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::Assistant,
                ..
            }
        ));
    }

    #[test]
    fn late_delegation_reuses_the_last_user_transcript_under_a_detached_turn() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("what time is it")))
            .unwrap();
        state
            .apply_frame(frame(output_delta("let me check")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(delegation_created("dlg_late", "client")))
            .unwrap();
        let observations = drain(&mut state);
        assert_eq!(observations.len(), 2);
        let GptLiveBrokerObservation::TurnStarted {
            turn: minted,
            role: GptLiveTurnRole::User,
        } = &observations[0]
        else {
            panic!("detached user turn must be announced first: {observations:?}");
        };
        assert!(matches!(
            &observations[1],
            GptLiveBrokerObservation::ClientDelegationFinal { turn, transcript, .. }
                if turn == minted && transcript == "what time is it"
        ));
        // The assistant turn stayed open and continues under its own ref.
        state.apply_frame(frame(output_delta(" now"))).unwrap();
        let cont = drain(&mut state);
        assert!(matches!(
            &cont[0],
            GptLiveBrokerObservation::AssistantTranscriptFragment { text, .. } if text == " now"
        ));
        assert!(
            !cont
                .iter()
                .any(|o| matches!(o, GptLiveBrokerObservation::TurnStarted { .. }))
        );
    }

    #[test]
    fn delegation_without_user_input_is_unsupported_not_a_join() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(delegation_created("dlg_0", "client")))
            .unwrap();
        let observations = drain(&mut state);
        assert!(matches!(
            observations.as_slice(),
            [GptLiveBrokerObservation::DelegationActionableInputUnsupported { delegation }]
                if delegation.__opaque_provider_id() == "dlg_0"
        ));
    }

    #[test]
    fn delegation_identity_and_target_fail_closed() {
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("x"))).unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_r", "responses")))
            .unwrap();
        assert!(matches!(
            drain(&mut state).last(),
            Some(GptLiveBrokerObservation::DelegationActionableInputUnsupported { .. })
        ));
        state
            .apply_frame(frame(delegation_created("dlg_dup", "client")))
            .unwrap();
        drain(&mut state);
        assert_protocol_error(
            state
                .apply_frame(frame(delegation_created("dlg_dup", "client")))
                .expect_err("duplicate delegation identity"),
        );
        assert_protocol_error(
            state
                .apply_frame(frame(delegation_created("  ", "client")))
                .expect_err("blank delegation identity"),
        );
    }

    #[test]
    fn late_utterance_speech_after_the_join_opens_a_new_turn_and_is_not_lost() {
        // The public protocol has no utterance-complete event. A transcript
        // delta the transcriber delivers after the join is a separate
        // utterance: it opens a new user turn, so its words reach the
        // durable transcript, and a following delegation takes that turn.
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta_at("tell me what you", 9600.0)))
            .unwrap();
        let started = drain(&mut state);
        let GptLiveBrokerObservation::TurnStarted {
            turn: user_turn, ..
        } = &started[0]
        else {
            panic!("user turn start");
        };
        state
            .apply_frame(frame(delegation_created_at("dlg_early", "client", 10400.0)))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::ClientDelegationFinal { transcript, .. }]
                if transcript == "tell me what you"
        ));
        state
            .apply_frame(frame(input_delta_at(" named it", 10400.0)))
            .unwrap();
        let late = drain(&mut state);
        assert!(matches!(
            &late[0],
            GptLiveBrokerObservation::TurnStarted { role: GptLiveTurnRole::User, turn }
                if turn != user_turn
        ));
        assert!(matches!(
            &late[1],
            GptLiveBrokerObservation::UserTranscriptFragment { text, .. } if text == " named it"
        ));
        // The assistant reply finishes that turn as a plain user turn: the
        // tail is a committed row, not a lost fragment.
        state
            .apply_frame(frame(output_delta_span("Sure,", 10600.0, 10800.0)))
            .unwrap();
        let finished = drain(&mut state);
        assert!(matches!(
            &finished[0],
            GptLiveBrokerObservation::TurnFinished { role: GptLiveTurnRole::User, transcript, .. }
                if transcript == " named it"
        ));
        // A following detached delegation takes the latest user turn.
        state
            .apply_frame(frame(delegation_created_at("dlg_again", "client", 11000.0)))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [
                GptLiveBrokerObservation::TurnStarted {
                    role: GptLiveTurnRole::User,
                    ..
                },
                GptLiveBrokerObservation::ClientDelegationFinal { transcript, .. },
            ] if transcript == " named it"
        ));
    }

    #[test]
    fn append_acknowledgements_correlate_by_client_event_id_not_position() {
        let mut state = SessionState::default();
        let session_token = state.reserve_append(PendingAppendLane::Session).unwrap();
        let delegation_token = state.reserve_append(PendingAppendLane::Delegation).unwrap();
        assert_ne!(session_token, delegation_token);
        // Two appends pending: an acknowledgement without an id is ambiguous.
        assert_protocol_error(
            state
                .apply_frame(frame(ack(None)))
                .expect_err("uncorrelated acknowledgement with two pending appends"),
        );
        // Out-of-order acknowledgement resolves by echoed id, not by position.
        state
            .apply_frame(frame(ack(Some(&pending_event_id(delegation_token)))))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token }]
                if *token == delegation_token
        ));
        // A single pending append accepts an id-less acknowledgement.
        state.apply_frame(frame(ack(None))).unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::SessionContextAppendAcknowledged { token }]
                if *token == session_token
        ));
        assert_protocol_error(
            state
                .apply_frame(frame(ack(None)))
                .expect_err("acknowledgement without a pending append"),
        );
        let stray = state.reserve_append(PendingAppendLane::Session).unwrap();
        assert_protocol_error(
            state
                .apply_frame(frame(ack(Some("meerkat-append-999"))))
                .expect_err("acknowledgement for an unknown append id"),
        );
        assert_eq!(state.pending_appends.len(), 1);
        let _ = stray;
        state.append_delivery_ambiguous = true;
        assert!(matches!(
            state.reserve_append(PendingAppendLane::Session),
            Err(GptLiveBrokerError::AppendInFlight)
        ));
    }

    #[test]
    fn thinking_commands_are_quiet_bounded_utf8_fragments_without_authority_fields() {
        for text in [
            "factual context ".repeat(130),
            "🦀日本語 é\n".repeat(150),
            format!("{}🦀tail", "x".repeat(499)),
        ] {
            let fragments = context_fragments(&text);
            assert_eq!(fragments.concat(), text);
            assert!(fragments.len() > 1);
            for (index, &fragment) in fragments.iter().enumerate() {
                assert!(!fragment.is_empty() && fragment.len() <= 500);
                let event = PublicLiveBrokerSession::thinking_event(
                    GptLiveAppendToken(5),
                    index,
                    fragment.to_owned(),
                );
                assert!(matches!(
                    &event.command,
                    Command::ThinkingAppend { content, delegation_id: Nullable(None) }
                        if content == fragment
                ));
                event.validate().unwrap();
                let encoded = serde_json::to_value(&event).unwrap();
                assert_eq!(
                    encoded,
                    json!({
                        "type": "session.thinking.append",
                        "event_id": thinking_event_id(GptLiveAppendToken(5), index),
                        "content": fragment,
                        "delegation_id": null
                    })
                );
                assert!(!format!("{event:?}").contains(fragment));
            }
        }
        assert_eq!(context_fragments("").len(), 0);
        assert_eq!(context_fragments(&"x".repeat(500)).len(), 1);
        assert_eq!(context_fragments(&"x".repeat(501)).len(), 2);
    }

    #[test]
    fn thinking_ack_requires_every_exact_fragment_receipt_without_consuming_other_lanes() {
        let mut state = SessionState::default();
        let session = state.reserve_append(PendingAppendLane::Session).unwrap();
        let thinking = state.reserve_thinking_append(3).unwrap();
        let delegation = state.reserve_append(PendingAppendLane::Delegation).unwrap();
        for index in [2, 0] {
            state
                .apply_frame(frame(thinking_ack(Some(&thinking_event_id(
                    thinking, index,
                )))))
                .unwrap();
            assert!(
                drain(&mut state).is_empty(),
                "partial ACK is not full delivery"
            );
        }
        assert_protocol_error(
            state
                .apply_frame(frame(thinking_ack(Some(&thinking_event_id(thinking, 0)))))
                .unwrap_err(),
        );
        assert_protocol_error(
            state
                .apply_frame(frame(thinking_ack(Some("unknown"))))
                .unwrap_err(),
        );
        assert_eq!(state.outstanding_receipt_count(), 3);
        state
            .apply_frame(frame(ack(Some(&pending_event_id(delegation)))))
            .unwrap();
        state
            .apply_frame(frame(thinking_ack(Some(&thinking_event_id(thinking, 1)))))
            .unwrap();
        // The legacy single-commentary ID-less receipt still works.
        state.apply_frame(frame(ack(None))).unwrap();
        assert_eq!(
            drain(&mut state),
            vec![
                GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: delegation },
                GptLiveBrokerObservation::ThinkingContextAppendAcknowledged { token: thinking },
                GptLiveBrokerObservation::SessionContextAppendAcknowledged { token: session },
            ]
        );
        assert!(state.pending_appends.is_empty());
    }

    #[test]
    fn cross_lane_and_idless_thinking_receipts_fail_without_spending_reservations() {
        let mut state = SessionState::default();
        let thinking = state.reserve_thinking_append(1).unwrap();
        let id = thinking_event_id(thinking, 0);
        for value in [ack(Some(&id)), ack(None), thinking_ack(None)] {
            assert_protocol_error(state.apply_frame(frame(value)).unwrap_err());
            assert_eq!(state.outstanding_receipt_count(), 1);
            assert!(drain(&mut state).is_empty());
        }
        let mut instructions = ack(Some(&id));
        instructions["type"] = json!("session.instructions.appended");
        assert_protocol_error(state.apply_frame(frame(instructions)).unwrap_err());
        assert_eq!(state.outstanding_receipt_count(), 1);
        for lane in [PendingAppendLane::Session, PendingAppendLane::Delegation] {
            let token = state.reserve_append(lane).unwrap();
            let before = state.outstanding_receipt_count();
            assert_protocol_error(
                state
                    .apply_frame(frame(thinking_ack(Some(&pending_event_id(token)))))
                    .unwrap_err(),
            );
            assert_eq!(state.outstanding_receipt_count(), before);
        }
    }

    #[test]
    fn thinking_partial_rejection_is_exact_once_and_never_becomes_acknowledged() {
        for closing in [false, true] {
            let mut state = SessionState::default();
            let thinking = state.reserve_thinking_append(4).unwrap();
            let session = state.reserve_append(PendingAppendLane::Session).unwrap();
            state.close_requested = closing;
            state
                .apply_frame(frame(thinking_ack(Some(&thinking_event_id(thinking, 2)))))
                .unwrap();
            assert!(drain(&mut state).is_empty());
            state
                .apply_frame(frame(append_rejected(Some(&thinking_event_id(
                    thinking, 1,
                )))))
                .unwrap();
            let observations = drain(&mut state);
            assert_eq!(
                observations,
                vec![GptLiveBrokerObservation::ThinkingContextAppendRejected { token: thinking }]
            );
            assert!(!format!("{observations:?}").contains("PRIVATE_REJECTION"));
            state
                .apply_frame(frame(append_rejected(Some(&thinking_event_id(
                    thinking, 3,
                )))))
                .unwrap();
            state
                .apply_frame(frame(thinking_ack(Some(&thinking_event_id(thinking, 0)))))
                .unwrap();
            assert!(
                drain(&mut state).is_empty(),
                "rejected aggregate cannot also ACK"
            );
            assert_eq!(state.outstanding_receipt_count(), 1);
            state
                .apply_frame(frame(ack(Some(&pending_event_id(session)))))
                .unwrap();
            assert_eq!(
                drain(&mut state),
                vec![GptLiveBrokerObservation::SessionContextAppendAcknowledged { token: session }]
            );
        }
    }

    #[test]
    fn only_confirmed_session_close_interrupts_unresolved_thinking_tokens() {
        for close_requested in [false, true] {
            let mut state = SessionState::default();
            let session = state.reserve_append(PendingAppendLane::Session).unwrap();
            let partial = state.reserve_thinking_append(3).unwrap();
            let delegation = state.reserve_append(PendingAppendLane::Delegation).unwrap();
            let untouched = state.reserve_thinking_append(1).unwrap();
            let completed = state.reserve_thinking_append(1).unwrap();
            state.close_requested = close_requested;
            state
                .apply_frame(frame(thinking_ack(Some(&thinking_event_id(partial, 1)))))
                .unwrap();
            assert!(drain(&mut state).is_empty());
            state
                .apply_frame(frame(thinking_ack(Some(&thinking_event_id(completed, 0)))))
                .unwrap();
            assert_eq!(
                drain(&mut state),
                vec![
                    GptLiveBrokerObservation::ThinkingContextAppendAcknowledged {
                        token: completed
                    }
                ]
            );
            state
                .apply_frame(frame(output_delta("final transcript")))
                .unwrap();
            drain(&mut state);
            state.apply_frame(frame(session_closed())).unwrap();
            let observations = drain(&mut state);
            assert!(matches!(
                observations.as_slice(),
                [
                    GptLiveBrokerObservation::TurnFinished { transcript, .. },
                    GptLiveBrokerObservation::ThinkingContextAppendInterruptedByClose { token: first },
                    GptLiveBrokerObservation::ThinkingContextAppendInterruptedByClose { token: second },
                ] if transcript == "final transcript" && *first == partial && *second == untouched
            ));
            assert!(
                format!("{:?}", observations[1])
                    .contains("thinking_context_append_interrupted_by_close")
            );
            assert_eq!(state.pending_appends.len(), 2);
            assert_eq!(state.pending_appends[0].token, session);
            assert_eq!(state.pending_appends[1].token, delegation);
            assert!(state.closed_observed);
            state.apply_frame(frame(session_closed())).unwrap();
            assert!(
                drain(&mut state).is_empty(),
                "repeated closure emits no duplicate result"
            );
            for token in [partial, untouched] {
                assert_protocol_error(
                    state
                        .apply_frame(frame(thinking_ack(Some(&thinking_event_id(token, 0)))))
                        .expect_err("late receipt cannot ACK a close-interrupted append"),
                );
            }
            assert!(drain(&mut state).is_empty());
        }
    }

    #[test]
    fn native_thinking_error_is_not_a_close_interruption_even_while_closing() {
        for close_requested in [false, true] {
            let mut state = SessionState::default();
            let thinking = state.reserve_thinking_append(3).unwrap();
            state.close_requested = close_requested;
            state
                .apply_frame(frame(thinking_ack(Some(&thinking_event_id(thinking, 0)))))
                .unwrap();
            assert!(drain(&mut state).is_empty());
            state
                .apply_frame(frame(append_rejected(Some(&thinking_event_id(
                    thinking, 1,
                )))))
                .unwrap();
            assert_eq!(
                drain(&mut state),
                vec![GptLiveBrokerObservation::ThinkingContextAppendRejected { token: thinking }]
            );
            assert_eq!(state.outstanding_receipt_count(), 1);
            state.apply_frame(frame(session_closed())).unwrap();
            assert!(state.pending_appends.is_empty());
            assert!(
                drain(&mut state).is_empty(),
                "native rejection is never relabeled as close interruption"
            );
            assert_protocol_error(
                state
                    .apply_frame(frame(thinking_ack(Some(&thinking_event_id(thinking, 2)))))
                    .expect_err("close cannot restore rejected append receipt"),
            );
            assert!(drain(&mut state).is_empty());
        }
    }

    #[test]
    fn uncorrelated_or_conflicting_thinking_rejections_do_not_spend_fragments() {
        let mut state = SessionState::default();
        let thinking = state.reserve_thinking_append(2).unwrap();
        for id in [None, Some("unrelated"), Some("meerkat-thinking-999-0")] {
            state.apply_frame(frame(append_rejected(id))).unwrap();
            assert_eq!(
                drain(&mut state),
                vec![GptLiveBrokerObservation::UnsupportedProviderEvent]
            );
            assert_eq!(state.outstanding_receipt_count(), 2);
        }
        let mut conflicting = append_rejected(Some(&thinking_event_id(thinking, 0)));
        conflicting["client_event_id"] = json!(thinking_event_id(thinking, 1));
        assert_protocol_error(state.apply_frame(frame(conflicting)).unwrap_err());
        assert_eq!(state.outstanding_receipt_count(), 2);
        // The outer-only error receipt is also exact evidence.
        let mut outer = append_rejected(None);
        outer["client_event_id"] = json!(thinking_event_id(thinking, 1));
        state.apply_frame(frame(outer)).unwrap();
        assert_eq!(
            drain(&mut state),
            vec![GptLiveBrokerObservation::ThinkingContextAppendRejected { token: thinking }]
        );
        assert_eq!(state.outstanding_receipt_count(), 1);
    }

    #[test]
    fn thinking_reservations_are_bounded_atomically_and_share_ambiguous_delivery_fence() {
        let mut state = SessionState::default();
        for count in [0, SessionState::MAX_PENDING_APPENDS + 1] {
            assert!(matches!(
                state.reserve_thinking_append(count),
                Err(GptLiveBrokerError::AppendInFlight)
            ));
            assert!(state.pending_appends.is_empty());
        }
        state.reserve_append(PendingAppendLane::Session).unwrap();
        assert!(matches!(
            state.reserve_thinking_append(SessionState::MAX_PENDING_APPENDS),
            Err(GptLiveBrokerError::AppendInFlight)
        ));
        assert_eq!(state.outstanding_receipt_count(), 1);
        let thinking = state
            .reserve_thinking_append(SessionState::MAX_PENDING_APPENDS - 1)
            .unwrap();
        assert_eq!(
            state.outstanding_receipt_count(),
            SessionState::MAX_PENDING_APPENDS
        );
        assert!(matches!(
            state.reserve_append(PendingAppendLane::Delegation),
            Err(GptLiveBrokerError::AppendInFlight)
        ));
        state
            .apply_frame(frame(thinking_ack(Some(&thinking_event_id(thinking, 0)))))
            .unwrap();
        state.reserve_append(PendingAppendLane::Delegation).unwrap();
        state.append_delivery_ambiguous = true;
        for lane in [
            PendingAppendLane::Session,
            PendingAppendLane::Delegation,
            PendingAppendLane::Thinking,
        ] {
            assert!(matches!(
                state.reserve_append(lane),
                Err(GptLiveBrokerError::AppendInFlight)
            ));
        }
    }

    #[test]
    fn client_delegation_joins_speech_after_its_decision_offset() {
        // The delegation offset is the model's decision point, not the end
        // of the utterance: speech at and after it belongs to the request.
        // Measured against gpt-live-1, the final word starts at the offset.
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta_at("tell me what you", 10000.0)))
            .unwrap();
        state
            .apply_frame(frame(input_delta_at(" named it", 10400.0)))
            .unwrap();
        let started = drain(&mut state);
        let GptLiveBrokerObservation::TurnStarted {
            turn: first_turn, ..
        } = &started[0]
        else {
            panic!("user turn start");
        };
        state
            .apply_frame(frame(delegation_created_at(
                "dlg_offset",
                "client",
                10400.0,
            )))
            .unwrap();
        let joined = drain(&mut state);
        assert!(matches!(
            joined.as_slice(),
            [GptLiveBrokerObservation::ClientDelegationFinal { turn, transcript, .. }]
                if turn == first_turn && transcript == "tell me what you named it"
        ));
        assert!(
            state.open_turn.is_none(),
            "no second user turn for the final word"
        );
    }

    #[test]
    fn assistant_interjection_mid_request_does_not_split_the_executor_request() {
        // S100 shape: the model backchannels while the user is still asking.
        // Turn synthesis still alternates (display), but the executor
        // request is every user delta since open; the backchannel travels
        // as labelled context.
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta_at(
                "please write the standup notes",
                1000.0,
            )))
            .unwrap();
        state
            .apply_frame(frame(output_delta_span("mm-hm", 1800.0, 2000.0)))
            .unwrap();
        state
            .apply_frame(frame(input_delta_at(" with two headings", 2100.0)))
            .unwrap();
        let before = drain(&mut state);
        let second_user_turn = before
            .iter()
            .filter_map(|o| match o {
                GptLiveBrokerObservation::TurnStarted {
                    turn,
                    role: GptLiveTurnRole::User,
                } => Some(turn.clone()),
                _ => None,
            })
            .nth(1)
            .expect("the backchannel opened a second user turn");
        state
            .apply_frame(frame(delegation_created_at("dlg_split", "client", 2600.0)))
            .unwrap();
        let joined = drain(&mut state);
        let [
            GptLiveBrokerObservation::ClientDelegationFinal {
                turn,
                transcript,
                request_transcript,
                assistant_context,
                ..
            },
        ] = joined.as_slice()
        else {
            panic!("one join: {joined:?}");
        };
        assert_eq!(
            turn, &second_user_turn,
            "the canonical row is the open turn"
        );
        assert_eq!(
            transcript, " with two headings",
            "canonical rows keep their segmentation"
        );
        assert_eq!(
            request_transcript, "please write the standup notes with two headings",
            "the executor request spans the interjection"
        );
        assert_eq!(assistant_context, "mm-hm");
    }

    #[test]
    fn non_client_delegation_leaves_the_window_open() {
        // A delegation nobody acts on produces no executor input, so it must
        // not drain the user's words: the next client delegation still sees
        // everything since open.
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("first half ")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_r", "responses")))
            .unwrap();
        assert!(matches!(
            drain(&mut state).last(),
            Some(GptLiveBrokerObservation::DelegationActionableInputUnsupported { .. })
        ));
        state
            .apply_frame(frame(input_delta("second half")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_c", "client")))
            .unwrap();
        let joined: Vec<_> = drain(&mut state)
            .into_iter()
            .filter(|o| matches!(o, GptLiveBrokerObservation::ClientDelegationFinal { .. }))
            .collect();
        assert!(matches!(
            joined.as_slice(),
            [GptLiveBrokerObservation::ClientDelegationFinal { request_transcript, .. }]
                if request_transcript == "first half second half"
        ));
    }

    #[test]
    fn two_requests_without_assistant_speech_are_separate_windows() {
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("first task"))).unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_a", "client")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(input_delta(" second task")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_b", "client")))
            .unwrap();
        let joined: Vec<_> = drain(&mut state)
            .into_iter()
            .filter(|o| matches!(o, GptLiveBrokerObservation::ClientDelegationFinal { .. }))
            .collect();
        assert!(matches!(
            joined.as_slice(),
            [GptLiveBrokerObservation::ClientDelegationFinal { transcript, request_transcript, assistant_context, .. }]
                if transcript == " second task"
                    && request_transcript == "second task"
                    && assistant_context.is_empty()
        ));
    }

    #[test]
    fn native_answer_between_two_requests_is_context_not_request() {
        // S103 shape: request, executor delegation, the user asks something
        // the model answers natively, then a new request. The second window
        // holds both user utterances since the first delegation as the
        // request; the native answer is context, never part of the request.
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("write the report")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_1", "client")))
            .unwrap();
        state.apply_frame(frame(output_delta("on it"))).unwrap();
        state
            .apply_frame(frame(input_delta("what day is it")))
            .unwrap();
        state
            .apply_frame(frame(output_delta("it is Tuesday")))
            .unwrap();
        state
            .apply_frame(frame(input_delta("also add a summary")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(delegation_created("dlg_2", "client")))
            .unwrap();
        let joined = drain(&mut state);
        let [
            GptLiveBrokerObservation::ClientDelegationFinal {
                transcript,
                request_transcript,
                assistant_context,
                ..
            },
        ] = joined.as_slice()
        else {
            panic!("one join: {joined:?}");
        };
        assert_eq!(transcript, "also add a summary");
        assert_eq!(request_transcript, "what day is it also add a summary");
        assert_eq!(
            assistant_context, "on it it is Tuesday",
            "assistant speech in the window is context, including the reply to the first request"
        );
        // The window closed: later assistant speech belongs to the next one.
        state.apply_frame(frame(output_delta("sure"))).unwrap();
        state
            .apply_frame(frame(input_delta("and email it")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(delegation_created("dlg_3", "client")))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::ClientDelegationFinal { request_transcript, assistant_context, .. }]
                if request_transcript == "and email it" && assistant_context == "sure"
        ));
    }

    #[test]
    fn assistant_turn_open_across_a_delegation_splits_its_context_by_window() {
        // The assistant is mid-sentence when the delegation arrives; only
        // the part spoken after it belongs to the next window, while the
        // synthesized assistant turn keeps one identity.
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("book it"))).unwrap();
        state.apply_frame(frame(output_delta("let me "))).unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(delegation_created("dlg_open", "client")))
            .unwrap();
        let first = drain(&mut state);
        assert!(matches!(
            first.as_slice(),
            [
                GptLiveBrokerObservation::TurnStarted { role: GptLiveTurnRole::User, .. },
                GptLiveBrokerObservation::ClientDelegationFinal { transcript, request_transcript, assistant_context, .. },
            ] if transcript == "book it" && request_transcript == "book it" && assistant_context == "let me"
        ));
        let assistant_turn = state.open_turn.as_ref().unwrap().provider_ref.clone();
        state.apply_frame(frame(output_delta("book that"))).unwrap();
        state.apply_frame(frame(input_delta("and a taxi"))).unwrap();
        drain(&mut state);
        assert_eq!(
            state.open_turn.as_ref().unwrap().role,
            GptLiveTurnRole::User
        );
        state
            .apply_frame(frame(delegation_created("dlg_next", "client")))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::ClientDelegationFinal { request_transcript, assistant_context, .. }]
                if request_transcript == "and a taxi" && assistant_context == "book that"
        ));
        let _ = assistant_turn;
    }

    #[test]
    fn delegation_without_new_user_transcript_re_presents_the_previous_request() {
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("first "))).unwrap();
        state.apply_frame(frame(output_delta("uh-huh"))).unwrap();
        state.apply_frame(frame(input_delta("half"))).unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_1", "client")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(output_delta("working on it")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(delegation_created("dlg_2", "client")))
            .unwrap();
        let again = drain(&mut state);
        assert!(matches!(
            again.as_slice(),
            [
                GptLiveBrokerObservation::TurnStarted { role: GptLiveTurnRole::User, .. },
                GptLiveBrokerObservation::ClientDelegationFinal { transcript, request_transcript, assistant_context, .. },
            ] if transcript == "half"
                && request_transcript == "first half"
                && assistant_context == "working on it"
        ));
    }

    #[test]
    fn context_fragments_cut_at_whitespace_and_rejoin_exactly() {
        let text = "alpha ".repeat(120) + "omega";
        let fragments = context_fragments(&text);
        assert!(fragments.len() >= 2);
        assert_eq!(fragments.concat(), text, "byte-exact join");
        for fragment in &fragments {
            assert!(fragment.len() <= CONTEXT_FRAGMENT_MAX_BYTES);
        }
        for fragment in &fragments[..fragments.len() - 1] {
            assert!(
                fragment.ends_with(' '),
                "a fragment ends at the end of a whitespace run"
            );
        }
        for fragment in &fragments[1..] {
            assert!(
                fragment.starts_with(|ch: char| !ch.is_whitespace()),
                "the next fragment starts on a word"
            );
        }
        // Short text is one fragment, untouched.
        assert_eq!(context_fragments("short text"), vec!["short text"]);
    }

    #[test]
    fn context_fragments_fall_back_to_a_byte_split_for_one_long_token() {
        // No whitespace anywhere: the only legal cut is a UTF-8 boundary at
        // the bound. Multi-byte characters never straddle a fragment.
        let text = "ü".repeat(600);
        let fragments = context_fragments(&text);
        assert_eq!(fragments.concat(), text);
        assert!(
            fragments
                .iter()
                .all(|f| f.len() <= CONTEXT_FRAGMENT_MAX_BYTES)
        );
        assert_eq!(fragments.len(), 3);
        assert_eq!(fragments[0].len(), 500);
        // A long token after some words: the words go first, the token is
        // byte-split on its own.
        let mixed = format!("lead words {}", "x".repeat(900));
        let fragments = context_fragments(&mixed);
        assert_eq!(fragments.concat(), mixed);
        assert_eq!(fragments[0], "lead words ");
        assert_eq!(fragments[1].len(), 500);
    }

    #[tokio::test]
    async fn instructions_sections_never_share_a_fragment() {
        // A framing preface followed by a summary: the preface is its own
        // fragment even though both would fit one window together, the
        // summary starts intact at the next fragment boundary, and the
        // wire bytes concatenate to exactly preface + summary.
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        let attach_instructions =
            move |State(capture): State<SharedCapture>, upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(move |mut socket| async move {
                    let mut commands = Vec::new();
                    for _ in 0..3 {
                        let event = recv_json(&mut socket, &capture).await;
                        assert_eq!(event["type"], "session.instructions.append");
                        commands.push(event);
                    }
                    for command in &commands {
                        let mut ack = ack(command["event_id"].as_str());
                        ack["type"] = json!("session.instructions.appended");
                        send_json(&mut socket, ack).await;
                    }
                    let mute = recv_json(&mut socket, &capture).await;
                    assert_eq!(mute["type"], "session.input_audio.mute");
                    let close = recv_json(&mut socket, &capture).await;
                    assert_eq!(close["type"], "session.close");
                    send_json(&mut socket, session_closed()).await;
                })
            };
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route(
                "/v1/live/sessions/{session_id}/attach",
                get(attach_instructions),
            )
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
            realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
            &format!("http://{address}/v1/"),
        )
        .unwrap();
        let (_, session) = factory
            .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
            .await
            .unwrap()
            .into_parts();
        let framing = "Frame this. ".to_string();
        let summary = "Historical vault phrase: copper otter. ".repeat(20);
        let token = session
            .append_instructions_context_sections(vec![framing.clone(), summary.clone()])
            .await
            .unwrap();
        assert_eq!(
            session.state.lock().await.outstanding_receipt_count(),
            3,
            "one receipt reserved per fragment"
        );
        loop {
            match session.next_observation().await.unwrap() {
                Some(GptLiveBrokerObservation::InstructionsContextAppendAcknowledged {
                    token: acknowledged,
                }) => {
                    assert_eq!(acknowledged, token);
                    break;
                }
                Some(_) => {}
                None => panic!("stream ended before the acknowledgement"),
            }
        }
        let sent: Vec<String> = capture
            .lock()
            .unwrap()
            .client_events
            .iter()
            .filter(|event| event["type"] == "session.instructions.append")
            .map(|event| event["content"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0], framing, "the framing is fragment 0 alone");
        assert!(sent[1].starts_with("Historical vault phrase"));
        assert!(sent[1].ends_with(' '), "the summary is cut at whitespace");
        assert!(
            sent[2].starts_with(|ch: char| !ch.is_whitespace()),
            "the next fragment starts on a word"
        );
        assert!(sent.iter().all(|f| f.len() <= CONTEXT_FRAGMENT_MAX_BYTES));
        assert_eq!(sent.concat(), format!("{framing}{summary}"), "bytes exact");
        session.close().await.unwrap();
        while session.next_observation().await.unwrap().is_some() {}
        server.abort();
    }

    #[test]
    fn rejected_append_releases_its_reservation_and_surfaces_unsupported() {
        let mut state = SessionState::default();
        let token = state.reserve_append(PendingAppendLane::Session).unwrap();
        state
            .apply_frame(frame(json!({"type":"error","event_id":"e","error":{
                "type":"invalid_request_error","code":"invalid_value","message":"Invalid value FIXTURE_SECRET",
                "param":"content","client_event_id":pending_event_id(token)}})))
            .unwrap();
        assert!(state.pending_appends.is_empty());
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::UnsupportedProviderEvent]
        ));
        state
            .apply_frame(frame(
                json!({"type":"session.future.event","event_id":"f","payload":"FIXTURE_PAYLOAD"}),
            ))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::UnsupportedProviderEvent]
        ));
        state
            .apply_frame(frame(json!({"type":"response.event","event_id":"r","delegation_id":"dlg","event":{"type":"response.created"}})))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::UnsupportedProviderEvent]
        ));
    }

    #[test]
    fn close_rejections_preserve_exact_append_failures_and_terminal_tail() {
        let mut state = SessionState::default();
        let session = state.reserve_append(PendingAppendLane::Session).unwrap();
        let delegation = state.reserve_append(PendingAppendLane::Delegation).unwrap();
        state
            .apply_frame(frame(output_delta("final reply")))
            .unwrap();
        drain(&mut state);
        state.close_requested = true;
        for token in [delegation, session] {
            state
                .apply_frame(frame(json!({"type":"error","event_id":"e","error":{
                    "type":"invalid_request_error","code":null,"message":"PRIVATE_ERROR_PAYLOAD",
                    "client_event_id":pending_event_id(token)
                }})))
                .unwrap();
        }
        state
            .apply_frame(frame(
                json!({"type":"session.closed","event_id":"c","session":{
            "id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0
        },"reason":"close_requested","usage":{"seconds":1.0}}),
            ))
            .unwrap();
        let observations = drain(&mut state);
        assert!(matches!(
            observations.as_slice(),
            [
                GptLiveBrokerObservation::DelegationContextAppendRejected { token: rejected_delegation },
                GptLiveBrokerObservation::SessionContextAppendRejected { token: rejected_session },
                GptLiveBrokerObservation::TurnFinished { transcript, .. },
            ] if *rejected_delegation == delegation && *rejected_session == session
                && transcript == "final reply"
        ));
        assert!(state.pending_appends.is_empty());
        assert!(state.closed_observed);
        assert!(!format!("{observations:?}").contains("PRIVATE_ERROR_PAYLOAD"));
    }

    #[test]
    fn close_does_not_launder_unknown_or_conflicting_append_errors() {
        let mut state = SessionState::default();
        let pending = state.reserve_append(PendingAppendLane::Session).unwrap();
        state.close_requested = true;
        for reference in [None, Some("unrelated"), Some("meerkat-append-999")] {
            let mut event = json!({"type":"error","event_id":"e","error":{
                "type":"invalid_request_error","code":null,"message":"rejected"
            }});
            if let Some(reference) = reference {
                event["error"]["client_event_id"] = json!(reference);
            }
            state.apply_frame(frame(event)).unwrap();
            assert!(matches!(
                drain(&mut state).as_slice(),
                [GptLiveBrokerObservation::UnsupportedProviderEvent]
            ));
            assert_eq!(state.pending_appends.len(), 1);
        }
        assert_protocol_error(
            state
                .apply_frame(frame(json!({
                    "type":"error","event_id":"e","client_event_id":"unrelated","error":{
                        "type":"invalid_request_error","code":null,"message":"rejected",
                        "client_event_id":pending_event_id(pending)
                    }
                })))
                .unwrap_err(),
        );
        assert_eq!(state.pending_appends.len(), 1);
    }

    #[test]
    fn media_telemetry_and_readiness_frames_carry_no_observation() {
        let mut state = SessionState::default();
        for value in [
            json!({"type":"session.started","event_id":"s","session":{"id":"live_x","model":"gpt-live-1","status":"active","expires_at":1.0}}),
            json!({"type":"session.output_audio.delta","delta":"AAAA","start_ms":0.0,"end_ms":1.0}),
            json!({"type":"session.input_audio.append","audio":"AAAA"}),
            json!({"type":"session.usage.updated","event_id":"u","usage":{"seconds":1.0}}),
            json!({"type":"session.input_audio.muted","event_id":"m"}),
            json!({"type":"info","event_id":"i","code":"note","message":"FIXTURE"}),
        ] {
            state.apply_frame(frame(value)).unwrap();
        }
        assert!(state.queued_observations.is_empty());
    }

    #[test]
    fn live_errors_lower_to_sanitized_terminal_classes() {
        let class = |error: LiveError| match map_live_error(error) {
            GptLiveBrokerError::Transport { class } => class,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(class(LiveError::Closed), GptLiveBrokerTerminalClass::Closed);
        assert_eq!(
            class(LiveError::Timeout),
            GptLiveBrokerTerminalClass::WebSocket
        );
        assert_eq!(
            class(LiveError::Invalid("x".into())),
            GptLiveBrokerTerminalClass::Protocol
        );
        assert_eq!(
            class(LiveError::ContinuityLost),
            GptLiveBrokerTerminalClass::Protocol
        );
    }

    #[derive(Default)]
    struct Capture {
        create_body: Option<Value>,
        create_authorization: Option<String>,
        attach_authorization: Option<String>,
        client_events: Vec<Value>,
    }
    type SharedCapture = Arc<std::sync::Mutex<Capture>>;

    async fn create_session(
        State(capture): State<SharedCapture>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let mut capture = capture.lock().expect("capture lock");
        capture.create_body = serde_json::from_slice(&body).ok();
        capture.create_authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        (
            StatusCode::CREATED,
            [("content-type", "application/json")],
            json!({"session":{"id":"live_fixture"},"transport":{"type":"webrtc","sdp":"v=0\r\nPUBLIC_ANSWER_SDP"}})
                .to_string(),
        )
            .into_response()
    }

    async fn attach(
        State(capture): State<SharedCapture>,
        headers: HeaderMap,
        upgrade: WebSocketUpgrade,
    ) -> Response {
        capture.lock().expect("capture lock").attach_authorization = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        upgrade.on_upgrade(move |socket| serve_sideband(socket, capture))
    }

    async fn send_json(socket: &mut WebSocket, value: Value) {
        socket
            .send(AxumMessage::Text(value.to_string().into()))
            .await
            .expect("fixture send");
    }

    async fn recv_json(socket: &mut WebSocket, capture: &SharedCapture) -> Value {
        loop {
            match socket.recv().await {
                Some(Ok(AxumMessage::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).expect("client event JSON");
                    capture
                        .lock()
                        .expect("capture lock")
                        .client_events
                        .push(value.clone());
                    return value;
                }
                Some(Ok(_)) => continue,
                other => panic!("sideband closed early: {other:?}"),
            }
        }
    }

    async fn serve_sideband(mut socket: WebSocket, capture: SharedCapture) {
        let snapshot = json!({"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":12345.5});
        send_json(
            &mut socket,
            json!({"type":"session.started","event_id":"s","session":snapshot}),
        )
        .await;
        // Media reflection and a transcript race ahead of the seed acknowledgement.
        send_json(
            &mut socket,
            json!({"type":"session.input_audio.append","audio":"AAAA"}),
        )
        .await;
        send_json(&mut socket, input_delta("book me a table")).await;
        let seed = recv_json(&mut socket, &capture).await;
        assert_eq!(seed["type"], "session.commentary.append");
        assert!(seed["delegation_id"].is_null());
        send_json(&mut socket, json!({"type":"session.commentary.appended","event_id":"a1","client_event_id":seed["event_id"],"start_ms":1.0,"end_ms":1.0})).await;
        send_json(&mut socket, delegation_created("dlg_public", "client")).await;
        send_json(&mut socket, output_delta("one moment")).await;
        let release = recv_json(&mut socket, &capture).await;
        assert_eq!(release["type"], "session.commentary.append");
        assert_eq!(release["delegation_id"], "dlg_public");
        send_json(&mut socket, json!({"type":"session.commentary.appended","event_id":"a2","client_event_id":release["event_id"],"start_ms":2.0,"end_ms":2.0})).await;
        let mute = recv_json(&mut socket, &capture).await;
        assert_eq!(mute["type"], "session.input_audio.mute");
        let close = recv_json(&mut socket, &capture).await;
        assert_eq!(close["type"], "session.close");
        send_json(&mut socket, json!({"type":"session.closed","event_id":"c","session":snapshot,"reason":"close_requested","usage":{"seconds":2.5}})).await;
        drop(socket);
    }

    async fn local_server() -> (String, SharedCapture, tokio::task::JoinHandle<()>) {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        (format!("http://{address}/v1/"), capture, server)
    }

    #[tokio::test]
    async fn broker_creates_attaches_seeds_and_lowers_public_events() {
        let (base_url, capture, server) = local_server().await;
        let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
            realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
            &base_url,
        )
        .expect("admitted factory");
        let config = PublicLiveOpenConfig::new("v=0\r\nOFFER_SDP", "marin")
            .expect("valid config")
            .with_instructions("Catalog guidance.");
        let bootstrap = factory.open(config).await.expect("public bootstrap");
        assert_eq!(bootstrap.answer_sdp(), "v=0\r\nPUBLIC_ANSWER_SDP");
        assert!(!format!("{bootstrap:?}").contains("PUBLIC_ANSWER_SDP"));
        let (_, session) = bootstrap.into_parts();
        {
            let capture = capture.lock().expect("capture lock");
            let body = capture.create_body.as_ref().expect("create body");
            assert_eq!(body["session"]["model"], "gpt-live-1");
            assert_eq!(body["session"]["delegation"]["type"], "client");
            assert_eq!(body["session"]["audio"]["output"]["voice"], "marin");
            assert!(body["session"]["audio"].get("format").is_none());
            assert_eq!(body["session"]["instructions"], "Catalog guidance.");
            assert_eq!(body["transport"]["type"], "webrtc");
            assert_eq!(body["transport"]["sdp"], "v=0\r\nOFFER_SDP");
            assert_eq!(
                capture.create_authorization.as_deref(),
                Some("Bearer credential-secret")
            );
            assert_eq!(
                capture.attach_authorization.as_deref(),
                Some("Bearer credential-secret")
            );
        }

        session
            .await_ready_and_seed_session_context(Some("{\"canonical_messages\":[]}".to_string()))
            .await
            .expect("seed acknowledged");
        // Observations that raced ahead of the acknowledgement are preserved in order.
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::User,
                ..
            })
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::UserTranscriptFragment { text, .. }) if text == "book me a table"
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnSnapshotDelta { .. })
        ));
        let delegation = match session.next_observation().await.unwrap() {
            Some(GptLiveBrokerObservation::ClientDelegationFinal {
                delegation,
                target: GptLiveDelegationTarget::Client,
                transcript,
                ..
            }) if transcript == "book me a table" => delegation,
            other => panic!("expected joined delegation, got {other:?}"),
        };
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnStarted {
                role: GptLiveTurnRole::Assistant,
                ..
            })
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::AssistantTranscriptFragment { text, .. }) if text == "one moment"
        ));
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnSnapshotDelta { .. })
        ));
        let token = session
            .append_delegation_context(&delegation, "Table booked for two.")
            .await
            .expect("delegation append");
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: acked }) if acked == token
        ));
        session.close().await.expect("close requested");
        session
            .close()
            .await
            .expect("duplicate request is idempotent");
        // The terminal event flushes the assistant transcript exactly once.
        let mut finished = Vec::new();
        while let Some(observation) = session.next_observation().await.unwrap() {
            finished.push(observation);
        }
        session
            .close()
            .await
            .expect("confirmed close is idempotent");
        assert!(matches!(
            finished.as_slice(),
            [GptLiveBrokerObservation::TurnFinished { role: GptLiveTurnRole::Assistant, transcript, .. }]
                if transcript == "one moment"
        ));
        let events = capture.lock().expect("capture lock").client_events.clone();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0]["content"], "{\"canonical_messages\":[]}");
        assert_eq!(events[1]["content"], "Table booked for two.");
        assert!(
            events[..2]
                .iter()
                .all(|event| event["event_id"].is_string())
        );
        // Close mutes input first so a pending quiet append can be injected
        // and the provider can confirm closure.
        assert_eq!(events[2]["type"], "session.input_audio.mute");
        assert_eq!(events[3]["type"], "session.close");
        server.abort();
    }

    #[tokio::test]
    async fn thinking_append_sends_only_native_quiet_fragments_and_drains_exact_receipts() {
        for reject_middle in [false, true] {
            let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
            let attach_thinking =
                move |State(capture): State<SharedCapture>, upgrade: WebSocketUpgrade| async move {
                    upgrade.on_upgrade(move |mut socket| async move {
                    let mut commands = Vec::new();
                    for _ in 0..3 {
                        let event = recv_json(&mut socket, &capture).await;
                        assert_eq!(event["type"], "session.thinking.append");
                        assert!(event["delegation_id"].is_null());
                        commands.push(event);
                    }
                    send_json(&mut socket, output_delta("existing reply")).await;
                    send_json(&mut socket, thinking_ack(commands[2]["event_id"].as_str())).await;
                    send_json(
                        &mut socket,
                        if reject_middle {
                            append_rejected(commands[1]["event_id"].as_str())
                        } else {
                            thinking_ack(commands[1]["event_id"].as_str())
                        },
                    ).await;
                    send_json(&mut socket, output_delta(" continues")).await;
                    send_json(&mut socket, thinking_ack(commands[0]["event_id"].as_str())).await;
                    let mute = recv_json(&mut socket, &capture).await;
                    assert_eq!(mute["type"], "session.input_audio.mute");
                    let close = recv_json(&mut socket, &capture).await;
                    assert_eq!(close["type"], "session.close");
                    send_json(&mut socket, json!({
                        "type":"session.closed","event_id":"c","session":{
                            "id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0
                        },"reason":"close_requested","usage":{"seconds":1.0}
                    })).await;
                })
                };
            let app = Router::new()
                .route("/v1/live/sessions", post(create_session))
                .route(
                    "/v1/live/sessions/{session_id}/attach",
                    get(attach_thinking),
                )
                .with_state(Arc::clone(&capture));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let thinking_evidence = thinking_capture::Capture::new().for_channel(9);
            let factory = thinking_evidence
                .scope(async {
                    PublicLiveBrokerFactory::__try_from_target_with_base_url(
                        realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
                        &format!("http://{address}/v1/"),
                    )
                    .unwrap()
                })
                .await;
            let (_, session) = factory
                .open(
                    PublicLiveOpenConfig::new("v=0", "marin")
                        .unwrap()
                        .with_pending_context(),
                )
                .await
                .unwrap()
                .into_parts();
            {
                let captured = capture.lock().unwrap();
                let body = captured.create_body.as_ref().unwrap();
                assert!(body["session"].get("input").is_none(), "{body}");
                let startup = body["session"]["instructions"].as_str().unwrap();
                assert!(startup.ends_with(
                    "Historical session context is being prepared and is not yet available."
                ));
                assert!(
                    captured.client_events.is_empty(),
                    "pending startup never sends commentary"
                );
            }
            assert!(matches!(
                session.append_thinking_context("  ").await,
                Err(GptLiveBrokerError::MissingContext)
            ));
            assert!(matches!(
                session.append_thinking_context("x".repeat(500 * 65)).await,
                Err(GptLiveBrokerError::AppendInFlight)
            ));
            let text = "fact ".repeat(250);
            let token = session.append_thinking_context(text.clone()).await.unwrap();
            let mut observations = Vec::new();
            for _ in 0..6 {
                observations.push(
                    tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        session.next_observation(),
                    )
                    .await
                    .expect("receipt or transcript arrives")
                    .unwrap()
                    .unwrap(),
                );
            }
            let expected = if reject_middle {
                GptLiveBrokerObservation::ThinkingContextAppendRejected { token }
            } else {
                GptLiveBrokerObservation::ThinkingContextAppendAcknowledged { token }
            };
            assert_eq!(
                observations
                    .iter()
                    .filter(|item| **item == expected)
                    .count(),
                1
            );
            let turn_ids = observations
                .iter()
                .filter_map(|observation| match observation {
                    GptLiveBrokerObservation::TurnStarted { turn, .. }
                    | GptLiveBrokerObservation::TurnSnapshotDelta { turn, .. } => Some(turn),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(turn_ids.len(), 3);
            assert!(turn_ids.iter().all(|turn| *turn == turn_ids[0]));
            session.close().await.unwrap();
            assert!(matches!(
                session.next_observation().await.unwrap(),
                Some(GptLiveBrokerObservation::TurnFinished { transcript, .. })
                    if transcript == "existing reply continues"
            ));
            assert!(session.next_observation().await.unwrap().is_none());
            let events = capture.lock().unwrap().client_events.clone();
            assert_eq!(
                events.len(),
                5,
                "three fragments, the close-time input mute, and the close; no automatic retry"
            );
            assert_eq!(
                events[..3]
                    .iter()
                    .map(|event| event["content"].as_str().unwrap())
                    .collect::<String>(),
                text
            );
            let ids = events[..3]
                .iter()
                .map(|event| event["event_id"].as_str().unwrap())
                .collect::<HashSet<_>>();
            assert_eq!(ids.len(), 3);
            let recorded = thinking_evidence.drain().unwrap();
            assert!(thinking_evidence.fault().is_none());
            assert!(recorded.iter().all(|event| event.channel_ordinal == 9));
            let recorded_fragments = recorded
                .iter()
                .filter_map(|event| match &event.event {
                    thinking_capture::EventKind::ThinkingAppendAttempt {
                        client_event_id,
                        text,
                    } => {
                        assert!(ids.contains(client_event_id.as_str()));
                        Some(text.as_str())
                    }
                    _ => None,
                })
                .collect::<String>();
            assert_eq!(
                recorded_fragments, text,
                "recorder must retain exact outgoing fragments"
            );
            let acknowledgements = recorded
                .iter()
                .filter(|event| {
                    matches!(&event.event,
                        thinking_capture::EventKind::ThinkingAppended {
                            client_event_id: Some(id), matched_owned: true, accepted: true,
                        } if ids.contains(id.as_str())
                    )
                })
                .count();
            assert_eq!(acknowledgements, if reject_middle { 2 } else { 3 });
            let encoded = serde_json::to_string(&recorded).unwrap();
            for forbidden in [
                "authorization",
                "offer_sdp",
                "instructions",
                "api_key",
                "existing reply",
            ] {
                assert!(!encoded.contains(forbidden));
            }
            assert!(session.state.lock().await.pending_appends.is_empty());
            // A failed transport write still retains the whole append token and
            // fences every later lane instead of retrying an uncertain fragment.
            let error = session
                .append_thinking_context("x".repeat(501))
                .await
                .unwrap_err();
            let GptLiveBrokerError::AppendDeliveryAmbiguous { token: ambiguous } = error else {
                panic!("closed sender must preserve ambiguous delivery");
            };
            assert_ne!(ambiguous, token);
            assert_eq!(session.state.lock().await.outstanding_receipt_count(), 2);
            assert!(matches!(
                session.append_thinking_context("later").await,
                Err(GptLiveBrokerError::AppendInFlight)
            ));
            assert!(matches!(
                session.append_session_context("later").await,
                Err(GptLiveBrokerError::AppendInFlight)
            ));
            server.abort();
        }
    }

    #[tokio::test]
    async fn thinking_close_interruption_drains_through_ingress_but_transport_eof_does_not() {
        for confirmed_close in [false, true] {
            let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
            let attach_closing = move |State(capture): State<SharedCapture>,
                                       upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(move |mut socket| async move {
                    let first = recv_json(&mut socket, &capture).await;
                    let second = recv_json(&mut socket, &capture).await;
                    assert_eq!(first["type"], "session.thinking.append");
                    assert_eq!(second["type"], "session.thinking.append");
                    send_json(&mut socket, thinking_ack(first["event_id"].as_str())).await;
                    if confirmed_close {
                        send_json(&mut socket, session_closed()).await;
                    } else {
                        socket.send(AxumMessage::Close(None)).await.unwrap();
                    }
                })
            };
            let app = Router::new()
                .route("/v1/live/sessions", post(create_session))
                .route("/v1/live/sessions/{session_id}/attach", get(attach_closing))
                .with_state(capture);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
                realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
                &format!("http://{address}/v1/"),
            )
            .unwrap();
            let (_, session) = factory
                .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
                .await
                .unwrap()
                .into_parts();
            let token = session
                .append_thinking_context("x".repeat(501))
                .await
                .unwrap();
            let observation = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                session.next_observation(),
            )
            .await
            .expect("provider closes the transport or session");
            if confirmed_close {
                assert_eq!(
                    observation.unwrap(),
                    Some(
                        GptLiveBrokerObservation::ThinkingContextAppendInterruptedByClose { token }
                    )
                );
                assert!(session.next_observation().await.unwrap().is_none());
                assert!(session.state.lock().await.pending_appends.is_empty());
            } else {
                assert!(matches!(
                    observation,
                    Err(GptLiveBrokerError::Transport {
                        class: GptLiveBrokerTerminalClass::WebSocket
                    })
                ));
                let mut state = session.state.lock().await;
                assert!(!state.closed_observed);
                assert_eq!(state.outstanding_receipt_count(), 1);
                assert_eq!(state.pending_appends[0].token, token);
                assert!(
                    drain(&mut state).is_empty(),
                    "bare EOF cannot mint a close interruption"
                );
            }
            server.abort();
        }
    }

    #[test]
    fn delegation_acknowledgement_does_not_finish_assistant_output() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("book a table")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_hold", "client")))
            .unwrap();
        state
            .apply_frame(frame(output_delta("one moment")))
            .unwrap();
        let original_turn = state.open_turn.as_ref().unwrap().provider_ref.clone();
        drain(&mut state);
        let token = state.reserve_append(PendingAppendLane::Delegation).unwrap();
        state
            .apply_frame(frame(ack(Some(&pending_event_id(token)))))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: acknowledged }]
                if *acknowledged == token
        ));
        state.apply_frame(frame(output_delta(", booked"))).unwrap();
        assert_eq!(
            state.open_turn.as_ref().unwrap().provider_ref,
            original_turn
        );
        assert_eq!(
            join_segments(&state.open_turn.as_ref().unwrap().segments),
            "one moment, booked"
        );
    }

    #[tokio::test]
    async fn long_pause_and_delayed_delegation_readout_preserve_output_identity() {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        async fn quiet_after_output(mut socket: WebSocket, capture: SharedCapture) {
            let snapshot = json!({"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0});
            send_json(
                &mut socket,
                json!({"type":"session.started","event_id":"s","session":snapshot}),
            )
            .await;
            send_json(&mut socket, input_delta("book a table")).await;
            send_json(&mut socket, delegation_created("dlg_pause", "client")).await;
            send_json(&mut socket, output_delta("one moment")).await;
            send_json(&mut socket, json!({"type":"session.output_audio.delta","delta":"AAAA","start_ms":1.0,"end_ms":2.0})).await;
            let result = recv_json(&mut socket, &capture).await;
            send_json(&mut socket, ack(result["event_id"].as_str())).await;
            // Exceed both the old quiet timeout and its result-readout grace.
            tokio::time::sleep(std::time::Duration::from_millis(4500)).await;
            send_json(&mut socket, output_delta(", booked")).await;
            tokio::time::sleep(std::time::Duration::from_millis(1750)).await;
            send_json(&mut socket, output_delta(" for two")).await;
            let mute = recv_json(&mut socket, &capture).await;
            assert_eq!(mute["type"], "session.input_audio.mute");
            let close = recv_json(&mut socket, &capture).await;
            assert_eq!(close["type"], "session.close");
            send_json(&mut socket, json!({"type":"session.closed","event_id":"c","session":snapshot,"reason":"close_requested","usage":{"seconds":6.5}})).await;
        }
        async fn attach_quiet(
            State(capture): State<SharedCapture>,
            upgrade: WebSocketUpgrade,
        ) -> Response {
            upgrade.on_upgrade(move |socket| quiet_after_output(socket, capture))
        }
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach_quiet))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
            realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
            &format!("http://{address}/v1/"),
        )
        .expect("admitted factory");
        let (_, session) = factory
            .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
            .await
            .expect("bootstrap")
            .into_parts();
        let delegation = loop {
            if let Some(GptLiveBrokerObservation::ClientDelegationFinal { delegation, .. }) =
                session.next_observation().await.unwrap()
            {
                break delegation;
            }
        };
        let mut observations = Vec::new();
        for _ in 0..3 {
            observations.push(session.next_observation().await.unwrap().unwrap());
        }
        let token = session
            .append_delegation_context(&delegation, "Booked.")
            .await
            .unwrap();
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: acknowledged })
                if acknowledged == token
        ));
        for _ in 0..4 {
            let observation = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                session.next_observation(),
            )
            .await
            .expect("provider continuation arrives")
            .unwrap()
            .expect("stream stays open");
            observations.push(observation);
        }
        assert!(matches!(
            observations.as_slice(),
            [
                GptLiveBrokerObservation::TurnStarted { role: GptLiveTurnRole::Assistant, turn: started_turn },
                GptLiveBrokerObservation::AssistantTranscriptFragment { text: first, .. },
                GptLiveBrokerObservation::TurnSnapshotDelta { turn: first_turn, .. },
                GptLiveBrokerObservation::AssistantTranscriptFragment { text: second, .. },
                GptLiveBrokerObservation::TurnSnapshotDelta { turn: second_turn, .. },
                GptLiveBrokerObservation::AssistantTranscriptFragment { text: third, .. },
                GptLiveBrokerObservation::TurnSnapshotDelta { turn: third_turn, .. },
            ] if started_turn == first_turn && started_turn == second_turn && started_turn == third_turn
                && first == "one moment" && second == ", booked" && third == " for two"
        ));
        session.close().await.unwrap();
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::TurnFinished { role: GptLiveTurnRole::Assistant, transcript, .. })
                if transcript == "one moment, booked for two"
        ));
        assert!(session.next_observation().await.unwrap().is_none());
        server.abort();
    }

    #[tokio::test]
    async fn transport_eof_before_or_during_close_without_session_closed_never_finishes_output() {
        for during_close in [false, true] {
            let attach_unconfirmed = move |upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(move |mut socket| async move {
                    send_json(
                    &mut socket,
                    json!({"type":"session.started","event_id":"s","session":{
                        "id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0
                    }}),
                )
                .await;
                    send_json(&mut socket, output_delta("unfinished")).await;
                    if during_close {
                        for expected in ["session.input_audio.mute", "session.close"] {
                            let message = socket.recv().await.unwrap().unwrap();
                            let AxumMessage::Text(text) = message else {
                                panic!("expected {expected} request");
                            };
                            let request: Value = serde_json::from_str(&text).unwrap();
                            assert_eq!(request["type"], expected);
                        }
                    }
                    socket.send(AxumMessage::Close(None)).await.unwrap();
                })
            };
            let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
            let app = Router::new()
                .route("/v1/live/sessions", post(create_session))
                .route(
                    "/v1/live/sessions/{session_id}/attach",
                    get(attach_unconfirmed),
                )
                .with_state(capture);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
                realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
                &format!("http://{address}/v1/"),
            )
            .unwrap();
            let (_, session) = factory
                .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
                .await
                .unwrap()
                .into_parts();
            for _ in 0..3 {
                let observation = session.next_observation().await.unwrap().unwrap();
                assert!(!matches!(
                    observation,
                    GptLiveBrokerObservation::TurnFinished { .. }
                ));
            }
            if during_close {
                session
                    .close()
                    .await
                    .expect("request close before bare transport EOF");
            }
            assert!(matches!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    session.next_observation()
                )
                .await
                .expect("closed socket is observed"),
                Err(GptLiveBrokerError::Transport {
                    class: GptLiveBrokerTerminalClass::WebSocket
                })
            ));
            let state = session.state.lock().await;
            assert!(!state.closed_observed);
            assert_eq!(state.close_requested, during_close);
            assert_eq!(
                join_segments(&state.open_turn.as_ref().unwrap().segments),
                "unfinished"
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn seed_failure_closes_without_retrying() {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        async fn reject_seed(mut socket: WebSocket, capture: SharedCapture) {
            let snapshot = json!({"id":"live_fixture","model":"gpt-live-1","status":"active","expires_at":1.0});
            send_json(
                &mut socket,
                json!({"type":"session.started","event_id":"s","session":snapshot}),
            )
            .await;
            let _seed = recv_json(&mut socket, &capture).await;
            send_json(
                &mut socket,
                json!({"type":"session.future.event","event_id":"x"}),
            )
            .await;
            let mute = recv_json(&mut socket, &capture).await;
            assert_eq!(mute["type"], "session.input_audio.mute");
            let close = recv_json(&mut socket, &capture).await;
            assert_eq!(close["type"], "session.close");
        }
        async fn attach_reject(
            State(capture): State<SharedCapture>,
            upgrade: WebSocketUpgrade,
        ) -> Response {
            upgrade.on_upgrade(move |socket| reject_seed(socket, capture))
        }
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach_reject))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve fixture");
        });
        let factory = PublicLiveBrokerFactory::__try_from_target_with_base_url(
            realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
            &format!("http://{address}/v1/"),
        )
        .expect("admitted factory");
        let (_, session) = factory
            .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
            .await
            .expect("bootstrap")
            .into_parts();
        assert_protocol_error(
            session
                .await_ready_and_seed_session_context(Some("seed".to_string()))
                .await
                .expect_err("unsupported event during seed fails closed"),
        );
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if capture
                    .lock()
                    .unwrap()
                    .client_events
                    .iter()
                    .any(|event| event["type"] == "session.close")
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("close requested after seed failure");
        assert_eq!(
            capture
                .lock()
                .unwrap()
                .client_events
                .iter()
                .filter(|event| event["type"] == "session.commentary.append")
                .count(),
            1,
            "the seed append is never retried"
        );
        server.abort();
    }
}

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
//!
//! Measured provider behavior (Release Turbo S S106, 2026-10-01): a session
//! created with a long verbatim startup history (25-29 user and assistant
//! items, only 3.5-3.8 KB, about 1.2k estimated tokens) stopped emitting input
//! transcription for the user's next utterance after a long assistant answer
//! in 8 of 10 attempts, against 1 of 8 when the startup history was a summary
//! plus at most the recent-turns window and 0 of 18 on the fresh-summary path.
//! The input deltas simply stop about 0.5 s before the speech ends: no turn
//! end, no reply and no error event follow. It is not size-related, and the
//! provider-side cause is unknown, so callers keep verbatim startup history to
//! the recent-turns window.

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
    GptLiveDelegationRef, GptLiveDelegationTarget, GptLiveRepresentedUserTurn,
    GptLiveTranscriptItemRef, GptLiveTurnRef, GptLiveTurnRole, protocol_error, require_context,
    summarize_unknown_provider_event,
};

pub use crate::gpt_live_broker::{GptLiveProviderInputLatency, GptLiveProviderInputLatencyStatus};
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
            /// The developer item summarizes only the history before the
            /// verbatim items that follow it (a retained summary).
            #[serde(default)]
            preceding_history_summary: bool,
            /// Text bytes of every startup input item.
            #[serde(default)]
            input_bytes: usize,
            /// The startup budget's conservative token estimate of them.
            #[serde(default)]
            estimated_tokens: usize,
            /// Text of every startup input item, in order (test fixtures
            /// only, like the append attempt texts).
            #[serde(default)]
            input_texts: Vec<String>,
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
        /// A provider `info` notice (for example a throttle notice): evidence
        /// only, never acted on.
        ProviderInfo {
            event_id: String,
            code: String,
            message: String,
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
                EventKind::ProviderInfo {
                    event_id,
                    code,
                    message,
                } => {
                    event_id.len() <= Self::MAX_ID_BYTES
                        && code.len() <= Self::MAX_ID_BYTES
                        && message.len() <= Self::MAX_TEXT_BYTES
                }
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
    /// A summary as one developer item, plus canonical turns seeded verbatim
    /// after it. What the summary covers decides which of those turns the
    /// startup input budget may drop.
    FactualSummary {
        summary: String,
        recent: Vec<InitialItem>,
        coverage: SummaryCoverage,
    },
    /// Historical context is still being prepared (a late summary). The most
    /// recent canonical turns, when any are given, already ride the startup
    /// input verbatim so the model can answer about them before the summary
    /// lands.
    HistoricalContextPending {
        recent: Vec<InitialItem>,
    },
}

/// What a seeded summary covers relative to the verbatim turns after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SummaryCoverage {
    /// The summary covers the whole context at open; the verbatim turns are
    /// the most recent ones it also covers, and the budget drops the oldest
    /// of them first.
    Opening,
    /// The summary covers everything before the verbatim turns, which are
    /// the only record of the conversation after it. None of them is ever
    /// dropped: an oversize seed rejects channel creation instead.
    PrecedingHistory,
}

/// Provider limits on startup history: 128 messages and 8,192 rendered
/// tokens. Tokens are estimated conservatively at three bytes each; the
/// provider's own tokenizer verdict stays visible as a startup error.
pub const LIVE_STARTUP_INPUT_MAX_ITEMS: usize = 128;

/// Most verbatim history items a summary-bearing startup seed carries beside
/// its summary item, counted in provider startup items (the rows
/// `history_item` keeps). Measured 2026-10-01 on S106 (BuildBuddy ce0642cd,
/// 4e3671d0 against 5c1e7fa4, 04bbffbc, 6d78a046): a final reopen seeded
/// with the summary plus 9-14 verbatim items left gpt-live-1 silent after a
/// long answer (the next utterance is transcribed, then no reply, no turn
/// end, no error) in 14 of 48 runs (29%); the same scenario seeded with 0 or
/// 5 items (summary plus at most four) did so in 0 of 22. Earlier: 8/10 at
/// 25-29 verbatim items, 1/8 within four, 0/18 on a fresh summary. The cause
/// is provider behaviour; we can only keep verbatim startup history short.
/// A retained summary (covering only what precedes its verbatim rows) whose
/// rows exceed this is refused, so the open summarizes afresh; a fresh
/// summary covers everything, so its oldest recent items are dropped.
pub const LIVE_STARTUP_VERBATIM_ITEMS_MAX: usize = 4;
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

/// The developer-role startup item carrying `summary`, framed by what it
/// covers.
fn summary_item(summary: &str, coverage: SummaryCoverage) -> InitialItem {
    let prefix = match coverage {
        SummaryCoverage::Opening => PublicLiveContextSeed::SUMMARY_ITEM_PREFIX,
        SummaryCoverage::PrecedingHistory => {
            PublicLiveContextSeed::PRECEDING_HISTORY_SUMMARY_ITEM_PREFIX
        }
    };
    InitialItem {
        role: InitialRole::Developer,
        content: vec![InitialText {
            text: format!("{prefix}\n{summary}"),
            text_type: Some(InitialTextType::InputText),
        }],
        id: Field::Absent,
        status: Field::Absent,
        item_type: Some(MessageType::Message),
    }
}

/// One canonical message as a startup history item under its own role, or
/// `None` for rows the tool-less voice model never sees (executor system
/// messages, notices, tool results) and for empty text.
fn history_item(message: &Message) -> Option<InitialItem> {
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
}

/// Whether a summary of everything before `following` plus every one of
/// those messages fits the startup input limits with nothing dropped (see
/// [`PublicLiveOpenConfig::with_preceding_history_summary`]). The same
/// conservative token estimate as the budget applies.
#[must_use]
pub fn preceding_history_summary_fits(summary: &str, following: &[Message]) -> bool {
    let items: Vec<InitialItem> = following.iter().filter_map(history_item).collect();
    if items.len() > LIVE_STARTUP_VERBATIM_ITEMS_MAX {
        return false;
    }
    let tokens = items
        .iter()
        .chain(std::iter::once(&summary_item(
            summary,
            SummaryCoverage::PrecedingHistory,
        )))
        .map(estimated_startup_tokens)
        .sum::<usize>();
    items.len() < LIVE_STARTUP_INPUT_MAX_ITEMS && tokens <= LIVE_STARTUP_INPUT_TOKEN_BUDGET
}

/// Whether these messages, seeded verbatim with no summary, fit the startup
/// input limits with nothing dropped (see
/// [`PublicLiveOpenConfig::with_pending_context_after_recent`]).
#[must_use]
pub fn recent_history_fits(recent: &[Message]) -> bool {
    let items = bounded_late_recent_items(recent);
    let tokens = items.iter().map(estimated_startup_tokens).sum::<usize>();
    items.len() < LIVE_STARTUP_INPUT_MAX_ITEMS && tokens <= LIVE_STARTUP_INPUT_TOKEN_BUDGET
}

/// The verbatim items a Late (summary-pending) open seeds from `recent`: the
/// newest [`LIVE_STARTUP_VERBATIM_ITEMS_MAX`], starting at a user row so no
/// reply is seeded without its question. Trimming is safe here: the late
/// summary covers the whole history, these rows included; they only let the
/// model answer about the newest turns before it lands.
fn bounded_late_recent_items(recent: &[Message]) -> Vec<InitialItem> {
    let items: Vec<InitialItem> = recent.iter().filter_map(history_item).collect();
    let newest = &items[items.len().saturating_sub(LIVE_STARTUP_VERBATIM_ITEMS_MAX)..];
    let first_user = newest
        .iter()
        .position(|item| item.role == InitialRole::User)
        .unwrap_or(newest.len());
    newest[first_user..].to_vec()
}

/// Compose the startup input from a leading item that is always kept and a
/// tail of recent items, dropping the oldest tail items until both limits
/// hold, then any replies left at the front of the tail so the verbatim part
/// starts at a user row (no answer seeded without its question, as on the
/// Late path). The truncation is typed and reported by the caller.
fn budget_startup_input(
    keep: InitialItem,
    recent: &[InitialItem],
) -> (Vec<InitialItem>, LiveStartupInputTruncation) {
    let mut truncation = LiveStartupInputTruncation::default();
    let mut items = Vec::with_capacity(recent.len() + 1);
    let mut tokens = estimated_startup_tokens(&keep);
    // Newest first so the oldest are the ones left out. The summary covers
    // every recent item, so the verbatim bound only trims repetition.
    let mut kept_recent = Vec::new();
    for item in recent.iter().rev() {
        let item_tokens = estimated_startup_tokens(item);
        if kept_recent.len() < LIVE_STARTUP_VERBATIM_ITEMS_MAX
            && kept_recent.len() + 1 < LIVE_STARTUP_INPUT_MAX_ITEMS
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
    kept_recent.reverse();
    let first_user = kept_recent
        .iter()
        .position(|item| item.role == InitialRole::User)
        .unwrap_or(kept_recent.len());
    for item in kept_recent.drain(..first_user) {
        truncation.dropped_items += 1;
        truncation.dropped_bytes += item
            .content
            .iter()
            .map(|part| part.text.len())
            .sum::<usize>();
    }
    items.push(keep);
    items.extend(kept_recent);
    (items, truncation)
}

impl PublicLiveContextSeed {
    /// Prefix of the developer-role startup item that carries a summary.
    pub(crate) const SUMMARY_ITEM_PREFIX: &'static str = "Factual summary of the background agent's context at voice-channel open (context data, not a new user request):";

    /// Prefix of the developer-role startup item that carries a summary of
    /// everything before the verbatim messages that follow it (a summary
    /// retained from an earlier open plus the conversation since).
    pub(crate) const PRECEDING_HISTORY_SUMMARY_ITEM_PREFIX: &'static str = "Factual summary of this conversation before the messages that follow, which continue it verbatim (context data, not a new user request):";

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
            Self::HistoricalContextPending { recent } => {
                (!recent.is_empty()).then(|| recent.clone())
            }
            Self::Absent => None,
        }
    }

    /// The budgeted startup input for a summary seed and what the budget
    /// dropped. Only meaningful for [`Self::FactualSummary`].
    fn startup_input_plan(&self) -> (Vec<InitialItem>, LiveStartupInputTruncation) {
        match self {
            Self::FactualSummary {
                summary,
                recent,
                coverage,
            } => {
                let developer = summary_item(summary, *coverage);
                if *coverage == SummaryCoverage::PrecedingHistory {
                    // Every verbatim turn is uncovered history: nothing is
                    // dropped, and the provider rejects an oversize seed.
                    let mut items = Vec::with_capacity(recent.len() + 1);
                    items.push(developer);
                    items.extend(recent.iter().cloned());
                    return (items, LiveStartupInputTruncation::default());
                }
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
            Self::HistoricalContextPending { recent } if recent.is_empty() => Some(
                "Voice-channel context availability (factual state, not a new user request):\nHistorical session context is being prepared and is not yet available."
                    .to_string(),
            ),
            Self::HistoricalContextPending { .. } => Some(
                "Voice-channel context availability (factual state, not a new user request):\nThe most recent conversation turns are in the session input. A summary of the earlier history is being prepared and is not yet available."
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
            Self::FactualSummary {
                recent, coverage, ..
            } => formatter
                .debug_struct("FactualSummary")
                .field("summary", &"<redacted>")
                .field("recent", &recent.len())
                .field("coverage", coverage)
                .finish(),
            Self::HistoricalContextPending { recent } => formatter
                .debug_struct("HistoricalContextPending")
                .field("recent", &recent.len())
                .finish(),
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
        let input = messages.iter().filter_map(history_item).collect();
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
            PublicLiveContextSeed::HistoricalContextPending { recent } => recent,
            PublicLiveContextSeed::Absent => Vec::new(),
        };
        self.context_seed = PublicLiveContextSeed::FactualSummary {
            summary: summary.to_owned(),
            recent,
            coverage: SummaryCoverage::Opening,
        };
        self
    }

    /// Lower a summary of everything before `following` as one
    /// `developer`-role startup item, then every message of `following`
    /// verbatim under its own role: a summary retained from an earlier open
    /// plus the conversation since it. The developer item says the summary
    /// ends where the verbatim messages begin. Nothing is dropped to fit the
    /// provider limits, because the verbatim messages are the only record of
    /// that conversation; check [`preceding_history_summary_fits`] first, and
    /// an oversize seed rejects channel creation. Replaces any history or
    /// summary selected before.
    #[must_use]
    pub fn with_preceding_history_summary(mut self, summary: &str, following: &[Message]) -> Self {
        self.context_seed = PublicLiveContextSeed::FactualSummary {
            summary: summary.to_owned(),
            recent: following.iter().filter_map(history_item).collect(),
            coverage: SummaryCoverage::PrecedingHistory,
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
        self.context_seed = PublicLiveContextSeed::HistoricalContextPending { recent: Vec::new() };
        self
    }

    /// [`Self::with_pending_context`], with the most recent canonical turns
    /// seeded verbatim as startup input under their own roles: the late
    /// summary still covers the whole history, but a question about the
    /// newest turns no longer waits for it. At most
    /// [`LIVE_STARTUP_VERBATIM_ITEMS_MAX`] of the newest items are seeded,
    /// starting at a user row (the late summary covers the rest); nothing
    /// else is dropped to fit the provider limits, so check
    /// [`recent_history_fits`] first.
    #[must_use]
    pub fn with_pending_context_after_recent(mut self, recent: &[Message]) -> Self {
        self.context_seed = PublicLiveContextSeed::HistoricalContextPending {
            recent: bounded_late_recent_items(recent),
        };
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
                input_bytes: items
                    .iter()
                    .flat_map(|item| item.content.iter())
                    .map(|part| part.text.len())
                    .sum(),
                estimated_tokens: items.iter().map(estimated_startup_tokens).sum(),
                input_texts: items
                    .iter()
                    .map(|item| {
                        item.content
                            .iter()
                            .map(|part| part.text.as_str())
                            .collect::<String>()
                    })
                    .collect(),
                preceding_history_summary: items.iter().any(|item| {
                    item.role == InitialRole::Developer
                        && item.content.iter().any(|part| {
                            part.text.starts_with(
                                PublicLiveContextSeed::PRECEDING_HISTORY_SUMMARY_ITEM_PREFIX,
                            )
                        })
                }),
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
        self.append_delegation_commentary(delegation, text, false)
            .await
    }

    /// Append an executor result to an observed client delegation, as
    /// commentary exactly like [`Self::append_delegation_context`].
    ///
    /// Measured against gpt-live-1, the model occasionally acknowledges a
    /// result commentary and never voices it. When the result's
    /// acknowledgement (the Delivered transition) finds the session idle, so
    /// no transcript delta from either speaker arrived since this
    /// delegation's release began, the broker follows the result with one
    /// short instructions-lane cue, the lane the provider documents for
    /// prompting speech. The cue is broker-owned: its receipt is consumed
    /// here and never surfaced.
    pub async fn append_delegation_result(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        self.append_delegation_commentary(delegation, text, true)
            .await
    }

    async fn append_delegation_commentary(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
        result: bool,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let token = self
            .state
            .lock()
            .await
            .reserve_delegation_commentary(result)?;
        let event = Self::commentary_event(token, text, Nullable(Some(delegation.0.clone())));
        self.deliver_append(token, event).await
    }

    /// Send the result cues whose idle condition held at acknowledgement.
    async fn send_due_result_cues(&self) -> Result<(), GptLiveBrokerError> {
        loop {
            let Some(token) = self.state.lock().await.reserve_due_result_cue()? else {
                return Ok(());
            };
            let event = Self::instructions_event(token, 0, LIVE_RESULT_CUE.to_owned());
            self.deliver_append(token, event).await?;
            tracing::info!("public Live result cue sent");
        }
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
            if let (
                Some(capture),
                ServerEvent::Info {
                    event_id,
                    code,
                    message,
                },
            ) = (&self.thinking_capture, &frame.event)
            {
                // A notice is evidence; an oversized one is truncated rather
                // than faulting the whole capture.
                let bounded = |text: &str, limit: usize| {
                    let mut end = text.len().min(limit);
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text[..end].to_owned()
                };
                capture.record(thinking_capture::EventKind::ProviderInfo {
                    event_id: bounded(event_id, thinking_capture::Capture::MAX_ID_BYTES),
                    code: bounded(code, thinking_capture::Capture::MAX_ID_BYTES),
                    message: bounded(message, thinking_capture::Capture::MAX_TEXT_BYTES),
                });
            }
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
            let cues_due = state.due_result_cues > 0 && !state.close_requested;
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
            if cues_due {
                drop(state);
                self.send_due_result_cues().await?;
            }
        }
    }

    /// Latest provider input latency reading beside the current reflected
    /// input clock (see [`GptLiveProviderInputLatency`]). Telemetry only.
    pub async fn provider_input_latency(&self) -> GptLiveProviderInputLatencyStatus {
        let state = self.state.lock().await;
        GptLiveProviderInputLatencyStatus {
            latest: state.provider_input_latency,
            reflected_input_clock_ms: state.reflected_input_clock_ms(),
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
    /// A broker-owned result cue: its receipts drain here and nothing about
    /// it is surfaced, so the sideband's append correlation never sees it.
    internal: bool,
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
    /// The committed user rows behind this utterance, in order: the turn
    /// itself when a `TurnFinished` or an open-turn delegation committed it,
    /// or the rows a detached delegation re-presented.
    rows: Vec<GptLiveRepresentedUserTurn>,
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
    /// User turns finished (by a speaker change) since the previous client
    /// delegation: what a delegation that finds no open user turn
    /// re-presents.
    window_finished_user_turns: Vec<GptLiveRepresentedUserTurn>,
    /// The previous delegation's executor request, re-presented when a
    /// delegation arrives without any new user transcript in its window.
    last_request: Option<String>,
    /// Provider id of the client delegation created while a user turn was
    /// open (mid-utterance): the next user turn to finish before another
    /// client delegation continues that utterance.
    continuation_of: Option<String>,
    window: DelegationWindow,
    seen_delegation_ids: HashSet<String>,
    queued_observations: VecDeque<GptLiveBrokerObservation>,
    reflected_output_audio_frames: u64,
    /// Samples of reflected provider input (PCM16 24 kHz): the provider's
    /// input clock, which input-transcript spans are expressed in.
    reflected_input_samples: u64,
    /// Latest provider input latency, measured on each input-transcript delta.
    provider_input_latency: Option<GptLiveProviderInputLatency>,
    /// Reflected input clock of the last latency telemetry emission.
    provider_input_latency_emitted_at_ms: u64,
    /// Session-timeline end of the last output transcript delta.
    last_output_end_ms: Option<f64>,
    /// Whether an input transcript delta arrived after the last output one.
    input_since_output: bool,
    /// Session-timeline start of the commentary acknowledgement being
    /// applied (`session.commentary.appended.start_ms`).
    commentary_ack_start_ms: Option<f64>,
    /// Result appends awaiting their acknowledgement.
    result_cue_candidates: HashSet<GptLiveAppendToken>,
    /// Result cues whose idle condition held at acknowledgement, not yet sent.
    due_result_cues: usize,
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
            window_finished_user_turns: Vec::new(),
            last_request: None,
            continuation_of: None,
            window: DelegationWindow::default(),
            seen_delegation_ids: HashSet::new(),
            queued_observations: VecDeque::new(),
            reflected_output_audio_frames: 0,
            reflected_input_samples: 0,
            provider_input_latency: None,
            provider_input_latency_emitted_at_ms: 0,
            last_output_end_ms: None,
            input_since_output: false,
            commentary_ack_start_ms: None,
            result_cue_candidates: HashSet::new(),
            due_result_cues: 0,
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

    /// Reserve one delegation-lane commentary append; a result is also a
    /// cue candidate decided at its acknowledgement.
    fn reserve_delegation_commentary(
        &mut self,
        result: bool,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let token = self.reserve_append(PendingAppendLane::Delegation)?;
        if result {
            self.result_cue_candidates.insert(token);
        }
        Ok(token)
    }

    /// Reserve the next due result cue as a broker-owned instructions append.
    fn reserve_due_result_cue(&mut self) -> Result<Option<GptLiveAppendToken>, GptLiveBrokerError> {
        if self.due_result_cues == 0 {
            return Ok(None);
        }
        self.due_result_cues -= 1;
        let token = self.reserve_instructions_append(1)?;
        if let Some(pending) = self.pending_appends.back_mut() {
            pending.internal = true;
        }
        Ok(Some(token))
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
            internal: false,
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
                    if !pending.rejected && !pending.internal {
                        observations.push_back(interrupted);
                    }
                    false
                });
            }
            ServerEvent::CommentaryAppended { start_ms, .. } => {
                self.commentary_ack_start_ms = Some(start_ms);
                let acknowledged = self
                    .acknowledge_append(AppendReceiptKind::Commentary, client_event_id.as_deref());
                self.commentary_ack_start_ms = None;
                acknowledged?;
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
                self.input_since_output = true;
                self.record_transcript_delta(GptLiveTurnRole::User, delta);
                self.measure_provider_input_latency(end_ms);
            }
            ServerEvent::OutputTranscriptDelta { delta, end_ms, .. } => {
                self.last_output_end_ms = Some(end_ms);
                self.input_since_output = false;
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
            ServerEvent::InputAudio { audio } => {
                // Reflected input is not conversational authority; it is the
                // provider's input clock for the backlog measurement.
                self.reflected_input_samples = self
                    .reflected_input_samples
                    .saturating_add(reflected_pcm16_samples(&audio));
                // Keep the telemetry's clock current even when no transcript
                // arrives (a total stall), paced by the provider's own input
                // clock rather than a timer.
                if self.reflected_input_clock_ms()
                    >= self
                        .provider_input_latency_emitted_at_ms
                        .saturating_add(PROVIDER_INPUT_LATENCY_CLOCK_STEP_MS)
                {
                    self.emit_provider_input_latency();
                }
            }
            // A provider notice (for example a throttle notice) is telemetry:
            // logged with its identity so it becomes evidence, and nothing
            // in the runtime decides on it.
            ServerEvent::Info {
                event_id,
                code,
                message,
            } => {
                tracing::info!(%event_id, %code, %message, "public Live provider info");
            }
            ServerEvent::InputAudioMuted { .. }
            | ServerEvent::InputAudioUnmuted { .. }
            | ServerEvent::UsageUpdated { .. }
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
                    if pending.internal {
                        tracing::info!("public Live result cue rejected by the provider");
                        pending.rejected = true;
                    } else if report_rejection && !pending.rejected {
                        self.queued_observations
                            .push_back(pending.lane.rejected(pending.token));
                        pending.rejected = true;
                    }
                    // Retain unmatched fragments after partial rejection so
                    // later exact receipts can drain without fabricating ACK.
                    if pending.outstanding_receipts.is_empty() {
                        let token = pending.token;
                        self.pending_appends.remove(append_index.0);
                        self.result_cue_candidates.remove(&token);
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
            let (token, rejected, internal) = (pending.token, pending.rejected, pending.internal);
            if internal {
                tracing::info!(rejected, "public Live result cue acknowledged");
            } else if !rejected {
                self.queued_observations
                    .push_back(pending.lane.acknowledged(token));
            }
            self.pending_appends.remove(append_index.0);
            if self.result_cue_candidates.remove(&token) && !rejected {
                self.decide_result_cue();
            }
        }
        Ok(())
    }

    /// The result's acknowledgement is the Delivered transition. Both the
    /// acknowledgement and every transcript delta carry session-timeline
    /// positions, so idleness is read from the provider's own ordering, not a
    /// wait: the model is idle when no input followed its last output and its
    /// last output word ended at least [`LIVE_RESULT_CUE_MIN_GAP_MS`] before
    /// the result landed. Only then does the result get a speak cue; output in
    /// progress (or just paused) means the model is already answering.
    fn decide_result_cue(&mut self) {
        let Some(ack_start_ms) = self.commentary_ack_start_ms else {
            return;
        };
        // A channel whose model never spoke has no last output word.
        let gap_ms = self
            .last_output_end_ms
            .map_or(f64::INFINITY, |end| ack_start_ms - end);
        let idle = !self.input_since_output && gap_ms >= LIVE_RESULT_CUE_MIN_GAP_MS;
        if idle {
            tracing::info!(
                gap_ms,
                "public Live result delivered while idle; result cue due"
            );
            self.due_result_cues = self.due_result_cues.saturating_add(1);
        } else {
            tracing::info!(
                gap_ms,
                input_since_output = self.input_since_output,
                "public Live result delivered with speech in progress; result cue suppressed"
            );
        }
    }

    fn reflected_input_clock_ms(&self) -> u64 {
        self.reflected_input_samples.saturating_mul(1000) / SIDEBAND_INPUT_SAMPLE_RATE_HZ
    }

    fn measure_provider_input_latency(&mut self, transcribed_through_ms: f64) {
        let measured_at_reflected_clock_ms = self.reflected_input_clock_ms();
        // Provider spans are non-negative milliseconds; anything else
        // measures from the clock origin rather than inventing a lag.
        let transcribed_through_ms = if transcribed_through_ms.is_finite() {
            transcribed_through_ms.max(0.0) as u64
        } else {
            0
        };
        let latency = GptLiveProviderInputLatency {
            backlog_ms: measured_at_reflected_clock_ms.saturating_sub(transcribed_through_ms),
            measured_at_reflected_clock_ms,
        };
        tracing::debug!(
            backlog_ms = latency.backlog_ms,
            measured_at_reflected_clock_ms,
            "public Live provider input latency"
        );
        self.provider_input_latency = Some(latency);
        self.emit_provider_input_latency();
    }

    fn emit_provider_input_latency(&mut self) {
        let status = GptLiveProviderInputLatencyStatus {
            latest: self.provider_input_latency,
            reflected_input_clock_ms: self.reflected_input_clock_ms(),
        };
        self.provider_input_latency_emitted_at_ms = status.reflected_input_clock_ms;
        self.queued_observations
            .push_back(GptLiveBrokerObservation::ProviderInputLatency(status));
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
    ///
    /// Turns are synthesized from speaker changes because the protocol has no
    /// input item identity and no input-completed event
    /// (`session.input_transcript.delta` carries only `event_id`, `delta`,
    /// `start_ms` and `end_ms`). A user delta that arrives after the model
    /// began answering (the user's last word landing during the reply)
    /// therefore opens a new user turn of its own: nothing in the protocol
    /// joins it to the turn the speaker change finished. This is expected,
    /// not a lost or duplicated utterance.
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
            if let Some(delegation) = self.continuation_of.take()
                && !transcript.trim().is_empty()
            {
                self.queued_observations.push_back(
                    GptLiveBrokerObservation::UserTurnContinuesDelegation {
                        turn: GptLiveTurnRef(open.provider_ref.clone()),
                        delegation: GptLiveDelegationRef(delegation),
                        transcript: transcript.clone(),
                    },
                );
            }
            let row = GptLiveRepresentedUserTurn {
                turn: GptLiveTurnRef(open.provider_ref.clone()),
                transcript: transcript.clone(),
            };
            self.window_finished_user_turns.push(row.clone());
            self.last_user_turn = Some(FinishedUserTurn { rows: vec![row] });
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
        //
        // A detached delegation's user words are already canonical: the
        // speaker change that finished those turns reported them with
        // `TurnFinished`, and the transcript committed them. The fresh turn
        // exists only for lifecycle pairing; the delegation re-presents every
        // user turn finished since the previous delegation (or, with none,
        // the rows behind the last user utterance), so the canonical commit
        // verifies those rows and appends none.
        let finished_in_window = std::mem::take(&mut self.window_finished_user_turns);
        // A new client delegation ends any earlier continuation; one taking
        // an open user turn starts its own (the user may still be speaking).
        self.continuation_of = None;
        let (turn_ref, transcript, represented_turns) = match self.open_turn.take() {
            Some(open) if open.role == GptLiveTurnRole::User => {
                self.continuation_of = Some(reference.0.clone());
                let transcript = join_segments(&open.segments);
                (open.provider_ref, transcript, Vec::new())
            }
            other => {
                self.open_turn = other;
                let represented = if finished_in_window.is_empty() {
                    self.last_user_turn
                        .as_ref()
                        .map(|last| last.rows.clone())
                        .unwrap_or_default()
                } else {
                    finished_in_window
                };
                let transcript = if represented.is_empty() {
                    request_transcript.clone()
                } else {
                    join_window_chunks(represented.iter().map(|row| row.transcript.as_str()))
                };
                (
                    self.mint_turn(GptLiveTurnRole::User).0,
                    transcript,
                    represented,
                )
            }
        };
        tracing::debug!(
            offset_ms,
            represented_turns = represented_turns.len(),
            "public Live client delegation joined its user turn"
        );
        self.last_user_turn = Some(FinishedUserTurn {
            rows: if represented_turns.is_empty() {
                vec![GptLiveRepresentedUserTurn {
                    turn: GptLiveTurnRef(turn_ref.clone()),
                    transcript: transcript.clone(),
                }]
            } else {
                represented_turns.clone()
            },
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
                represented_turns,
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

/// The sideband carries input and output audio as PCM16 at 24 kHz regardless
/// of the negotiated media codec.
const SIDEBAND_INPUT_SAMPLE_RATE_HZ: u64 = 24_000;

/// Reflected-clock advance between provider input latency telemetry
/// emissions when no input transcript arrives.
const PROVIDER_INPUT_LATENCY_CLOCK_STEP_MS: u64 = 1_000;

/// PCM16 samples in one base64 sideband audio payload, from its length alone
/// (the payload is never decoded here).
fn reflected_pcm16_samples(audio: &str) -> u64 {
    let encoded = audio.trim_end_matches('=').len() as u64;
    let bytes = encoded / 4 * 3
        + match encoded % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        };
    bytes / 2
}

/// Test support: the `client_event_id` a public Live commentary append with
/// this token carries, which `session.commentary.appended` echoes.
#[cfg(feature = "test-realtime-fixtures")]
#[doc(hidden)]
#[must_use]
pub fn __commentary_client_event_id(token: GptLiveAppendToken) -> String {
    pending_event_id(token)
}

fn thinking_event_id(token: GptLiveAppendToken, index: usize) -> String {
    format!("meerkat-thinking-{}-{index}", token.0)
}

/// Minimum session-timeline gap between the model's last output word and a
/// result's acknowledgement for the model to count as idle there. Measured on
/// gpt-live-1 (S106, 71 result deliveries, positions quantized to 200 ms):
/// every voiced delivery with no input since the model's last output had a
/// gap of -200 to +400 ms (the model finished a phrase, paused, then read the
/// result); the one captured result the model never voiced landed about
/// 3 s after its last word. This compares provider timestamps; it is not a
/// wait.
const LIVE_RESULT_CUE_MIN_GAP_MS: f64 = 1000.0;

/// The speak cue that follows a result delivered while the session is idle.
/// Neutral and short: it names no content, so it cannot carry or alter the
/// result it points at.
const LIVE_RESULT_CUE: &str = "Tell the user this result now.";

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

    /// Conversational observations; provider input latency telemetry is
    /// asserted by its own tests.
    fn drain(state: &mut SessionState) -> Vec<GptLiveBrokerObservation> {
        state
            .queued_observations
            .drain(..)
            .filter(|observation| {
                !matches!(
                    observation,
                    GptLiveBrokerObservation::ProviderInputLatency(_)
                )
            })
            .collect()
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
        // The verbatim bound holds too: 130 tiny turns keep the developer
        // item and the newest LIVE_STARTUP_VERBATIM_ITEMS_MAX turns (the
        // summary covers all of them).
        let many: Vec<Message> = (0..130)
            .map(|index| Message::User(UserMessage::text(format!("t{index}"))))
            .collect();
        let capped = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&many)
            .with_context_summary("summary");
        let items = capped.context_seed.initial_input().unwrap();
        assert_eq!(items.len(), 1 + LIVE_STARTUP_VERBATIM_ITEMS_MAX);
        assert_eq!(
            capped.startup_input_truncation().dropped_items,
            130 - LIVE_STARTUP_VERBATIM_ITEMS_MAX
        );
        assert_eq!(items[1].content[0].text, "t126", "only the newest kept");
        assert_eq!(
            items[LIVE_STARTUP_VERBATIM_ITEMS_MAX].content[0].text,
            "t129"
        );
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
    fn preceding_history_summary_frames_the_summary_as_ending_where_the_verbatim_turns_begin() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, UserMessage};
        let following = vec![
            Message::User(UserMessage::text("typed while the call was closed")),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "typed answer".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            )),
        ];
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_instructions("Speak briefly.")
            .with_preceding_history_summary("The user planned a trip.", &following);
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let session = factory.session_config(&config);
        session
            .validate()
            .expect("developer item plus the verbatim turns after it");
        let encoded = serde_json::to_value(session).unwrap();
        let input = encoded["input"].as_array().unwrap();
        assert_eq!(input.len(), 3, "{encoded}");
        assert_eq!(input[0]["role"], "developer");
        let text = input[0]["content"][0]["text"].as_str().unwrap();
        // The summary covers only what precedes the verbatim turns: the
        // framing says so, and it never claims to describe the context at
        // this voice-channel open.
        assert_eq!(
            text,
            "Factual summary of this conversation before the messages that follow, which continue it verbatim (context data, not a new user request):\nThe user planned a trip."
        );
        assert!(text.starts_with(PublicLiveContextSeed::PRECEDING_HISTORY_SUMMARY_ITEM_PREFIX));
        assert!(!text.contains("voice-channel open"));
        assert_eq!(input[1]["role"], "user");
        assert_eq!(
            input[1]["content"][0]["text"],
            "typed while the call was closed"
        );
        assert_eq!(input[2]["role"], "assistant");
        assert_eq!(input[2]["content"][0]["text"], "typed answer");
        assert_eq!(encoded["instructions"], "Speak briefly.");
        assert!(!format!("{config:?}").contains("planned a trip"));
        assert!(format!("{config:?}").contains("PrecedingHistory"));
    }

    /// Pins the verbatim startup bound to the measured safe shape: a summary
    /// plus at most four verbatim items (S106 29% silent turns at 9-14 items
    /// against 0% at 0-5; see `LIVE_STARTUP_VERBATIM_ITEMS_MAX`).
    #[test]
    fn live_startup_verbatim_items_bound_is_the_measured_safe_shape() {
        assert_eq!(LIVE_STARTUP_VERBATIM_ITEMS_MAX, 4);
        use meerkat_core::types::UserMessage;
        let rows: Vec<Message> = (0..14)
            .map(|index| Message::User(UserMessage::text(format!("r{index}"))))
            .collect();
        // A retained summary with more verbatim rows than the bound is
        // refused (the open summarizes afresh).
        assert!(!preceding_history_summary_fits("summary", &rows[..10]));
        assert!(preceding_history_summary_fits("summary", &rows[..4]));
        // A fresh summary seeds at most the bound beside its summary item.
        let fresh = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_history(&rows)
            .with_context_summary("summary");
        assert!(
            fresh.context_seed.initial_input().unwrap().len()
                <= 1 + LIVE_STARTUP_VERBATIM_ITEMS_MAX
        );
    }

    /// A fresh-summary open trims its verbatim tail the way the Late path
    /// does: the newest bounded items, starting at a user row. The fresh
    /// summary covers the whole history, so dropped replies are counted as
    /// truncation, never lost.
    #[test]
    fn fresh_summary_seed_keeps_the_newest_bounded_items_from_a_user_row() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, UserMessage};
        let assistant = |text: &str| {
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: text.to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            ))
        };
        let user = |text: &str| Message::User(UserMessage::text(text));
        // u0 a0 u1 a1 ... u6 a6: the newest four are u5 a5 u6 a6.
        let rows: Vec<Message> = (0..7)
            .flat_map(|i| [user(&format!("u{i}")), assistant(&format!("a{i}"))])
            .collect();
        let seed_texts = |rows: &[Message]| {
            let config = PublicLiveOpenConfig::new("v=0", "marin")
                .unwrap()
                .with_history(rows)
                .with_context_summary("summary");
            let items = config.context_seed.initial_input().unwrap();
            assert_eq!(items[0].role, InitialRole::Developer);
            let texts: Vec<String> = items[1..]
                .iter()
                .map(|item| item.content[0].text.clone())
                .collect();
            (texts, config.startup_input_truncation())
        };
        let (texts, truncation) = seed_texts(&rows);
        assert_eq!(texts, ["u5", "a5", "u6", "a6"]);
        assert_eq!(truncation.dropped_items, rows.len() - 4);
        // A cut that lands on a reply drops it: rows ending at u6 have
        // a4 u5 a5 u6 as the newest four, and a4 goes with the rest.
        let (texts, truncation) = seed_texts(&rows[..13]);
        assert_eq!(texts, ["u5", "a5", "u6"]);
        assert_eq!(truncation.dropped_items, 13 - 3);
        assert_eq!(
            truncation.dropped_bytes,
            "u0a0u1a1u2a2u3a3u4a4".len(),
            "every dropped row is reported"
        );
        // A tail with no user row at all seeds the summary alone.
        let (texts, truncation) = seed_texts(&[assistant("only a reply")]);
        assert!(texts.is_empty(), "{texts:?}");
        assert_eq!(truncation.dropped_items, 1);
    }

    /// A Late open seeds at most the verbatim bound of the newest items,
    /// starting at a user row: its late summary covers the whole history.
    #[test]
    fn late_recent_seed_keeps_the_newest_bounded_items_from_a_user_row() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, UserMessage};
        let assistant = |text: &str| {
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: text.to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            ))
        };
        let user = |text: &str| Message::User(UserMessage::text(text));
        // 14 rows: u0 a0 u1 a1 ... u6 a6. The newest four are u5 a5 u6 a6.
        let rows: Vec<Message> = (0..7)
            .flat_map(|i| [user(&format!("u{i}")), assistant(&format!("a{i}"))])
            .collect();
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_pending_context_after_recent(&rows);
        let items = config.context_seed.initial_input().unwrap();
        let texts: Vec<&str> = items
            .iter()
            .map(|item| item.content[0].text.as_str())
            .collect();
        assert_eq!(texts, ["u5", "a5", "u6", "a6"]);
        // A cut that lands on a reply drops it: no answer without its
        // question. Rows ending at u6 have a4 u5 a5 u6 as the newest four.
        let items = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_pending_context_after_recent(&rows[..13])
            .context_seed
            .initial_input()
            .unwrap();
        let texts: Vec<&str> = items
            .iter()
            .map(|item| item.content[0].text.as_str())
            .collect();
        assert_eq!(texts, ["u5", "a5", "u6"]);
        // The fit check judges the bounded seed, so a long recent tail fits.
        assert!(recent_history_fits(&rows));
    }

    #[test]
    fn preceding_history_summary_never_drops_a_verbatim_turn() {
        use meerkat_core::types::UserMessage;
        // Three 9,000-byte turns would lose the oldest under the opening
        // budget; after a preceding-history summary they are the only record
        // of that conversation, so all three stay and the fit check refuses.
        let following: Vec<Message> = (1..=3)
            .map(|index| {
                Message::User(UserMessage::text(format!(
                    "turn {index} {}",
                    "x".repeat(9_000)
                )))
            })
            .collect();
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_preceding_history_summary("summary", &following);
        assert_eq!(
            config.startup_input_truncation(),
            LiveStartupInputTruncation::default()
        );
        let items = config.context_seed.initial_input().unwrap();
        assert_eq!(items.len(), 4);
        assert!(items[1].content[0].text.starts_with("turn 1 "));
        assert!(!preceding_history_summary_fits("summary", &following));
        assert!(preceding_history_summary_fits("summary", &following[..2]));
        // The verbatim bound: the summary plus LIVE_STARTUP_VERBATIM_ITEMS_MAX
        // verbatim items fit, one more does not (the open then summarizes
        // afresh instead of seeding a long verbatim history).
        let many: Vec<Message> = (0..128)
            .map(|index| Message::User(UserMessage::text(format!("t{index}"))))
            .collect();
        assert!(preceding_history_summary_fits(
            "summary",
            &many[..LIVE_STARTUP_VERBATIM_ITEMS_MAX]
        ));
        assert!(!preceding_history_summary_fits(
            "summary",
            &many[..=LIVE_STARTUP_VERBATIM_ITEMS_MAX]
        ));
        // Rows the voice model never sees do not count against the limits.
        let mut with_system = many[..LIVE_STARTUP_VERBATIM_ITEMS_MAX].to_vec();
        with_system.push(Message::System(meerkat_core::types::SystemMessage::new(
            "executor instructions",
        )));
        assert!(preceding_history_summary_fits("summary", &with_system));
    }

    /// A late-summary open can still carry the newest turns verbatim: they
    /// are startup history under their own roles, and the availability
    /// notice says the earlier history is still being summarized.
    /// Recent turns past either startup limit do not fit: the Late open then
    /// carries only the plain pending notice.
    #[test]
    fn recent_turns_past_the_startup_limits_do_not_fit() {
        let utterance = |text: String| Message::User(meerkat_core::types::UserMessage::text(text));
        // Item count no longer refuses a Late seed: only the newest
        // LIVE_STARTUP_VERBATIM_ITEMS_MAX are seeded (the late summary covers
        // the rest), so a long tail of short turns fits.
        let many: Vec<Message> = (0..LIVE_STARTUP_INPUT_MAX_ITEMS)
            .map(|turn| utterance(format!("turn {turn}")))
            .collect();
        assert!(recent_history_fits(&many));
        // The token budget still refuses: one turn over it never fits.
        let long = vec![utterance(
            "word ".repeat(LIVE_STARTUP_INPUT_TOKEN_BUDGET * 2),
        )];
        assert!(!recent_history_fits(&long));
        assert!(recent_history_fits(&[]));
    }

    #[test]
    fn pending_context_can_carry_the_most_recent_turns_verbatim() {
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let recent = vec![
            Message::User(meerkat_core::types::UserMessage::text(
                "typed while the call was closed: the budget code is kestrel",
            )),
            Message::System(meerkat_core::types::SystemMessage::new("executor policy")),
        ];
        assert!(recent_history_fits(&recent));
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_instructions("Catalog behavior.")
            .with_pending_context_after_recent(&recent);
        let session = factory.session_config(&config);
        session
            .validate()
            .expect("pinned SDK accepts recent startup history");
        let encoded = serde_json::to_value(session).unwrap();
        let input = encoded["input"].as_array().expect("startup input");
        assert_eq!(
            input.len(),
            1,
            "system rows never reach the voice model: {encoded}"
        );
        assert_eq!(input[0]["role"], "user");
        assert!(
            input[0]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("kestrel")
        );
        assert!(
            encoded["instructions"]
                .as_str()
                .unwrap()
                .contains("A summary of the earlier history is being prepared")
        );
        // No recent turns: the original pending notice and no input.
        let empty = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_pending_context_after_recent(&[]);
        let encoded = serde_json::to_value(factory.session_config(&empty)).unwrap();
        assert!(encoded.get("input").is_none());
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
            PublicLiveContextSeed::HistoricalContextPending { .. }
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

    fn reflected_input_frame(bytes: usize) -> Value {
        use base64::Engine as _;
        json!({
            "type": "session.input_audio.append",
            "audio": base64::engine::general_purpose::STANDARD.encode(vec![0_u8; bytes]),
        })
    }

    /// A provider `info` notice is telemetry: it is logged, never lowered
    /// into an observation the runtime could act on.
    #[test]
    fn provider_info_notice_is_telemetry_only() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(json!({
                "type": "info",
                "event_id": "n1",
                "code": "rate_limited",
                "message": "slow down",
            })))
            .unwrap();
        assert!(state.queued_observations.is_empty());
    }

    #[test]
    fn reflected_pcm16_samples_counts_from_base64_length() {
        use base64::Engine as _;
        let encode =
            |bytes: usize| base64::engine::general_purpose::STANDARD.encode(vec![0_u8; bytes]);
        // 200 ms of PCM16 at 24 kHz.
        assert_eq!(reflected_pcm16_samples(&encode(9_600)), 4_800);
        // Padded tails: 4 bytes ("...==" is 1 byte past a group) and 2 bytes.
        assert_eq!(reflected_pcm16_samples(&encode(4)), 2);
        assert_eq!(reflected_pcm16_samples(&encode(2)), 1);
        // Unpadded payloads count the same bytes.
        assert_eq!(reflected_pcm16_samples(encode(2).trim_end_matches('=')), 1);
        assert_eq!(reflected_pcm16_samples(""), 0);
    }

    /// The provider input backlog is the reflected input clock at an
    /// input-transcript delta minus the span it transcribes: about a second
    /// on a healthy session, growing on a degraded one whose transcription
    /// runs behind the audio it has received.
    #[test]
    fn provider_input_latency_measures_reflected_clock_minus_transcribed_span() {
        let mut state = SessionState::default();
        assert_eq!(state.provider_input_latency, None);
        // Reflected input alone (silence) measures nothing.
        for _ in 0..10 {
            state
                .apply_frame(frame(reflected_input_frame(9_600)))
                .unwrap();
        }
        assert_eq!(state.provider_input_latency, None);

        state
            .apply_frame(frame(input_delta_span("hello ", 1_000.0, 1_200.0)))
            .unwrap();
        assert_eq!(
            state.provider_input_latency,
            Some(GptLiveProviderInputLatency {
                backlog_ms: 800,
                measured_at_reflected_clock_ms: 2_000,
            })
        );

        // Ten more seconds of received audio while the provider has only
        // transcribed through 1.4 s: the backlog grows with the clock.
        for _ in 0..50 {
            state
                .apply_frame(frame(reflected_input_frame(9_600)))
                .unwrap();
        }
        state
            .apply_frame(frame(input_delta_span("there", 1_200.0, 1_400.0)))
            .unwrap();
        assert_eq!(
            state.provider_input_latency,
            Some(GptLiveProviderInputLatency {
                backlog_ms: 10_600,
                measured_at_reflected_clock_ms: 12_000,
            })
        );

        // A span past the reflected clock never reports a negative lag.
        state
            .apply_frame(frame(input_delta_span("ahead", 12_000.0, 12_500.0)))
            .unwrap();
        assert_eq!(
            state
                .provider_input_latency
                .map(|latency| latency.backlog_ms),
            Some(0)
        );
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

    /// gpt-live-1 has no assistant completion event: one spoken reply is one
    /// open provider turn until the speaker changes or the session closes,
    /// however many transcript deltas it streams. Consumers group by the turn
    /// ref, never by delta.
    #[test]
    fn word_by_word_assistant_deltas_stay_on_one_provider_turn() {
        let words = [
            " keeper.",
            " pine",
            " opal",
            " hazel",
            ",",
            " verification",
            " code:",
            " opal",
            " gold",
            " hazel",
            " iris.",
            " The",
            " keeper",
            " confirmed",
            " the",
            " code",
            " and",
            " the",
            " route",
            " is",
            " clear",
            " now",
            ".",
        ];
        assert_eq!(words.len(), 23);
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("what is the code")))
            .unwrap();
        drain(&mut state);
        for word in words {
            state.apply_frame(frame(output_delta(word))).unwrap();
        }
        let observations = drain(&mut state);
        let GptLiveBrokerObservation::TurnFinished {
            role: GptLiveTurnRole::User,
            ..
        } = &observations[0]
        else {
            panic!("the assistant reply finishes the user turn first: {observations:?}");
        };
        let GptLiveBrokerObservation::TurnStarted {
            turn: assistant_turn,
            role: GptLiveTurnRole::Assistant,
        } = &observations[1]
        else {
            panic!("one assistant turn starts: {observations:?}");
        };
        let snapshot_turns: Vec<_> = observations
            .iter()
            .filter_map(|observation| match observation {
                GptLiveBrokerObservation::TurnSnapshotDelta { turn, delta } => {
                    Some((turn, delta.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(snapshot_turns.len(), 23);
        assert!(
            snapshot_turns
                .iter()
                .all(|(turn, _)| *turn == assistant_turn),
            "every delta belongs to the one open assistant turn"
        );
        assert_eq!(
            snapshot_turns
                .iter()
                .map(|(_, delta)| *delta)
                .collect::<String>(),
            words.concat()
        );
        assert_eq!(
            observations
                .iter()
                .filter(|observation| matches!(
                    observation,
                    GptLiveBrokerObservation::TurnStarted { .. }
                        | GptLiveBrokerObservation::TurnFinished { .. }
                ))
                .count(),
            2,
            "no delta starts or finishes a turn"
        );
        // Only the speaker change or the session end finishes the turn.
        state.apply_frame(frame(session_closed())).unwrap();
        let closed = drain(&mut state);
        assert!(matches!(
            closed.first(),
            Some(GptLiveBrokerObservation::TurnFinished {
                turn,
                role: GptLiveTurnRole::Assistant,
                transcript,
            }) if turn == assistant_turn && *transcript == words.concat()
        ));
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
            GptLiveBrokerObservation::ClientDelegationFinal { delegation, target: GptLiveDelegationTarget::Client, turn, transcript, request_transcript, assistant_context, represented_turns }
                if delegation.__opaque_provider_id() == "dlg_1" && turn == user_turn && transcript == "book a table"
                    && request_transcript == "book a table" && assistant_context.is_empty()
                    && represented_turns.is_empty()
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
        let started = drain(&mut state);
        let GptLiveBrokerObservation::TurnStarted {
            turn: user_turn, ..
        } = &started[0]
        else {
            panic!("user turn start");
        };
        let user_turn = user_turn.clone();
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
        // The finished user turn is already canonical: the detached turn
        // re-presents it rather than carrying its words as a new row.
        assert!(matches!(
            &observations[1],
            GptLiveBrokerObservation::ClientDelegationFinal { turn, transcript, represented_turns, .. }
                if turn == minted && transcript == "what time is it"
                    && represented_turns.len() == 1
                    && represented_turns[0].turn == user_turn
                    && represented_turns[0].transcript == "what time is it"
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

    /// BuildBuddy 2e218ad5 (S106): the model spoke after "...once it", the
    /// provider's late tail "'s saved" opened its own user turn, the model
    /// spoke again, then `session.delegation.created` arrived with no user
    /// turn open. The delegation re-presents both finished user turns, in
    /// order, with their joined text; nothing new is left to commit.
    #[test]
    fn detached_delegation_re_presents_every_user_turn_finished_in_its_window() {
        let mut state = SessionState::default();
        let mut user_turns = Vec::new();
        for (input, output) in [
            ("just tell me the word count once it", "Okay,"),
            ("'s saved", " on it."),
        ] {
            state.apply_frame(frame(input_delta(input))).unwrap();
            state.apply_frame(frame(output_delta(output))).unwrap();
            for observation in drain(&mut state) {
                if let GptLiveBrokerObservation::TurnStarted {
                    turn,
                    role: GptLiveTurnRole::User,
                } = observation
                {
                    user_turns.push(turn);
                }
            }
        }
        assert_eq!(
            user_turns.len(),
            2,
            "the late tail opened its own user turn"
        );
        state
            .apply_frame(frame(delegation_created("dlg_split", "client")))
            .unwrap();
        let observations = drain(&mut state);
        let Some(GptLiveBrokerObservation::ClientDelegationFinal {
            transcript,
            request_transcript,
            represented_turns,
            ..
        }) = observations.last()
        else {
            panic!("delegation final: {observations:?}");
        };
        let represented: Vec<_> = represented_turns
            .iter()
            .map(|row| (row.turn.clone(), row.transcript.clone()))
            .collect();
        assert_eq!(
            represented,
            vec![
                (
                    user_turns[0].clone(),
                    "just tell me the word count once it".to_owned()
                ),
                (user_turns[1].clone(), "'s saved".to_owned()),
            ]
        );
        assert_eq!(transcript, "just tell me the word count once it 's saved");
        assert_eq!(
            request_transcript,
            "just tell me the word count once it 's saved"
        );

        // A second delegation with no new user speech re-presents the same
        // committed rows, not the joined text as a new row.
        state
            .apply_frame(frame(delegation_created("dlg_again", "client")))
            .unwrap();
        let again = drain(&mut state);
        assert!(matches!(
            again.last(),
            Some(GptLiveBrokerObservation::ClientDelegationFinal { represented_turns, .. })
                if represented_turns.len() == 2 && represented_turns[1].turn == user_turns[1]
        ));
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
        // The assistant reply finishes that turn as a user turn: the tail is
        // a committed row, not a lost fragment. It continues the utterance
        // the delegation was created in, so it is marked as continuing
        // "dlg_early" first.
        state
            .apply_frame(frame(output_delta_span("Sure,", 10600.0, 10800.0)))
            .unwrap();
        let finished = drain(&mut state);
        assert!(matches!(
            &finished[0],
            GptLiveBrokerObservation::UserTurnContinuesDelegation { delegation, transcript, .. }
                if delegation.0 == "dlg_early" && transcript == " named it"
        ));
        assert!(matches!(
            &finished[1],
            GptLiveBrokerObservation::TurnFinished { role: GptLiveTurnRole::User, transcript, .. }
                if transcript == " named it"
        ));
        // A following detached delegation re-presents that committed turn
        // (the only user turn finished in its window) instead of carrying
        // its words as a new row.
        state
            .apply_frame(frame(delegation_created_at("dlg_again", "client", 11000.0)))
            .unwrap();
        let GptLiveBrokerObservation::TurnStarted {
            turn: tail_turn, ..
        } = &late[0]
        else {
            panic!("tail turn start");
        };
        assert!(matches!(
            drain(&mut state).as_slice(),
            [
                GptLiveBrokerObservation::TurnStarted {
                    role: GptLiveTurnRole::User,
                    ..
                },
                GptLiveBrokerObservation::ClientDelegationFinal { transcript, represented_turns, .. },
            ] if transcript == "named it"
                && represented_turns.len() == 1
                && &represented_turns[0].turn == tail_turn
                && represented_turns[0].transcript == " named it"
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

    fn instructions_ack(client_event_id: &str) -> Value {
        let mut value = ack(Some(client_event_id));
        value["type"] = json!("session.instructions.appended");
        value
    }

    /// A commentary acknowledgement landing at `start_ms` on the session
    /// timeline.
    fn ack_at(client_event_id: &str, start_ms: f64) -> Value {
        let mut value = ack(Some(client_event_id));
        value["start_ms"] = json!(start_ms);
        value["end_ms"] = json!(start_ms);
        value
    }

    /// A delegation whose model said "one moment", ending at 2000 ms.
    fn state_with_spoken_delegation() -> SessionState {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("book a table")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_cue", "client")))
            .unwrap();
        state
            .apply_frame(frame(output_delta_span("one moment", 1500.0, 2000.0)))
            .unwrap();
        drain(&mut state);
        state
    }

    #[test]
    fn a_result_landing_well_after_the_last_word_gets_one_broker_owned_speak_cue() {
        let mut state = state_with_spoken_delegation();
        let narration = state.reserve_delegation_commentary(false).unwrap();
        let result = state.reserve_delegation_commentary(true).unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(narration), 5000.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, 0, "narration is never cued");
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 5000.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, 1);
        drain(&mut state);
        let cue = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("one cue due");
        assert_eq!(state.reserve_due_result_cue().unwrap(), None, "exactly one");
        state
            .apply_frame(frame(instructions_ack(&instructions_event_id(cue, 0))))
            .unwrap();
        assert!(
            drain(&mut state).is_empty(),
            "the cue's acknowledgement is consumed by the broker, never surfaced"
        );
        assert_eq!(state.outstanding_receipt_count(), 0);
    }

    #[test]
    fn speech_in_progress_or_just_paused_suppresses_the_cue() {
        // Measured normal deliveries reach +400 ms after the last word: a
        // result landing inside the margin is never cued, at the boundary it
        // is.
        for (ack_ms, cued) in [
            (1800.0, 0),
            (2000.0, 0),
            (2400.0, 0),
            (2999.0, 0),
            (3000.0, 1),
        ] {
            let mut state = state_with_spoken_delegation();
            let result = state.reserve_delegation_commentary(true).unwrap();
            state
                .apply_frame(frame(ack_at(&pending_event_id(result), ack_ms)))
                .unwrap();
            assert_eq!(state.due_result_cues, cued, "ack at {ack_ms} ms");
        }
        // The user speaking after the model's last word: never cued.
        let mut state = state_with_spoken_delegation();
        let result = state.reserve_delegation_commentary(true).unwrap();
        state.apply_frame(frame(input_delta("and also"))).unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 9000.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, 0);
        // The model speaking again clears the input flag and moves the gap.
        state
            .apply_frame(frame(output_delta_span(" sure", 9000.0, 9200.0)))
            .unwrap();
        let result = state.reserve_delegation_commentary(true).unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 9600.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, 0);
    }

    #[test]
    fn a_model_that_never_spoke_on_the_channel_is_idle() {
        let mut state = SessionState::default();
        let result = state.reserve_delegation_commentary(true).unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 100.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, 1);
    }

    #[test]
    fn a_rejected_result_or_cue_surfaces_nothing_extra() {
        let mut state = state_with_spoken_delegation();
        let result = state.reserve_delegation_commentary(true).unwrap();
        state
            .apply_frame(frame(append_rejected(Some(&pending_event_id(result)))))
            .unwrap();
        assert_eq!(state.due_result_cues, 0);
        assert!(state.result_cue_candidates.is_empty());
        drain(&mut state);

        state.due_result_cues = 1;
        let cue = state.reserve_due_result_cue().unwrap().unwrap();
        state
            .apply_frame(frame(append_rejected(Some(&instructions_event_id(cue, 0)))))
            .unwrap();
        assert!(
            !drain(&mut state).iter().any(|observation| matches!(
                observation,
                GptLiveBrokerObservation::InstructionsContextAppendRejected { .. }
            )),
            "a rejected cue is not a rejected owner append"
        );
        assert_eq!(state.outstanding_receipt_count(), 0);
    }

    fn continuation_markers(observations: &[GptLiveBrokerObservation]) -> Vec<(String, String)> {
        observations
            .iter()
            .filter_map(|observation| match observation {
                GptLiveBrokerObservation::UserTurnContinuesDelegation {
                    turn, delegation, ..
                } => Some((turn.0.clone(), delegation.0.clone())),
                _ => None,
            })
            .collect()
    }

    /// A delegation created at a pause mid-sentence: the rest of the
    /// sentence is the next user turn to finish, marked as continuing that
    /// delegation immediately before its `TurnFinished`; only that one turn.
    #[test]
    fn speech_after_a_mid_utterance_delegation_continues_it_once() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("write a note of two hundred words")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_split", "client")))
            .unwrap();
        drain(&mut state);
        state
            .apply_frame(frame(input_delta(" into notes dot md")))
            .unwrap();
        state.apply_frame(frame(output_delta("On it."))).unwrap();
        let observations = drain(&mut state);
        let markers = continuation_markers(&observations);
        assert_eq!(markers.len(), 1, "{observations:?}");
        assert_eq!(markers[0].1, "dlg_split");
        let marker_at = observations
            .iter()
            .position(|o| {
                matches!(
                    o,
                    GptLiveBrokerObservation::UserTurnContinuesDelegation { .. }
                )
            })
            .unwrap();
        assert!(matches!(
            &observations[marker_at + 1],
            GptLiveBrokerObservation::TurnFinished { turn, role: GptLiveTurnRole::User, transcript }
                if turn.0 == markers[0].0 && transcript == " into notes dot md"
        ));
        // A later user turn is ordinary speech.
        state.apply_frame(frame(input_delta("and thanks"))).unwrap();
        state.apply_frame(frame(output_delta(" Sure."))).unwrap();
        assert!(continuation_markers(&drain(&mut state)).is_empty());
    }

    #[test]
    fn a_detached_delegation_or_a_newer_delegation_starts_no_stale_continuation() {
        // Detached: the assistant spoke first, no user turn was open.
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("book a table")))
            .unwrap();
        state.apply_frame(frame(output_delta("Sure."))).unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_detached", "client")))
            .unwrap();
        state.apply_frame(frame(input_delta("for two"))).unwrap();
        state.apply_frame(frame(output_delta(" Booking."))).unwrap();
        assert!(continuation_markers(&drain(&mut state)).is_empty());

        // A second mid-utterance delegation replaces the first one's
        // continuation before any user turn finished.
        let mut state = SessionState::default();
        state.apply_frame(frame(input_delta("first part"))).unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_one", "client")))
            .unwrap();
        state
            .apply_frame(frame(input_delta(" second part")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_two", "client")))
            .unwrap();
        state
            .apply_frame(frame(input_delta(" third part")))
            .unwrap();
        state.apply_frame(frame(output_delta("On it."))).unwrap();
        let markers = continuation_markers(&drain(&mut state));
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].1, "dlg_two");
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
        // Each input-transcript delta is followed by its latency telemetry.
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::ProviderInputLatency(
                GptLiveProviderInputLatencyStatus {
                    latest: Some(_),
                    ..
                }
            ))
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

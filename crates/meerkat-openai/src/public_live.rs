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

use std::collections::{HashMap, HashSet, VecDeque};

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
        /// A `session.commentary.append` sent on the delegation lane (a
        /// narration or result, `delegation` set) or the session lane (a
        /// voiced canonical row). `text` is the first
        /// [`Capture::MAX_TEXT_BYTES`] of the content on a char boundary;
        /// `text_bytes` is the whole content's length.
        CommentaryAppendAttempt {
            client_event_id: String,
            delegation: bool,
            text: String,
            text_bytes: usize,
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
                }
                | EventKind::CommentaryAppendAttempt {
                    client_event_id,
                    text,
                    ..
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

/// Scoped recording of the raw provider stream at the adapter boundary, for
/// replay fixtures (Turbo S recurrence fix 2): the create request and
/// response, every outbound [`ClientEvent`] and every inbound
/// [`ServerFrame`]'s lossless `raw` JSON, in the order this adapter saw them.
/// Opt-in per task like [`thinking_capture`]; test-fixture builds only. The
/// file is an evidence artifact, never committed as written: the fixture
/// scrubber replaces SDP, ids, tokens and audio before a stream is committed.
#[cfg(feature = "test-realtime-fixtures")]
#[doc(hidden)]
pub mod provider_recording {
    use std::io::Write;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use serde_json::Value;

    /// One recorded crossing of the adapter boundary.
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "dir", rename_all = "snake_case")]
    pub enum Entry {
        /// The `POST /v1/live/sessions` body (session config plus offer SDP).
        CreateRequest { body: Value },
        /// Its response (session identity plus answer SDP).
        CreateResponse { body: Value },
        /// One client event sent on the sideband.
        ClientEvent { event: Value },
        /// One server frame received on the sideband, as the provider sent it.
        ServerFrame { raw: Value },
        /// The sideband receiver ended (`error` set when it failed).
        ReceiverEnd { error: Option<String> },
        /// A test-driven step outside the adapter (a browser utterance
        /// scheduled, a peer disconnect). Everything recorded after it may
        /// depend on it, so a replay holds later server frames until the
        /// replaying test reaches the same step.
        Marker { step: String },
    }

    /// One JSONL line of a recording.
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct Line {
        pub seq: u64,
        pub channel_ordinal: u32,
        pub elapsed_ms: u64,
        pub entry: Entry,
    }

    struct Inner {
        started: Instant,
        seq: AtomicU64,
        out: Mutex<Option<std::fs::File>>,
        failed: Mutex<Option<String>>,
    }

    /// A task-scoped recorder writing one JSONL file.
    #[derive(Clone)]
    pub struct Recorder {
        inner: Arc<Inner>,
        channel_ordinal: u32,
    }

    tokio::task_local! {
        static CURRENT: Recorder;
    }

    /// The recorder a client constructed outside any recorder scope captures,
    /// while a [`FallbackGuard`] lives. A host that opens channels on its own
    /// spawned tasks (the JSON-RPC server dispatches each request on one)
    /// never sees the test's task-local scope. Process-wide: one guard at a
    /// time, held only around the open whose client should record.
    static FALLBACK: Mutex<Option<Recorder>> = Mutex::new(None);

    /// Clears the fallback recorder when dropped.
    #[must_use = "the fallback recorder is cleared when the guard drops"]
    pub struct FallbackGuard(());

    impl Drop for FallbackGuard {
        fn drop(&mut self) {
            if let Ok(mut fallback) = FALLBACK.lock() {
                *fallback = None;
            }
        }
    }

    impl Recorder {
        /// Create the recording file (it must not exist; owner-only mode).
        pub fn create(path: &Path) -> std::io::Result<Self> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(path)?;
            Ok(Self {
                inner: Arc::new(Inner {
                    started: Instant::now(),
                    seq: AtomicU64::new(0),
                    out: Mutex::new(Some(file)),
                    failed: Mutex::new(None),
                }),
                channel_ordinal: 0,
            })
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
            CURRENT
                .try_with(Clone::clone)
                .ok()
                .or_else(|| FALLBACK.lock().ok().and_then(|fallback| fallback.clone()))
        }

        /// Record clients constructed outside any recorder scope into this
        /// recorder until the guard drops (see [`FALLBACK`]).
        pub fn install_fallback(&self) -> FallbackGuard {
            if let Ok(mut fallback) = FALLBACK.lock() {
                *fallback = Some(self.clone());
            }
            FallbackGuard(())
        }

        /// The first write failure, if any: a recording that lost a line is
        /// not a fixture.
        pub fn failure(&self) -> Option<String> {
            self.inner
                .failed
                .lock()
                .ok()
                .and_then(|failed| failed.clone())
        }

        pub(super) fn record(&self, entry: Entry) {
            let line = Line {
                seq: self.inner.seq.fetch_add(1, Ordering::AcqRel),
                channel_ordinal: self.channel_ordinal,
                elapsed_ms: u64::try_from(self.inner.started.elapsed().as_millis())
                    .unwrap_or(u64::MAX),
                entry,
            };
            let result = serde_json::to_string(&line)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    let mut out = self
                        .inner
                        .out
                        .lock()
                        .map_err(|_| "recording file lock poisoned".to_string())?;
                    let file = out.as_mut().ok_or("recording file closed")?;
                    writeln!(file, "{text}").map_err(|error| error.to_string())
                });
            if let Err(error) = result
                && let Ok(mut failed) = self.inner.failed.lock()
                && failed.is_none()
            {
                *failed = Some(error);
            }
        }

        /// Record a test-driven step (see [`Entry::Marker`]).
        pub fn mark(&self, step: impl Into<String>) {
            self.record(Entry::Marker { step: step.into() });
        }

        pub(super) fn record_value<T: serde::Serialize>(
            &self,
            entry: impl FnOnce(Value) -> Entry,
            value: &T,
        ) {
            match serde_json::to_value(value) {
                Ok(value) => self.record(entry(value)),
                Err(error) => {
                    if let Ok(mut failed) = self.inner.failed.lock()
                        && failed.is_none()
                    {
                        *failed = Some(error.to_string());
                    }
                }
            }
        }
    }

    /// Read a recording back, in recorded order.
    pub fn read(path: &Path) -> std::io::Result<Vec<Line>> {
        let text = std::fs::read_to_string(path)?;
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
            })
            .collect()
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn recorder_is_opt_in_task_scoped_and_writes_ordered_lines() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("stream.jsonl");
            let recorder = Recorder::create(&path).unwrap().for_channel(3);
            assert!(Recorder::current().is_none());
            let scoped = recorder
                .scope(async {
                    assert!(
                        tokio::spawn(async { Recorder::current().is_none() })
                            .await
                            .unwrap()
                    );
                    Recorder::current().unwrap()
                })
                .await;
            scoped.record(Entry::ClientEvent {
                event: serde_json::json!({"type": "close"}),
            });
            scoped.record(Entry::ServerFrame {
                raw: serde_json::json!({"type": "session.closed"}),
            });
            recorder.mark("disconnect:graceful");
            let lines = read(&path).unwrap();
            assert_eq!(lines.len(), 3);
            assert_eq!(lines[0].seq, 0);
            assert_eq!(lines[1].seq, 1);
            assert_eq!(
                lines[2].entry,
                Entry::Marker {
                    step: "disconnect:graceful".into()
                }
            );
            assert!(lines.iter().all(|line| line.channel_ordinal == 3));
            assert!(recorder.failure().is_none());
            assert!(
                Recorder::create(&path).is_err(),
                "never overwrites a recording"
            );

            // A client constructed on a spawned task (an RPC host's request
            // handler) records into the fallback while its guard lives, and
            // nowhere once it drops. One test owns the process-wide fallback,
            // so this stays sequential with the scope checks above.
            let guard = recorder.install_fallback();
            let captured = tokio::spawn(async { Recorder::current().map(|r| r.channel_ordinal) })
                .await
                .unwrap();
            assert_eq!(captured, Some(3));
            drop(guard);
            assert!(
                tokio::spawn(async { Recorder::current().is_none() })
                    .await
                    .unwrap()
            );
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

/// Startup notice of a summary-pending seed with no verbatim turns: the
/// history is being prepared.
const LIVE_PENDING_CONTEXT_NOTICE: &str = "Voice-channel context availability (factual state, not a new user request):\nHistorical session context is being prepared and is not yet available.";
/// Startup notice of a summary-pending seed that carries the newest turns
/// verbatim ([`PublicLiveOpenConfig::with_pending_context_after_recent`]): it
/// claims those turns as known and says nothing about the pending summary.
/// With the answer in the seeded turns, a pending-summary sentence was the
/// one claim that mapped a "text chat" question onto something unavailable
/// (Turbo S S99 control: "I don't know" or a delegation as the first
/// exchange); the late summary arrives with its own framing.
const LIVE_PENDING_CONTEXT_AFTER_RECENT_NOTICE: &str = "Voice-channel context availability (factual state, not a new user request):\nThe most recent turns of the earlier text conversation are in the session input. They are part of that text conversation and you know them: answer questions about them yourself, directly.";

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
///
/// The third value counts the items dropped by a provider limit (the item
/// count or the token budget) rather than by the deliberate verbatim cap and
/// user-row start, so only a genuine provider-limit truncation is reported
/// as a warning.
fn budget_startup_input(
    keep: InitialItem,
    recent: &[InitialItem],
) -> (Vec<InitialItem>, LiveStartupInputTruncation, usize) {
    let mut truncation = LiveStartupInputTruncation::default();
    let mut provider_limited = 0;
    let mut items = Vec::with_capacity(recent.len() + 1);
    let mut tokens = estimated_startup_tokens(&keep);
    // Newest first so the oldest are the ones left out. The summary covers
    // every recent item, so the verbatim bound only trims repetition. The
    // walk stops at the first item that does not fit: the verbatim tail is
    // always contiguous, never newer turns with a gap where a large one was.
    let mut kept_recent = Vec::new();
    let mut stopped = false;
    for item in recent.iter().rev() {
        let item_tokens = estimated_startup_tokens(item);
        let under_cap = kept_recent.len() < LIVE_STARTUP_VERBATIM_ITEMS_MAX;
        if !stopped
            && under_cap
            && kept_recent.len() + 1 < LIVE_STARTUP_INPUT_MAX_ITEMS
            && tokens + item_tokens <= LIVE_STARTUP_INPUT_TOKEN_BUDGET
        {
            tokens += item_tokens;
            kept_recent.push(item.clone());
        } else {
            stopped = true;
            if under_cap {
                provider_limited += 1;
            }
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
    (items, truncation, provider_limited)
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
                let (items, truncation, provider_limited) = budget_startup_input(developer, recent);
                if provider_limited > 0 {
                    tracing::warn!(
                        provider_limited_items = provider_limited,
                        dropped_items = truncation.dropped_items,
                        dropped_bytes = truncation.dropped_bytes,
                        max_items = LIVE_STARTUP_INPUT_MAX_ITEMS,
                        token_budget = LIVE_STARTUP_INPUT_TOKEN_BUDGET,
                        "public Live startup input dropped recent turns to fit the provider limits; the summary covers them"
                    );
                } else if truncation != LiveStartupInputTruncation::default() {
                    // The deliberate verbatim cap (and its user-row start), not
                    // an incident: the fresh summary covers every dropped row.
                    tracing::debug!(
                        dropped_items = truncation.dropped_items,
                        dropped_bytes = truncation.dropped_bytes,
                        verbatim_items_max = LIVE_STARTUP_VERBATIM_ITEMS_MAX,
                        "public Live startup seed applied the verbatim item cap; the fresh summary covers the dropped rows"
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
            Self::HistoricalContextPending { recent } if recent.is_empty() => {
                Some(LIVE_PENDING_CONTEXT_NOTICE.to_string())
            }
            // The recent turns are startup input the model has: say so, so a
            // pending summary is not read as "no history at all". Saying only
            // that history "is not yet available" made gpt-live-1 answer
            // "I don't know that yet" about a fact in those very turns (S99
            // positive control, BuildBuddy 045430ec). The turns are named as
            // part of the earlier text conversation, so a question about "our
            // text chat" is not filed under the pending summary (1139a4ff).
            Self::HistoricalContextPending { .. } => {
                Some(LIVE_PENDING_CONTEXT_AFTER_RECENT_NOTICE.to_string())
            }
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
    #[cfg(feature = "test-realtime-fixtures")]
    provider_recording: Option<provider_recording::Recorder>,
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
            #[cfg(feature = "test-realtime-fixtures")]
            provider_recording: provider_recording::Recorder::current(),
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
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(recorder) = &self.provider_recording {
            recorder.record_value(
                |body| provider_recording::Entry::CreateRequest { body },
                &request,
            );
        }
        let created = self
            .client
            .create_webrtc(&request)
            .await
            .map_err(map_live_error)?;
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(recorder) = &self.provider_recording {
            recorder.record_value(
                |body| provider_recording::Entry::CreateResponse { body },
                &created,
            );
        }
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
                #[cfg(feature = "test-realtime-fixtures")]
                provider_recording: self.provider_recording.clone(),
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
    #[cfg(feature = "test-realtime-fixtures")]
    provider_recording: Option<provider_recording::Recorder>,
}

impl std::fmt::Debug for PublicLiveBrokerSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PublicLiveBrokerSession(<connected>)")
    }
}

impl PublicLiveBrokerSession {
    #[cfg(feature = "test-realtime-fixtures")]
    fn record_client_event(&self, event: &ClientEvent) {
        if let Some(recorder) = &self.provider_recording {
            recorder.record_value(
                |event| provider_recording::Entry::ClientEvent { event },
                event,
            );
        }
    }

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
        self.append_delegation_commentary(delegation, text, None)
            .await
    }

    /// Append the narration that ends a delegation without a result (its
    /// work failed or could not start), exactly like
    /// [`Self::append_delegation_context`], and stop naming it as still
    /// running in later in-progress notices and result cues.
    pub async fn append_terminal_delegation_narration(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        self.state
            .lock()
            .await
            .end_outstanding_delegation(&delegation.0);
        self.append_delegation_commentary(delegation, text, None)
            .await
    }

    /// Append an executor result to an observed client delegation, as
    /// commentary exactly like [`Self::append_delegation_context`].
    ///
    /// Measured against gpt-live-1, the model occasionally acknowledges a
    /// result commentary and never voices it, and nothing the provider sends
    /// tells the two apart: there is no assistant completion event, and
    /// output audio streams continuously, silence included. So at the
    /// result's acknowledgement (the Delivered transition) the broker always
    /// follows the result with one short thinking-lane cue, bound to the
    /// result's delegation and phrased so it is safe either way: tell the
    /// user the result unless it was already told. The cue is not an
    /// instructions append: instructions persist, and a persisted cue's
    /// delegation framing carried into the next question (S99). The cue is broker-owned: its
    /// receipt is consumed here and never surfaced.
    ///
    /// Delegation commentary (this result, or a narration through
    /// [`Self::append_delegation_context`]) appended while the user holds
    /// the floor with an unanswered utterance (input whose speech began
    /// after the model's last output and the last delegation) would divert
    /// the model from that request, so it is held: reserved and owned, not
    /// yet sent. Provider ordering releases it when the utterance is
    /// answered, by the model's next output transcript or by the utterance's
    /// `session.delegation.created`. A close or teardown first leaves it
    /// unsent, and the owner's close settlement resolves it as interrupted by
    /// close: a result then merges into the source member, while a narration
    /// (ephemeral progress speech) is dropped.
    pub async fn append_delegation_result(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        self.append_delegation_commentary(delegation, text, Some(delegation))
            .await
    }

    /// [`Self::append_delegation_result`] for a result whose delegated work
    /// asked other members (`peers`, display labels) that have not answered:
    /// outbound peer requests its session sent with no terminal response
    /// committed. Such a result reports only that they were asked (Turbo S
    /// S102 r2: "I asked Analyst Pemberton..."), and the voice model invented
    /// Pemberton's answer the moment it landed, before any cue.
    ///
    /// So a broker-owned instructions notice bound to the delegation goes
    /// ahead of the result in the same outbox order: their answer has not
    /// arrived, tell the user only that you asked. Both are reserved and
    /// placed under one state lock, so they are sent in that order or held
    /// behind the user's floor in that order. The result's cue then reads
    /// the awaiting-peer form of the result cue ([`result_cue_text`]) instead of the outcome form. The
    /// member's answer reaches the channel later as its own row, through the
    /// session lane like any committed reply.
    pub async fn append_delegation_result_awaiting_peer_replies(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
        peers: Vec<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        if peers.is_empty() {
            return self.append_delegation_result(delegation, text).await;
        }
        let text = require_context(text)?;
        let (result_token, sends) = {
            let mut state = self.state.lock().await;
            let result_token = state.reserve_delegation_commentary(Some(delegation.0.clone()))?;
            state.awaiting_peer_results.insert(delegation.0.clone());
            let notice = match state.reserve_peer_reply_pending_notice() {
                Ok(token) => Some((
                    token,
                    ClientEvent {
                        event_id: Field::Value(instructions_event_id(token, 0)),
                        command: Command::InstructionsAppend {
                            content: peer_reply_pending_notice(&peers),
                            delegation_id: Nullable(Some(delegation.0.clone())),
                        },
                    },
                )),
                Err(error) => {
                    // The result still goes out; only the notice ahead of
                    // it is lost (the awaiting cue still follows it).
                    tracing::warn!(
                        ?error,
                        "public Live peer reply pending notice could not be reserved"
                    );
                    None
                }
            };
            let result =
                Self::commentary_event(result_token, text, Nullable(Some(delegation.0.clone())));
            let mut sends = Vec::with_capacity(2);
            for (token, event) in notice.into_iter().chain([(result_token, result)]) {
                if let Some(event) = state.hold_or_send_commentary(token, event) {
                    sends.push((token, event));
                }
            }
            (result_token, sends)
        };
        for (token, event) in sends {
            if token != result_token {
                tracing::info!("public Live peer reply pending notice sent ahead of its result");
            }
            self.deliver_append(token, event).await.map_err(|_| {
                GptLiveBrokerError::AppendDeliveryAmbiguous {
                    token: result_token,
                }
            })?;
        }
        Ok(result_token)
    }

    async fn append_delegation_commentary(
        &self,
        delegation: &GptLiveDelegationRef,
        text: impl Into<String>,
        result_of: Option<&GptLiveDelegationRef>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let text = require_context(text)?;
        let (token, event) = {
            let mut state = self.state.lock().await;
            let token = state
                .reserve_delegation_commentary(result_of.map(|delegation| delegation.0.clone()))?;
            let event = Self::commentary_event(token, text, Nullable(Some(delegation.0.clone())));
            match state.hold_or_send_commentary(token, event) {
                Some(event) => (token, event),
                None => return Ok(token),
            }
        };
        self.deliver_append(token, event).await
    }

    /// Send the in-progress notices of created client delegations, each
    /// bound to its delegation ([`LIVE_DELEGATION_IN_PROGRESS`]).
    async fn send_due_progress_notices(&self) -> Result<(), GptLiveBrokerError> {
        loop {
            let Some((token, delegation_id, fragments)) =
                self.state.lock().await.reserve_due_progress_notice()?
            else {
                return Ok(());
            };
            let names_outstanding = fragments.len() > 1;
            for (index, content) in fragments.into_iter().enumerate() {
                let event = ClientEvent {
                    event_id: Field::Value(instructions_event_id(token, index)),
                    command: Command::InstructionsAppend {
                        content,
                        delegation_id: Nullable(Some(delegation_id.clone())),
                    },
                };
                self.deliver_append(token, event).await?;
            }
            tracing::info!(
                names_outstanding,
                "public Live delegation in-progress notice sent"
            );
        }
    }

    /// Send the cues of acknowledged results, each bound to its delegation.
    async fn send_due_result_cues(&self) -> Result<(), GptLiveBrokerError> {
        loop {
            let Some((token, delegation_id, wording)) =
                self.state.lock().await.reserve_due_result_cue()?
            else {
                return Ok(());
            };
            for (index, content) in result_cue_fragments(&wording).into_iter().enumerate() {
                let event = ClientEvent {
                    event_id: Field::Value(thinking_event_id(token, index)),
                    command: Command::ThinkingAppend {
                        content,
                        delegation_id: Nullable(Some(delegation_id.clone())),
                    },
                };
                self.deliver_append(token, event).await?;
            }
            tracing::info!(
                awaiting_peer_replies = wording.awaiting_peer_replies,
                user_request_open = wording.user_request_open,
                names_outstanding = wording.outstanding.is_some(),
                user_spoke_first = wording.user_spoke_first,
                "public Live result cue sent"
            );
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
        #[cfg(feature = "test-realtime-fixtures")]
        if let Some(capture) = &self.thinking_capture
            && let ClientEvent {
                event_id: Field::Value(client_event_id),
                command:
                    Command::CommentaryAppend {
                        content,
                        delegation_id,
                    },
            } = &event
        {
            let mut end = content.len().min(thinking_capture::Capture::MAX_TEXT_BYTES);
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            capture.record(thinking_capture::EventKind::CommentaryAppendAttempt {
                client_event_id: client_event_id.clone(),
                delegation: delegation_id.0.is_some(),
                text: content[..end].to_owned(),
                text_bytes: content.len(),
            });
        }
        #[cfg(feature = "test-realtime-fixtures")]
        self.record_client_event(&event);
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
            #[cfg(feature = "test-realtime-fixtures")]
            if let Some(recorder) = &self.provider_recording {
                recorder.record(match &next {
                    Ok(Some(frame)) => provider_recording::Entry::ServerFrame {
                        raw: frame.raw.clone(),
                    },
                    Ok(None) => provider_recording::Entry::ReceiverEnd { error: None },
                    Err(error) => provider_recording::Entry::ReceiverEnd {
                        error: Some(error.to_string()),
                    },
                });
            }
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
            let cues_due = !state.due_result_cues.is_empty() && !state.close_requested;
            let notices_due = !state.due_progress_notices.is_empty() && !state.close_requested;
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
            // Commentary held behind an unanswered utterance goes out, in order,
            // as soon as provider ordering answers it. The state lock stays
            // held while they are sent, so a result appended concurrently
            // cannot overtake them.
            for (token, event) in state.take_releasable_held_commentary() {
                // Recorded like every other client event (`deliver_append`):
                // a held result released here is a real provider append,
                // and replay and the result-timing oracles key on it.
                #[cfg(feature = "test-realtime-fixtures")]
                self.record_client_event(&event);
                if self.sender.send(event).await.is_err() {
                    state.append_delivery_ambiguous = true;
                    tracing::warn!(
                        token = token.0,
                        "released public Live delegation commentary could not be sent; the transport is gone and its close settles the delivery"
                    );
                } else {
                    tracing::info!(
                        token = token.0,
                        "public Live held delegation commentary released"
                    );
                }
            }
            if cues_due || notices_due {
                drop(state);
                if notices_due {
                    self.send_due_progress_notices().await?;
                }
                if cues_due {
                    self.send_due_result_cues().await?;
                }
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
        let mute = ClientEvent::new(Command::InputAudioMute);
        #[cfg(feature = "test-realtime-fixtures")]
        self.record_client_event(&mute);
        self.sender.send(mute).await.map_err(map_live_error)?;
        let close = ClientEvent::new(Command::Close);
        #[cfg(feature = "test-realtime-fixtures")]
        self.record_client_event(&close);
        self.sender.send(close).await.map_err(map_live_error)?;
        state.close_requested = true;
        state.drop_held_commentary_for_close();
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
    /// A broker-owned append (a result cue or an in-progress notice): its
    /// receipts drain here and nothing about it is surfaced, so the
    /// sideband's append correlation never sees it.
    internal: Option<InternalAppend>,
}

/// Which broker-owned instructions append a pending append is, so its
/// receipts are logged under their own name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InternalAppend {
    /// The cue that follows an acknowledged result ([`LIVE_RESULT_CUE`]).
    ResultCue,
    /// The notice sent at a client delegation's creation
    /// ([`LIVE_DELEGATION_IN_PROGRESS`]).
    ProgressNotice,
    /// The notice sent ahead of a result whose work asked members that have
    /// not answered ([`peer_reply_pending_notice`]).
    PeerReplyPendingNotice,
}

impl InternalAppend {
    const fn label(self) -> &'static str {
        match self {
            Self::ResultCue => "result cue",
            Self::ProgressNotice => "in-progress notice",
            Self::PeerReplyPendingNotice => "peer reply pending notice",
        }
    }
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
    /// Session-timeline end of the commentary acknowledgement being applied
    /// (`session.commentary.appended.end_ms`): the point from which the
    /// model's generation has the appended content in context.
    commentary_ack_end_ms: Option<f64>,
    /// Result appends awaiting their acknowledgement, with the delegation
    /// each one answers.
    result_cue_candidates: HashMap<GptLiveAppendToken, String>,
    /// Delegations whose result was acknowledged and whose cue is not yet
    /// sent, in acknowledgement order.
    due_result_cues: VecDeque<String>,
    /// Results acknowledged while the model's response was in progress, in
    /// acknowledgement order. Their cue waits for the response to end
    /// ([`OUTPUT_SILENCE_RELEASE_MS`] of output silence) instead of landing
    /// inside it, where the in-progress response absorbs it.
    deferred_result_cues: VecDeque<String>,
    /// Audio-clock length of model output silence since its last speech
    /// frame (`session.output_audio.delta` frames below
    /// [`USER_FLOOR_SPEECH_DBFS`]). Starts at the release, so a model that
    /// has not spoken is silent.
    output_silence_run_ms: u64,
    /// Session-timeline end of each acknowledged result's insertion
    /// (`session.commentary.appended.end_ms`) whose cue is not yet sent.
    /// Output starting at or after it was generated with the result in
    /// context; output starting before it is the tail of speech already
    /// under way when the result landed (S97 r3: " ready." over the
    /// insertion's own span).
    result_inserted_through_ms: HashMap<String, f64>,
    /// Client delegations created whose in-progress notice is not yet sent,
    /// in creation order ([`LIVE_DELEGATION_IN_PROGRESS`]).
    due_progress_notices: VecDeque<String>,
    /// Delegations whose result reported members asked and not yet
    /// answering; their cue takes the awaiting-peer form ([`result_cue_text`]).
    awaiting_peer_results: HashSet<String>,
    /// The user's latest utterance is unanswered: input whose speech began
    /// after the model's last output began and after the last delegation,
    /// with no output or delegation since. Read from session-timeline
    /// positions, so a transcription tail delivered late is not mistaken for
    /// a new utterance.
    unanswered_user_input: bool,
    /// The user's latest request is open: set with [`Self::unanswered_user_input`]
    /// when an utterance takes the floor, cleared only by the model's output
    /// or a delegation. Unlike the floor, reflected-input silence does not
    /// clear it: a request the user finished and nobody answered is still
    /// open (S103 r2).
    user_request_open: bool,
    /// Session-timeline start of the utterance that opened the current
    /// request. An output transcript delta that started before it is the
    /// lagging tail of the model's previous reply (transcripts trail the
    /// audio), not an answer: it clears neither the floor nor the request
    /// (S99 #1630 r3: the tail of a long readout wiped the floor of the
    /// user's next question and released a deferred cue into it).
    request_utterance_start_ms: Option<f64>,
    /// Per result awaiting its cue: the earliest session-timeline start of
    /// model output at or after the end of its insertion (generated with the
    /// result in context).
    result_first_output_ms: HashMap<String, f64>,
    /// Per result awaiting its cue: the earliest session-timeline start of a
    /// floor-taking user utterance at or after the end of its insertion.
    result_first_utterance_ms: HashMap<String, f64>,
    /// Session-timeline start of each acknowledged result's insertion
    /// (`session.commentary.appended.start_ms`) whose cue is not yet sent.
    result_inserted_from_ms: HashMap<String, f64>,
    /// The latest output transcript spans (`start_ms`, `end_ms`) in arrival
    /// order, bounded by [`RECENT_OUTPUT_SPANS_MAX`]: a result acknowledged
    /// after the model already began speaking over its insertion (the
    /// sideband can deliver the delta first) is judged against them.
    recent_output_spans: VecDeque<(f64, f64)>,
    /// Client delegations created on this channel whose outcome the model
    /// has not received (no acknowledged result, no terminal narration), in
    /// creation order, each with the user's words for it: the label its
    /// "Started voice request" narration quotes (the user's last turn
    /// before the delegation).
    outstanding_delegations: Vec<(String, String)>,
    /// A reflected input frame has carried speech: from then on the
    /// reflected-input silence run says whether the user is still speaking.
    reflected_input_speech_seen: bool,
    /// Session-timeline start of the last output transcript delta.
    last_output_start_ms: Option<f64>,
    /// Audio-clock length of reflected-input silence since the user's last
    /// speech frame (frames below [`USER_FLOOR_SPEECH_DBFS`]).
    input_silence_run_ms: u64,
    /// Session-timeline offset of the last `session.delegation.created`.
    last_delegation_offset_ms: Option<f64>,
    /// Delegation commentary appends (narrations and results) held behind an
    /// unanswered utterance, reserved but not yet sent, in append order.
    held_commentary: VecDeque<(GptLiveAppendToken, ClientEvent)>,
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
            commentary_ack_end_ms: None,
            result_cue_candidates: HashMap::new(),
            due_result_cues: VecDeque::new(),
            deferred_result_cues: VecDeque::new(),
            output_silence_run_ms: OUTPUT_SILENCE_RELEASE_MS,
            result_inserted_through_ms: HashMap::new(),
            due_progress_notices: VecDeque::new(),
            awaiting_peer_results: HashSet::new(),
            unanswered_user_input: false,
            user_request_open: false,
            request_utterance_start_ms: None,
            result_first_output_ms: HashMap::new(),
            result_first_utterance_ms: HashMap::new(),
            result_inserted_from_ms: HashMap::new(),
            recent_output_spans: VecDeque::new(),
            outstanding_delegations: Vec::new(),
            reflected_input_speech_seen: false,
            input_silence_run_ms: 0,
            last_output_start_ms: None,
            last_delegation_offset_ms: None,
            held_commentary: VecDeque::new(),
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

    /// Reserve one delegation-lane commentary append; a result (with the
    /// delegation it answers) is also cued at its acknowledgement.
    fn reserve_delegation_commentary(
        &mut self,
        result_of: Option<String>,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let token = self.reserve_append(PendingAppendLane::Delegation)?;
        if let Some(delegation_id) = result_of {
            self.result_cue_candidates.insert(token, delegation_id);
        }
        Ok(token)
    }

    /// Reserve the next due in-progress notice as a broker-owned
    /// instructions append.
    fn reserve_due_progress_notice(
        &mut self,
    ) -> Result<Option<(GptLiveAppendToken, String, Vec<String>)>, GptLiveBrokerError> {
        let Some(delegation_id) = self.due_progress_notices.pop_front() else {
            return Ok(None);
        };
        // The notice names the other delegations still running: the model
        // claimed an earlier job done right after a later job's notice, from
        // nothing but notices and narrations (S101 r3 on 37b1cebb9).
        let mut sections = vec![LIVE_DELEGATION_IN_PROGRESS.to_owned()];
        sections.extend(self.outstanding_line_except(&delegation_id));
        let fragments = pack_sections(sections);
        let token = match self.reserve_instructions_append(fragments.len()) {
            Ok(token) => token,
            Err(error) => {
                self.due_progress_notices.push_front(delegation_id);
                return Err(error);
            }
        };
        if let Some(pending) = self.pending_appends.back_mut() {
            pending.internal = Some(InternalAppend::ProgressNotice);
        }
        Ok(Some((token, delegation_id, fragments)))
    }

    /// The delegation's outcome reached the model (its result was
    /// acknowledged) or it ended without one (a terminal narration).
    fn end_outstanding_delegation(&mut self, delegation_id: &str) {
        self.outstanding_delegations
            .retain(|(outstanding, _)| outstanding != delegation_id);
    }

    /// The still-running line for a notice or cue about `delegation_id`,
    /// naming every other outstanding delegation.
    fn outstanding_line_except(&self, delegation_id: &str) -> Option<String> {
        let labels: Vec<&str> = self
            .outstanding_delegations
            .iter()
            .filter(|(outstanding, _)| outstanding != delegation_id)
            .map(|(_, label)| label.as_str())
            .collect();
        outstanding_delegations_line(&labels)
    }

    /// Record an output delta against every result awaiting its cue that it
    /// voices ([`output_voices_result`]), keeping the earliest start.
    fn note_output_voicing_results(&mut self, start_ms: f64, previous_output_end_ms: Option<f64>) {
        for (delegation_id, inserted_through) in &self.result_inserted_through_ms {
            let Some(inserted_from) = self.result_inserted_from_ms.get(delegation_id) else {
                continue;
            };
            if output_voices_result(
                start_ms,
                previous_output_end_ms,
                *inserted_from,
                *inserted_through,
            ) {
                self.result_first_output_ms
                    .entry(delegation_id.clone())
                    .and_modify(|first| *first = first.min(start_ms))
                    .or_insert(start_ms);
            }
        }
    }

    /// Record `start_ms` against every result awaiting its cue whose
    /// insertion ended at or before it, keeping the earliest.
    fn note_earliest_after_insertion(
        inserted_through: &HashMap<String, f64>,
        earliest: &mut HashMap<String, f64>,
        start_ms: f64,
    ) {
        for (delegation_id, through) in inserted_through {
            if start_ms >= *through {
                earliest
                    .entry(delegation_id.clone())
                    .and_modify(|first| *first = first.min(start_ms))
                    .or_insert(start_ms);
            }
        }
    }

    /// Reserve the next due result cue as a broker-owned thinking append,
    /// with the typed wording it is sent with.
    ///
    /// A result the model has already spoken after, before any user turn,
    /// gets no cue. Output starting at or after the end of the result's
    /// insertion was generated with the result in context; when it starts
    /// before any floor-taking user utterance after the insertion, it is the
    /// model's own continuation, and in every recorded readout it voiced the
    /// result before any cue (S97 on #1630: 10 of 10 readouts came before
    /// the cue, none after). A cue after it only repeats the readout (S100
    /// r2 on 37b1cebb9: the full readout, then the cue, a second readout,
    /// and 1500 ms of talk-over into the user's next turn). When a user
    /// utterance took the floor after the insertion and before the model's
    /// first output since, that output was answering the user, not
    /// reporting the result, so the cue is still owed. Both are
    /// provider-timeline starts, so a transcript delivered late is placed
    /// where it was spoken (S99 on #1630 r3: the readout's tail arrived after
    /// the user's question but was spoken before it). A delta that started inside or before the insertion
    /// span is the tail of speech already under way (S97 r3: " ready." at
    /// 23800-24000 over an insertion at 23800-24000), not output since the
    /// result.
    fn reserve_due_result_cue(
        &mut self,
    ) -> Result<Option<(GptLiveAppendToken, String, ResultCueWording)>, GptLiveBrokerError> {
        loop {
            let Some(delegation_id) = self.due_result_cues.pop_front() else {
                return Ok(None);
            };
            let first_output = self.result_first_output_ms.remove(&delegation_id);
            let first_utterance = self.result_first_utterance_ms.remove(&delegation_id);
            // The model spoke with the result in context before any user
            // turn: its own continuation, which voiced the result.
            let user_spoke_first = first_output.is_some_and(|output_start| {
                first_utterance.is_some_and(|utterance_start| utterance_start < output_start)
            });
            let voiced_since_result = first_output.is_some() && !user_spoke_first;
            self.result_inserted_from_ms.remove(&delegation_id);
            if voiced_since_result {
                self.awaiting_peer_results.remove(&delegation_id);
                self.result_inserted_through_ms.remove(&delegation_id);
                tracing::info!(
                    delegation_id = %delegation_id,
                    "public Live result voiced since it landed; no result cue"
                );
                continue;
            }
            let wording = ResultCueWording {
                awaiting_peer_replies: self.awaiting_peer_results.contains(&delegation_id),
                user_request_open: self.user_request_open,
                outstanding: self.outstanding_line_except(&delegation_id),
                user_spoke_first,
            };
            // The thinking lane, not the instructions lane: instructions
            // persist as standing session instructions, and a persisted
            // cue's delegation framing carried into the next question (S99
            // A/B: recall delegated 0/10 without the deferred cue, 3-5/10
            // with it on the instructions lane).
            let token = match self.reserve_thinking_append(result_cue_fragments(&wording).len()) {
                Ok(token) => token,
                Err(error) => {
                    self.due_result_cues.push_front(delegation_id);
                    return Err(error);
                }
            };
            if let Some(pending) = self.pending_appends.back_mut() {
                pending.internal = Some(InternalAppend::ResultCue);
            }
            self.awaiting_peer_results.remove(&delegation_id);
            self.result_inserted_through_ms.remove(&delegation_id);
            return Ok(Some((token, delegation_id, wording)));
        }
    }

    /// Reserve the notice that goes ahead of a result awaiting members'
    /// answers as a broker-owned instructions append.
    fn reserve_peer_reply_pending_notice(
        &mut self,
    ) -> Result<GptLiveAppendToken, GptLiveBrokerError> {
        let token = self.reserve_instructions_append(1)?;
        if let Some(pending) = self.pending_appends.back_mut() {
            pending.internal = Some(InternalAppend::PeerReplyPendingNotice);
        }
        Ok(token)
    }

    /// Whether the user holds the floor: a real user turn (see the input
    /// transcript rule in `apply_frame`) that neither the model's output nor
    /// its delegation has answered, and whose speech has not been followed by
    /// [`USER_FLOOR_SILENCE_RELEASE_MS`] of reflected-input silence.
    fn user_holds_floor(&self) -> bool {
        self.unanswered_user_input
    }

    /// Advance the reflected-input silence run on the audio clock and end
    /// the user's floor once it reaches [`USER_FLOOR_SILENCE_RELEASE_MS`].
    /// The floor only ever opens on an input transcript delta, so audio
    /// energy alone (noise) never opens one; it only ends one. A model that
    /// stays silent after the user stopped can no longer keep commentary
    /// held forever.
    /// Track model output silence on the provider's output audio frames and
    /// release deferred result cues once the response has ended: a run of
    /// [`OUTPUT_SILENCE_RELEASE_MS`] below [`USER_FLOOR_SPEECH_DBFS`]. The run
    /// is counted on provider frames, never a wall-clock timer.
    fn observe_output_audio_energy(&mut self, audio: &str) {
        let frame_ms =
            reflected_pcm16_samples(audio).saturating_mul(1000) / SIDEBAND_INPUT_SAMPLE_RATE_HZ;
        if reflected_pcm16_dbfs(audio).is_some_and(|dbfs| dbfs >= USER_FLOOR_SPEECH_DBFS) {
            self.output_silence_run_ms = 0;
            return;
        }
        self.output_silence_run_ms = self.output_silence_run_ms.saturating_add(frame_ms);
        self.release_deferred_result_cues_when_due();
    }

    /// The user is audibly speaking: a reflected input frame carried speech
    /// and fewer than [`USER_FLOOR_SILENCE_RELEASE_MS`] of reflected-input
    /// silence have followed it. Read on the audio clock, so it holds even
    /// when the transcript has not caught up.
    fn reflected_input_speaking(&self) -> bool {
        self.reflected_input_speech_seen
            && self.input_silence_run_ms < USER_FLOOR_SILENCE_RELEASE_MS
    }

    /// Release deferred result cues once both the model's response has ended
    /// ([`OUTPUT_SILENCE_RELEASE_MS`] of output silence) and the user does
    /// not hold the floor ([`Self::user_holds_floor`], the state that holds
    /// commentary). While the user speaks the model's output is silent, so
    /// output silence alone released the cue into the middle of the user's
    /// question, and the model then delegated that question (S99 pre-merge
    /// r3 and r10: 2 of 3 such runs, against 0 of 5 where the cue landed
    /// before the question). Nor while the user is audibly speaking
    /// ([`Self::reflected_input_speaking`]): the floor is read from
    /// transcripts, which trail the audio. Called on each output frame, when
    /// the floor ends, and when reflected input goes quiet; all are
    /// provider-clock transitions, never a timer.
    fn release_deferred_result_cues_when_due(&mut self) {
        if self.deferred_result_cues.is_empty()
            || self.output_silence_run_ms < OUTPUT_SILENCE_RELEASE_MS
        {
            return;
        }
        if self.user_holds_floor() || self.reflected_input_speaking() {
            return;
        }
        tracing::info!(
            silence_ms = self.output_silence_run_ms,
            cues = self.deferred_result_cues.len(),
            "public Live response ended and the user does not hold the floor; deferred result cues due"
        );
        self.due_result_cues
            .extend(self.deferred_result_cues.drain(..));
    }

    fn observe_reflected_input_energy(&mut self, audio: &str, samples: u64) {
        let frame_ms = samples.saturating_mul(1000) / SIDEBAND_INPUT_SAMPLE_RATE_HZ;
        let speech = reflected_pcm16_dbfs(audio).is_some_and(|dbfs| dbfs >= USER_FLOOR_SPEECH_DBFS);
        if speech {
            self.input_silence_run_ms = 0;
            self.reflected_input_speech_seen = true;
            return;
        }
        let was_speaking = self.reflected_input_speaking();
        self.input_silence_run_ms = self.input_silence_run_ms.saturating_add(frame_ms);
        if was_speaking && !self.reflected_input_speaking() {
            // The user went quiet: a deferred cue held by their speech may
            // go now.
            self.release_deferred_result_cues_when_due();
        }
        if self.unanswered_user_input && self.input_silence_run_ms >= USER_FLOOR_SILENCE_RELEASE_MS
        {
            self.unanswered_user_input = false;
            tracing::info!(
                silence_ms = self.input_silence_run_ms,
                held = self.held_commentary.len(),
                "public Live user floor ended by reflected-input silence; held commentary is released"
            );
            self.release_deferred_result_cues_when_due();
        }
    }

    /// Hold a reserved delegation commentary append (a narration or a
    /// result) behind an unanswered utterance, or hand it back to be sent
    /// now. An append is also held while earlier ones are, so commentary
    /// reaches the provider in append order.
    fn hold_or_send_commentary(
        &mut self,
        token: GptLiveAppendToken,
        event: ClientEvent,
    ) -> Option<ClientEvent> {
        if self.close_requested || self.closed_observed {
            // Never held past a close: sent (or refused) now, and settled
            // by the close like any in-flight append.
            return Some(event);
        }
        if self.user_holds_floor() || !self.held_commentary.is_empty() {
            tracing::info!(
                token = token.0,
                "public Live delegation commentary held: the user's latest utterance is unanswered"
            );
            self.held_commentary.push_back((token, event));
            return None;
        }
        Some(event)
    }

    /// The held commentary provider ordering has released: all of it, in
    /// order, once the latest utterance is answered.
    fn take_releasable_held_commentary(&mut self) -> Vec<(GptLiveAppendToken, ClientEvent)> {
        if self.user_holds_floor() || self.close_requested || self.closed_observed {
            return Vec::new();
        }
        self.held_commentary.drain(..).collect()
    }

    /// A close leaves held commentary unsent. Each reservation stays
    /// pending, so the owner's close settlement resolves it as interrupted by
    /// close: a held result merges into the source member as runtime work,
    /// and a held narration is dropped (it is ephemeral progress speech for a
    /// channel that no longer exists).
    fn drop_held_commentary_for_close(&mut self) {
        for (token, _) in self.held_commentary.drain(..) {
            tracing::info!(
                token = token.0,
                "public Live held delegation commentary left unsent by the close; settled as interrupted by close"
            );
        }
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
            internal: None,
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
                self.drop_held_commentary_for_close();
                self.finish_open_turn();
                let observations = &mut self.queued_observations;
                self.pending_appends.retain(|pending| {
                    let Some(interrupted) = pending.lane.interrupted_by_close(pending.token) else {
                        return true;
                    };
                    if !pending.rejected && pending.internal.is_none() {
                        observations.push_back(interrupted);
                    }
                    false
                });
            }
            ServerEvent::CommentaryAppended {
                start_ms, end_ms, ..
            } => {
                self.commentary_ack_start_ms = Some(start_ms);
                self.commentary_ack_end_ms = Some(end_ms);
                let acknowledged = self
                    .acknowledge_append(AppendReceiptKind::Commentary, client_event_id.as_deref());
                self.commentary_ack_start_ms = None;
                self.commentary_ack_end_ms = None;
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
                // A real user turn: speech that began after the model's last
                // output ended and after the last delegation. A backchannel
                // over the model's speech ("mm-hm") overlaps that output and
                // takes no floor; a late transcription tail began before it.
                let answered_through_ms =
                    match (self.last_output_end_ms, self.last_delegation_offset_ms) {
                        (Some(output), Some(delegation)) => Some(output.max(delegation)),
                        (output, delegation) => output.or(delegation),
                    };
                // Transcription lags the audio: a delta whose speech has
                // already been followed by the full release silence takes no
                // floor either.
                if answered_through_ms.is_none_or(|answered| start_ms > answered)
                    && self.input_silence_run_ms < USER_FLOOR_SILENCE_RELEASE_MS
                {
                    if !self.user_request_open {
                        self.request_utterance_start_ms = Some(start_ms);
                        Self::note_earliest_after_insertion(
                            &self.result_inserted_through_ms,
                            &mut self.result_first_utterance_ms,
                            start_ms,
                        );
                    }
                    self.unanswered_user_input = true;
                    self.user_request_open = true;
                }
                self.record_transcript_delta(GptLiveTurnRole::User, delta);
                self.measure_provider_input_latency(end_ms);
            }
            ServerEvent::OutputTranscriptDelta {
                delta,
                start_ms,
                end_ms,
                ..
            } => {
                let previous_output_end_ms = self.recent_output_spans.back().map(|span| span.1);
                self.last_output_start_ms = Some(start_ms);
                self.last_output_end_ms = Some(end_ms);
                self.input_since_output = false;
                self.note_output_voicing_results(start_ms, previous_output_end_ms);
                if self.recent_output_spans.len() == RECENT_OUTPUT_SPANS_MAX {
                    self.recent_output_spans.pop_front();
                }
                self.recent_output_spans.push_back((start_ms, end_ms));
                // Output that began before the open request's utterance is
                // the lagging transcript of the previous reply, not an
                // answer to it.
                let lagging_tail = self
                    .request_utterance_start_ms
                    .is_some_and(|utterance_start| start_ms < utterance_start);
                if lagging_tail {
                    tracing::debug!(
                        start_ms,
                        "public Live output transcript tail predates the open utterance; the user's floor stands"
                    );
                } else {
                    self.unanswered_user_input = false;
                    self.user_request_open = false;
                    self.request_utterance_start_ms = None;
                }
                self.record_transcript_delta(GptLiveTurnRole::Assistant, delta);
            }
            ServerEvent::DelegationCreated {
                delegation,
                offset_ms,
                ..
            } => {
                self.record_delegation(delegation, offset_ms)?;
                // The utterance is answered by its delegation.
                self.last_delegation_offset_ms = Some(offset_ms);
                self.unanswered_user_input = false;
                self.user_request_open = false;
                self.request_utterance_start_ms = None;
            }
            // Reflected media, mute state, accounting telemetry, telephony
            // signalling, and informational notices carry no conversational,
            // transcript, delegation, or effect authority.
            ServerEvent::OutputAudioDelta { delta, .. } => {
                // Reflected media is not evidence of transcript or playback
                // completion; frames may also represent silence. Its energy
                // is the provider-clock signal that the model's response has
                // ended (see `observe_output_audio_energy`).
                self.reflected_output_audio_frames =
                    self.reflected_output_audio_frames.saturating_add(1);
                self.observe_output_audio_energy(&delta);
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
                let samples = reflected_pcm16_samples(&audio);
                self.reflected_input_samples = self.reflected_input_samples.saturating_add(samples);
                self.observe_reflected_input_energy(&audio, samples);
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
                    if let Some(kind) = pending.internal {
                        tracing::info!("public Live {} rejected by the provider", kind.label());
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
                        if let Some(delegation_id) = self.result_cue_candidates.remove(&token) {
                            self.awaiting_peer_results.remove(&delegation_id);
                        }
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
            if let Some(kind) = internal {
                tracing::info!(rejected, "public Live {} acknowledged", kind.label());
            } else if !rejected {
                self.queued_observations
                    .push_back(pending.lane.acknowledged(token));
            }
            self.pending_appends.remove(append_index.0);
            if let Some(delegation_id) = self.result_cue_candidates.remove(&token) {
                if rejected {
                    self.awaiting_peer_results.remove(&delegation_id);
                } else {
                    self.cue_acknowledged_result(delegation_id);
                }
            }
        }
        Ok(())
    }

    /// The result's acknowledgement is the Delivered transition. The cue is
    /// decided by provider ordering and the provider's output clock, never by
    /// a wall-clock wait:
    /// - The model's response is in progress at the result's insertion point:
    ///   output observed at or past it (an output transcript delta ending at
    ///   or after the acknowledgement's `start_ms`; gap 0 is the generation
    ///   frontier, not silence), or fewer than [`OUTPUT_SILENCE_RELEASE_MS`]
    ///   of output silence since the model's last speech frame. The cue is
    ///   deferred to the response's end: sent inside the response, it is
    ///   absorbed by content the response is already committed to (S97 r3),
    ///   and an instructions append during output can cut the answer off
    ///   mid-sentence (final soak: 5 of 44 such cues).
    /// - Otherwise the model is silent. A gap cannot tell "about to voice it"
    ///   from "never will" (a result 400 ms after the last word was never read
    ///   out), so the result gets one cue now.
    ///
    /// Either way the result gets exactly one cue, phrased to be safe if the
    /// model already read it ([`result_cue_text`]).
    fn cue_acknowledged_result(&mut self, delegation_id: String) {
        // The model now holds this delegation's outcome.
        self.end_outstanding_delegation(&delegation_id);
        let ack_start_ms = self.commentary_ack_start_ms;
        if let (Some(inserted_from), Some(inserted_through)) =
            (ack_start_ms, self.commentary_ack_end_ms)
        {
            self.result_inserted_through_ms
                .insert(delegation_id.clone(), inserted_through);
            self.result_inserted_from_ms
                .insert(delegation_id.clone(), inserted_from);
            // Output the sideband delivered before this acknowledgement may
            // already voice the result: the model starts answering as the
            // result lands (S100 r1 on 93b6aaec: " Done." over the
            // insertion's own span, its delta ahead of the receipt).
            let mut previous_end = None;
            let mut first_voicing = None::<f64>;
            for (start, end) in &self.recent_output_spans {
                if output_voices_result(*start, previous_end, inserted_from, inserted_through) {
                    first_voicing = Some(first_voicing.map_or(*start, |first| first.min(*start)));
                }
                previous_end = Some(*end);
            }
            if let Some(first) = first_voicing {
                self.result_first_output_ms
                    .insert(delegation_id.clone(), first);
            }
        }
        let gap_ms = match (ack_start_ms, self.last_output_end_ms) {
            (Some(ack_start_ms), Some(end)) => ack_start_ms - end,
            _ => f64::INFINITY,
        };
        // Output at or past the insertion point: a result inserted exactly at
        // the end of the output so far (gap 0) landed at the generation
        // frontier of a response still being voiced, not into silence.
        let output_reaches_insertion = matches!(
            (ack_start_ms, self.last_output_end_ms),
            (Some(ack_start_ms), Some(end)) if end >= ack_start_ms
        );
        let response_in_progress =
            output_reaches_insertion || self.output_silence_run_ms < OUTPUT_SILENCE_RELEASE_MS;
        if response_in_progress {
            // A cue sent now lands inside the in-progress response, which may
            // already be committed to other content (S97 r3: absorbed by the
            // narration's response, then 60 s of silence), and an
            // instructions append during output can cut the answer off. The
            // cue waits for the response to end.
            tracing::info!(
                gap_ms,
                output_silence_ms = self.output_silence_run_ms,
                delegation_id = %delegation_id,
                "public Live result delivered while the model's response is in progress; result cue deferred to the response end"
            );
            self.deferred_result_cues.push_back(delegation_id);
            return;
        }
        tracing::info!(
            gap_ms,
            delegation_id = %delegation_id,
            input_since_output = self.input_since_output,
            "public Live result delivered; result cue due"
        );
        self.due_result_cues.push_back(delegation_id);
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
        let label = self
            .last_user_turn
            .as_ref()
            .and_then(|turn| turn.rows.last())
            .map(|row| row.transcript.trim().to_owned())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| request_transcript.trim().to_owned());
        self.outstanding_delegations
            .push((reference.0.clone(), label));
        self.due_progress_notices.push_back(reference.0.clone());
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
/// Reflected-input speech floor (dBFS, frame RMS of PCM16): a reflected
/// input frame at or above it is user speech for the floor rule.
///
/// Derived from the 197 Turbo S provider streams on the 0.8.51 soaks
/// (/tmp/rb/tsoak, 2026-10-03): of the reflected frames overlapping an input
/// transcript span, 13861 carry energy, with p2 = -54 dBFS, p5 = -42 dBFS
/// and the median at -25 dBFS; the rest are inter-word gaps. Of the frames
/// more than 500 ms from any transcript span, 98.5% are digital silence
/// (-112 dBFS); the remainder sit at speech levels (median -28 dBFS), being
/// untranscribed speech. -50 dBFS is 62 dB above the silence the harness
/// injects and below 97% of transcribed speech frames.
///
/// Frozen like the Turbo S bounds: a run that needs a different floor is a
/// finding, never a bump.
const USER_FLOOR_SPEECH_DBFS: f64 = -50.0;

/// Audio-clock length of reflected-input silence after the user's last
/// speech frame that ends the user's floor (see
/// [`SessionState::user_holds_floor`]).
///
/// Derived from the 39 scripted Turbo S utterance fixtures
/// (the live-smoke browser `gpt_live_client` fixture set, 20 ms windows against
/// [`USER_FLOOR_SPEECH_DBFS`]): the longest pause inside one utterance is
/// 1020 ms (S103 `interrupt_monologue`, deliberately paused mid-clause), then
/// 880 ms (`remember`), 760 ms (`recall`) and 720 ms (`current`). 1600 ms
/// clears the longest by 580 ms, three 200 ms reflected frames, so a pause
/// inside a sentence never ends the floor.
///
/// Frozen like the Turbo S bounds: an utterance whose own pause exceeds it is
/// a finding, never a bump.
const USER_FLOOR_SILENCE_RELEASE_MS: u64 = 1_600;

/// Audio-clock length of model output silence after which a response is
/// treated as ended and a deferred result cue is sent
/// ([`SessionState::observe_output_audio_energy`]).
///
/// Measured on the provider's `session.output_audio.delta` frames in 15
/// recorded runs (combined5 S105 x5 and S99 x5, combined3 S99 x5; 200 ms
/// frames against [`USER_FLOOR_SPEECH_DBFS`]): pauses inside one response
/// are at most 1000 ms, and silences before the model's next response, with
/// no user speech between, are at least 4000 ms. 1600 ms clears the longest
/// pause by three 200 ms frames, the derivation of
/// [`USER_FLOOR_SILENCE_RELEASE_MS`].
///
/// Frozen like the Turbo S bounds: a response whose own pause exceeds it is
/// a finding, never a bump.
const OUTPUT_SILENCE_RELEASE_MS: u64 = 1_600;

/// Frame RMS of a reflected PCM16 (little-endian) frame in dBFS, or `None`
/// when it does not decode. Telemetry and floor input only.
fn reflected_pcm16_dbfs(audio: &str) -> Option<f64> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio)
        .ok()?;
    let samples = bytes.len() / 2;
    if samples == 0 {
        return None;
    }
    let sum: f64 = bytes
        .chunks_exact(2)
        .map(|pair| f64::from(i16::from_le_bytes([pair[0], pair[1]])).powi(2))
        .sum();
    let rms = (sum / samples as f64).sqrt();
    Some(if rms > 0.0 {
        20.0 * (rms / 32768.0).log10()
    } else {
        f64::NEG_INFINITY
    })
}

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

/// The speak cue that follows an acknowledged result the model is not
/// already voicing, bound to the result's delegation by its `delegation_id`.
/// It names no content, so it cannot carry or alter the result it points at.
/// It is conditional, because the broker cannot know whether the model
/// already voiced the result: the model reads the result if it has not, and
/// stays quiet if it has. The dedup clause is anchored to the delivery:
/// anything the model said about the request before the result arrived (an
/// intention such as "I'll use Friday") is not a report of it (soak 35728bf0,
/// S103 run 1).
///
/// A cue is composed at send time from typed broker state
/// ([`ResultCueWording`], [`result_cue_text`]): the variant's head and
/// action, the open-request clause only while the user's latest request is
/// open, the variant's tail, and [`LIVE_RESULT_CUE_SCOPE`] on every cue.
///
/// Head of the cue for a result the model has not spoken since: no output
/// starts at or after the end of the result's insertion, so the model cannot
/// have reported it and the cue offers no "already reported" exception (S97
/// v3 r4: a greeting that ended 1 ms before the result was taken as the
/// report, and the result was never voiced; S97 r3 at 65294c7ca: the tail
/// " ready." of a reply under way spanned the insertion itself and was taken
/// as the report).
const LIVE_RESULT_UNREPORTED_CUE_HEAD: &str = "This delegation's result has just arrived and you have not told the user its outcome yet; anything you said about this request before now was said before it was done.";
const LIVE_RESULT_UNREPORTED_CUE_ACTION: &str = "the user the actual outcome of this result now, even when it reports an error or that nothing could be done.";
/// Tail of the outcome cue: an "I asked them" result is not their answer
/// (S102 r2).
const LIVE_RESULT_CUE_TAIL: &str = "Report only what the result itself says: when it says someone else was asked, their answer is still pending.";
/// Head of the cue for a result whose delegated work asked other members
/// that have not answered yet (outbound peer requests with no terminal
/// response committed). The result reports only that they were asked, so the
/// model says it asked and states no answer (S102 r2 voiced an invented
/// one); the pending notice sent ahead of the result carries the same
/// guard, so a result already spoken after needs no cue either. The
/// member's real answer arrives later as its own row.
const LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_HEAD: &str = "This delegation's result has just arrived and you have not told the user about it yet: it reports that someone was asked, and their answer has not arrived yet.";
const LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_ACTION: &str =
    "the user now that you asked; do not state, guess, or imply their answer.";
/// Sent only while the user's latest request is open: a request neither the
/// model's output nor a delegation has answered. A held result can go out
/// once the user's floor ends on silence with that request still open (S103
/// r2). With no request open the clause is dropped: it ties "the user's
/// request" to the delegation flow for a question not yet asked.
const LIVE_RESULT_CUE_OPEN_REQUEST: &str =
    "If the user's latest request is still unanswered, answer it first.";
/// The closing sentence of every result cue. Cues go on the instructions
/// lane and persist; without a scope, a cue's delegation framing carried
/// into the next question, and recall questions were delegated (S99 on
/// trees with the deferred cue: 4 of 5 misses had a cue land before the
/// question). Short enough that every cue sent with no request open fits a
/// single append fragment with it (the outcome cue with an open request is
/// the exception, see [`result_cue_fragments`]); questions about the
/// conversation are the ones the session's routing rule keeps native, and
/// work still goes to the executor.
const LIVE_RESULT_CUE_SCOPE: &str =
    "Only this result: answer questions about this conversation yourself.";

/// What a result's cue must say, read from broker state when it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResultCueWording {
    /// Its delegated work still awaits members' answers.
    awaiting_peer_replies: bool,
    /// The user's latest request is open: neither the model's output nor a
    /// delegation has answered it.
    user_request_open: bool,
    /// The other delegations still running ([`outstanding_delegations_line`]),
    /// when there are any.
    outstanding: Option<String>,
    /// The cue is owed because a user turn came after the result and before
    /// the model's first output since: that output answered the user. The
    /// user may have said how to report the result ("skip the details, just
    /// say done", S100 r1 on 93b6aaec), which no typed fact can tell, so the
    /// cue defers to it ([`LIVE_RESULT_CUE_USER_SPOKE_FIRST`]).
    user_spoke_first: bool,
}

/// The cue for an acknowledged result the model has not spoken since: the
/// awaiting-peer form when its delegated work still awaits members'
/// answers, the open-request clause only while a request is open, and the
/// scope sentence always.
#[cfg(test)]
fn result_cue_text(wording: &ResultCueWording) -> String {
    result_cue_sections(wording).join(" ")
}

/// The sentences of a result cue in order: its body, the delegations still
/// running (when any), and the scope sentence last. No fragment boundary
/// ever falls inside one.
fn result_cue_sections(wording: &ResultCueWording) -> Vec<String> {
    let (head, action, tail) = if wording.awaiting_peer_replies {
        (
            LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_HEAD,
            LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_ACTION,
            None,
        )
    } else {
        (
            LIVE_RESULT_UNREPORTED_CUE_HEAD,
            LIVE_RESULT_UNREPORTED_CUE_ACTION,
            Some(LIVE_RESULT_CUE_TAIL),
        )
    };
    let mut body = String::from(head);
    if wording.user_request_open {
        body.push(' ');
        body.push_str(LIVE_RESULT_CUE_OPEN_REQUEST);
        body.push_str(" Then tell ");
    } else {
        body.push_str(" Tell ");
    }
    body.push_str(action);
    if let Some(tail) = tail {
        body.push(' ');
        body.push_str(tail);
    }
    let mut sections = vec![body];
    if wording.user_spoke_first {
        sections.push(LIVE_RESULT_CUE_USER_SPOKE_FIRST.to_owned());
    }
    sections.extend(wording.outstanding.clone());
    sections.push(LIVE_RESULT_CUE_SCOPE.to_owned());
    sections
}

/// The append fragments of a result cue: its sentences packed in order into
/// fragments of at most [`CONTEXT_FRAGMENT_MAX_BYTES`], splitting only
/// between sentences (a mid-word seam halves recall on gpt-live-1).
fn result_cue_fragments(wording: &ResultCueWording) -> Vec<String> {
    pack_sections(result_cue_sections(wording))
}

/// Pack whole sections, in order, into fragments of at most
/// [`CONTEXT_FRAGMENT_MAX_BYTES`] joined by a space. Each section is itself
/// within the bound.
fn pack_sections(sections: Vec<String>) -> Vec<String> {
    let mut fragments: Vec<String> = Vec::new();
    for section in sections {
        match fragments.last_mut() {
            Some(last) if last.len() + 1 + section.len() <= CONTEXT_FRAGMENT_MAX_BYTES => {
                last.push(' ');
                last.push_str(&section);
            }
            _ => fragments.push(section),
        }
    }
    fragments
}

/// Sent on a cue owed through a user turn that came after the result and
/// before the model's next output: the user may have said how to report it.
const LIVE_RESULT_CUE_USER_SPOKE_FIRST: &str =
    "The user has spoken since this result arrived: if they said how to report it, do that.";

/// Output that voices a result awaiting its cue: it starts at or after the
/// end of the result's insertion, or it starts at or after the insertion's
/// start and opens a new response (the previous output ended at least
/// [`OUTPUT_SILENCE_RELEASE_MS`] earlier on the transcript timeline). The
/// model often begins answering exactly as the result lands (S100 r1 on
/// 93b6aaec: " Done." over the insertion's own span, after 3000 ms of
/// silence), while output that started inside the span as the continuation
/// of a sentence already under way is not a readout (S97 r3: " ready." over
/// 23800-24000, the tail of "Voice channel").
fn output_voices_result(
    start_ms: f64,
    previous_output_end_ms: Option<f64>,
    inserted_from_ms: f64,
    inserted_through_ms: f64,
) -> bool {
    #[allow(clippy::cast_precision_loss)]
    let response_gap_ms = OUTPUT_SILENCE_RELEASE_MS as f64;
    start_ms >= inserted_through_ms
        || (start_ms >= inserted_from_ms
            && previous_output_end_ms.is_none_or(|end| start_ms - end >= response_gap_ms))
}

/// Bound on [`SessionState::recent_output_spans`].
const RECENT_OUTPUT_SPANS_MAX: usize = 64;

/// Labels named in [`outstanding_delegations_line`]; further delegations are
/// counted, not named, so the line stays within one fragment.
const OUTSTANDING_MAX_NAMES: usize = 3;
/// Characters kept of each label in [`outstanding_delegations_line`].
const OUTSTANDING_LABEL_CHARS: usize = 60;

/// The delegations still running, by the user's words for them, and the
/// rule that goes with them: none of them is done until its result arrives
/// (S101 on 37b1cebb9: the model said "the second one is also done" 10-14 s
/// before that job's result existed, in 5 of 5 runs, right after the cues
/// of the two jobs that had finished).
fn outstanding_delegations_line(labels: &[&str]) -> Option<String> {
    if labels.is_empty() {
        return None;
    }
    let mut names: Vec<String> = labels
        .iter()
        .take(OUTSTANDING_MAX_NAMES)
        .map(|label| {
            let label = label.trim();
            if label.chars().count() > OUTSTANDING_LABEL_CHARS {
                let kept: String = label.chars().take(OUTSTANDING_LABEL_CHARS - 3).collect();
                format!("\"{}...\"", kept.trim_end())
            } else {
                format!("\"{label}\"")
            }
        })
        .collect();
    let unnamed = labels.len().saturating_sub(names.len());
    if unnamed > 0 {
        names.push(format!("{unnamed} other request(s)"));
    }
    let listed = match names.as_slice() {
        [one] => one.clone(),
        [init @ .., last] => format!("{}; {last}", init.join("; ")),
        [] => return None,
    };
    Some(if labels.len() == 1 {
        format!("Still running: {listed}. Do not say it is done until its result arrives.")
    } else {
        format!(
            "Still running: {listed}. Do not say any of them is done until its own result arrives."
        )
    })
}

/// Labels named in [`peer_reply_pending_notice`]; further members are
/// counted, not named, so the notice stays one append.
const PEER_REPLY_NOTICE_MAX_NAMES: usize = 3;
/// Characters kept of each label in [`peer_reply_pending_notice`].
const PEER_REPLY_NOTICE_NAME_CHARS: usize = 64;

/// The notice sent ahead of a result whose delegated work asked `peers`
/// (display labels) that have not answered: bound to the delegation and
/// placed before the result in the outbox, so the provider holds it when
/// the "I asked them" result lands.
fn peer_reply_pending_notice(peers: &[String]) -> String {
    let mut names: Vec<String> = peers
        .iter()
        .take(PEER_REPLY_NOTICE_MAX_NAMES)
        .map(|peer| {
            peer.trim()
                .chars()
                .take(PEER_REPLY_NOTICE_NAME_CHARS)
                .collect()
        })
        .collect();
    let unnamed = peers.len().saturating_sub(names.len());
    if unnamed > 0 {
        names.push(format!("{unnamed} other member(s)"));
    }
    let who = match names.as_slice() {
        [] => "Another member".to_string(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    format!(
        "The result that follows reports asking {who}. Their answer has not arrived yet. Until it arrives as its own update, tell the user only that you asked: do not state, guess, or imply what they said."
    )
}

/// The notice sent on the instructions lane at every client
/// `session.delegation.created`, bound to that delegation: the request is in
/// progress, so the model must not describe it as done, or state its
/// outcome, before its result arrives (S103 r1's premature completion
/// claim). The public protocol has no way to make the result itself a turn
/// the model must answer; delegation-bound trusted steering is the strongest
/// lever it offers.
///
/// A request that asks another member something completes with "I asked
/// them": the member's answer comes later, as its own update. Turbo S S102
/// r2 voiced "Pemberton said it feels like it's around mid-afternoon" the
/// moment the "I asked Analyst Pemberton" result landed, 6.7 s before
/// Pemberton's real answer. The notice is in context before any result, so
/// it carries the peer case too, with the correction once the answer
/// arrives.
const LIVE_DELEGATION_IN_PROGRESS: &str = "This delegated request is now in progress. Until its result arrives, do not say or imply that it is done and do not state its outcome; you may say that you are working on it. If its result says someone else was asked, their answer is not part of that result: do not state, guess, or imply what they said until their answer arrives as its own update, then tell the user what they actually said, correcting anything said before.";

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

    /// Only a provider-limit drop counts as one: the deliberate verbatim cap
    /// (and its user-row start) is covered by the fresh summary, so it is
    /// logged at debug, not warned as an incident.
    #[test]
    fn startup_budget_separates_the_verbatim_cap_from_provider_limit_drops() {
        use meerkat_core::types::UserMessage;
        let items = |messages: Vec<Message>| -> Vec<InitialItem> {
            messages.iter().filter_map(history_item).collect()
        };
        let summary = summary_item("summary", SummaryCoverage::Opening);
        // 130 tiny turns: 126 dropped, all by the cap.
        let tiny = items(
            (0..130)
                .map(|index| Message::User(UserMessage::text(format!("t{index}"))))
                .collect(),
        );
        let (kept, truncation, provider_limited) = budget_startup_input(summary.clone(), &tiny);
        assert_eq!(kept.len(), 1 + LIVE_STARTUP_VERBATIM_ITEMS_MAX);
        assert_eq!(
            truncation.dropped_items,
            130 - LIVE_STARTUP_VERBATIM_ITEMS_MAX
        );
        assert_eq!(provider_limited, 0, "the cap is not a provider-limit drop");
        // Three 9,000-byte turns: the token budget drops the oldest while the
        // cap still has room, which is a genuine provider-limit truncation.
        let large = items(
            (1..=3)
                .map(|index| {
                    Message::User(UserMessage::text(format!(
                        "turn {index} {}",
                        "x".repeat(9_000)
                    )))
                })
                .collect(),
        );
        let (kept, truncation, provider_limited) = budget_startup_input(summary, &large);
        assert_eq!(kept.len(), 3);
        assert_eq!(truncation.dropped_items, 1);
        assert_eq!(provider_limited, 1);
    }

    /// The budget walk stops at the first item that does not fit, so a large
    /// turn never leaves a gap with older turns seeded around it.
    #[test]
    fn startup_budget_keeps_a_contiguous_verbatim_tail() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, UserMessage};
        let assistant = |text: String| {
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text { text, meta: None }],
                StopReason::EndTurn,
            ))
        };
        let user = |text: &str| Message::User(UserMessage::text(text));
        let big = "x".repeat(30_000);
        let seed = |rows: &[Message]| {
            let config = PublicLiveOpenConfig::new("v=0", "marin")
                .unwrap()
                .with_history(rows)
                .with_context_summary("summary");
            let items = config.context_seed.initial_input().unwrap();
            assert_eq!(items[0].role, InitialRole::Developer);
            let texts: Vec<String> = items[1..]
                .iter()
                .map(|item| item.content[0].text.chars().take(8).collect())
                .collect();
            (texts, config.startup_input_truncation())
        };
        // u0 a0(large) u1 a1: a1 and u1 fit, a0 does not, so u0 is not seeded
        // either, although it would fit on its own.
        let rows = vec![
            user("u0"),
            assistant(big.clone()),
            user("u1"),
            assistant("a1".to_string()),
        ];
        let (texts, truncation) = seed(&rows);
        assert_eq!(texts, ["u1", "a1"]);
        assert_eq!(truncation.dropped_items, 2, "a0 and u0 are reported");
        assert!(truncation.dropped_bytes >= big.len() + "u0".len());
        // A large newest turn leaves no verbatim tail: the summary covers it
        // and everything before it.
        let rows = vec![
            user("u0"),
            assistant("a0".to_string()),
            user("u1"),
            assistant(big),
        ];
        let (texts, truncation) = seed(&rows);
        assert!(texts.is_empty(), "{texts:?}");
        assert_eq!(truncation.dropped_items, 4);
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
                .contains("you know them: answer questions about them yourself, directly")
        );
        // No recent turns: the original pending notice and no input.
        let empty = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_pending_context_after_recent(&[]);
        let encoded = serde_json::to_value(factory.session_config(&empty)).unwrap();
        assert!(encoded.get("input").is_none());
        assert!(
            encoded["instructions"]
                .as_str()
                .unwrap()
                .contains(LIVE_PENDING_CONTEXT_NOTICE)
        );
    }

    /// Both pending notices claim only what the seed carries, so the model
    /// never claims facts older than the seeded turns before the summary
    /// lands: without verbatim turns, the history is still being prepared;
    /// with them, only those turns are claimed as known, and nothing about
    /// older history is.
    #[test]
    fn pending_notices_claim_only_what_the_seed_carries() {
        assert!(
            LIVE_PENDING_CONTEXT_NOTICE
                .contains("Historical session context is being prepared and is not yet available.")
        );
        let after_recent = LIVE_PENDING_CONTEXT_AFTER_RECENT_NOTICE;
        assert!(after_recent.contains(
            "The most recent turns of the earlier text conversation are in the session input."
        ));
        assert!(after_recent.contains("you know them: answer questions about them yourself"));
        for over_claim in [
            "everything",
            "whole conversation",
            "all of",
            "older",
            "earlier facts",
            "summary",
        ] {
            assert!(
                !after_recent.contains(over_claim),
                "the seeded-turns notice claims {over_claim:?}"
            );
        }
    }

    /// A summary-pending seed with recent turns tells the model those turns
    /// are known and only the earlier history is pending, so a question about
    /// the seeded turns is answered directly (S99 positive control).
    #[test]
    fn pending_context_after_recent_says_the_recent_turns_are_known() {
        use meerkat_core::types::UserMessage;
        let factory = PublicLiveBrokerFactory::try_from_target(realtime_target(
            "gpt-live-1",
            OpenAiBackendKind::OpenAiApi,
        ))
        .unwrap();
        let config = PublicLiveOpenConfig::new("v=0", "marin")
            .unwrap()
            .with_instructions("Catalog behavior.")
            .with_pending_context_after_recent(&[Message::User(UserMessage::text(
                "today I parked on level nine",
            ))]);
        let encoded = serde_json::to_value(factory.session_config(&config)).unwrap();
        assert_eq!(
            encoded["instructions"],
            format!("Catalog behavior.\n\n{LIVE_PENDING_CONTEXT_AFTER_RECENT_NOTICE}")
        );
        let notice = LIVE_PENDING_CONTEXT_AFTER_RECENT_NOTICE;
        assert!(notice.contains(
            "The most recent turns of the earlier text conversation are in the session input"
        ));
        assert!(notice.contains("you know them: answer questions about them yourself, directly"));
        // Turbo S S99 (combined3 r2, chk R2): the control question about the
        // newest seeded turn got "I don't know" or a delegation, its first
        // exchange, with nothing before it but the seed and this notice. The
        // notice's pending-summary sentence ("only the older part ... is
        // being prepared and is not yet available") is the one claim that
        // can map a "text chat" question onto something unavailable, so a
        // notice for seeded answer-bearing turns makes no pending claim; the
        // late summary arrives with its own framing.
        assert!(!notice.contains("not yet available"));
        assert!(!notice.contains("summary"));
        assert_eq!(
            encoded["input"][0]["content"][0]["text"],
            "today I parked on level nine"
        );
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

    /// Three client delegations in turn, as S101 creates them: the user's
    /// words, the delegation, and the model's acknowledgement.
    fn state_with_three_running_delegations() -> SessionState {
        let mut state = SessionState::default();
        for (index, (words, id)) in [
            ("what is two minus two", "dlg_quick"),
            ("create marker one dot txt", "dlg_job1"),
            ("create marker two dot txt", "dlg_job2"),
        ]
        .into_iter()
        .enumerate()
        {
            let at = 3000.0 * index as f64;
            state.apply_frame(frame(input_delta_at(words, at))).unwrap();
            state
                .apply_frame(frame(delegation_created_at(id, "client", at + 500.0)))
                .unwrap();
            state
                .apply_frame(frame(output_delta_span("on it", at + 1000.0, at + 1400.0)))
                .unwrap();
        }
        drain(&mut state);
        state
    }

    fn acknowledge_result(state: &mut SessionState, delegation: &str, at: f64) {
        let result = state
            .reserve_delegation_commentary(Some(delegation.to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), at)))
            .unwrap();
    }

    #[test]
    fn the_outstanding_line_names_up_to_three_and_counts_the_rest() {
        assert_eq!(outstanding_delegations_line(&[]), None);
        assert_eq!(
            outstanding_delegations_line(&["create marker two dot txt"]).as_deref(),
            Some(
                "Still running: \"create marker two dot txt\". Do not say it is done until its result arrives."
            )
        );
        let five = ["a", "b", "c", "d", "e"];
        let line = outstanding_delegations_line(&five).unwrap();
        assert_eq!(
            line,
            "Still running: \"a\"; \"b\"; \"c\"; 2 other request(s). Do not say any of them is done until its own result arrives."
        );
        let long = "word ".repeat(40);
        let line =
            outstanding_delegations_line(&[long.as_str(), long.as_str(), long.as_str(), "x"])
                .unwrap();
        assert!(
            line.contains("...\""),
            "a long label is cut to its first characters"
        );
        assert!(line.len() <= CONTEXT_FRAGMENT_MAX_BYTES, "one fragment");
    }

    /// S101 on 37b1cebb9 (5/5): right after the cues of the quick job and
    /// job1, the model said "the second one is done too", 10-14 s before
    /// job2's result existed. Each cue now names the delegations still
    /// running, never the one it is about or one already reported.
    #[test]
    fn each_result_cue_names_the_delegations_still_running() {
        let mut state = state_with_three_running_delegations();
        assert_eq!(state.outstanding_delegations.len(), 3);
        acknowledge_result(&mut state, "dlg_quick", 9000.0);
        drain(&mut state);
        let (_, delegation, wording) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("the quick job's cue");
        assert_eq!(delegation, "dlg_quick");
        let outstanding = wording.outstanding.clone().expect("two still running");
        assert!(outstanding.contains("\"create marker one dot txt\""));
        assert!(outstanding.contains("\"create marker two dot txt\""));
        assert!(
            !outstanding.contains("two minus two"),
            "not the cue's own job"
        );
        let cue = result_cue_text(&wording);
        assert!(
            cue.ends_with(&format!("{outstanding} {LIVE_RESULT_CUE_SCOPE}")),
            "the still-running line comes before the scope sentence"
        );
        drain(&mut state);
        acknowledge_result(&mut state, "dlg_job1", 9600.0);
        drain(&mut state);
        let (_, _, wording) = state.reserve_due_result_cue().unwrap().expect("job1's cue");
        assert_eq!(
            wording.outstanding.as_deref(),
            Some(
                "Still running: \"create marker two dot txt\". Do not say it is done until its result arrives."
            ),
            "job2 is still running; the quick job and job1 are not named"
        );
        let fragments = result_cue_fragments(&wording);
        assert!(
            fragments
                .iter()
                .all(|fragment| fragment.len() <= CONTEXT_FRAGMENT_MAX_BYTES)
        );
        assert_eq!(fragments.join(" "), result_cue_text(&wording));
        drain(&mut state);
        acknowledge_result(&mut state, "dlg_job2", 12000.0);
        drain(&mut state);
        let (_, _, wording) = state.reserve_due_result_cue().unwrap().expect("job2's cue");
        assert_eq!(wording.outstanding, None, "nothing left running");
        assert!(state.outstanding_delegations.is_empty());
    }

    /// The in-progress notice of a new delegation names the earlier ones
    /// still running (S101 r3: "the first one is done" right after job2's
    /// notice, before any result), in a second fragment so the notice
    /// itself stays intact.
    #[test]
    fn an_in_progress_notice_names_the_other_delegations_still_running() {
        let mut state = SessionState::default();
        let mut notices = Vec::new();
        for (index, (words, id)) in [
            ("create marker one dot txt", "dlg_job1"),
            ("create marker two dot txt", "dlg_job2"),
        ]
        .into_iter()
        .enumerate()
        {
            let at = 3000.0 * index as f64;
            state.apply_frame(frame(input_delta_at(words, at))).unwrap();
            state
                .apply_frame(frame(delegation_created_at(id, "client", at + 500.0)))
                .unwrap();
            drain(&mut state);
            let (_, delegation, fragments) = state
                .reserve_due_progress_notice()
                .unwrap()
                .expect("notice due");
            assert_eq!(delegation, id);
            notices.push(fragments);
            drain(&mut state);
        }
        assert_eq!(
            notices[0],
            [LIVE_DELEGATION_IN_PROGRESS],
            "nothing else running: the notice alone, one fragment"
        );
        assert_eq!(
            notices[1],
            [
                LIVE_DELEGATION_IN_PROGRESS.to_owned(),
                "Still running: \"create marker one dot txt\". Do not say it is done until its result arrives."
                    .to_owned(),
            ]
        );
    }

    /// S104 R1: a delegation with no actionable input (a reopened channel
    /// delegating before any user speech) is refused without ever entering
    /// the outstanding list, so no later notice or cue names it as still
    /// running.
    #[test]
    fn a_refused_stray_delegation_is_never_named_as_running() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(delegation_created_at("dlg_stray", "client", 1350.0)))
            .unwrap();
        assert!(matches!(
            drain(&mut state).as_slice(),
            [GptLiveBrokerObservation::DelegationActionableInputUnsupported { delegation }]
                if delegation.__opaque_provider_id() == "dlg_stray"
        ));
        assert!(state.outstanding_delegations.is_empty());
        assert!(
            state.reserve_due_progress_notice().unwrap().is_none(),
            "no in-progress notice for the refused stray"
        );
        state
            .apply_frame(frame(input_delta_at("create marker one dot txt", 3000.0)))
            .unwrap();
        state
            .apply_frame(frame(delegation_created_at("dlg_job1", "client", 3500.0)))
            .unwrap();
        drain(&mut state);
        let (_, delegation, fragments) = state
            .reserve_due_progress_notice()
            .unwrap()
            .expect("the real delegation's notice");
        assert_eq!(delegation, "dlg_job1");
        assert_eq!(
            fragments,
            [LIVE_DELEGATION_IN_PROGRESS],
            "no still-running line names the refused stray"
        );
    }

    /// A delegation that ends without a result (its Failed narration) is no
    /// longer named as running.
    #[test]
    fn a_delegation_ended_without_a_result_is_no_longer_named() {
        let mut state = state_with_three_running_delegations();
        state.end_outstanding_delegation("dlg_job2");
        acknowledge_result(&mut state, "dlg_job1", 9000.0);
        drain(&mut state);
        let (_, _, wording) = state.reserve_due_result_cue().unwrap().expect("job1's cue");
        assert_eq!(
            wording.outstanding.as_deref(),
            Some(
                "Still running: \"what is two minus two\". Do not say it is done until its result arrives."
            ),
            "the failed job2 is not named"
        );
    }

    #[test]
    fn a_result_landing_well_after_the_last_word_gets_one_broker_owned_speak_cue() {
        let mut state = state_with_spoken_delegation();
        let narration = state.reserve_delegation_commentary(None).unwrap();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(narration), 5000.0)))
            .unwrap();
        assert!(state.due_result_cues.is_empty(), "narration is never cued");
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 5000.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, ["dlg_cue"]);
        drain(&mut state);
        let (cue, delegation_id, _) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("one cue due");
        assert_eq!(delegation_id, "dlg_cue", "the cue is bound to its result");
        assert_eq!(state.reserve_due_result_cue().unwrap(), None, "exactly one");
        state
            .apply_frame(frame(thinking_ack(Some(&thinking_event_id(cue, 0)))))
            .unwrap();
        assert!(
            drain(&mut state).is_empty(),
            "the cue's acknowledgement is consumed by the broker, never surfaced"
        );
        assert_eq!(state.outstanding_receipt_count(), 0);
    }

    /// turbo-det S106: the model's last word ended 400 ms before the result
    /// landed and the model never read the result out. A gap cannot tell
    /// that apart from a result the model is about to voice (measured voiced
    /// results land -200 to +400 ms after the last word), so a result landing
    /// at or after the last word is cued, whatever the gap or the input
    /// since.
    #[test]
    fn a_result_landing_after_the_last_word_is_cued_whatever_the_gap() {
        for ack_ms in [2400.0, 2999.0, 3000.0, 9000.0] {
            let mut state = state_with_spoken_delegation();
            let result = state
                .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
                .unwrap();
            state
                .apply_frame(frame(ack_at(&pending_event_id(result), ack_ms)))
                .unwrap();
            assert_eq!(
                state.due_result_cues,
                ["dlg_cue"],
                "ack at {ack_ms} ms (gap {} ms)",
                ack_ms - 2000.0
            );
        }
        // The user speaking after the model's last word: still cued.
        let mut state = state_with_spoken_delegation();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state.apply_frame(frame(input_delta("and also"))).unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 9000.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, ["dlg_cue"]);
    }

    /// No cue lands while output is in progress, read from provider
    /// ordering: output already observed past the result's insertion point
    /// means the model is speaking after the result landed, and an
    /// instructions append there can cut the answer off mid-sentence.
    #[test]
    fn no_cue_while_output_runs_past_the_results_insertion_point() {
        // The model's output (1500..2000 ms) runs past a result inserted at
        // 1800 ms.
        let mut state = state_with_spoken_delegation();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 1800.0)))
            .unwrap();
        assert!(state.due_result_cues.is_empty(), "no cue during output");
        assert_eq!(
            state.deferred_result_cues,
            ["dlg_cue"],
            "the cue waits for the response to end instead of being dropped"
        );
        // The final-soak truncation shape: output observed through 54000 ms
        // before the acknowledgement of a result inserted at 53800 ms.
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(output_delta_span(" The number is", 53600.0, 54000.0)))
            .unwrap();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 53800.0)))
            .unwrap();
        assert!(state.due_result_cues.is_empty());
    }

    /// One 200 ms provider output audio frame: a tone for speech, digital
    /// zero for silence.
    fn output_audio(speech: bool) -> Value {
        use base64::Engine as _;
        let samples: Vec<u8> = (0..4_800_u32)
            .flat_map(|index| {
                let value = if speech {
                    (8_000.0 * (f64::from(index) * 0.13).sin()) as i16
                } else {
                    0
                };
                value.to_le_bytes()
            })
            .collect();
        json!({"type":"session.output_audio.delta",
            "delta": base64::engine::general_purpose::STANDARD.encode(samples)})
    }

    fn model_output(state: &mut SessionState, speech: bool, frames: usize) {
        for _ in 0..frames {
            state.apply_frame(frame(output_audio(speech))).unwrap();
        }
    }

    /// S97 r3: the result landed at the generation frontier (gap 0) of the
    /// narration's response while it was still being voiced. Sent there, the
    /// cue was absorbed by the narration and the result was never read. The
    /// cue now waits for the response to end and is sent exactly once.
    #[test]
    fn a_result_at_the_frontier_of_a_voiced_response_is_cued_once_after_it_ends() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
            .unwrap();
        assert!(
            state.due_result_cues.is_empty(),
            "no cue inside the response"
        );
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        // The narration's tail, then a pause shorter than the release: still
        // the same response.
        model_output(&mut state, true, 2);
        model_output(&mut state, false, 5);
        model_output(&mut state, true, 1);
        assert!(state.due_result_cues.is_empty(), "a pause is not the end");
        model_output(&mut state, false, 7);
        assert!(
            state.due_result_cues.is_empty(),
            "1400 ms of silence is not the end"
        );
        model_output(&mut state, false, 1);
        assert_eq!(
            state.due_result_cues,
            ["dlg_cue"],
            "1600 ms of output silence ends the response and releases the cue"
        );
        assert!(state.deferred_result_cues.is_empty());
        drain(&mut state);
        let (_, delegation_id, _) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("one cue due");
        assert_eq!(delegation_id, "dlg_cue");
        model_output(&mut state, false, 20);
        assert_eq!(state.reserve_due_result_cue().unwrap(), None, "exactly one");
    }

    /// S97 v3 r4: the model's output ended 1 ms before the result's insertion
    /// point and nothing followed. The deferred cue is released after the
    /// response ends and uses the "unreported" wording, with no "already
    /// reported" exception the model can take its greeting for.
    #[test]
    fn a_result_with_no_output_since_it_landed_gets_the_unreported_cue() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2001.0)))
            .unwrap();
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        model_output(&mut state, false, 8);
        drain(&mut state);
        let (_, delegation_id, wording) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("one cue due");
        assert_eq!(delegation_id, "dlg_cue");
        assert!(result_cue_text(&wording).starts_with(LIVE_RESULT_UNREPORTED_CUE_HEAD));
    }

    /// Output starting at or after the end of the insertion (a folded-in
    /// readout) was generated with the result in context: the result gets no
    /// cue at all, so it is neither read twice nor followed by standing
    /// delegation framing (S99).
    #[test]
    fn output_after_the_insertion_end_sends_no_cue() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
            .unwrap();
        state
            .apply_frame(frame(output_delta_span(" Table booked.", 2000.0, 2600.0)))
            .unwrap();
        model_output(&mut state, false, 8);
        drain(&mut state);
        assert_eq!(state.due_result_cues, ["dlg_cue"], "the response ended");
        assert_eq!(state.reserve_due_result_cue().unwrap(), None, "no cue");
        assert!(state.due_result_cues.is_empty());
        assert!(state.result_inserted_through_ms.is_empty());
        assert_eq!(state.outstanding_receipt_count(), 0, "nothing reserved");
    }

    /// The awaiting-peer cue follows the same rule: the pending notice sent
    /// ahead of such a result already carries the "say only that you asked,
    /// do not state their answer" guard, so a result the model spoke after
    /// gets no cue; one it has not spoken after gets the awaiting cue.
    #[test]
    fn an_awaiting_peer_result_spoken_after_gets_no_cue() {
        for spoken_after in [true, false] {
            let mut state = state_with_spoken_delegation();
            model_output(&mut state, true, 3);
            state.awaiting_peer_results.insert("dlg_cue".to_owned());
            let result = state
                .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
                .unwrap();
            state
                .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
                .unwrap();
            if spoken_after {
                state
                    .apply_frame(frame(output_delta_span(
                        " I asked analyst-pemberton.",
                        2000.0,
                        2800.0,
                    )))
                    .unwrap();
            }
            model_output(&mut state, false, 8);
            drain(&mut state);
            let reserved = state.reserve_due_result_cue().unwrap();
            if spoken_after {
                assert_eq!(reserved, None, "spoken after: no cue");
            } else {
                let (_, _, wording) = reserved.expect("not spoken after: the awaiting cue");
                assert!(wording.awaiting_peer_replies);
                assert!(
                    result_cue_text(&wording)
                        .starts_with(LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_HEAD)
                );
            }
            assert!(
                state.awaiting_peer_results.is_empty(),
                "the entry is consumed"
            );
        }
    }

    /// S100 r2 on 37b1cebb9: the model read the result out in full right
    /// after it landed; the deferred cue still went out once the response
    /// ended, the model read the result out again, and talked 1500 ms over
    /// the user's next turn. A readout with no user turn before it means the
    /// result was voiced: no cue, before or after the user's next turn.
    #[test]
    fn a_full_readout_with_no_user_turn_before_it_gets_no_cue() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_span(&pending_event_id(result), 2000.0, 2200.0)))
            .unwrap();
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        // The readout, generated with the result in context.
        state
            .apply_frame(frame(output_delta_span(
                " The table is booked for two at seven.",
                2200.0,
                4400.0,
            )))
            .unwrap();
        model_output(&mut state, true, 4);
        // The response ends; the user then starts the next turn.
        model_output(&mut state, false, 8);
        reflect_input(&mut state, true, 2);
        state
            .apply_frame(frame(input_delta_at(" thanks, and one more thing", 6400.0)))
            .unwrap();
        reflect_input(&mut state, false, 8);
        model_output(&mut state, false, 2);
        drain(&mut state);
        assert_eq!(
            state.reserve_due_result_cue().unwrap(),
            None,
            "the readout came first: no repeat readout over the user"
        );
        assert!(state.result_first_output_ms.is_empty());
        assert!(state.result_first_utterance_ms.is_empty());
        assert_eq!(state.outstanding_receipt_count(), 0);
    }

    /// S100 r1 on 93b6aaec: the model began a fresh response (" Done.",
    /// 3000 ms after its previous output) over the combined result's own
    /// insertion span, its delta ahead of the receipt; the user's barge-in
    /// came next. That " Done." is the readout: no cue, whichever of the
    /// delta and the receipt arrives first.
    #[test]
    fn a_fresh_response_over_the_insertion_span_voices_the_result() {
        for delta_first in [true, false] {
            let mut state = state_with_spoken_delegation();
            model_output(&mut state, true, 3);
            let result = state
                .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
                .unwrap();
            let done = output_delta_span(" Done.", 41000.0, 41200.0);
            let receipt = ack_span(&pending_event_id(result), 41000.0, 41200.0);
            if delta_first {
                state.apply_frame(frame(done)).unwrap();
                state.apply_frame(frame(receipt)).unwrap();
            } else {
                state.apply_frame(frame(receipt)).unwrap();
                state.apply_frame(frame(done)).unwrap();
            }
            model_output(&mut state, true, 2);
            // The barge-in, then the model's answer to it.
            reflect_input(&mut state, true, 3);
            state
                .apply_frame(frame(input_delta_at(
                    " skip the details, just say done",
                    43400.0,
                )))
                .unwrap();
            state
                .apply_frame(frame(output_delta_span(" Done.", 46200.0, 46400.0)))
                .unwrap();
            reflect_input(&mut state, false, 8);
            model_output(&mut state, false, 8);
            drain(&mut state);
            assert_eq!(
                state.reserve_due_result_cue().unwrap(),
                None,
                "the fresh response over the insertion span was the readout (delta first: {delta_first})"
            );
        }
    }

    /// S97 R5: a sentence the model began before the insertion started
    /// ("I'm done. There's nothing") and finished after it ended. Unchanged
    /// by the frontier rule: only the continuation past the insertion end
    /// counts, and it voices the result, so no cue.
    #[test]
    fn a_sentence_begun_before_the_insertion_counts_only_from_its_end() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        for (text, start, end) in [
            (" I'm done", 24200.0, 24400.0),
            (". There's", 24400.0, 24600.0),
            (" nothing", 24600.0, 24800.0),
        ] {
            state
                .apply_frame(frame(output_delta_span(text, start, end)))
                .unwrap();
        }
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_span(&pending_event_id(result), 24800.0, 25000.0)))
            .unwrap();
        assert!(
            !state.result_first_output_ms.contains_key("dlg_cue"),
            "nothing before the insertion start counts"
        );
        state
            .apply_frame(frame(output_delta_span(
                " in the current directory.",
                25200.0,
                26000.0,
            )))
            .unwrap();
        assert_eq!(state.result_first_output_ms.get("dlg_cue"), Some(&25200.0));
        model_output(&mut state, false, 8);
        drain(&mut state);
        assert_eq!(state.reserve_due_result_cue().unwrap(), None);
    }

    /// The frontier rule itself: inside the insertion span only a new
    /// response counts; at or after the insertion end anything does.
    #[test]
    fn output_voices_a_result_from_its_end_or_as_a_new_response_inside_it() {
        // S97 r3: the contiguous tail of a sentence under way.
        assert!(!output_voices_result(
            23800.0,
            Some(23600.0),
            23800.0,
            24000.0
        ));
        // S100 r1: a fresh response after 3000 ms of silence.
        assert!(output_voices_result(
            41000.0,
            Some(38000.0),
            41000.0,
            41200.0
        ));
        // Silence just short of the response boundary: still the same response.
        assert!(!output_voices_result(
            41000.0,
            Some(39500.0),
            41000.0,
            41200.0
        ));
        // No earlier output at all.
        assert!(output_voices_result(41000.0, None, 41000.0, 41200.0));
        // Before the insertion start: never.
        assert!(!output_voices_result(40800.0, None, 41000.0, 41200.0));
        // At or after the insertion end: always.
        assert!(output_voices_result(
            41200.0,
            Some(41100.0),
            41000.0,
            41200.0
        ));
    }

    fn ack_span(client_event_id: &str, start_ms: f64, end_ms: f64) -> Value {
        let mut value = ack(Some(client_event_id));
        value["start_ms"] = json!(start_ms);
        value["end_ms"] = json!(end_ms);
        value
    }

    /// S97 r3 (65294c7ca): the model was finishing "Voice channel ready."
    /// from a response that began before the result landed. Its last delta,
    /// " ready.", spans the insertion's own provider span (23800-24000) and
    /// its sideband frame arrives after the acknowledgement. That tail is not
    /// output since the result: the cue uses the unreported wording.
    #[test]
    fn a_tail_crossing_the_insertion_span_selects_the_unreported_cue() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        state
            .apply_frame(frame(output_delta_span(" channel", 1800.0, 2000.0)))
            .unwrap();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_span(&pending_event_id(result), 2000.0, 2200.0)))
            .unwrap();
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        state
            .apply_frame(frame(output_delta_span(" ready.", 2000.0, 2200.0)))
            .unwrap();
        model_output(&mut state, false, 8);
        drain(&mut state);
        let (_, delegation_id, wording) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("one cue due");
        assert_eq!(delegation_id, "dlg_cue");
        assert!(
            result_cue_text(&wording).starts_with(LIVE_RESULT_UNREPORTED_CUE_HEAD),
            "a delta that started inside the insertion span is pre-insertion speech"
        );
    }

    /// S97 r10 / S106 r1: the response under way when the result landed goes
    /// on to voice it ("I'm here and | ready. And the directory is empty").
    /// Output starting at the insertion's end was generated with the result
    /// in context, so the result gets no cue.
    #[test]
    fn a_response_continuing_past_the_insertion_end_sends_no_cue() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_span(&pending_event_id(result), 2000.0, 2200.0)))
            .unwrap();
        state
            .apply_frame(frame(output_delta_span(" I'm here and", 1800.0, 2200.0)))
            .unwrap();
        state
            .apply_frame(frame(output_delta_span(
                " ready. And the directory is empty.",
                2200.0,
                3400.0,
            )))
            .unwrap();
        model_output(&mut state, false, 8);
        drain(&mut state);
        assert_eq!(state.reserve_due_result_cue().unwrap(), None, "no cue");
        assert_eq!(state.outstanding_receipt_count(), 0);
    }

    /// A result landing into silence is cued at once with the unreported
    /// wording: nothing was said since it landed.
    #[test]
    fn an_immediate_cue_into_silence_uses_the_unreported_wording() {
        let mut state = state_with_spoken_delegation();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 4000.0)))
            .unwrap();
        drain(&mut state);
        let (_, _, wording) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("one cue due");
        assert!(result_cue_text(&wording).starts_with(LIVE_RESULT_UNREPORTED_CUE_HEAD));
    }

    /// A result acknowledged within the release window after the model's last
    /// speech frame is still inside its response: deferred, then cued.
    #[test]
    fn a_result_shortly_after_the_last_speech_frame_waits_for_the_response_end() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        model_output(&mut state, false, 2);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2400.0)))
            .unwrap();
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        model_output(&mut state, false, 6);
        assert_eq!(state.due_result_cues, ["dlg_cue"]);
    }

    /// A model silent for the release before the result lands gets its cue at
    /// once, as before.
    #[test]
    fn a_result_landing_into_settled_silence_is_cued_at_once() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        model_output(&mut state, false, 8);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 4000.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, ["dlg_cue"]);
        assert!(state.deferred_result_cues.is_empty());
    }

    /// Audible user speech holds a deferred cue even when its transcript
    /// takes no floor (here a tail that began before the model's last
    /// output): the floor is read from transcripts, which trail the audio,
    /// and the model's silence while the user speaks is not the end of a
    /// turn (S99 #1630 r3). The cue goes out once the user stops.
    #[test]
    fn audible_user_speech_holds_a_deferred_cue() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
            .unwrap();
        state.apply_frame(frame(input_delta("and also"))).unwrap();
        assert!(!state.user_holds_floor(), "the transcript takes no floor");
        reflect_input(&mut state, true, 4);
        model_output(&mut state, false, 8);
        assert!(
            state.due_result_cues.is_empty(),
            "held while the user is audibly speaking"
        );
        reflect_input(&mut state, false, 7);
        assert!(state.due_result_cues.is_empty(), "1400 ms of quiet");
        reflect_input(&mut state, false, 1);
        assert_eq!(state.due_result_cues, ["dlg_cue"]);
    }

    /// A deferred cue holds no provider receipt, so a close never waits on
    /// it. Closing first means it is never sent.
    #[test]
    fn a_deferred_cue_holds_nothing_close_waits_on() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
            .unwrap();
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        assert_eq!(
            state.outstanding_receipt_count(),
            0,
            "the result is acknowledged and the deferred cue is not an append"
        );
        assert!(state.pending_appends.is_empty());
    }

    #[test]
    fn the_400_ms_gap_result_gets_a_cue_bound_to_its_delegation() {
        // Last word ends at 2000 ms; the result lands at 2400 ms.
        let mut state = state_with_spoken_delegation();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2400.0)))
            .unwrap();
        drain(&mut state);
        let (_, delegation_id, _) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("the 400 ms-gap result is cued, not suppressed");
        assert_eq!(delegation_id, "dlg_cue");
    }

    #[test]
    fn results_are_cued_in_acknowledgement_order() {
        let mut state = state_with_spoken_delegation();
        let first = state
            .reserve_delegation_commentary(Some("dlg_a".to_owned()))
            .unwrap();
        let second = state
            .reserve_delegation_commentary(Some("dlg_b".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(second), 3000.0)))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(first), 3200.0)))
            .unwrap();
        assert_eq!(state.due_result_cues, ["dlg_b", "dlg_a"]);
    }

    #[test]
    fn a_rejected_result_or_cue_surfaces_nothing_extra() {
        let mut state = state_with_spoken_delegation();
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(append_rejected(Some(&pending_event_id(result)))))
            .unwrap();
        assert!(state.due_result_cues.is_empty());
        assert!(state.result_cue_candidates.is_empty());
        drain(&mut state);

        state.due_result_cues.push_back("dlg_cue".to_owned());
        let (cue, _, _) = state.reserve_due_result_cue().unwrap().unwrap();
        state
            .apply_frame(frame(append_rejected(Some(&thinking_event_id(cue, 0)))))
            .unwrap();
        assert!(
            !drain(&mut state).iter().any(|observation| matches!(
                observation,
                GptLiveBrokerObservation::InstructionsContextAppendRejected { .. }
                    | GptLiveBrokerObservation::ThinkingContextAppendRejected { .. }
            )),
            "a rejected cue is not a rejected owner append"
        );
        assert_eq!(state.outstanding_receipt_count(), 0);
    }

    fn result_append(
        state: &mut SessionState,
        delegation: &str,
    ) -> (GptLiveAppendToken, ClientEvent) {
        let token = state
            .reserve_delegation_commentary(Some(delegation.to_owned()))
            .unwrap();
        let event = PublicLiveBrokerSession::commentary_event(
            token,
            "executor result".to_owned(),
            Nullable(Some(delegation.to_owned())),
        );
        (token, event)
    }

    fn released_tokens(state: &mut SessionState) -> Vec<GptLiveAppendToken> {
        state
            .take_releasable_held_commentary()
            .into_iter()
            .map(|(token, _)| token)
            .collect()
    }

    /// One 200 ms reflected input frame (PCM16, 24 kHz): a -12 dBFS tone for
    /// speech, digital zero for silence.
    fn input_audio(speech: bool) -> Value {
        use base64::Engine as _;
        let samples: Vec<u8> = (0..4_800_u32)
            .flat_map(|index| {
                let value = if speech {
                    (8_000.0 * (f64::from(index) * 0.13).sin()) as i16
                } else {
                    0
                };
                value.to_le_bytes()
            })
            .collect();
        json!({"type":"session.input_audio.append",
            "audio": base64::engine::general_purpose::STANDARD.encode(samples)})
    }

    fn reflect_input(state: &mut SessionState, speech: bool, frames: usize) {
        for _ in 0..frames {
            state.apply_frame(frame(input_audio(speech))).unwrap();
        }
    }

    /// S99 pre-merge r3: a cue deferred during the model's response; the
    /// response ends and the user starts a question. The model's output is
    /// silent while the user speaks, so output silence alone released the
    /// cue mid-question and the model delegated the question. The cue now
    /// waits until the user's floor ends, then goes out exactly once.
    #[test]
    fn a_deferred_cue_waits_for_the_users_floor_to_end() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
            .unwrap();
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        // The response ends and the user begins a question right away.
        model_output(&mut state, false, 2);
        reflect_input(&mut state, true, 2);
        state
            .apply_frame(frame(input_delta_at(" which venue did I mention", 2600.0)))
            .unwrap();
        assert!(state.user_holds_floor());
        // The model stays silent through the question: well past the output
        // release, and still no cue.
        for _ in 0..12 {
            model_output(&mut state, false, 1);
            reflect_input(&mut state, true, 1);
        }
        assert!(state.output_silence_run_ms >= OUTPUT_SILENCE_RELEASE_MS);
        assert!(
            state.due_result_cues.is_empty(),
            "no cue while the user holds the floor"
        );
        assert_eq!(state.reserve_due_result_cue().unwrap(), None);
        // The user stops: the floor ends at 1600 ms of reflected silence.
        reflect_input(&mut state, false, 7);
        assert!(state.due_result_cues.is_empty(), "1400 ms: still the floor");
        reflect_input(&mut state, false, 1);
        assert!(!state.user_holds_floor());
        assert_eq!(
            state.due_result_cues,
            ["dlg_cue"],
            "the floor's end releases the deferred cue"
        );
        drain(&mut state);
        let (_, delegation_id, _) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("one cue due");
        assert_eq!(delegation_id, "dlg_cue");
        model_output(&mut state, false, 20);
        assert_eq!(state.reserve_due_result_cue().unwrap(), None, "exactly one");
    }

    /// The floor can also end on the model's own answer to the question.
    /// The deferred cue then waits for that answer's end (output silence),
    /// not for the floor alone. The answer followed the user's question, not
    /// the result, so the result still gets its cue.
    #[test]
    fn a_deferred_cue_after_an_answered_question_waits_for_the_answer_to_end() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
            .unwrap();
        model_output(&mut state, false, 2);
        reflect_input(&mut state, true, 3);
        state
            .apply_frame(frame(input_delta_at(" which venue did I mention", 2600.0)))
            .unwrap();
        assert!(state.user_holds_floor());
        // The model answers: the floor ends on its output.
        state
            .apply_frame(frame(output_delta_span(" Lisbon.", 3400.0, 3800.0)))
            .unwrap();
        model_output(&mut state, true, 2);
        assert!(!state.user_holds_floor());
        assert!(
            state.due_result_cues.is_empty(),
            "the answer is still being voiced"
        );
        // The user is quiet while the model answers.
        reflect_input(&mut state, false, 8);
        model_output(&mut state, false, 7);
        assert!(state.due_result_cues.is_empty(), "1400 ms after the answer");
        model_output(&mut state, false, 1);
        assert_eq!(state.due_result_cues, ["dlg_cue"]);
        // The user's question took the floor after the result's insertion
        // and before the model's first output since: that output answered
        // the user, not the result, so the result's cue is still owed.
        drain(&mut state);
        let (_, delegation_id, wording) = state
            .reserve_due_result_cue()
            .unwrap()
            .expect("the answer to the user does not report the result");
        assert_eq!(delegation_id, "dlg_cue");
        assert!(result_cue_text(&wording).starts_with(LIVE_RESULT_UNREPORTED_CUE_HEAD));
        assert!(wording.user_spoke_first, "owed through the user's turn");
        assert!(
            result_cue_text(&wording).contains(LIVE_RESULT_CUE_USER_SPOKE_FIRST),
            "the cue defers to how the user asked for it to be reported"
        );
    }

    /// S99 on #1630, r3: the model's long readout ended on the audio clock,
    /// the user began the next question, and the readout's transcript tail
    /// was still arriving. Each output delta cleared the user's floor, so the
    /// deferred cue went out mid-question and the question was delegated. A
    /// tail that started before the user's utterance is not an answer: the
    /// floor and the open request stand, and the cue goes out once, after
    /// the user stops, with the question still open.
    #[test]
    fn a_lagging_output_tail_does_not_release_a_cue_into_the_users_question() {
        let mut state = state_with_spoken_delegation();
        model_output(&mut state, true, 3);
        let result = state
            .reserve_delegation_commentary(Some("dlg_cue".to_owned()))
            .unwrap();
        state
            .apply_frame(frame(ack_at(&pending_event_id(result), 2000.0)))
            .unwrap();
        assert_eq!(state.deferred_result_cues, ["dlg_cue"]);
        // The readout's audio ends; the user starts the next question.
        model_output(&mut state, false, 2);
        reflect_input(&mut state, true, 2);
        state
            .apply_frame(frame(input_delta_at(
                " now tell me my vault phrase",
                2600.0,
            )))
            .unwrap();
        assert!(state.user_holds_floor());
        // The readout's transcript tail arrives late: it started at 2200,
        // before the user's utterance at 2600.
        state
            .apply_frame(frame(output_delta_span(" e018/_tmp", 2200.0, 2400.0)))
            .unwrap();
        assert!(state.user_holds_floor(), "a lagging tail is not an answer");
        for _ in 0..10 {
            model_output(&mut state, false, 1);
            reflect_input(&mut state, true, 1);
        }
        assert!(state.output_silence_run_ms >= OUTPUT_SILENCE_RELEASE_MS);
        assert!(
            state.due_result_cues.is_empty(),
            "no cue into the user's question"
        );
        reflect_input(&mut state, false, 7);
        assert!(state.due_result_cues.is_empty(), "1400 ms: still the floor");
        reflect_input(&mut state, false, 1);
        assert_eq!(state.due_result_cues, ["dlg_cue"]);
        assert!(
            state.user_request_open,
            "the question is still open: the tail did not answer it"
        );
        // The tail is the readout of this result itself (it started after
        // the insertion end), so the result was voiced and gets no cue.
        drain(&mut state);
        assert_eq!(state.reserve_due_result_cue().unwrap(), None, "no cue");
    }

    /// S103 r2: the user keeps talking past `session.delegation.created`,
    /// the model never answers, and the user stops. The result held behind
    /// that speech is released once the user's floor ends: 1600 ms of
    /// reflected-input silence on the audio clock, not an answer that never
    /// comes.
    #[test]
    fn a_held_result_is_released_when_the_user_stops_and_the_model_stays_silent() {
        let mut state = state_with_spoken_delegation();
        reflect_input(&mut state, true, 3);
        state
            .apply_frame(frame(input_delta_at(" and also check the room", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(
            state.hold_or_send_commentary(token, event).is_none(),
            "held while the user holds the floor"
        );
        // The user stops; the model stays silent (no output, no delegation).
        reflect_input(&mut state, false, 7);
        assert!(
            released_tokens(&mut state).is_empty(),
            "1400 ms of silence: still the user's floor"
        );
        reflect_input(&mut state, false, 1);
        assert_eq!(
            released_tokens(&mut state),
            [token],
            "the floor ends at 1600 ms of silence and the held result goes out"
        );
    }

    /// A pause inside a sentence, shorter than the release silence, keeps the
    /// hold; the silence run restarts at the next speech frame.
    #[test]
    fn a_pause_shorter_than_the_release_silence_keeps_the_hold() {
        let mut state = state_with_spoken_delegation();
        reflect_input(&mut state, true, 2);
        state
            .apply_frame(frame(input_delta_at(" and then", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_none());
        reflect_input(&mut state, false, 5);
        reflect_input(&mut state, true, 1);
        reflect_input(&mut state, false, 7);
        assert!(
            released_tokens(&mut state).is_empty(),
            "a 1000 ms pause, speech, then 1400 ms: the floor holds"
        );
        reflect_input(&mut state, false, 1);
        assert_eq!(released_tokens(&mut state), [token]);
    }

    /// S106: a backchannel over the model's speech ("mm-hm") overlaps that
    /// output and takes no floor, so commentary is not held behind it.
    #[test]
    fn a_backchannel_over_the_models_speech_takes_no_floor() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(output_delta_span(
                " here is the plan",
                3000.0,
                4000.0,
            )))
            .unwrap();
        reflect_input(&mut state, true, 1);
        state
            .apply_frame(frame(input_delta_span("mm-hm", 3500.0, 3700.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(
            state.hold_or_send_commentary(token, event).is_some(),
            "sent at once: a backchannel is not a user turn"
        );
    }

    /// Reflected audio energy alone (noise, no transcript) never opens a
    /// floor that would then need releasing.
    #[test]
    fn audio_energy_without_a_transcript_opens_no_floor() {
        let mut state = state_with_spoken_delegation();
        reflect_input(&mut state, true, 10);
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_some());
    }

    /// A delta whose speech was already followed by the full release silence
    /// (transcription lagging the audio) takes no floor.
    #[test]
    fn a_late_delta_after_the_release_silence_takes_no_floor() {
        let mut state = state_with_spoken_delegation();
        reflect_input(&mut state, true, 2);
        reflect_input(&mut state, false, 8);
        state
            .apply_frame(frame(input_delta_at(" thanks", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_some());
    }

    #[test]
    fn reflected_frame_energy_is_measured_in_dbfs() {
        use base64::Engine as _;
        let encode = |samples: &[i16]| {
            base64::engine::general_purpose::STANDARD.encode(
                samples
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect::<Vec<u8>>(),
            )
        };
        assert_eq!(
            reflected_pcm16_dbfs(&encode(&[0; 8])),
            Some(f64::NEG_INFINITY)
        );
        let full = reflected_pcm16_dbfs(&encode(&[i16::MAX, i16::MIN, i16::MAX, i16::MIN]))
            .expect("decodes");
        assert!(full > -0.1 && full <= 0.1, "full scale is 0 dBFS: {full}");
        assert_eq!(reflected_pcm16_dbfs("not base64!"), None);
    }

    /// S100: a result landing while the user's request is still unanswered
    /// is held, and the request's own delegation releases it.
    #[test]
    fn a_result_mid_utterance_is_held_then_released_by_delegation_created() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(input_delta_at("and also order a taxi", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(
            state.hold_or_send_commentary(token, event).is_none(),
            "held behind the unanswered utterance"
        );
        assert!(
            released_tokens(&mut state).is_empty(),
            "nothing releases it yet"
        );
        state
            .apply_frame(frame(delegation_created("dlg_taxi", "client")))
            .unwrap();
        assert_eq!(released_tokens(&mut state), [token]);
        assert!(state.held_commentary.is_empty());
    }

    #[test]
    fn a_held_result_is_released_by_the_models_next_output() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(input_delta_at("what time is it", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_none());
        state
            .apply_frame(frame(output_delta_span("it is noon", 4000.0, 4600.0)))
            .unwrap();
        assert_eq!(released_tokens(&mut state), [token]);
    }

    #[test]
    fn a_hold_follows_the_newest_utterance_while_the_model_stays_silent() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(input_delta_at("thanks", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_none());
        // The model stays silent; the user speaks again.
        state
            .apply_frame(frame(input_delta("are you there")))
            .unwrap();
        assert!(
            released_tokens(&mut state).is_empty(),
            "another unanswered utterance keeps the hold"
        );
        state
            .apply_frame(frame(output_delta_span("yes", 6000.0, 6200.0)))
            .unwrap();
        assert_eq!(released_tokens(&mut state), [token]);
    }

    /// S106 sign-off: the model never answers, and the channel closes. The
    /// held result is never sent; its reservation stays pending for the
    /// owner's close settlement (interrupted by close).
    #[test]
    fn a_silent_model_then_close_leaves_the_held_result_unsent_for_close_settlement() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(input_delta_at("bye", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_none());
        state.apply_frame(frame(session_closed())).unwrap();
        assert!(
            state.held_commentary.is_empty(),
            "never sent after the close"
        );
        assert!(released_tokens(&mut state).is_empty());
        assert!(
            state
                .pending_appends
                .iter()
                .any(|pending| pending.token == token),
            "the reservation stays pending for the close settlement"
        );
        // A result appended after the close is never held.
        let (late, event) = result_append(&mut state, "dlg_late");
        assert!(state.hold_or_send_commentary(late, event).is_some());
    }

    #[test]
    fn a_close_request_drops_held_commentary_unsent() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(input_delta_at("bye", 2500.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_none());
        state.close_requested = true;
        state.drop_held_commentary_for_close();
        assert!(state.held_commentary.is_empty());
        assert!(released_tokens(&mut state).is_empty());
    }

    #[test]
    fn an_answered_utterance_does_not_hold_the_result() {
        // Answered by output.
        let mut state = state_with_spoken_delegation();
        let (token, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(token, event).is_some());
        // Answered by its delegation before the result arrives.
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("book a table")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_book", "client")))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_prev");
        assert!(state.hold_or_send_commentary(token, event).is_some());
    }

    /// S101: a narration ("Finished voice request") appended while the user
    /// is still speaking the next request diverted the model ("Okay.") and
    /// the request was never delegated. Narration is held like a result, and
    /// the request's own delegation releases it.
    #[test]
    fn a_narration_mid_utterance_is_held_then_released_by_delegation_created() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(input_delta_at(
                "and then the second slow job",
                2500.0,
            )))
            .unwrap();
        let narration = state.reserve_delegation_commentary(None).unwrap();
        let event = PublicLiveBrokerSession::commentary_event(
            narration,
            "Finished voice request".to_owned(),
            Nullable(Some("dlg_cue".to_owned())),
        );
        assert!(
            state.hold_or_send_commentary(narration, event).is_none(),
            "narration is held behind the unanswered utterance"
        );
        // A result appended after it queues behind it.
        let (result, event) = result_append(&mut state, "dlg_cue");
        assert!(state.hold_or_send_commentary(result, event).is_none());
        assert!(released_tokens(&mut state).is_empty());
        state
            .apply_frame(frame(delegation_created("dlg_job2", "client")))
            .unwrap();
        assert_eq!(released_tokens(&mut state), [narration, result]);
    }

    /// S103 run 1 (soak 35728bf0): the model said "I'll use Friday" 5 ms
    /// before the Friday result arrived, then took the old cue's "if you
    /// already have, do not repeat it" as satisfied and never voiced the
    /// executor's confirmation. The cue anchors to the delivery: speech
    /// before it is not a report, and it offers no "already reported"
    /// exception (a cue is only sent when nothing was said since the result).
    #[test]
    fn the_result_cue_anchors_to_the_delivery() {
        let cue = result_cue_text(&ResultCueWording {
            awaiting_peer_replies: false,
            user_request_open: true,
            outstanding: None,
            user_spoke_first: false,
        });
        assert!(
            cue.contains(
                "anything you said about this request before now was said before it was done"
            ),
            "speech before the delivery is not a report of the result"
        );
        assert!(
            cue.contains("tell the user the actual outcome of this result"),
            "the model confirms the actual outcome now"
        );
        assert!(
            cue.contains("If the user's latest request is still unanswered, answer it first"),
            "a still-open user request comes first"
        );
        assert!(
            !cue.contains("unless"),
            "no exception: the cue is sent only when nothing was said since"
        );
        assert!(
            cue.contains(
                "Report only what the result itself says: when it says someone else was asked, their answer is still pending"
            ),
            "an \"I asked them\" result is not their answer (S102 r2)"
        );
        assert!(
            !cue.contains("If you already have"),
            "the unanchored dedup clause is gone"
        );
    }

    /// Broker-owned appends are tagged by kind, so their receipts are
    /// logged under their own name ("in-progress notice acknowledged", not
    /// "result cue acknowledged").
    #[test]
    fn broker_owned_appends_carry_their_kind() {
        let mut state = SessionState::default();
        state.due_result_cues.push_back("dlg_cue".to_owned());
        state
            .due_progress_notices
            .push_back("dlg_notice".to_owned());
        let (cue, _, _) = state.reserve_due_result_cue().unwrap().expect("cue due");
        let (notice, _, _) = state
            .reserve_due_progress_notice()
            .unwrap()
            .expect("notice due");
        let kind_of = |token| {
            state
                .pending_appends
                .iter()
                .find(|pending| pending.token == token)
                .and_then(|pending| pending.internal)
        };
        assert_eq!(kind_of(cue), Some(InternalAppend::ResultCue));
        assert_eq!(kind_of(notice), Some(InternalAppend::ProgressNotice));
        assert_eq!(InternalAppend::ResultCue.label(), "result cue");
        assert_eq!(InternalAppend::ProgressNotice.label(), "in-progress notice");
    }

    /// Measured rule (S99): a context text that names delegating as something
    /// to avoid ("without lookup, tool, or delegate") primes gpt-live-1 to
    /// delegate recall questions. #1588's notice carried exactly that phrase
    /// and S99 went from 0/5 to 3/5 delegated recalls (BuildBuddy c43aa3db).
    /// The broker's startup and context texts state what the model knows
    /// positively and never mention delegation as something to avoid.
    #[test]
    fn no_startup_or_context_text_names_delegation_as_something_to_avoid() {
        for (name, text) in [
            ("LIVE_PENDING_CONTEXT_NOTICE", LIVE_PENDING_CONTEXT_NOTICE),
            (
                "LIVE_PENDING_CONTEXT_AFTER_RECENT_NOTICE",
                LIVE_PENDING_CONTEXT_AFTER_RECENT_NOTICE,
            ),
        ] {
            assert_eq!(names_delegation_as_avoided(text), None, "{name}: {text}");
        }
    }

    fn names_delegation_as_avoided(text: &str) -> Option<&'static str> {
        let lower = text.to_lowercase();
        [
            "or delegat",
            "nor delegat",
            "not delegat",
            "never delegat",
            "without delegat",
            "instead of delegat",
            "no delegat",
            "don't delegat",
            "avoid delegat",
        ]
        .into_iter()
        .find(|phrase| lower.contains(phrase))
    }

    #[test]
    fn the_in_progress_notice_forbids_describing_the_request_as_done() {
        assert!(LIVE_DELEGATION_IN_PROGRESS.contains("in progress"));
        assert!(LIVE_DELEGATION_IN_PROGRESS.contains(
            "Until its result arrives, do not say or imply that it is done and do not state its outcome"
        ));
        assert!(
            LIVE_DELEGATION_IN_PROGRESS.contains("you may say that you are working on it"),
            "the model can still acknowledge the request"
        );
        assert!(LIVE_DELEGATION_IN_PROGRESS.len() <= CONTEXT_FRAGMENT_MAX_BYTES);
    }

    /// The cue for a result follows whether its work still awaits members'
    /// answers: the awaiting cue tells the model to say only that it asked.
    #[test]
    fn the_result_cue_is_selected_by_whether_peer_answers_are_pending() {
        let wording = |awaiting_peer_replies| ResultCueWording {
            awaiting_peer_replies,
            user_request_open: true,
            outstanding: None,
            user_spoke_first: false,
        };
        assert!(result_cue_text(&wording(false)).starts_with(LIVE_RESULT_UNREPORTED_CUE_HEAD));
        let awaiting = result_cue_text(&wording(true));
        assert!(awaiting.starts_with(LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_HEAD));
        for cue in [result_cue_text(&wording(false)), awaiting.clone()] {
            assert!(
                !cue.contains("unless"),
                "a result the model has not spoken since gets no exception"
            );
        }
        assert!(
            awaiting.contains(
                "it reports that someone was asked, and their answer has not arrived yet"
            )
        );
        assert!(
            awaiting.contains("tell the user now that you asked"),
            "the result's outcome is that the member was asked"
        );
        assert!(
            awaiting.contains("do not state, guess, or imply their answer"),
            "no invented answer (S102 r2)"
        );
        assert!(
            awaiting.contains("If the user's latest request is still unanswered, answer it first"),
            "a still-open user request comes first, as for any result"
        );
        assert!(
            !awaiting.contains("actual outcome"),
            "the awaiting cue never asks for an outcome the result does not carry"
        );
    }

    /// Every cue variant, with or without an open request, is exactly its
    /// head, the open-request clause when a request is open, its action, its
    /// tail, and the scope sentence last (S99: the cue's delegation framing
    /// persisted into the next question).
    #[test]
    fn every_result_cue_variant_closes_with_its_scope_and_route() {
        let variants = [
            (
                false,
                LIVE_RESULT_UNREPORTED_CUE_HEAD,
                LIVE_RESULT_UNREPORTED_CUE_ACTION,
                Some(LIVE_RESULT_CUE_TAIL),
            ),
            (
                true,
                LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_HEAD,
                LIVE_RESULT_AWAITING_PEER_UNREPORTED_CUE_ACTION,
                None,
            ),
        ];
        for (awaiting_peer_replies, head, action, tail) in variants {
            for user_request_open in [true, false] {
                let wording = ResultCueWording {
                    awaiting_peer_replies,
                    user_request_open,
                    outstanding: None,
                    user_spoke_first: false,
                };
                let cue = result_cue_text(&wording);
                let open = if user_request_open {
                    format!("{LIVE_RESULT_CUE_OPEN_REQUEST} Then tell")
                } else {
                    "Tell".to_owned()
                };
                let tail = tail.map(|tail| format!(" {tail}")).unwrap_or_default();
                assert_eq!(
                    cue,
                    format!("{head} {open} {action}{tail} {LIVE_RESULT_CUE_SCOPE}")
                );
                assert!(cue.ends_with(LIVE_RESULT_CUE_SCOPE), "the scope is last");
                assert_eq!(
                    cue.contains(LIVE_RESULT_CUE_OPEN_REQUEST),
                    user_request_open,
                    "the open-request clause only while a request is open"
                );
                // One append fragment, except the outcome cue with an open
                // request, which splits before the scope sentence.
                let fragments = result_cue_fragments(&wording);
                let split = !awaiting_peer_replies && user_request_open;
                assert_eq!(fragments.len(), if split { 2 } else { 1 });
                assert_eq!(fragments.join(" "), cue, "fragments rejoin to the cue");
                assert!(
                    fragments
                        .iter()
                        .all(|fragment| fragment.len() <= CONTEXT_FRAGMENT_MAX_BYTES)
                );
                if split {
                    assert_eq!(fragments[1], LIVE_RESULT_CUE_SCOPE, "split at the sentence");
                }
            }
        }
        assert_eq!(
            LIVE_RESULT_CUE_SCOPE,
            "Only this result: answer questions about this conversation yourself."
        );
    }

    /// The open-request clause is a typed send-time fact: an utterance that
    /// takes the floor opens a request; the model's output or a delegation
    /// answers it; reflected-input silence ends the floor but leaves an
    /// unanswered request open (S103 r2).
    #[test]
    fn the_open_request_clause_follows_whether_the_latest_request_was_answered() {
        let cue_open = |state: &mut SessionState| {
            state.due_result_cues.push_back("dlg_cue".to_owned());
            let (_, _, wording) = state.reserve_due_result_cue().unwrap().unwrap();
            drain(state);
            wording.user_request_open
        };
        let mut state = state_with_spoken_delegation();
        assert!(!cue_open(&mut state), "the delegation answered the request");
        reflect_input(&mut state, true, 2);
        state
            .apply_frame(frame(input_delta_at(" which venue did I mention", 2600.0)))
            .unwrap();
        assert!(cue_open(&mut state), "a new utterance holds the floor");
        reflect_input(&mut state, false, 8);
        assert!(!state.user_holds_floor(), "silence ended the floor");
        assert!(
            cue_open(&mut state),
            "the request the model never answered is still open"
        );
        state
            .apply_frame(frame(output_delta_span(" Lisbon.", 4400.0, 4800.0)))
            .unwrap();
        assert!(!cue_open(&mut state), "the model's answer closes it");
        reflect_input(&mut state, true, 2);
        state
            .apply_frame(frame(input_delta_at(" and book a table", 5400.0)))
            .unwrap();
        state
            .apply_frame(frame(delegation_created_at("dlg_two", "client", 5600.0)))
            .unwrap();
        assert!(!cue_open(&mut state), "a delegation closes it too");
    }

    /// The notice ahead of such a result names the members asked, holds
    /// their answer back until it arrives, and stays one append however many
    /// members or however long their labels.
    #[test]
    fn the_peer_reply_pending_notice_names_who_was_asked_and_withholds_their_answer() {
        let one = peer_reply_pending_notice(&["analyst-pemberton".to_string()]);
        assert_eq!(
            one,
            "The result that follows reports asking analyst-pemberton. Their answer has not arrived yet. Until it arrives as its own update, tell the user only that you asked: do not state, guess, or imply what they said."
        );
        let two = peer_reply_pending_notice(&["ana".to_string(), "bo".to_string()]);
        assert!(two.contains("reports asking ana and bo."));
        let many: Vec<String> = (0..6)
            .map(|index| "x".repeat(200) + &index.to_string())
            .collect();
        let notice = peer_reply_pending_notice(&many);
        assert!(notice.contains("3 other member(s)"));
        assert!(notice.len() <= CONTEXT_FRAGMENT_MAX_BYTES, "one append");
    }

    /// A result marked as awaiting members' answers gets the awaiting cue at
    /// its acknowledgement; an ordinary result keeps the ordinary cue.
    #[test]
    fn an_acknowledged_result_awaiting_peer_answers_gets_the_awaiting_cue() {
        for awaiting in [true, false] {
            let mut state = SessionState::default();
            let token = state
                .reserve_delegation_commentary(Some("dlg_peer".to_string()))
                .expect("result reserved");
            if awaiting {
                state.awaiting_peer_results.insert("dlg_peer".to_string());
            }
            let receipt = pending_event_id(token);
            state
                .acknowledge_append(AppendReceiptKind::Commentary, Some(receipt.as_str()))
                .expect("result acknowledged");
            let (_, delegation_id, wording) = state
                .reserve_due_result_cue()
                .expect("cue reserved")
                .expect("cue due");
            assert_eq!(delegation_id, "dlg_peer");
            assert_eq!(wording.awaiting_peer_replies, awaiting);
            assert!(
                state.awaiting_peer_results.is_empty(),
                "consumed by its cue"
            );
        }
    }

    /// S102 r2: the delegation completed with "I asked Analyst Pemberton
    /// what time they think it is", and the model voiced an invented answer
    /// from Pemberton 6.7 s before the real one arrived. The notice, already
    /// in context when such a result lands, keeps the member's answer out of
    /// that result and asks for the correction once it arrives.
    #[test]
    fn the_in_progress_notice_keeps_a_peers_answer_pending_until_it_arrives() {
        assert!(LIVE_DELEGATION_IN_PROGRESS.contains(
            "If its result says someone else was asked, their answer is not part of that result"
        ));
        assert!(LIVE_DELEGATION_IN_PROGRESS.contains(
            "do not state, guess, or imply what they said until their answer arrives as its own update"
        ));
        assert!(
            LIVE_DELEGATION_IN_PROGRESS.contains(
                "then tell the user what they actually said, correcting anything said before"
            ),
            "the real answer corrects an earlier claim"
        );
    }

    /// Every client `session.delegation.created` makes one in-progress
    /// notice due, bound to that delegation; a non-client delegation does
    /// not. The notice is broker-owned: its reservation is internal.
    #[test]
    fn a_client_delegation_makes_one_in_progress_notice_due() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta("move it to friday")))
            .unwrap();
        state
            .apply_frame(frame(delegation_created("dlg_friday", "client")))
            .unwrap();
        assert_eq!(state.due_progress_notices, ["dlg_friday"]);
        let (token, delegation, _) = state
            .reserve_due_progress_notice()
            .unwrap()
            .expect("notice due");
        assert_eq!(delegation, "dlg_friday");
        assert!(state.due_progress_notices.is_empty());
        assert!(
            state
                .pending_appends
                .iter()
                .any(|pending| pending.token == token
                    && pending.internal == Some(InternalAppend::ProgressNotice)),
            "broker-owned: its receipt is consumed by the broker"
        );
        state
            .apply_frame(frame(delegation_created("dlg_responses", "responses")))
            .unwrap();
        assert!(
            state.due_progress_notices.is_empty(),
            "no notice for a non-client delegation"
        );
    }

    /// A transcription tail delivered after the model's response, but whose
    /// speech began before it, belongs to the answered utterance.
    #[test]
    fn a_late_transcription_tail_is_not_an_unanswered_utterance() {
        let mut state = SessionState::default();
        state
            .apply_frame(frame(input_delta_span("book a table", 0.0, 500.0)))
            .unwrap();
        state
            .apply_frame(frame(delegation_created_at("dlg_book", "client", 600.0)))
            .unwrap();
        state
            .apply_frame(frame(output_delta_span("on it", 900.0, 1500.0)))
            .unwrap();
        state
            .apply_frame(frame(input_delta_span(" for two", 600.0, 1200.0)))
            .unwrap();
        let (token, event) = result_append(&mut state, "dlg_prev");
        assert!(
            state.hold_or_send_commentary(token, event).is_some(),
            "the late tail is part of the answered utterance"
        );
    }

    #[test]
    fn held_commentary_release_in_append_order() {
        let mut state = state_with_spoken_delegation();
        state
            .apply_frame(frame(input_delta_at("one more thing", 2500.0)))
            .unwrap();
        let (first, event) = result_append(&mut state, "dlg_a");
        assert!(state.hold_or_send_commentary(first, event).is_none());
        let (second, event) = result_append(&mut state, "dlg_b");
        assert!(state.hold_or_send_commentary(second, event).is_none());
        state
            .apply_frame(frame(output_delta_span("sure", 5000.0, 5200.0)))
            .unwrap();
        assert_eq!(released_tokens(&mut state), [first, second]);
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

    /// S102 r2's shape, deterministically: a result whose work asked
    /// analyst-pemberton, whose answer has not arrived. The provider gets the
    /// pending-answer notice, bound to the delegation, strictly before the
    /// A result held behind the user's unanswered utterance and released by
    /// provider ordering (`session.delegation.created`) is recorded as a
    /// client event, after the server frame that released it, exactly like a
    /// result sent at once (Turbo S S101 93b6aaec R2: the quick result was
    /// held and released but missing from the recording, so the
    /// result-timing oracle saw no result at all).
    #[cfg(feature = "test-realtime-fixtures")]
    #[tokio::test]
    async fn a_held_then_released_result_is_recorded_as_a_client_event() {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        let held = Arc::new(tokio::sync::Notify::new());
        let server_held = Arc::clone(&held);
        let attach = move |State(capture): State<SharedCapture>, upgrade: WebSocketUpgrade| {
            let held = Arc::clone(&server_held);
            async move {
                upgrade.on_upgrade(move |mut socket| async move {
                    send_json(&mut socket, input_delta("book a table")).await;
                    send_json(&mut socket, delegation_created("dlg_cue", "client")).await;
                    send_json(&mut socket, output_delta_span("one moment", 1500.0, 2000.0)).await;
                    send_json(&mut socket, input_delta_at("and also order a taxi", 2500.0)).await;
                    // The result is now appended and held behind that
                    // utterance; the next delegation answers it.
                    held.notified().await;
                    send_json(&mut socket, delegation_created("dlg_taxi", "client")).await;
                    loop {
                        let event = recv_json(&mut socket, &capture).await;
                        match event["type"].as_str() {
                            Some("session.commentary.append") => {
                                send_json(&mut socket, ack(event["event_id"].as_str())).await;
                            }
                            Some("session.thinking.append" | "session.instructions.append") => {
                                let mut receipt = ack(event["event_id"].as_str());
                                receipt["type"] =
                                    json!(if event["type"] == "session.thinking.append" {
                                        "session.thinking.appended"
                                    } else {
                                        "session.instructions.appended"
                                    });
                                send_json(&mut socket, receipt).await;
                            }
                            Some("session.close") => {
                                send_json(&mut socket, session_closed()).await;
                                break;
                            }
                            _ => {}
                        }
                    }
                })
            }
        };
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach))
            .with_state(Arc::clone(&capture));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider-stream.jsonl");
        let recorder = provider_recording::Recorder::create(&path).unwrap();
        // The client captures the task's recorder when it is constructed.
        let (_, session) = recorder
            .scope(async {
                PublicLiveBrokerFactory::__try_from_target_with_base_url(
                    realtime_target("gpt-live-1", OpenAiBackendKind::OpenAiApi),
                    &format!("http://{address}/v1/"),
                )
                .unwrap()
                .open(PublicLiveOpenConfig::new("v=0", "marin").unwrap())
                .await
            })
            .await
            .unwrap()
            .into_parts();
        loop {
            if let Some(GptLiveBrokerObservation::UserTranscriptFragment { text, .. }) =
                session.next_observation().await.unwrap()
                && text.contains("taxi")
            {
                break;
            }
        }
        let delegation = GptLiveDelegationRef("dlg_cue".to_string());
        let token = session
            .append_delegation_result(&delegation, "Table booked for two.")
            .await
            .expect("result appended");
        held.notify_one();
        loop {
            if let Some(GptLiveBrokerObservation::DelegationContextAppendAcknowledged {
                token: acked,
            }) = session.next_observation().await.unwrap()
                && acked == token
            {
                break;
            }
        }
        session.close().await.expect("close requested");
        while session.next_observation().await.unwrap().is_some() {}
        server.abort();

        let lines = provider_recording::read(&path).unwrap();
        let released_at = lines.iter().position(|line| {
            matches!(&line.entry, provider_recording::Entry::ServerFrame { raw }
                if raw["type"] == "session.delegation.created"
                    && raw["delegation"]["id"] == "dlg_taxi")
        });
        let recorded_at = lines.iter().position(|line| {
            matches!(&line.entry, provider_recording::Entry::ClientEvent { event }
                if event["type"] == "session.commentary.append"
                    && event["content"] == "Table booked for two.")
        });
        let (Some(released_at), Some(recorded_at)) = (released_at, recorded_at) else {
            panic!("the released result must be recorded: {lines:?}");
        };
        assert!(
            released_at < recorded_at,
            "recorded after the frame that released it"
        );
        let acked_at = lines.iter().position(|line| {
            matches!(&line.entry, provider_recording::Entry::ServerFrame { raw }
                if raw["type"] == "session.commentary.appended")
        });
        assert!(acked_at.is_some_and(|acked| recorded_at < acked));
    }

    /// result that says "I asked", and the result's cue is the awaiting cue.
    #[tokio::test]
    async fn a_result_awaiting_a_peer_answer_is_preceded_by_the_pending_notice() {
        let capture = Arc::new(std::sync::Mutex::new(Capture::default()));
        let attach_peer = move |State(capture): State<SharedCapture>, upgrade: WebSocketUpgrade| async move {
            upgrade.on_upgrade(move |mut socket| async move {
                let notice = recv_json(&mut socket, &capture).await;
                assert_eq!(notice["type"], "session.instructions.append");
                assert_eq!(notice["delegation_id"], "dlg_peer");
                assert_eq!(
                    notice["content"],
                    peer_reply_pending_notice(&["analyst-pemberton".to_string()])
                );
                let result = recv_json(&mut socket, &capture).await;
                assert_eq!(result["type"], "session.commentary.append");
                assert_eq!(result["delegation_id"], "dlg_peer");
                assert_eq!(
                    result["content"],
                    "I asked analyst-pemberton what time they think it is."
                );
                let mut notice_ack = ack(notice["event_id"].as_str());
                notice_ack["type"] = json!("session.instructions.appended");
                send_json(&mut socket, notice_ack).await;
                send_json(&mut socket, ack(result["event_id"].as_str())).await;
                let cue = recv_json(&mut socket, &capture).await;
                // The cue is a thinking append: instructions persist (S99).
                assert_eq!(cue["type"], "session.thinking.append");
                assert_eq!(cue["delegation_id"], "dlg_peer");
                // Nothing was spoken after the result landed: the awaiting
                // cue carries no "already said so" exception.
                assert_eq!(
                    cue["content"],
                    result_cue_text(&ResultCueWording {
                        awaiting_peer_replies: true,
                        user_request_open: false,
                        outstanding: None,
                        user_spoke_first: false,
                    })
                );
                let mut cue_ack = ack(cue["event_id"].as_str());
                cue_ack["type"] = json!("session.thinking.appended");
                send_json(&mut socket, cue_ack).await;
                let mute = recv_json(&mut socket, &capture).await;
                assert_eq!(mute["type"], "session.input_audio.mute");
                let close = recv_json(&mut socket, &capture).await;
                assert_eq!(close["type"], "session.close");
                send_json(&mut socket, session_closed()).await;
            })
        };
        let app = Router::new()
            .route("/v1/live/sessions", post(create_session))
            .route("/v1/live/sessions/{session_id}/attach", get(attach_peer))
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
        let delegation = GptLiveDelegationRef("dlg_peer".to_string());
        let token = session
            .append_delegation_result_awaiting_peer_replies(
                &delegation,
                "I asked analyst-pemberton what time they think it is.",
                vec!["analyst-pemberton".to_string()],
            )
            .await
            .expect("result appended");
        // The notice's receipt is broker-owned; only the result's surfaces.
        assert!(matches!(
            session.next_observation().await.unwrap(),
            Some(GptLiveBrokerObservation::DelegationContextAppendAcknowledged { token: acked })
                if acked == token
        ));
        session.close().await.expect("close requested");
        while session.next_observation().await.unwrap().is_some() {}
        let events = capture.lock().expect("capture lock").client_events.clone();
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| event["type"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            kinds,
            [
                "session.instructions.append",
                "session.commentary.append",
                "session.thinking.append",
                "session.input_audio.mute",
                "session.close",
            ],
            "notice, then the result, then its cue"
        );
        server.abort();
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
        expect_progress_notice(&mut socket, &capture, "dlg_public").await;
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

    /// The broker's in-progress notice for a just-created client delegation:
    /// one instructions append bound to it, acknowledged like the provider
    /// does.
    async fn expect_progress_notice(
        socket: &mut WebSocket,
        capture: &SharedCapture,
        delegation: &str,
    ) {
        let notice = recv_json(socket, capture).await;
        assert_eq!(notice["type"], "session.instructions.append");
        assert_eq!(notice["delegation_id"], delegation);
        assert_eq!(notice["content"], LIVE_DELEGATION_IN_PROGRESS);
        send_json(socket, json!({"type":"session.instructions.appended","event_id":"n1","client_event_id":notice["event_id"],"start_ms":1.0,"end_ms":1.0})).await;
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
        assert_eq!(events.len(), 5);
        assert_eq!(events[0]["content"], "{\"canonical_messages\":[]}");
        // The delegation's in-progress notice, bound to it, then its result.
        assert_eq!(events[1]["type"], "session.instructions.append");
        assert_eq!(events[1]["content"], LIVE_DELEGATION_IN_PROGRESS);
        assert_eq!(events[2]["content"], "Table booked for two.");
        assert!(
            events[..3]
                .iter()
                .all(|event| event["event_id"].is_string())
        );
        // Close mutes input first so a pending quiet append can be injected
        // and the provider can confirm closure.
        assert_eq!(events[3]["type"], "session.input_audio.mute");
        assert_eq!(events[4]["type"], "session.close");
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
            expect_progress_notice(&mut socket, &capture, "dlg_pause").await;
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

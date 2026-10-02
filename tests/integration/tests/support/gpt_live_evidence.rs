//! Opt-in S99 diagnostic evidence. This is not playback or context authority.
//! One private, continuously flushed file lives outside the scenario TempDir.
//! Any Fault invalidates the entire journal, including an earlier Passed row.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use meerkat::experimental_gpt_live::provider_recording;
use meerkat::experimental_gpt_live::thinking_capture;
use serde::{Deserialize, Serialize};

use super::{AudioEvidence, TimelineEntry};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Setup,
    Opening,
    Connected,
    InitialUnknown,
    TypedCorrection,
    SpokenCorrection,
    DelegatedWork,
    ReleasingSummary,
    ProviderAcknowledged,
    HistoricalRecall,
    CurrentFactsRecall,
    Closing,
    Reopening,
    ObsoleteJobRelease,
    ReplacementUnknown,
    ReplacementRecall,
    Finished,
    // S100 morning standup.
    StandupSilence,
    StandupOpen,
    StandupDelegation,
    StandupBargeIn,
    StandupReadback,
    StandupFarewell,
    // S102 who are you.
    WhoAreYouCapabilities,
    WhoAreYouRoster,
    WhoAreYouAsk,
    // S103 interrupt and recover.
    InterruptMonologue,
    InterruptBargeIn,
    // S107 stuck close convergence.
    StuckCloseJob,
    StuckCloseCut,
    StuckCloseJobCommit,
    // S104 handoff voice -> typed -> voice.
    HandoffJob,
    HandoffClose,
    HandoffTyped,
    HandoffBack,
    // Shared: a silence hold right after an open or reopen (greeting check).
    SilenceHold,
    // S101 busy backend.
    BusyJobs,
    // S105 fork and merge.
    ForkRequests,
    ForkCorrection,
    // S106 long haul.
    HaulExchanges,
    HaulHold,
    HaulReopen,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    TimedOut,
    CancelledOrPanicked,
    /// The provider's input processing ran degraded during the run (see
    /// [`provider_degradation_verdict`]): the run is void, neither green nor
    /// red, and a [`Record::ProviderDegraded`] names the cause.
    ProviderDegraded,
}

/// Turbo S provider-degradation rule: one exchange whose speech end to input
/// final lag is at least this is provider-degraded evidence.
pub const PROVIDER_DEGRADED_EXCHANGE_LAG_MS: i64 = 10_000;
/// Turbo S provider-degradation rule: a run whose speech end to input final
/// lag p90 exceeds this is provider-degraded evidence.
pub const PROVIDER_DEGRADED_P90_LAG_MS: i64 = 2_000;

/// Why a run was classified provider-degraded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderDegradationCause {
    /// One exchange's speech end to input final lag reached
    /// [`PROVIDER_DEGRADED_EXCHANGE_LAG_MS`].
    ExchangeLag { lag_ms: i64 },
    /// The run's lag p90 exceeded [`PROVIDER_DEGRADED_P90_LAG_MS`].
    LagP90 { p90_ms: i64 },
    /// An exchange never reached its input final while the provider's own
    /// measured input backlog was at least
    /// [`PROVIDER_DEGRADED_EXCHANGE_LAG_MS`]: the provider received the
    /// speech and was behind processing it. A timeout without that provider
    /// evidence is not degradation; it stays a failure.
    TimedOutBehindProviderBacklog { backlog_ms: u64 },
}

/// The provider input latency read from `live/status` when an exchange timed
/// out (all `None` when the provider reported no measurement).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProviderInputLatencyAtTimeout {
    pub backlog_ms: Option<u64>,
    pub reflected_input_clock_ms: Option<u64>,
    pub reflected_clock_since_reading_ms: Option<u64>,
}

/// The typed provider-degraded verdict for one run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDegradation {
    /// The exchange that established the verdict.
    pub exchange: String,
    pub cause: ProviderDegradationCause,
    /// Lag p90 over the exchanges that reached an input final.
    pub p90_ms: Option<i64>,
    /// The provider's measured input backlog at the timeout, when one was
    /// read.
    pub provider_input_backlog_ms: Option<u64>,
}

/// Classify a run from its own evidence: the speech end to input final lag
/// of every exchange that reached its final, plus the exchange that timed
/// out before its final (if any) with the provider input backlog read at
/// that moment. `None` is a valid (healthy) run.
pub fn provider_degradation_verdict(
    lags: &[(String, i64)],
    timed_out: Option<(&str, Option<u64>)>,
) -> Option<ProviderDegradation> {
    let mut sorted: Vec<i64> = lags.iter().map(|(_, lag)| *lag).collect();
    sorted.sort_unstable();
    let p90_ms =
        (!sorted.is_empty()).then(|| sorted[((sorted.len() * 9) / 10).min(sorted.len() - 1)]);
    let backlog_ms = timed_out.and_then(|(_, backlog)| backlog);
    if let Some((exchange, lag_ms)) = lags
        .iter()
        .find(|(_, lag)| *lag >= PROVIDER_DEGRADED_EXCHANGE_LAG_MS)
    {
        return Some(ProviderDegradation {
            exchange: exchange.clone(),
            cause: ProviderDegradationCause::ExchangeLag { lag_ms: *lag_ms },
            p90_ms,
            provider_input_backlog_ms: backlog_ms,
        });
    }
    if let Some(p90) = p90_ms.filter(|p90| *p90 > PROVIDER_DEGRADED_P90_LAG_MS) {
        let exchange = lags
            .iter()
            .find(|(_, lag)| *lag == p90)
            .map(|(exchange, _)| exchange.clone())
            .unwrap_or_default();
        return Some(ProviderDegradation {
            exchange,
            cause: ProviderDegradationCause::LagP90 { p90_ms: p90 },
            p90_ms,
            provider_input_backlog_ms: backlog_ms,
        });
    }
    match timed_out {
        Some((exchange, Some(backlog)))
            if backlog >= PROVIDER_DEGRADED_EXCHANGE_LAG_MS.unsigned_abs() =>
        {
            Some(ProviderDegradation {
                exchange: exchange.to_owned(),
                cause: ProviderDegradationCause::TimedOutBehindProviderBacklog {
                    backlog_ms: backlog,
                },
                p90_ms,
                provider_input_backlog_ms: Some(backlog),
            })
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preparation {
    NotRequested,
    Capturing,
    Generating,
    Delivering,
    ProviderAcknowledged,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    RecordLimit,
    StringLimit,
    RecordBytesLimit,
    FileBytesLimit,
    Io,
    Poisoned,
    BrowserQueueLimit,
    BrowserStringLimit,
    BrowserReaderClosed,
    InvalidBrowserEvidence,
    ProviderOverflow,
    ProviderStringLimit,
    ProviderContention,
    MissingScopedProviderCapture,
    AfterFinish,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub records: usize,
    pub file_bytes: usize,
    pub record_bytes: usize,
    pub string_bytes: usize,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CaptureLimits {
    pub files: usize,
    pub browser_pending: usize,
    pub browser_records: usize,
    pub browser_string_bytes: usize,
    pub provider_records: usize,
    pub provider_string_bytes: usize,
    pub provider_id_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            records: 20_000,
            file_bytes: 16 * 1024 * 1024,
            record_bytes: 128 * 1024,
            string_bytes: 32 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceFact {
    pub source_row: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerRevision {
    NotExposedBySummarySnapshot,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeRecord {
    Transcript {
        direction: Direction,
        delta: String,
        event_index: u64,
        browser_ms: f64,
        provider_start_ms: Option<f64>,
        audio: AudioEvidence,
    },
    Audio {
        browser_ms: f64,
        audio: AudioEvidence,
    },
    Fault {
        fault: BrowserFault,
        /// Soft faults are sampled through the evidence chain and carry the
        /// media snapshot; hard faults are written directly without one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audio: Option<AudioEvidence>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFault {
    // Hard: the browser evidence stream is no longer trustworthy.
    QueueLimit,
    StringLimit,
    CaptureFailure,
    // Soft: architecture observations the scenario asserts on.
    /// The assistant kept speaking over a playing fixture for `ms` beyond
    /// the fixture's overlap bound (talk-over / late barge-in cancel).
    Overlap {
        ms: u64,
        fixture: String,
        bound_ms: u64,
        /// The peer's raw facts around the overlap, joined into bursts by
        /// `overlap_bursts` for the backchannel classifier.
        #[serde(default)]
        facts: Option<OverlapFacts>,
    },
    /// The same assistant sentence was delivered twice within one response.
    DuplicateReadout {
        text: String,
        response: u32,
    },
}

/// The browser peer's raw facts around one fixture's overlap: its assistant
/// energy bursts and the arrival times of output transcript deltas, user
/// input deltas and delegations. `overlap_bursts` joins them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OverlapFacts {
    /// Peer clock when the facts were taken (the fixture's end).
    pub now_ms: u64,
    /// Silence after the last active window that ends a burst.
    pub hysteresis_ms: u64,
    pub bursts: Vec<BurstFact>,
    pub output: Vec<OutputDeltaFact>,
    pub inputs: Vec<u64>,
    pub delegations: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BurstFact {
    pub started_ms: u64,
    pub last_active_ms: u64,
    pub ended: bool,
    /// Overlap this burst contributed to the fixture.
    pub overlap_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputDeltaFact {
    pub t_ms: u64,
    pub text: String,
}

/// One assistant energy burst that overlapped a playing user fixture, joined
/// from the peer's facts by `overlap_bursts`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlapBurst {
    pub started_ms: u64,
    pub last_active_ms: u64,
    /// The burst ended (the assistant went quiet) before the fixture ended.
    #[serde(default)]
    pub ended: bool,
    /// Overlap this burst contributed to the fixture.
    pub overlap_ms: u64,
    /// The transcript words the voicing queue assigns to this burst:
    /// evidence of what it said.
    #[serde(default)]
    pub text: String,
    /// Every output transcript that arrived in the burst's decision window
    /// (from the previous yield point through its end plus
    /// `TRANSCRIPT_LAG_MS`), whichever burst voiced it.
    #[serde(default)]
    pub window_text: String,
    /// The user resumed speaking after the burst, before the assistant spoke
    /// again.
    #[serde(default)]
    pub yielded: bool,
    /// A delegation arrived in the burst's window.
    #[serde(default)]
    pub acted: bool,
}

/// Upper bound on the words one second of assistant audio can voice, for
/// the evidence queue only; the allow decision does not rest on it.
pub const SPOKEN_WORDS_PER_SECOND: u64 = 4;

/// How long after a burst's end its transcript may still arrive and count
/// toward the burst's decision window.
pub const TRANSCRIPT_LAG_MS: u64 = 1000;

/// Join the peer's facts into the bursts that overlapped the fixture.
///
/// Evidence text (`text`): audio plays in order, so the output words, in
/// arrival order, are voiced by the bursts as a queue, each burst voicing
/// the words that had arrived by its end, up to what its duration can hold
/// (`SPOKEN_WORDS_PER_SECOND`); words a burst could not hold carry to the
/// next. The tail of an earlier response that plays after the user resumed
/// so carries that response's words, whatever the new response's own
/// transcript says.
///
/// Decision window (`window_text`, `acted`): from the previous yield point
/// (the earlier of the last user input delta before the burst and the
/// previous burst's end) through the burst's end plus the hysteresis and
/// `TRANSCRIPT_LAG_MS`. Every output delta that arrived in it is included,
/// whichever burst the queue gave it to.
///
/// A burst yielded when an input delta arrived after it went quiet and
/// before the next burst (or the facts) began.
pub fn overlap_bursts(facts: &OverlapFacts) -> Vec<OverlapBurst> {
    // Words in arrival order, each stamped with the arrival of the delta that
    // completed it (deltas split words: "Mm-h" + " mm.").
    let mut words: Vec<(u64, String)> = Vec::new();
    let mut open = false;
    for delta in &facts.output {
        for (index, piece) in delta.text.split(char::is_whitespace).enumerate() {
            let continues = index == 0 && open && !piece.is_empty();
            if continues {
                if let Some(last) = words.last_mut() {
                    last.0 = delta.t_ms;
                    last.1.push_str(piece);
                }
            } else if !piece.is_empty() {
                words.push((delta.t_ms, piece.to_owned()));
            }
        }
        open = !delta.text.is_empty() && !delta.text.ends_with(char::is_whitespace);
    }
    words.retain(|(_, word)| word.chars().any(char::is_alphanumeric));

    let mut bursts = facts.bursts.clone();
    bursts.sort_by_key(|burst| burst.started_ms);
    let mut next_word = 0;
    let mut joined = Vec::new();
    for (index, burst) in bursts.iter().enumerate() {
        let window_end = if burst.ended {
            burst.last_active_ms + facts.hysteresis_ms
        } else {
            facts.now_ms
        };
        let arrived = words[next_word..]
            .iter()
            .take_while(|(t_ms, _)| *t_ms <= window_end)
            .count();
        let capacity = if burst.ended {
            let duration_ms = burst.last_active_ms.saturating_sub(burst.started_ms);
            usize::try_from(
                (duration_ms * SPOKEN_WORDS_PER_SECOND)
                    .div_ceil(1000)
                    .max(1),
            )
            .unwrap_or(usize::MAX)
        } else {
            usize::MAX
        };
        let voiced = arrived.min(capacity);
        let text = words[next_word..next_word + voiced]
            .iter()
            .map(|(_, word)| word.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        next_word += voiced;

        let previous_end = index
            .checked_sub(1)
            .map(|previous| bursts[previous].last_active_ms);
        let last_input = facts
            .inputs
            .iter()
            .copied()
            .filter(|t_ms| *t_ms <= burst.started_ms)
            .max();
        let decision_start = match (previous_end, last_input) {
            (Some(end), Some(input)) => Some(end.min(input)),
            (end, input) => end.or(input),
        };
        let decision_end = window_end + TRANSCRIPT_LAG_MS;
        let in_decision =
            |t_ms: u64| decision_start.is_none_or(|start| t_ms > start) && t_ms <= decision_end;
        let window_text = facts
            .output
            .iter()
            .filter(|delta| in_decision(delta.t_ms))
            .map(|delta| delta.text.as_str())
            .collect::<String>();
        let acted = facts.delegations.iter().any(|t_ms| in_decision(*t_ms));
        let quiet_until = bursts
            .get(index + 1)
            .map_or(facts.now_ms, |next| next.started_ms);
        let yielded = burst.ended
            && facts
                .inputs
                .iter()
                .any(|t_ms| *t_ms > burst.last_active_ms && *t_ms <= quiet_until);
        if burst.overlap_ms > 0 {
            joined.push(OverlapBurst {
                started_ms: burst.started_ms,
                last_active_ms: burst.last_active_ms,
                ended: burst.ended,
                overlap_ms: burst.overlap_ms,
                text,
                window_text,
                yielded,
                acted,
            });
        }
    }
    joined
}

/// Longest assistant burst that can still be a backchannel.
pub const BACKCHANNEL_MAX_MS: u64 = 1200;

/// Backchannel phrases, normalized (lowercase, punctuation and hyphens as
/// spaces, so "Mm-hm" is "mm hm" and a split "Mm-h mm." is "mm h mm"):
/// acknowledgements (mm-hm, uh-huh, mm, hmm, okay, got it, I see, alright,
/// go ahead, go on) and continuers ("sure", "yeah", "yes", "right": said
/// into a pause they invite the user to go on). Phrases announcing an action
/// ("on it", "I'll ...", "will do") are not here.
pub const BACKCHANNEL_PHRASES: &[&str] = &[
    "mm h mm",
    "mm hm",
    "uh huh",
    "got it",
    "i see",
    "go ahead",
    "go on",
    "all right",
    "mhm",
    "mmhm",
    "mm",
    "hm",
    "hmm",
    "okay",
    "ok",
    "alright",
    "sure",
    "yeah",
    "yes",
    "right",
];

fn backchannel_words(text: &str) -> Vec<String> {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// The text tokenizes, greedily and longest phrase first, into backchannel
/// phrases with no word left over. Empty text tokenizes trivially.
fn only_backchannel_phrases(text: &str) -> bool {
    let words = backchannel_words(text);
    let mut phrases: Vec<Vec<&str>> = BACKCHANNEL_PHRASES
        .iter()
        .map(|phrase| phrase.split(' ').collect())
        .collect();
    phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.len()));
    let mut at = 0;
    while at < words.len() {
        let Some(phrase) = phrases.iter().find(|phrase| {
            words.len() - at >= phrase.len()
                && phrase
                    .iter()
                    .zip(&words[at..])
                    .all(|(expected, word)| expected == word)
        }) else {
            return false;
        };
        at += phrase.len();
    }
    true
}

/// A backchannel, decided fail-closed: a short burst that yielded to the
/// user (who resumed before the assistant spoke again), opened no
/// delegation in its decision window, has non-empty evidence of what it said
/// made only of backchannel phrases, and every output that arrived in its
/// decision window is backchannel phrases too. Empty evidence (a late or
/// missing transcript) or any other word counts.
pub fn is_backchannel(burst: &OverlapBurst) -> bool {
    let short =
        burst.ended && burst.last_active_ms.saturating_sub(burst.started_ms) <= BACKCHANNEL_MAX_MS;
    let evidence =
        !backchannel_words(&burst.text).is_empty() && only_backchannel_phrases(&burst.text);
    short
        && burst.yielded
        && !burst.acted
        && evidence
        && only_backchannel_phrases(&burst.window_text)
}

/// Overlap of one fixture split into what counts and the backchannels that
/// are allowed. Everything that is not a classified backchannel counts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlapClassification {
    pub counted_ms: u64,
    pub backchannels: Vec<OverlapBurst>,
}

pub fn classify_overlap(total_ms: u64, bursts: &[OverlapBurst]) -> OverlapClassification {
    let backchannels: Vec<OverlapBurst> = bursts
        .iter()
        .filter(|burst| is_backchannel(burst))
        .cloned()
        .collect();
    let allowed_ms: u64 = backchannels.iter().map(|burst| burst.overlap_ms).sum();
    OverlapClassification {
        counted_ms: total_ms.saturating_sub(allowed_ms),
        backchannels,
    }
}

/// Drop the Overlap faults whose overlap beyond the classified backchannels
/// is within the fixture's bound; return the remaining faults and the allowed
/// backchannels (fixture name, burst) for evidence.
pub fn reconcile_overlap_faults(
    faults: Vec<BrowserFault>,
) -> (Vec<BrowserFault>, Vec<(String, OverlapBurst)>) {
    let mut remaining = Vec::new();
    let mut allowed = Vec::new();
    for fault in faults {
        match &fault {
            BrowserFault::Overlap {
                ms,
                fixture,
                bound_ms,
                facts,
            } => {
                let bursts = facts.as_ref().map(overlap_bursts).unwrap_or_default();
                let classification = classify_overlap(*ms, &bursts);
                if classification.counted_ms <= *bound_ms && !classification.backchannels.is_empty()
                {
                    for burst in classification.backchannels {
                        if !allowed
                            .iter()
                            .any(|(name, known): &(String, OverlapBurst)| {
                                name == fixture && known == &burst
                            })
                        {
                            allowed.push((fixture.clone(), burst));
                        }
                    }
                } else {
                    remaining.push(fault);
                }
            }
            _ => remaining.push(fault),
        }
    }
    (remaining, allowed)
}

impl BrowserFault {
    pub fn is_hard(&self) -> bool {
        matches!(
            self,
            Self::QueueLimit | Self::StringLimit | Self::CaptureFailure
        )
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAction {
    OpenRequested,
    Connected,
    CloseRequested,
    Closed,
    BrowserDropping,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobAction {
    ReleaseRequested,
    PermissionReleased,
    ReleaseRejected,
    Returned,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Fixture {
        expected_phrase: String,
        limits: Limits,
        capture_limits: CaptureLimits,
    },
    Stage {
        stage: Stage,
    },
    Preparation {
        channel: u32,
        status: Preparation,
    },
    Source {
        job: u32,
        channel: u32,
        canonical_cursor: u64,
        projection_sha256: String,
        owner_revision: OwnerRevision,
        facts: Vec<SourceFact>,
    },
    Summary {
        job: u32,
        text: String,
        expected_fact_present: bool,
        input_tokens: u64,
        output_tokens: u64,
    },
    Job {
        job: u32,
        action: JobAction,
    },
    Channel {
        channel: u32,
        action: ChannelAction,
    },
    Thinking {
        event: thinking_capture::Event,
    },
    Native {
        channel: u32,
        record: NativeRecord,
    },
    Exchange {
        exchange: u32,
        channel: u32,
        stage: Stage,
        start_event_index: usize,
        input_timeout_ms: u64,
        response_timeout_ms: u64,
        baseline: AudioEvidence,
    },
    ResponseWindow {
        exchange: u32,
        timeout_ms: u64,
    },
    ExchangeEnd {
        exchange: u32,
        matched: bool,
        audio: AudioEvidence,
    },
    Fault {
        fault: Fault,
    },
    Outcome {
        outcome: Outcome,
        last_stage: Stage,
    },
    /// One exchange's speech end to input final lag (provider health
    /// evidence for [`provider_degradation_verdict`]).
    ExchangeLag {
        exchange: String,
        speech_end_to_input_final_ms: i64,
    },
    /// The provider input backlog read when an exchange timed out before its
    /// input final.
    ExchangeTimedOut {
        exchange: String,
        provider_input_backlog_ms: Option<u64>,
        /// The provider's reflected input clock when the timeout was read.
        reflected_input_clock_ms: Option<u64>,
        /// Reflected-clock time since the backlog reading was measured (the
        /// last input transcript delta): a large value means the reading is
        /// stale because transcription stopped entirely.
        reflected_clock_since_reading_ms: Option<u64>,
    },
    /// The run's provider-degraded verdict.
    ProviderDegraded {
        degradation: ProviderDegradation,
    },
    /// Downsampled assistant energy windows (t_ms, rms) for one channel.
    Energy {
        channel: u32,
        windows: Vec<(u32, f32)>,
    },
    /// The browser peer's ordered timeline for one channel.
    Timeline {
        channel: u32,
        entries: Vec<TimelineEntry>,
    },
    /// Per-turn response latency: user input final to first assistant
    /// audio, and end of user speech to first assistant audio.
    /// Protocol-anchored by arrival (join by arrival, alternation by
    /// arrival), mirroring the runtime: for a delegated turn the utterance is
    /// the `session.input_transcript.delta`s that arrived before
    /// `session.delegation.created`; for a plain turn those that arrived
    /// before the response's first `output_transcript.delta`. The input final
    /// is the arrival of the last such delta and `input_final_end_ms` its
    /// provider `end_ms`; `input_final_to_delegation_ms` is the
    /// delegation.created arrival minus that (non-negative by construction).
    Latency {
        channel: u32,
        turn: u32,
        input_final_to_audio_ms: Option<i64>,
        speech_end_to_audio_ms: Option<i64>,
        input_final_end_ms: Option<f64>,
        input_final_to_delegation_ms: Option<i64>,
    },
    /// Whether the first assistant transcript of a continuing conversation
    /// opened with a fresh greeting (measurement, not a gate).
    Greeting {
        channel: u32,
        greeted: bool,
        transcript: String,
    },
    /// Client-side disconnect to host-observed Closed.
    CloseConvergence {
        channel: u32,
        converged_before_host_close: bool,
        ms: u64,
    },
    /// Time-to-talk breakdown for one channel, every mark on the journal
    /// clock (ms since the journal was created; `None` when not observed):
    /// live/open request -> open returned (pending handle) -> provider
    /// session attached (thinking capture `SessionAttached`) -> host answer
    /// delivered -> browser WebRTC connected / data channel open -> first
    /// outbound user audio packet -> first user speech -> first user input
    /// transcript delta. The public path carries media browser <-> provider
    /// directly, so the host never accepts an audio packet; the browser's
    /// first outbound RTP packet is the media-path-up mark.
    TimeToTalk {
        channel: u32,
        open_request_ms: u64,
        open_returned_ms: u64,
        session_attached_ms: Option<u64>,
        answer_delivered_ms: u64,
        webrtc_connected_ms: Option<u64>,
        data_channel_open_ms: Option<u64>,
        first_audio_packet_ms: Option<u64>,
        first_user_speech_ms: Option<u64>,
        first_input_delta_ms: Option<u64>,
    },
    /// Barge-in breakdown on the browser clock, relative to the user's
    /// speech onset (fixture start): first provider input transcript delta
    /// of the interruption, the assistant's last energetic window, the
    /// overlap the fixture accumulated, and every provider event type seen
    /// in the window. Media is browser <-> provider on the public path, so
    /// the peer plays a live track with no queued playback to flush.
    BargeIn {
        channel: u32,
        onset_ms: u64,
        first_input_delta_after_onset_ms: Option<i64>,
        assistant_quiet_after_onset_ms: Option<i64>,
        overlap_ms: u64,
        overlap_bound_ms: u64,
        provider_events: Vec<String>,
    },
    /// Browser uplink health at close: outbound audio RTP packets sent
    /// against the ~50 packets/s a continuous 20 ms Opus track produces
    /// since `connected`. A ratio well under 1.0 means the headless
    /// browser's audio rendering stalled (host CPU starvation), which loses
    /// user speech before it ever reaches the provider.
    Uplink {
        channel: u32,
        packets_sent: u64,
        expected_packets: u64,
        ratio: f32,
    },
    /// Host `/proc/loadavg` at a scenario moment (open, close), so provider
    /// transcript loss can be correlated with machine load.
    HostLoad {
        moment: String,
        loadavg: String,
    },
    /// Mob-scoped WorkGraph items behind the channel's delegations: `items`
    /// at or above `expected_delegations` is parallel mode (the coordinator
    /// scheduled through WorkGraph); none is the serial fallback.
    WorkGraph {
        channel: u32,
        items: usize,
        expected_delegations: usize,
        mode: String,
        titles: Vec<String>,
    },
    /// One close/reopen-with-summary cycle: close convergence, reopen
    /// connect time, summary delivery time (reopen -> fragments acknowledged),
    /// and the instructions-lane fragment accounting for the cycle.
    ReopenCycle {
        channel: u32,
        close_ms: Option<u64>,
        reopen_ms: u64,
        summary_delivery_ms: u64,
        framed_before: usize,
        framed_after: usize,
        fragments: usize,
        expected_fragments: usize,
        fragment_bytes: usize,
        acknowledged: usize,
        greeted: bool,
    },
    /// The runtime's verdict on the client's decoded-audio counters for a
    /// channel's first assistant output (`live/media_health`).
    MediaHealthJudged {
        channel: u32,
        output_id: String,
        decoded_frames: u64,
        audible_frames: u64,
        max_rms: f64,
        media_fault: bool,
        reopen_recommended: bool,
    },
    /// The runtime closed `from_channel` on a media fault and the harness
    /// reopened the session on `to_channel`; the exchange that waited on the
    /// silent output is spoken again there.
    MediaFaultReopened {
        from_channel: u32,
        to_channel: u32,
        exchange: String,
    },
    /// A tolerant (model-dependent) check: recorded with its outcome, never
    /// a gate on its own. The deterministic checks assert.
    Tolerant {
        channel: u32,
        check: String,
        passed: bool,
        detail: String,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Envelope {
    pub sequence: usize,
    pub elapsed_ms: u64,
    pub record: Record,
}

struct State {
    file: File,
    count: usize,
    bytes: usize,
    fault: Option<Fault>,
    finished: bool,
    stage: Stage,
    channel: u32,
    job: u32,
    exchange: u32,
    attached_channels: Vec<u32>,
    /// Journal-clock time of each channel's provider `SessionAttached`.
    session_attached_ms: Vec<(u32, u64)>,
    /// Texts of owned instructions-lane append attempts, in wire order.
    instructions_append_texts: Vec<String>,
    /// Running count of owned thinking-append attempts seen on the wire.
    thinking_append_attempts: usize,
    /// Bounded copy of the attempted thinking-append texts, for echo checks.
    thinking_append_texts: Vec<String>,
    /// Owned instructions-lane appends the provider acknowledged (matched an
    /// owned client event id and was accepted).
    instructions_acknowledged: usize,
    /// Owned thinking-lane appends the provider acknowledged.
    thinking_acknowledged: usize,
    /// Owned thinking appends reassembled per channel and append token
    /// (`meerkat-thinking-<token>-<index>`), in first-seen order.
    thinking_appends: Vec<(u32, String, String)>,
    /// Shape of each channel's `session.start` seed (host-side capture,
    /// recorded when the create request is built, before `SessionAttached`).
    session_input_seeds: Vec<(u32, SessionInputSeed)>,
    session_input_texts: Vec<(u32, Vec<String>)>,
    /// Session-lane (voiced canonical row) commentary appends per channel:
    /// `(channel, text prefix, whole content bytes)`.
    session_commentary_appends: Vec<(u32, String, usize)>,
    /// Owned instructions-lane attempts (one per wire fragment) and how many
    /// reassembled appends opened a framed summary.
    instructions_append_attempts: usize,
    framed_summary_attempts: usize,
    /// Instructions appends being reassembled from their fragments, keyed by
    /// channel ordinal and the append token shared by every fragment's client
    /// event id (`meerkat-instructions-<token>-<index>`).
    instructions_appends: HashMap<String, InstructionsAppendReassembly>,
    /// Soft browser faults (overlap, duplicate readout); never invalidate.
    browser_faults: Vec<BrowserFault>,
    /// Speech end to input final lag of every exchange that reached its
    /// final, in order.
    exchange_lags: Vec<(String, i64)>,
    /// The exchange awaiting its input final, if any.
    pending_exchange: Option<String>,
    /// The exchange that timed out before its final, with the provider input
    /// backlog read at that moment.
    timed_out_exchange: Option<(String, Option<u64>)>,
}

#[derive(Default)]
struct InstructionsAppendReassembly {
    text: String,
    counted_as_framed: bool,
}

/// The append token of an owned instructions fragment's client event id.
fn instructions_append_token(client_event_id: &str) -> Option<&str> {
    let rest = client_event_id.strip_prefix("meerkat-instructions-")?;
    let (token, _index) = rest.rsplit_once('-')?;
    Some(token)
}

/// What the owner injected into the provider so far, by lane.
/// What the host put into one channel's `session.start` body: the number
/// of history input items, how many of them are developer items (the seeded
/// summary), and whether the startup instructions frame that history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionInputSeed {
    pub input_items: usize,
    pub developer_items: usize,
    pub frames_history: bool,
    /// The developer item summarizes only the history before the verbatim
    /// items after it: a summary retained from an earlier open.
    pub preceding_history_summary: bool,
    /// Text bytes of the startup input and their conservative token estimate.
    pub input_bytes: usize,
    pub estimated_tokens: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerAppends {
    pub thinking_attempts: usize,
    pub instructions_attempts: usize,
    /// Instructions-lane fragments the provider acknowledged as owned and
    /// accepted.
    pub instructions_acknowledged: usize,
    /// Thinking-lane fragments the provider acknowledged as owned and
    /// accepted.
    pub thinking_acknowledged: usize,
    /// Instructions attempts that open a bootstrap summary (carry its framing).
    pub framed_summaries: usize,
}

struct Inner {
    state: Mutex<State>,
    started: Instant,
    path: PathBuf,
    label: &'static str,
    expected_phrase: String,
    secrets: Vec<String>,
    limits: Limits,
    wire: thinking_capture::Capture,
    /// Every provider crossing (create request/response, client events,
    /// raw server frames) in causal order: the raw input of a replay
    /// fixture, scrubbed by `scripts/gpt-live-scrub-provider-stream` before
    /// it is committed. Evidence only; a write failure never fails the run,
    /// it marks the recording incomplete at finish.
    provider_stream: provider_recording::Recorder,
}

#[derive(Clone)]
pub struct Journal(Arc<Inner>);

/// The provider-stream recording beside `journal.jsonl`.
pub const PROVIDER_STREAM_FILE: &str = "provider-stream.jsonl";

impl Journal {
    pub fn create(expected_phrase: String) -> Result<Self, Fault> {
        Self::create_for("S99", expected_phrase)
    }

    /// One journal under `target/e2e-live-audio-artifacts/<label>/<uuid>`.
    pub fn create_for(label: &'static str, expected_phrase: String) -> Result<Self, Fault> {
        let directory = super::workspace_root()
            .join("target/e2e-live-audio-artifacts")
            .join(label.to_ascii_lowercase())
            .join(uuid::Uuid::new_v4().to_string());
        let secrets = [
            "OPENAI_API_KEY",
            "RKAT_OPENAI_API_KEY",
            "OPENAI_API_KEY_OLD",
        ]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .filter(|secret| !secret.is_empty())
        .collect();
        Self::at_labeled(
            &directory,
            label,
            expected_phrase,
            secrets,
            Limits::default(),
        )
    }

    fn at(
        directory: &Path,
        expected_phrase: String,
        secrets: Vec<String>,
        limits: Limits,
    ) -> Result<Self, Fault> {
        Self::at_labeled(directory, "S99", expected_phrase, secrets, limits)
    }

    fn at_labeled(
        directory: &Path,
        label: &'static str,
        expected_phrase: String,
        mut secrets: Vec<String>,
        limits: Limits,
    ) -> Result<Self, Fault> {
        if limits.records < 5 || limits.file_bytes < 8192 || limits.record_bytes < 512 {
            return Err(Fault::RecordLimit);
        }
        std::fs::create_dir_all(directory).map_err(|_| Fault::Io)?;
        let path = directory.join("journal.jsonl");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|_| Fault::Io)?;
        let provider_stream =
            provider_recording::Recorder::create(&directory.join(PROVIDER_STREAM_FILE))
                .map_err(|_| Fault::Io)?;
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        let journal = Self(Arc::new(Inner {
            state: Mutex::new(State {
                file,
                count: 0,
                bytes: 0,
                fault: None,
                finished: false,
                stage: Stage::Setup,
                channel: 0,
                job: 0,
                exchange: 0,
                attached_channels: Vec::new(),
                session_attached_ms: Vec::new(),
                instructions_append_texts: Vec::new(),
                instructions_acknowledged: 0,
                thinking_acknowledged: 0,
                thinking_appends: Vec::new(),
                session_input_seeds: Vec::new(),
                session_input_texts: Vec::new(),
                session_commentary_appends: Vec::new(),
                thinking_append_attempts: 0,
                thinking_append_texts: Vec::new(),
                instructions_append_attempts: 0,
                framed_summary_attempts: 0,
                instructions_appends: HashMap::new(),
                browser_faults: Vec::new(),
                exchange_lags: Vec::new(),
                pending_exchange: None,
                timed_out_exchange: None,
            }),
            started: Instant::now(),
            path,
            label,
            expected_phrase: expected_phrase.clone(),
            secrets,
            limits,
            wire: thinking_capture::Capture::new(),
            provider_stream,
        }));
        journal.record(Record::Fixture {
            expected_phrase,
            limits,
            capture_limits: CaptureLimits {
                files: 1,
                browser_pending: 128,
                browser_records: 20_000,
                browser_string_bytes: 16_384,
                provider_records: thinking_capture::Capture::MAX_EVENTS,
                provider_string_bytes: thinking_capture::Capture::MAX_TEXT_BYTES,
                provider_id_bytes: thinking_capture::Capture::MAX_ID_BYTES,
            },
        })?;
        println!("{label}_EVIDENCE_JOURNAL path={}", journal.path().display());
        Ok(journal)
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }
    pub fn expected_phrase(&self) -> &str {
        &self.0.expected_phrase
    }
    pub fn wire(&self, channel: u32) -> thinking_capture::Capture {
        self.0.wire.for_channel(channel)
    }

    /// The provider-stream recorder for one channel; scope a connect with it
    /// (next to [`Self::wire`]) so that channel's broker records into
    /// `provider-stream.jsonl` beside this journal.
    pub fn provider_recording(&self, channel: u32) -> provider_recording::Recorder {
        self.0.provider_stream.for_channel(channel)
    }

    /// A recording that lost a line is not a fixture: rename it so the
    /// re-capture procedure cannot pick it up. The run's verdict is unchanged.
    fn seal_provider_stream(&self) {
        if self.0.provider_stream.failure().is_some()
            && let Some(directory) = self.0.path.parent()
        {
            let _ = std::fs::rename(
                directory.join(PROVIDER_STREAM_FILE),
                directory.join(format!("{PROVIDER_STREAM_FILE}.incomplete")),
            );
        }
    }

    /// An exchange's fixture is about to play; it awaits its input final.
    pub fn exchange_started(&self, exchange: &str) -> Result<(), Fault> {
        let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        state.pending_exchange = Some(exchange.to_owned());
        Ok(())
    }

    /// An exchange reached its input final `lag_ms` after its speech ended.
    pub fn exchange_heard(&self, exchange: &str, lag_ms: i64) -> Result<(), Fault> {
        {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            state.pending_exchange = None;
            state.exchange_lags.push((exchange.to_owned(), lag_ms));
        }
        self.record(Record::ExchangeLag {
            exchange: exchange.to_owned(),
            speech_end_to_input_final_ms: lag_ms,
        })
    }

    /// The pending exchange timed out before its input final, with the
    /// provider input latency read at that moment.
    pub fn exchange_timed_out(&self, latency: ProviderInputLatencyAtTimeout) -> Result<(), Fault> {
        let backlog_ms = latency.backlog_ms;
        let exchange = {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            let Some(exchange) = state.pending_exchange.take() else {
                return Ok(());
            };
            state.timed_out_exchange = Some((exchange.clone(), backlog_ms));
            exchange
        };
        self.record(Record::ExchangeTimedOut {
            exchange,
            provider_input_backlog_ms: backlog_ms,
            reflected_input_clock_ms: latency.reflected_input_clock_ms,
            reflected_clock_since_reading_ms: latency.reflected_clock_since_reading_ms,
        })
    }

    /// This run's provider-degraded verdict from its own evidence.
    pub fn provider_degradation(&self) -> Result<Option<ProviderDegradation>, Fault> {
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(provider_degradation_verdict(
            &state.exchange_lags,
            state
                .timed_out_exchange
                .as_ref()
                .map(|(exchange, backlog)| (exchange.as_str(), *backlog)),
        ))
    }

    pub fn stage(&self, stage: Stage) -> Result<(), Fault> {
        self.flush_wire()?;
        self.record(Record::Stage { stage })
    }

    pub fn next_channel(&self) -> Result<u32, Fault> {
        let channel = {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            state.channel = state.channel.checked_add(1).ok_or(Fault::RecordLimit)?;
            state.channel
        };
        self.channel(channel, ChannelAction::OpenRequested)?;
        Ok(channel)
    }

    pub fn current_channel(&self) -> Result<u32, Fault> {
        Ok(self.0.state.lock().map_err(|_| Fault::Poisoned)?.channel)
    }

    pub fn channel(&self, channel: u32, action: ChannelAction) -> Result<(), Fault> {
        self.flush_wire()?;
        self.record(Record::Channel { channel, action })
    }

    pub fn capture_source(
        &self,
        snapshot: &meerkat::session_runtime::live_summary::LiveContextSummarySnapshot<'_>,
    ) -> Result<u32, Fault> {
        let (job, channel) = {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            state.job = state.job.checked_add(1).ok_or(Fault::RecordLimit)?;
            (state.job, state.channel)
        };
        self.record(Record::Source {
            job,
            channel,
            canonical_cursor: snapshot.canonical_message_cursor(),
            projection_sha256: meerkat_core::session::transcript_messages_digest(
                snapshot.messages(),
            )
            .map_err(|_| Fault::Io)?,
            owner_revision: OwnerRevision::NotExposedBySummarySnapshot,
            facts: selected_source_facts(snapshot.messages(), self.expected_phrase()),
        })?;
        Ok(job)
    }

    pub fn exchange(
        &self,
        start_event_index: usize,
        baseline: AudioEvidence,
    ) -> Result<u32, Fault> {
        self.flush_wire()?;
        let (exchange, channel, stage) = {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            state.exchange = state.exchange.checked_add(1).ok_or(Fault::RecordLimit)?;
            (state.exchange, state.channel, state.stage)
        };
        self.record(Record::Exchange {
            exchange,
            channel,
            stage,
            start_event_index,
            input_timeout_ms: 120_000,
            response_timeout_ms: 90_000,
            baseline,
        })?;
        Ok(exchange)
    }

    pub fn native(&self, channel: u32, record: NativeRecord) -> Result<(), Fault> {
        if let NativeRecord::Fault { fault, .. } = &record {
            match fault {
                BrowserFault::QueueLimit => return self.fail(Fault::BrowserQueueLimit),
                BrowserFault::StringLimit => return self.fail(Fault::BrowserStringLimit),
                BrowserFault::CaptureFailure => return self.fail(Fault::InvalidBrowserEvidence),
                BrowserFault::Overlap { .. } | BrowserFault::DuplicateReadout { .. } => {
                    self.0
                        .state
                        .lock()
                        .map_err(|_| Fault::Poisoned)?
                        .browser_faults
                        .push(fault.clone());
                }
            }
        }
        self.record(Record::Native { channel, record })
    }

    /// Soft browser faults recorded so far. Scenarios assert on this; the
    /// journal itself stays valid.
    pub fn faults(&self) -> Result<Vec<BrowserFault>, Fault> {
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| Fault::Poisoned)?
            .browser_faults
            .clone())
    }

    pub fn flush_wire(&self) -> Result<(), Fault> {
        let events = self.0.wire.drain().map_err(|_| Fault::ProviderContention)?;
        for event in events {
            if matches!(event.event, thinking_capture::EventKind::SessionAttached) {
                let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
                state.attached_channels.push(event.channel_ordinal);
                state
                    .session_attached_ms
                    .push((event.channel_ordinal, event.elapsed_ms));
            }
            if let thinking_capture::EventKind::SessionInputSeeded {
                input_items,
                developer_items,
                frames_history,
                preceding_history_summary,
                input_bytes,
                estimated_tokens,
                input_texts,
            } = &event.event
            {
                let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
                state.session_input_seeds.push((
                    event.channel_ordinal,
                    SessionInputSeed {
                        input_items: *input_items,
                        developer_items: *developer_items,
                        frames_history: *frames_history,
                        preceding_history_summary: *preceding_history_summary,
                        input_bytes: *input_bytes,
                        estimated_tokens: *estimated_tokens,
                    },
                ));
                state
                    .session_input_texts
                    .push((event.channel_ordinal, input_texts.clone()));
            }
            if let thinking_capture::EventKind::ThinkingAppendAttempt {
                client_event_id,
                text,
            } = &event.event
            {
                let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
                state.thinking_append_attempts += 1;
                if state.thinking_append_texts.len() < thinking_capture::Capture::MAX_EVENTS {
                    state.thinking_append_texts.push(text.clone());
                }
                // Fragments of one thinking append share the token in their
                // client event id (`meerkat-thinking-<token>-<index>`).
                let token = client_event_id
                    .strip_prefix("meerkat-thinking-")
                    .and_then(|rest| rest.rsplit_once('-'))
                    .map(|(token, _)| token.to_owned())
                    .unwrap_or_else(|| client_event_id.clone());
                let channel = event.channel_ordinal;
                let position = state
                    .thinking_appends
                    .iter()
                    .position(|(c, t, _)| *c == channel && *t == token);
                match position {
                    Some(index) => state.thinking_appends[index].2.push_str(text),
                    None => {
                        if state.thinking_appends.len() < thinking_capture::Capture::MAX_EVENTS {
                            state.thinking_appends.push((channel, token, text.clone()));
                        }
                    }
                }
            }
            if let thinking_capture::EventKind::ThinkingAppended {
                matched_owned: true,
                accepted: true,
                ..
            } = &event.event
            {
                self.0
                    .state
                    .lock()
                    .map_err(|_| Fault::Poisoned)?
                    .thinking_acknowledged += 1;
            }
            if let thinking_capture::EventKind::InstructionsAppendAttempt {
                client_event_id,
                text,
            } = &event.event
            {
                let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
                state.instructions_append_attempts += 1;
                // The broker sends an append as ordered UTF-8 fragments; the
                // framing is recognised on the reassembled text so its length
                // is not bound to the fragment size.
                // Append tokens restart with every channel's broker, so the
                // key is scoped by the channel the fragment was sent on.
                let token =
                    instructions_append_token(client_event_id).unwrap_or(client_event_id.as_str());
                let key = format!("{}:{token}", event.channel_ordinal);
                let append = state.instructions_appends.entry(key).or_default();
                append.text.push_str(text);
                if !append.counted_as_framed
                    && append
                        .text
                        .starts_with(meerkat::experimental_gpt_live::LIVE_CONTEXT_BOOTSTRAP_FRAMING)
                {
                    append.counted_as_framed = true;
                    state.framed_summary_attempts += 1;
                }
                if state.instructions_append_texts.len() < thinking_capture::Capture::MAX_EVENTS {
                    state.instructions_append_texts.push(text.clone());
                }
            }
            if let thinking_capture::EventKind::CommentaryAppendAttempt {
                delegation: false,
                text,
                text_bytes,
                ..
            } = &event.event
            {
                let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
                if state.session_commentary_appends.len() < thinking_capture::Capture::MAX_EVENTS {
                    state.session_commentary_appends.push((
                        event.channel_ordinal,
                        text.clone(),
                        *text_bytes,
                    ));
                }
            }
            if let thinking_capture::EventKind::InstructionsAppended {
                matched_owned: true,
                accepted: true,
                ..
            } = &event.event
            {
                self.0
                    .state
                    .lock()
                    .map_err(|_| Fault::Poisoned)?
                    .instructions_acknowledged += 1;
            }
            self.record(Record::Thinking { event })?;
        }
        if let Some(fault) = self.0.wire.fault() {
            return self.fail(match fault {
                thinking_capture::Fault::Overflow => Fault::ProviderOverflow,
                thinking_capture::Fault::StringLimit => Fault::ProviderStringLimit,
                thinking_capture::Fault::Contention => Fault::ProviderContention,
            });
        }
        self.check()
    }

    /// Owned thinking-append attempts observed so far (summary fragments and
    /// causal-tail reassertions). Fresh post-acknowledgement speech must not
    /// add to it: a growing count between two post-ACK exchanges is the
    /// self-echo the 73a0b869 baseline shipped.
    pub fn thinking_append_attempts(&self) -> Result<usize, Fault> {
        self.flush_wire()?;
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| Fault::Poisoned)?
            .thinking_append_attempts)
    }

    /// Milliseconds on the journal clock at `at` (the thinking capture and
    /// the journal start together, so its `elapsed_ms` is the same clock).
    pub fn elapsed_ms_at(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.0.started).as_millis()).unwrap_or(u64::MAX)
    }

    /// Journal-clock time the provider session of `channel` attached.
    pub fn session_attached_ms(&self, channel: u32) -> Result<Option<u64>, Fault> {
        self.flush_wire()?;
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(state
            .session_attached_ms
            .iter()
            .find(|(ordinal, _)| *ordinal == channel)
            .map(|(_, ms)| *ms))
    }

    /// The `session.start` seed the host built for `channel`, if captured.
    pub fn session_input_seed(&self, channel: u32) -> Result<Option<SessionInputSeed>, Fault> {
        self.flush_wire()?;
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(state
            .session_input_seeds
            .iter()
            .find(|(c, _)| *c == channel)
            .map(|(_, seed)| *seed))
    }

    /// Text of every startup input item of `channel`'s session.start body.
    pub fn session_input_texts(&self, channel: u32) -> Result<Vec<String>, Fault> {
        self.flush_wire()?;
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(state
            .session_input_texts
            .iter()
            .find(|(c, _)| *c == channel)
            .map(|(_, texts)| texts.clone())
            .unwrap_or_default())
    }

    /// Reassembled texts of every owned thinking append on `channel`, in
    /// order.
    pub fn owned_thinking_appends(&self, channel: u32) -> Result<Vec<String>, Fault> {
        self.flush_wire()?;
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(state
            .thinking_appends
            .iter()
            .filter(|(c, _, _)| *c == channel)
            .map(|(_, _, text)| text.clone())
            .collect())
    }

    /// Session-lane commentary appends on `channel`, in order: the voiced
    /// canonical rows the provider had not heard (a typed row, or the
    /// executor's reply to a later non-voice input such as a peer response).
    /// Each is `(text prefix, whole content bytes)`; the prefix is the first
    /// capture text limit of the content.
    pub fn session_commentary_appends(&self, channel: u32) -> Result<Vec<(String, usize)>, Fault> {
        self.flush_wire()?;
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(state
            .session_commentary_appends
            .iter()
            .filter(|(c, _, _)| *c == channel)
            .map(|(_, text, bytes)| (text.clone(), *bytes))
            .collect())
    }

    /// Reassembled text of the first owned thinking append on `channel`
    /// (the late bootstrap summary rides there by default).
    pub fn first_owned_thinking_append(&self, channel: u32) -> Result<Option<String>, Fault> {
        self.flush_wire()?;
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(state
            .thinking_appends
            .iter()
            .find(|(c, _, _)| *c == channel)
            .map(|(_, _, text)| text.clone()))
    }

    /// Texts of every owned instructions-lane append attempt so far.
    pub fn instructions_append_attempt_texts(&self) -> Result<Vec<String>, Fault> {
        self.flush_wire()?;
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| Fault::Poisoned)?
            .instructions_append_texts
            .clone())
    }

    /// Owner appends observed so far, by lane.
    pub fn owner_appends(&self) -> Result<OwnerAppends, Fault> {
        self.flush_wire()?;
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        Ok(OwnerAppends {
            thinking_attempts: state.thinking_append_attempts,
            instructions_attempts: state.instructions_append_attempts,
            instructions_acknowledged: state.instructions_acknowledged,
            thinking_acknowledged: state.thinking_acknowledged,
            framed_summaries: state.framed_summary_attempts,
        })
    }

    /// Texts of every owned thinking-append attempt so far. Pre-ACK causal
    /// reassertions may trickle out for a while (the provider acknowledges
    /// thinking appends at turn boundaries), so echo detection compares
    /// content, not counts.
    pub fn thinking_append_attempt_texts(&self) -> Result<Vec<String>, Fault> {
        self.flush_wire()?;
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| Fault::Poisoned)?
            .thinking_append_texts
            .clone())
    }

    pub fn require_attached(&self, channel: u32) -> Result<(), Fault> {
        self.flush_wire()?;
        let present = self
            .0
            .state
            .lock()
            .map_err(|_| Fault::Poisoned)?
            .attached_channels
            .contains(&channel);
        if !present {
            return self.fail(Fault::MissingScopedProviderCapture);
        }
        Ok(())
    }

    pub fn check(&self) -> Result<(), Fault> {
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        state.fault.map_or(Ok(()), Err)
    }

    pub fn record(&self, record: Record) -> Result<(), Fault> {
        let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        if let Some(fault) = state.fault {
            return Err(fault);
        }
        if state.finished {
            self.fail_locked(&mut state, Fault::AfterFinish);
            return Err(Fault::AfterFinish);
        }
        let encoded = self.encode(state.count, &record, true);
        let fault = match &encoded {
            Err(fault) => Some(*fault),
            Ok(_) if state.count >= self.0.limits.records - 3 => Some(Fault::RecordLimit),
            Ok(bytes) if bytes.len() > self.0.limits.record_bytes => Some(Fault::RecordBytesLimit),
            Ok(bytes) if state.bytes + bytes.len() > self.0.limits.file_bytes - 4096 => {
                Some(Fault::FileBytesLimit)
            }
            Ok(_) => None,
        };
        if let Some(fault) = fault {
            self.fail_locked(&mut state, fault);
            return Err(fault);
        }
        if let Record::Stage { stage } = record {
            state.stage = stage;
        }
        let bytes = encoded?;
        if self.write_locked(&mut state, &bytes).is_err() {
            self.fail_locked(&mut state, Fault::Io);
            return Err(Fault::Io);
        }
        Ok(())
    }

    fn encode(
        &self,
        sequence: usize,
        record: &Record,
        enforce_text_limit: bool,
    ) -> Result<Vec<u8>, Fault> {
        let mut value = serde_json::to_value(record).map_err(|_| Fault::Io)?;
        redact_and_bound(
            &mut value,
            &self.0.secrets,
            if enforce_text_limit {
                self.0.limits.string_bytes
            } else {
                usize::MAX
            },
            false,
        )?;
        let mut bytes = serde_json::to_vec(&serde_json::json!({
            "sequence": sequence,
            "elapsed_ms": u64::try_from(self.0.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "record": value,
        }))
        .map_err(|_| Fault::Io)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn write_locked(&self, state: &mut State, bytes: &[u8]) -> Result<(), Fault> {
        state
            .file
            .write_all(bytes)
            .and_then(|()| state.file.sync_data())
            .map_err(|_| Fault::Io)?;
        state.bytes += bytes.len();
        state.count += 1;
        Ok(())
    }

    fn fail_locked(&self, state: &mut State, fault: Fault) {
        if state.fault.is_none() {
            state.fault = Some(fault);
            let result = self
                .encode(state.count, &Record::Fault { fault }, false)
                .and_then(|bytes| self.write_locked(state, &bytes));
            if result.is_err() {
                eprintln!(
                    "{}_EVIDENCE_WRITE_FAILURE path={}",
                    self.0.label,
                    self.path().display()
                );
            }
            eprintln!(
                "{}_EVIDENCE_FAULT fault={fault:?} path={}",
                self.0.label,
                self.path().display()
            );
        }
    }

    pub fn fail(&self, fault: Fault) -> Result<(), Fault> {
        let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        self.fail_locked(&mut state, fault);
        Err(fault)
    }

    /// Finish with this run's own provider-health verdict: when its evidence
    /// shows provider-degraded input processing, the run is recorded as
    /// [`Outcome::ProviderDegraded`] (void: neither green nor red) with the
    /// [`Record::ProviderDegraded`] that caused it, whatever `outcome` the
    /// scenario reached. Otherwise it finishes with `outcome`.
    pub fn finish_classified(
        &self,
        outcome: Outcome,
    ) -> Result<Option<ProviderDegradation>, Fault> {
        let already_finished = self.0.state.lock().map_err(|_| Fault::Poisoned)?.finished;
        let degradation = if already_finished {
            // The inner scenario already recorded its verdict; finishing
            // again is the journal's idempotent no-op.
            None
        } else {
            self.provider_degradation()?
        };
        let Some(degradation) = degradation else {
            return self.finish(outcome).map(|()| None);
        };
        self.record(Record::ProviderDegraded {
            degradation: degradation.clone(),
        })?;
        self.finish(Outcome::ProviderDegraded)?;
        Ok(Some(degradation))
    }

    /// The typed void verdict line and error text for a provider-degraded
    /// run. The run must never count as green, and it is not a red either.
    pub fn provider_degraded_verdict(&self, degradation: &ProviderDegradation) -> String {
        let cause = serde_json::to_string(&degradation.cause).unwrap_or_default();
        println!(
            "GPT_LIVE_VERDICT scenario={} verdict=provider_degraded exchange={} cause={cause} p90_ms={:?} provider_input_backlog_ms={:?}",
            self.0.label,
            degradation.exchange,
            degradation.p90_ms,
            degradation.provider_input_backlog_ms
        );
        format!(
            "PROVIDER_DEGRADED: {} is void (neither green nor red): provider input processing was degraded at {} ({cause}, p90_ms={:?}, provider_input_backlog_ms={:?}); a valid run needs a healthy provider window",
            self.0.label,
            degradation.exchange,
            degradation.p90_ms,
            degradation.provider_input_backlog_ms
        )
    }

    pub fn finish(&self, outcome: Outcome) -> Result<(), Fault> {
        if let Err(fault) = self.flush_wire() {
            let _ = self.fail(fault);
        }
        let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        if state.finished {
            return state.fault.map_or(Ok(()), Err);
        }
        self.seal_provider_stream();
        let outcome = if state.fault.is_some() {
            Outcome::Failed
        } else {
            outcome
        };
        let bytes = self.encode(
            state.count,
            &Record::Outcome {
                outcome,
                last_stage: state.stage,
            },
            false,
        )?;
        self.write_locked(&mut state, &bytes)?;
        state.finished = true;
        state.fault.map_or(Ok(()), Err)
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "live evidence failure: {self:?}")
    }
}
impl std::error::Error for Fault {}

pub struct FailureGuard(pub Journal);
impl Drop for FailureGuard {
    fn drop(&mut self) {
        if let Err(fault) = self.0.finish(Outcome::CancelledOrPanicked) {
            eprintln!("{fault}; journal={}", self.0.path().display());
        }
    }
}

pub fn selected_source_facts(messages: &[meerkat_core::Message], phrase: &str) -> Vec<SourceFact> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(source_row, message)| {
            let meerkat_core::Message::User(user) = message else {
                return Vec::new();
            };
            if !user.transcript_role.is_conversational() {
                return Vec::new();
            }
            user.content
                .iter()
                .filter_map(|block| match block {
                    meerkat_core::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .flat_map(|text| text.split_inclusive(['.', '\n']))
                .filter_map(|sentence| {
                    let lower = sentence.to_lowercase();
                    let fact = (!phrase.is_empty() && sentence.contains(phrase))
                        || (lower.contains("code word")
                            && ["tangerine", "violet", "cobalt"]
                                .iter()
                                .any(|word| lower.contains(word)))
                        || (lower.contains("favorite flower")
                            && ["daffodil", "marigold"]
                                .iter()
                                .any(|word| lower.contains(word)));
                    fact.then(|| SourceFact {
                        source_row,
                        text: sentence.trim().to_owned(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn redact_and_bound(
    value: &mut serde_json::Value,
    secrets: &[String],
    limit: usize,
    redact: bool,
) -> Result<(), Fault> {
    match value {
        serde_json::Value::String(text) => {
            if text.len() > limit {
                return Err(Fault::StringLimit);
            }
            if redact {
                for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
                    *text = text.replace(secret, "[REDACTED_CREDENTIAL]");
                }
            }
            if text.len() > limit {
                return Err(Fault::StringLimit);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_and_bound(item, secrets, limit, redact)?;
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                redact_and_bound(
                    value,
                    secrets,
                    limit,
                    matches!(
                        key.as_str(),
                        "text" | "delta" | "expected_phrase" | "client_event_id"
                    ),
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lags(values: &[(&str, i64)]) -> Vec<(String, i64)> {
        values
            .iter()
            .map(|(exchange, lag)| ((*exchange).to_owned(), *lag))
            .collect()
    }

    #[test]
    fn healthy_run_has_no_provider_degradation() {
        let healthy = lags(&[("e1", 950), ("e2", 1_050), ("e3", 1_300), ("e4", 1_100)]);
        assert_eq!(provider_degradation_verdict(&healthy, None), None);
    }

    #[test]
    fn one_exchange_at_ten_seconds_is_provider_degraded() {
        let run = lags(&[("e1", 1_000), ("e8", 15_200), ("e9", 44_000)]);
        let verdict = provider_degradation_verdict(&run, None).expect("degraded");
        assert_eq!(verdict.exchange, "e8");
        assert_eq!(
            verdict.cause,
            ProviderDegradationCause::ExchangeLag { lag_ms: 15_200 }
        );
    }

    #[test]
    fn lag_p90_above_two_seconds_is_provider_degraded() {
        let run = lags(&[
            ("e1", 900),
            ("e2", 2_600),
            ("e3", 2_800),
            ("e4", 3_100),
            ("e5", 1_000),
        ]);
        let verdict = provider_degradation_verdict(&run, None).expect("degraded");
        assert_eq!(
            verdict.cause,
            ProviderDegradationCause::LagP90 { p90_ms: 3_100 }
        );
        assert_eq!(verdict.exchange, "e4");
    }

    /// A timeout is degradation only with the provider's own evidence that it
    /// was behind: a meerkat-side input loss (healthy or unknown backlog)
    /// stays a failure, never void.
    #[test]
    fn timeout_is_degraded_only_behind_a_measured_provider_backlog() {
        let healthy = lags(&[("e8", 1_000), ("e9", 1_200)]);
        assert_eq!(
            provider_degradation_verdict(&healthy, Some(("e10", None))),
            None
        );
        assert_eq!(
            provider_degradation_verdict(&healthy, Some(("e10", Some(1_100)))),
            None
        );
        let verdict =
            provider_degradation_verdict(&healthy, Some(("e10", Some(71_000)))).expect("degraded");
        assert_eq!(verdict.exchange, "e10");
        assert_eq!(
            verdict.cause,
            ProviderDegradationCause::TimedOutBehindProviderBacklog { backlog_ms: 71_000 }
        );
        assert_eq!(verdict.provider_input_backlog_ms, Some(71_000));
    }

    fn root() -> tempfile::TempDir {
        let root = super::super::workspace_root().join("target/e2e-live-audio-artifacts/offline");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::Builder::new()
            .prefix("journal-")
            .tempdir_in(root)
            .unwrap()
    }

    fn records(journal: &Journal) -> Vec<Envelope> {
        std::fs::read_to_string(journal.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn required_evidence(journal: &Journal) {
        let source = vec![
            meerkat_core::Message::System(meerkat_core::SystemMessage::new("FORBIDDEN_SYSTEM")),
            meerkat_core::Message::User(meerkat_core::UserMessage::text(
                "Historical vault phrase: amber otter copper. Current code word: Tangerine. Do not run anything.",
            )),
            meerkat_core::Message::BlockAssistant(meerkat_core::BlockAssistantMessage::new(
                vec![meerkat_core::AssistantBlock::Reasoning {
                    text: "FORBIDDEN_REASONING".into(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            )),
        ];
        journal.stage(Stage::HistoricalRecall).unwrap();
        journal
            .record(Record::Source {
                job: 1,
                channel: 1,
                canonical_cursor: 3,
                projection_sha256: meerkat_core::session::transcript_messages_digest(&source)
                    .unwrap(),
                owner_revision: OwnerRevision::NotExposedBySummarySnapshot,
                facts: selected_source_facts(&source, journal.expected_phrase()),
            })
            .unwrap();
        journal
            .record(Record::Summary {
                job: 1,
                text: "amber otter copper; credential-fixture-not-a-real-key".into(),
                expected_fact_present: true,
                input_tokens: 100,
                output_tokens: 20,
            })
            .unwrap();
        for event in [
            thinking_capture::EventKind::ThinkingAppendAttempt {
                client_event_id: "owned-1".into(),
                text: "amber otter copper".into(),
            },
            thinking_capture::EventKind::ThinkingAppended {
                client_event_id: Some("owned-1".into()),
                matched_owned: true,
                accepted: true,
            },
        ] {
            journal
                .record(Record::Thinking {
                    event: thinking_capture::Event {
                        channel_ordinal: 1,
                        elapsed_ms: 30,
                        event,
                    },
                })
                .unwrap();
        }
        let audio = AudioEvidence {
            decoded_non_silent_frames: 4800,
            decoded_non_silent_seconds: 0.2,
            bytes_received: 3000,
            packets_received: 30,
            ..Default::default()
        };
        journal
            .record(Record::Exchange {
                exchange: 1,
                channel: 1,
                stage: Stage::HistoricalRecall,
                start_event_index: 8,
                input_timeout_ms: 120_000,
                response_timeout_ms: 90_000,
                baseline: AudioEvidence::default(),
            })
            .unwrap();
        for (direction, delta) in [
            (Direction::Input, "What was my historical vault phrase?"),
            (
                Direction::Output,
                "I don't know yet credential-fixture-not-a-real-key",
            ),
        ] {
            journal
                .native(
                    1,
                    NativeRecord::Transcript {
                        direction,
                        delta: delta.into(),
                        event_index: 9,
                        browser_ms: 42.0,
                        provider_start_ms: Some(40.0),
                        audio,
                    },
                )
                .unwrap();
        }
        journal
            .record(Record::ExchangeEnd {
                exchange: 1,
                matched: false,
                audio,
            })
            .unwrap();
        journal.channel(1, ChannelAction::Closed).unwrap();
        journal.channel(2, ChannelAction::OpenRequested).unwrap();
        journal
            .record(Record::Job {
                job: 1,
                action: JobAction::Cancelled,
            })
            .unwrap();
    }

    fn assert_retained(journal: &Journal) {
        let text = std::fs::read_to_string(journal.path()).unwrap();
        assert!(text.contains("amber otter copper"));
        assert!(text.contains("[REDACTED_CREDENTIAL]"));
        for forbidden in [
            "credential-fixture-not-a-real-key",
            "FORBIDDEN_SYSTEM",
            "FORBIDDEN_REASONING",
            "Do not run anything",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        let records = records(journal);
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].sequence + 1 == pair[1].sequence)
        );
        assert!(
            records
                .iter()
                .any(|row| matches!(&row.record, Record::Source {
            canonical_cursor: 3, projection_sha256, facts, ..
        } if !projection_sha256.is_empty() && facts.len() == 2))
        );
        assert!(records.iter().any(|row| matches!(&row.record,
            Record::Thinking { event: thinking_capture::Event {
                event: thinking_capture::EventKind::ThinkingAppended {
                    client_event_id: Some(id), matched_owned: true, accepted: true,
                }, ..
            }} if id == "owned-1"
        )));
        assert!(records.iter().any(|row| matches!(&row.record,
            Record::Native { record: NativeRecord::Transcript { audio, .. }, .. }
                if audio.decoded_non_silent_frames == 4800
        )));
        assert!(records.iter().any(|row| matches!(
            &row.record,
            Record::Outcome {
                last_stage: Stage::HistoricalRecall,
                ..
            }
        )));
    }

    #[test]
    fn forced_failure_retains_whitelisted_evidence_after_scenario_cleanup() {
        let root = root();
        let scenario = tempfile::Builder::new().tempdir_in(root.path()).unwrap();
        let journal = Journal::at(
            &root.path().join("retained"),
            "amber otter copper".into(),
            vec!["credential-fixture-not-a-real-key".into()],
            Limits::default(),
        )
        .unwrap();
        assert!(!journal.path().starts_with(scenario.path()));
        required_evidence(&journal);
        journal.finish(Outcome::Failed).unwrap();
        drop(scenario);
        assert_retained(&journal);
    }

    #[tokio::test]
    async fn cancellation_flushes_before_tempdir_and_owner_drop() {
        let root = root();
        let journal = Journal::at(
            &root.path().join("retained"),
            "amber otter copper".into(),
            vec!["credential-fixture-not-a-real-key".into()],
            Limits::default(),
        )
        .unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let copy = journal.clone();
        let scenario = tempfile::Builder::new().tempdir_in(root.path()).unwrap();
        let scenario_path = scenario.path().to_owned();
        let task = tokio::spawn(async move {
            let _scenario = scenario;
            let _guard = FailureGuard(copy.clone());
            required_evidence(&copy);
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!scenario_path.exists());
        assert_retained(&journal);
        assert!(records(&journal).iter().any(|row| matches!(
            row.record,
            Record::Outcome {
                outcome: Outcome::CancelledOrPanicked,
                ..
            }
        )));
    }

    #[test]
    fn every_bound_is_visible_and_can_never_finish_as_success() {
        for (fault, limits, text) in [
            (
                Fault::StringLimit,
                Limits {
                    string_bytes: 128,
                    ..Default::default()
                },
                "x".repeat(129),
            ),
            (
                Fault::RecordBytesLimit,
                Limits {
                    record_bytes: 512,
                    ..Default::default()
                },
                "x".repeat(1024),
            ),
            (
                Fault::FileBytesLimit,
                Limits {
                    file_bytes: 8192,
                    ..Default::default()
                },
                "x".repeat(4096),
            ),
            (
                Fault::RecordLimit,
                Limits {
                    records: 5,
                    ..Default::default()
                },
                "short".into(),
            ),
        ] {
            let root = root();
            let journal =
                Journal::at(root.path(), "amber otter copper".into(), Vec::new(), limits).unwrap();
            let mut failed = false;
            for _ in 0..8 {
                let result = journal.record(Record::Summary {
                    job: 1,
                    text: text.clone(),
                    expected_fact_present: false,
                    input_tokens: 1,
                    output_tokens: 1,
                });
                if result.is_err() {
                    assert_eq!(result, Err(fault));
                    failed = true;
                    break;
                }
            }
            assert!(failed);
            assert_eq!(journal.finish(Outcome::Passed), Err(fault));
            let rows = records(&journal);
            assert!(rows.iter().any(
                |row| matches!(row.record, Record::Fault { fault: actual } if actual == fault)
            ));
            assert!(!rows.iter().any(|row| matches!(
                row.record,
                Record::Outcome {
                    outcome: Outcome::Passed,
                    ..
                }
            )));
            assert!(rows.len() <= limits.records);
            assert!(std::fs::metadata(journal.path()).unwrap().len() <= limits.file_bytes as u64);
        }
    }

    #[test]
    fn browser_overflow_and_late_records_invalidate_the_journal() {
        let root = root();
        let journal = Journal::at(
            root.path(),
            "amber otter copper".into(),
            Vec::new(),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            journal.native(
                1,
                NativeRecord::Fault {
                    fault: BrowserFault::QueueLimit,
                    audio: None,
                }
            ),
            Err(Fault::BrowserQueueLimit)
        );
        assert_eq!(
            journal.finish(Outcome::Passed),
            Err(Fault::BrowserQueueLimit)
        );
        let other = root.path().join("late");
        let journal = Journal::at(
            &other,
            "amber otter copper".into(),
            Vec::new(),
            Limits::default(),
        )
        .unwrap();
        journal.finish(Outcome::Failed).unwrap();
        assert_eq!(
            journal.record(Record::Stage {
                stage: Stage::Finished
            }),
            Err(Fault::AfterFinish)
        );
        assert_eq!(journal.check(), Err(Fault::AfterFinish));
    }

    #[test]
    fn soft_browser_faults_are_collected_without_invalidating_the_journal() {
        let root = root();
        let journal = Journal::at(
            root.path(),
            "amber otter copper".into(),
            Vec::new(),
            Limits::default(),
        )
        .unwrap();
        let overlap: NativeRecord = serde_json::from_value(serde_json::json!({
            "kind": "fault",
            "fault": {"overlap": {"ms": 700, "fixture": "standup_barge_in", "bound_ms": 300}},
            "audio": {
                "decoded_non_silent_frames": 1, "decoded_non_silent_seconds": 0.1,
                "non_silent_frames": 1, "total_audio_energy": null,
                "total_samples_received": null, "total_samples_duration": null,
                "bytes_received": 1, "packets_received": 1
            }
        }))
        .unwrap();
        journal.native(1, overlap).unwrap();
        let duplicate: NativeRecord = serde_json::from_value(serde_json::json!({
            "kind": "fault",
            "fault": {"duplicate_readout": {"text": "the first line is ready", "response": 3}}
        }))
        .unwrap();
        journal.native(1, duplicate).unwrap();
        assert_eq!(journal.faults().unwrap().len(), 2);
        assert!(
            journal
                .faults()
                .unwrap()
                .iter()
                .all(|fault| !fault.is_hard())
        );
        assert!(matches!(
            &journal.faults().unwrap()[0],
            BrowserFault::Overlap {
                ms: 700,
                bound_ms: 300,
                ..
            }
        ));
        let hard: NativeRecord =
            serde_json::from_value(serde_json::json!({"kind": "fault", "fault": "queue_limit"}))
                .unwrap();
        assert_eq!(journal.native(1, hard), Err(Fault::BrowserQueueLimit));
        journal
            .record(Record::Latency {
                channel: 1,
                turn: 1,
                input_final_to_audio_ms: Some(850),
                speech_end_to_audio_ms: Some(1400),
                input_final_end_ms: None,
                input_final_to_delegation_ms: None,
            })
            .unwrap_err();
        assert_eq!(
            journal.finish(Outcome::Passed),
            Err(Fault::BrowserQueueLimit)
        );
    }

    #[test]
    fn source_selection_excludes_injected_and_compaction_rows() {
        let mut compacted = meerkat_core::UserMessage::text("amber otter copper");
        compacted.transcript_role = meerkat_core::types::TranscriptUserRole::CompactionSummary;
        let messages = [
            meerkat_core::Message::User(meerkat_core::UserMessage::injected_context(
                "amber otter copper",
            )),
            meerkat_core::Message::User(compacted),
        ];
        assert!(selected_source_facts(&messages, "amber otter copper").is_empty());
    }

    #[test]
    fn redaction_only_removes_the_explicit_known_credential_values() {
        let root = root();
        let secrets = ["known-key-one", "known-key-two", "known-key-three"];
        let journal = Journal::at(
            root.path(),
            "amber otter copper".into(),
            secrets.iter().map(|value| (*value).to_owned()).collect(),
            Limits::default(),
        )
        .unwrap();
        journal.record(Record::Summary {
            job: 1,
            text: "amber otter copper known-key-one known-key-two known-key-three sk-synthetic-fact".into(),
            expected_fact_present: true,
            input_tokens: 1,
            output_tokens: 1,
        }).unwrap();
        journal.finish(Outcome::Failed).unwrap();
        let text = std::fs::read_to_string(journal.path()).unwrap();
        for secret in secrets {
            assert!(!text.contains(secret));
        }
        assert!(text.contains("amber otter copper"));
        assert!(text.contains("sk-synthetic-fact"));
        assert_eq!(text.matches("[REDACTED_CREDENTIAL]").count(), 3);
    }

    fn burst(text: &str, duration_ms: u64, acted: bool) -> OverlapBurst {
        OverlapBurst {
            started_ms: 21_900,
            last_active_ms: 21_900 + duration_ms,
            ended: true,
            overlap_ms: 400,
            text: text.to_owned(),
            window_text: text.to_owned(),
            yielded: true,
            acted,
        }
    }

    /// Peer facts for one burst in the user's pause: the user's last delta
    /// before it, its transcript arriving as it starts, the user resuming
    /// after it, and a delegation in its window when `acted`.
    fn pause_facts(text: &str, duration_ms: u64, acted: bool) -> OverlapFacts {
        let last_active_ms = 21_900 + duration_ms;
        OverlapFacts {
            now_ms: last_active_ms + 3_000,
            hysteresis_ms: 600,
            bursts: vec![BurstFact {
                started_ms: 21_900,
                last_active_ms,
                ended: true,
                overlap_ms: 400,
            }],
            output: vec![OutputDeltaFact {
                t_ms: 21_850,
                text: text.to_owned(),
            }],
            inputs: vec![21_000, last_active_ms + 700],
            delegations: if acted { vec![22_000] } else { Vec::new() },
        }
    }

    fn overlap(facts: OverlapFacts) -> BrowserFault {
        BrowserFault::Overlap {
            ms: 400,
            fixture: "interrupt_monologue".into(),
            bound_ms: 300,
            facts: Some(facts),
        }
    }

    fn allowed(facts: OverlapFacts) -> bool {
        let (faults, allowed) = reconcile_overlap_faults(vec![overlap(facts)]);
        assert_eq!(
            faults.is_empty(),
            !allowed.is_empty(),
            "{faults:?} {allowed:?}"
        );
        !allowed.is_empty()
    }

    /// A short "mm-hm" in the user's pause, after which the user resumed:
    /// an allowed backchannel, recorded, and no fault.
    #[test]
    fn backchannel_in_a_pause_is_allowed_and_recorded() {
        let (faults, allowed) =
            reconcile_overlap_faults(vec![overlap(pause_facts("Mm-hm.", 300, false))]);
        assert!(faults.is_empty(), "{faults:?}");
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0].0, "interrupt_monologue");
        assert_eq!(allowed[0].1.text, "Mm-hm.");
        assert_eq!(
            classify_overlap(400, &[burst("Okay, got it.", 500, false)]).counted_ms,
            0
        );
    }

    /// Acknowledgements and continuers seen live: BuildBuddy S100's "Sure."
    /// (said into the pause, then the user went on and the model acted only
    /// after the full request), S103's "Go ahead.", and a split "Mm-h mm."
    /// (two deltas joined into one word stream).
    #[test]
    fn backchannels_seen_live_are_allowed() {
        assert!(allowed(pause_facts(" Sure.", 300, false)));
        assert!(allowed(pause_facts(" Go ahead.", 400, false)));
        let mut split = pause_facts(" Mm-h", 300, false);
        split.output.push(OutputDeltaFact {
            t_ms: 21_900,
            text: " mm.".into(),
        });
        let bursts = overlap_bursts(&split);
        assert_eq!(bursts[0].text, "Mm-h mm.");
        assert!(allowed(split));
    }

    /// Phrases are matched whole: "On it." is made of words that appear in
    /// allowed phrases ("go on", "got it") but announces an action, so it
    /// counts, as does a continuer followed by an announced action.
    #[test]
    fn announced_action_in_a_pause_counts() {
        for text in [
            "On it.",
            "Okay, on it.",
            "Got it, on it.",
            "Sure, I'll book it.",
            "Will do.",
        ] {
            assert!(!allowed(pause_facts(text, 300, false)), "{text}");
        }
    }

    /// A reply that carries content ("got it, Tuesday afternoon") in a pause
    /// is not a backchannel: the overlap counts and the fault stays.
    #[test]
    fn content_bearing_reply_in_a_pause_fails() {
        assert!(!allowed(pause_facts(
            "Got it, Tuesday afternoon.",
            900,
            false
        )));
        // Too long to be a backchannel even with lexicon text.
        assert_eq!(
            classify_overlap(400, &[burst("Okay.", BACKCHANNEL_MAX_MS + 100, false)]).counted_ms,
            400
        );
        // A burst the model kept speaking after, without the user resuming,
        // did not yield and counts.
        let mut kept = pause_facts("Mm-hm.", 300, false);
        kept.inputs.retain(|t_ms| *t_ms < 21_900);
        assert!(!overlap_bursts(&kept)[0].yielded);
        assert!(!allowed(kept));
    }

    /// A delegation opened in the pause acts on a partial request: never a
    /// backchannel, whatever was said.
    #[test]
    fn delegation_opened_in_a_pause_fails() {
        assert!(!allowed(pause_facts("Okay.", 300, true)));
    }

    /// A transcript that has not arrived by the burst's window end leaves no
    /// evidence of what the burst said: it counts.
    #[test]
    fn late_transcript_counts() {
        let mut late = pause_facts("Tuesday afternoon, noted.", 300, false);
        late.output[0].t_ms = 21_900 + 300 + 600 + 1;
        assert_eq!(overlap_bursts(&late)[0].text, "");
        assert!(!allowed(late));
        let mut missing = pause_facts("", 300, false);
        missing.output.clear();
        assert!(!allowed(missing));
    }

    /// Over-absorption: burst A (3 s, before the fixture) is still within its
    /// window when B's transcript "Tuesday afternoon, noted" arrives, so the
    /// voicing queue gives those words to A and B's own evidence is empty or
    /// lexicon-only. The decision window still holds them: B counts.
    #[test]
    fn earlier_burst_absorbing_a_later_bursts_words_counts() {
        let facts = OverlapFacts {
            now_ms: 30_000,
            hysteresis_ms: 600,
            bursts: vec![
                BurstFact {
                    started_ms: 18_000,
                    last_active_ms: 21_000,
                    ended: true,
                    overlap_ms: 0,
                },
                BurstFact {
                    started_ms: 22_400,
                    last_active_ms: 23_200,
                    ended: true,
                    overlap_ms: 400,
                },
            ],
            output: vec![
                OutputDeltaFact {
                    t_ms: 17_900,
                    text: "Here is the plan.".into(),
                },
                OutputDeltaFact {
                    t_ms: 21_300,
                    text: " Tuesday afternoon, noted.".into(),
                },
                OutputDeltaFact {
                    t_ms: 22_300,
                    text: " Mm-hm.".into(),
                },
            ],
            inputs: vec![20_500, 21_200, 24_000],
            delegations: Vec::new(),
        };
        let bursts = overlap_bursts(&facts);
        assert_eq!(bursts.len(), 1);
        assert!(
            !bursts[0].text.contains("Tuesday"),
            "the queue gave A the words: {bursts:?}"
        );
        assert!(bursts[0].window_text.contains("Tuesday"), "{bursts:?}");
        assert!(!allowed(facts));
    }

    /// The leak case. Response N-1's transcript arrives ahead of its audio
    /// and burst A voices its start; the user resumes, which closes N-1 in
    /// the peer; N-1's remaining audio then plays as burst B over the
    /// fixture, while response N's own transcript is only "Mm-hm." B voiced
    /// N-1's content, so its overlap counts: arrival windows would have
    /// given B "Mm-hm." and allowed it.
    #[test]
    fn earlier_response_tail_voiced_after_user_resumed_counts() {
        let facts = OverlapFacts {
            now_ms: 30_000,
            hysteresis_ms: 600,
            bursts: vec![
                // A: 1.5 s holds at most six words ("The second note covers
                // the flaky").
                BurstFact {
                    started_ms: 20_000,
                    last_active_ms: 21_500,
                    ended: true,
                    overlap_ms: 0,
                },
                // B: 0.8 s in the user's pause holds four.
                BurstFact {
                    started_ms: 23_000,
                    last_active_ms: 23_800,
                    ended: true,
                    overlap_ms: 400,
                },
            ],
            output: vec![
                OutputDeltaFact {
                    t_ms: 19_900,
                    text: "The second note covers the flaky login test and the".into(),
                },
                OutputDeltaFact {
                    t_ms: 20_100,
                    text: " retry budget.".into(),
                },
                OutputDeltaFact {
                    t_ms: 22_900,
                    text: " Mm-hm.".into(),
                },
            ],
            inputs: vec![22_200, 24_600],
            delegations: Vec::new(),
        };
        let bursts = overlap_bursts(&facts);
        assert_eq!(bursts.len(), 1, "only B overlapped: {bursts:?}");
        assert_eq!(bursts[0].text, "login test and the", "{bursts:?}");
        assert!(!is_backchannel(&bursts[0]));
        assert!(!allowed(facts.clone()));

        // The same B after A voiced all of N-1 (A long enough for every
        // word): B voices only "Mm-hm." and is an allowed backchannel.
        let mut voiced = facts;
        voiced.bursts[0].last_active_ms = 22_200;
        voiced.bursts[0].started_ms = 19_000;
        let bursts = overlap_bursts(&voiced);
        assert_eq!(bursts[0].text, "Mm-hm.");
        assert!(allowed(voiced));
    }

    /// The recorded payload shape the peer emits (`fixture_end.detail.facts`)
    /// deserializes into the facts the join reads.
    #[test]
    fn peer_overlap_facts_payload_deserializes() {
        let facts: OverlapFacts = serde_json::from_value(serde_json::json!({
            "now_ms": 24_000,
            "hysteresis_ms": 600,
            "bursts": [{"started_ms": 21_900, "last_active_ms": 22_200, "ended": true, "overlap_ms": 400}],
            "output": [{"t_ms": 21_850, "text": " Go ahead."}],
            "inputs": [21_000, 22_900],
            "delegations": []
        }))
        .unwrap();
        assert_eq!(facts, {
            let mut expected = pause_facts(" Go ahead.", 300, false);
            expected.now_ms = 24_000;
            expected
        });
        assert!(is_backchannel(&overlap_bursts(&facts)[0]));
    }
}

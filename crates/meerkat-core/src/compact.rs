//! Compactor trait — provider-agnostic context compaction.
//!
//! The `Compactor` trait defines how and when to compact (summarize) the
//! conversation history to reclaim context window space. Implementations
//! live in `meerkat-session` (behind the `session-compaction` feature).

use crate::types::Message;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Metadata key used to persist compaction cadence across session reuse.
pub const SESSION_COMPACTION_CADENCE_KEY: &str = "session_compaction_cadence";

/// Durable session-scoped cadence state for compaction decisions.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct SessionCompactionCadence {
    /// Monotonic index of pre-LLM boundaries seen in this session.
    pub session_boundary_index: u64,
    /// Boundary index where compaction last completed successfully.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_compaction_boundary_index: Option<u64>,
    /// Boundary index where compaction was last attempted, successful or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_compaction_attempt_boundary_index: Option<u64>,
    /// Estimated history tokens the last compaction rewrite left: the
    /// committed rewrite's transcript, or the unchanged transcript of a
    /// rewrite rejected as a no-op. A failed attempt leaves it as it was.
    ///
    /// The evidence for [`CompactionContext::last_rewrite_history_tokens`].
    /// `None` before the first rewrite, when the transcript the last rewrite
    /// left could not be measured, and for a session last written by a binary
    /// before 0.9.4, which neither writes nor reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rewrite_history_tokens: Option<u64>,
}

/// Context provided to `Compactor::should_compact` for trigger decisions.
#[derive(Debug, Clone)]
pub struct CompactionContext {
    /// Input token count from the last LLM response.
    pub last_input_tokens: u64,
    /// Total number of messages in the session.
    pub message_count: usize,
    /// Estimated history tokens (JSON bytes / 4).
    pub estimated_history_tokens: u64,
    /// Estimated serialized size in bytes of the transcript as an LLM
    /// request (content byte lengths, inline media payloads included, times
    /// a documented safety factor — see
    /// `crate::agent::compact::estimate_request_bytes`).
    ///
    /// Providers cap requests in BYTES, not tokens. A byte-heavy/token-light
    /// transcript (2026-07-29 production incident: inline media pushed a
    /// transcript past Anthropic's request-size cap, failing every turn with
    /// `request_too_large` while the token trigger stayed far below its
    /// threshold) must be visible to trigger decisions in the same unit the
    /// provider enforces.
    pub estimated_request_bytes: u64,
    /// Whole-request token budget for the exact request this boundary would
    /// send, when the active model profile declares a context window.
    ///
    /// The two token measures above are not commensurable with the threshold
    /// they are compared against. `last_input_tokens` is the provider's own
    /// count, so it already includes tool schemas and provider framing, and it
    /// lags the transcript by whatever the current boundary appended.
    /// `estimated_history_tokens` measures the transcript alone: no tool
    /// definitions, no blob payloads a later hydration pass inlines, no output
    /// reserve. A threshold derived from the model context window has to hold
    /// the whole request, so comparing it against a strictly smaller quantity
    /// under-triggers by exactly the omitted components. This fact is the same
    /// measurement the pre-dispatch preflight performs, over the fully
    /// hydrated request messages, the exact visible tool set, and the effective
    /// output reserve.
    ///
    /// `None` is truthful rather than tolerant: custom and self-hosted models
    /// may declare no context window, and no substitute ceiling may be invented
    /// for them.
    pub request_context_budget: Option<crate::ContextBudgetFact>,
    /// Exact provider-lowered request-body pressure, when the active client can
    /// prove it. This is measured after ordered System-message projection,
    /// blob hydration, tool selection, provider-parameter resolution, replay
    /// projection, and provider-native JSON lowering.
    ///
    /// Custom clients default to `None`: an unavailable witness is truthful,
    /// while relabeling the transcript estimate as exact would make the
    /// compaction safety decision unsound.
    pub provider_request_pressure: Option<ProviderRequestPressure>,
    /// Session-scoped pre-LLM boundary index used by the cadence guard.
    ///
    /// This is the latest successful compaction boundary or failed compaction
    /// attempt boundary, whichever is newer.
    pub last_compaction_boundary_index: Option<u64>,
    /// Current session-scoped pre-LLM boundary index.
    pub session_boundary_index: u64,
    /// Estimated history tokens, in the unit of `estimated_history_tokens`,
    /// that the last compaction rewrite left, or `None` before the first and
    /// when what it left could not be measured.
    ///
    /// A rewrite always keeps the System rows, the in-flight request and the
    /// recent turns, and adds a summary. When what it left is still at or
    /// above the threshold, that floor alone crosses it, and compacting again
    /// cannot bring the history under it.
    pub last_rewrite_history_tokens: Option<u64>,
}

/// Provider-authored witness for the exact JSON body an invocation may send.
///
/// `encoded_bytes` is the largest serialized JSON body among all request
/// bodies the invocation can issue. This matters for clients such as OpenAI
/// Responses, where a continuation request may fall back to a full replay.
/// `max_bytes` is the active provider's conservative request-body cap, when
/// known. Both values are recomputed through the currently active fallback
/// candidate for every model boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRequestPressure {
    pub encoded_bytes: u64,
    pub max_bytes: Option<u64>,
    /// Exact provider-issued input-token count for this fully lowered request,
    /// when the provider exposes a synchronous counting API before dispatch.
    /// Byte measurement alone must never populate this field.
    pub provider_issued_input_tokens: Option<u64>,
    /// Explicit identity of the fully lowered request body. `None` is truthful
    /// for custom clients that expose only a byte count.
    pub lowered_request_provenance: Option<crate::LoweredRequestProvenance>,
}

impl ProviderRequestPressure {
    pub fn new(encoded_bytes: u64, max_bytes: Option<u64>) -> Self {
        Self {
            encoded_bytes,
            max_bytes,
            provider_issued_input_tokens: None,
            lowered_request_provenance: None,
        }
    }

    /// Attach an exact provider-issued input-token count.
    pub fn with_provider_issued_input_tokens(mut self, input_tokens: u64) -> Self {
        self.provider_issued_input_tokens = Some(input_tokens);
        self
    }

    pub fn with_lowered_request_provenance(
        mut self,
        provenance: crate::LoweredRequestProvenance,
    ) -> Self {
        self.lowered_request_provenance = Some(provenance);
        self
    }

    /// Effective request-body cap after combining provider and host policy.
    ///
    /// Either authority may be stricter. A configured cap is not allowed to
    /// widen the provider's real limit, and an inferred provider limit is not
    /// allowed to widen an explicit deployment policy.
    pub fn effective_cap(self, configured_cap: Option<u64>) -> Option<u64> {
        match (configured_cap, self.max_bytes) {
            (Some(configured), Some(provider)) => Some(configured.min(provider)),
            (Some(configured), None) => Some(configured),
            (None, Some(provider)) => Some(provider),
            (None, None) => None,
        }
    }

    /// Four-fifths of the effective request-size cap.
    pub fn trigger_threshold(self, configured_cap: Option<u64>) -> Option<u64> {
        self.effective_cap(configured_cap).map(|cap| {
            cap.saturating_mul(REQUEST_BYTE_TRIGGER_NUMERATOR) / REQUEST_BYTE_TRIGGER_DENOMINATOR
        })
    }
}

/// Result of a compaction rebuild.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The rebuilt message history (summary + retained recent turns).
    pub messages: Vec<Message>,
    /// The one runtime-validated summary inserted into the rebuilt transcript.
    ///
    /// This mapping is explicit so validation never infers the summary from an
    /// otherwise-unowned rebuilt slot. Public compactors must identify the
    /// exact typed summary message and its rebuilt offset.
    pub summary: CompactionSummary,
    /// Source messages retained in `messages`, paired with exact source and
    /// rebuilt offsets.
    ///
    /// Retained provenance is explicit rather than inferred by value. Message
    /// values are not unique, so a value-only diff cannot prove which copy of
    /// a duplicate was removed and which copy remains authoritative.
    pub retained: Vec<CompactionRetained>,
    /// Messages removed from history, paired with their canonical offsets in
    /// the pre-compaction transcript.
    ///
    /// A compactor must not return a discard-slice-local index: provenance is
    /// interpreted against the full [`CompactionWindow::messages`] history and
    /// validated before any memory projection is written.
    pub discarded: Vec<CompactionDiscard>,
}

/// A compactor-proposed summary and its exact rebuilt-transcript slot.
///
/// The core runtime validates this mapping, the typed role, canonical content,
/// and chronology before any transcript or memory projection is committed.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionSummary {
    /// Offset of `message` in the rebuilt transcript.
    pub rebuilt_offset: u64,
    /// Typed compaction-summary message inserted by the rebuild.
    pub message: Message,
}

impl CompactionSummary {
    /// Bind the inserted summary to its rebuilt-transcript slot.
    pub fn new(rebuilt_offset: u64, message: Message) -> Self {
        Self {
            rebuilt_offset,
            message,
        }
    }
}

/// Canonical rendered prefix for the typed compaction-summary boundary.
///
/// Keeping this in the core contract lets the validator prove that the one
/// inserted message contains the exact summary produced for this attempt.
pub const COMPACTION_SUMMARY_PREFIX: &str = "\
[Context compacted] A previous context produced the following summary of work so far. \
The current tool and session state is preserved. Use this summary to continue without \
duplicating work:\n\n";

/// One message removed by compaction plus its source transcript offset.
///
/// The source offset is part of the compactor contract because reconstructing
/// it from the returned discard slice loses retained messages outside the
/// discarded region and cannot represent non-contiguous custom compaction
/// strategies without guessing.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionDiscard {
    /// Offset of `message` in the full pre-compaction transcript.
    pub source_offset: u64,
    /// Message removed from the active transcript.
    pub message: Message,
}

impl CompactionDiscard {
    /// Bind a discarded message to its canonical pre-compaction offset.
    pub fn new(source_offset: u64, message: Message) -> Self {
        Self {
            source_offset,
            message,
        }
    }
}

/// One source message retained by compaction and its rebuilt-transcript slot.
///
/// Together, [`CompactionResult::retained`] and
/// [`CompactionResult::discarded`] must partition the full source transcript.
/// Retained rebuilt offsets must identify the same message in
/// [`CompactionResult::messages`]. This explicit mapping makes removal
/// provenance exact even when multiple source messages have identical values.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionRetained {
    /// Offset of `message` in the full pre-compaction transcript.
    pub source_offset: u64,
    /// Offset of `message` in the rebuilt transcript.
    pub rebuilt_offset: u64,
    /// Message retained from the source transcript: the source row itself,
    /// or, for a `ToolResults` row of the run still in flight, that row with
    /// oversized results replaced by their canonical shortened copies
    /// ([`shortened_tool_result_for_retention`]).
    pub message: Message,
}

impl CompactionRetained {
    /// Bind one retained source message to its rebuilt-transcript slot.
    pub fn new(source_offset: u64, rebuilt_offset: u64, message: Message) -> Self {
        Self {
            source_offset,
            rebuilt_offset,
            message,
        }
    }
}

const SHORTENED_TOOL_RESULT_MARKER_PREFIX: &str =
    "[compaction shortened this tool result: kept the first ";
const SHORTENED_TOOL_RESULT_MARKER_MIDDLE: &str = " of ";
const SHORTENED_TOOL_RESULT_MARKER_SUFFIX: &str =
    " bytes of its text; media blocks are shown as text placeholders]";

fn shortened_tool_result_marker(kept: usize, total: usize) -> String {
    format!(
        "{SHORTENED_TOOL_RESULT_MARKER_PREFIX}{kept}{SHORTENED_TOOL_RESULT_MARKER_MIDDLE}{total}{SHORTENED_TOOL_RESULT_MARKER_SUFFIX}"
    )
}

fn parse_shortened_tool_result_marker(marker: &str) -> Option<(usize, usize)> {
    let counts = marker
        .strip_prefix(SHORTENED_TOOL_RESULT_MARKER_PREFIX)?
        .strip_suffix(SHORTENED_TOOL_RESULT_MARKER_SUFFIX)?;
    let (kept, total) = counts.split_once(SHORTENED_TOOL_RESULT_MARKER_MIDDLE)?;
    Some((kept.parse().ok()?, total.parse().ok()?))
}

/// The text a shortened tool result keeps the head of: every content block's
/// text projection, in order, joined by newlines. Media blocks project to
/// text placeholders.
fn tool_result_text_projection(result: &crate::types::ToolResult) -> String {
    result
        .content
        .iter()
        .map(crate::types::ContentBlock::text_projection)
        .collect::<Vec<_>>()
        .join("\n")
}

fn shortened_tool_result_with_head(
    result: &crate::types::ToolResult,
    projection: &str,
    kept: usize,
) -> Option<crate::types::ToolResult> {
    let head = projection.get(..kept)?;
    let mut shortened = result.clone();
    shortened.content = vec![
        crate::types::ContentBlock::Text {
            text: head.to_string(),
        },
        crate::types::ContentBlock::Text {
            text: shortened_tool_result_marker(kept, projection.len()),
        },
    ];
    Some(shortened)
}

/// The canonical shortened copy of `result` a compaction may retain in place
/// of an oversized result of the run still in flight.
///
/// A compaction must not summarize away the run's own completed rounds: the
/// model would see only a paraphrase of a tool call it already made and could
/// make it again. When those rounds alone exceed the retention budget, each
/// oversized result is kept with its identity (`tool_use_id`, `is_error`,
/// settlement diagnostics and host metadata) and the first `max_text_bytes`
/// bytes of its text projection (cut back to a character boundary), followed
/// by a marker block that records how much was kept. Media blocks become
/// their text placeholders. The copy is deterministic, so
/// [`is_shortened_tool_result_of`] can verify it against the source.
pub fn shortened_tool_result_for_retention(
    result: &crate::types::ToolResult,
    max_text_bytes: usize,
) -> crate::types::ToolResult {
    let projection = tool_result_text_projection(result);
    let mut kept = max_text_bytes.min(projection.len());
    while !projection.is_char_boundary(kept) {
        kept -= 1;
    }
    shortened_tool_result_with_head(result, &projection, kept).unwrap_or_else(|| result.clone())
}

/// Whether `candidate` is exactly the canonical shortened copy of `source`
/// for the head length its marker records.
pub fn is_shortened_tool_result_of(
    candidate: &crate::types::ToolResult,
    source: &crate::types::ToolResult,
) -> bool {
    let Some(crate::types::ContentBlock::Text { text: marker }) = candidate.content.last() else {
        return false;
    };
    let Some((kept, total)) = parse_shortened_tool_result_marker(marker) else {
        return false;
    };
    let projection = tool_result_text_projection(source);
    total == projection.len()
        && shortened_tool_result_with_head(source, &projection, kept)
            .is_some_and(|expected| &expected == candidate)
}

/// Whether `retained` is the `ToolResults` row `source` with each result
/// either unchanged or replaced by its canonical shortened copy, and at least
/// one of them shortened. The only form of a retained row that differs from
/// its source row.
pub fn is_shortened_tool_results_row_of(retained: &Message, source: &Message) -> bool {
    let (
        Message::ToolResults {
            results: kept,
            created_at: kept_at,
        },
        Message::ToolResults {
            results,
            created_at,
        },
    ) = (retained, source)
    else {
        return false;
    };
    kept_at == created_at
        && kept.len() == results.len()
        && kept != results
        && kept
            .iter()
            .zip(results)
            .all(|(kept, source)| kept == source || is_shortened_tool_result_of(kept, source))
}

/// Fraction of `CompactionConfig::max_request_bytes` at which the byte-aware
/// trigger fires, mirroring the model-aware token default (4/5 of the model
/// context window). The remaining 1/5 is headroom for the active turn's new
/// content plus request components the estimate cannot see (tool schemas,
/// provider parameters).
const REQUEST_BYTE_TRIGGER_NUMERATOR: u64 = 4;
const REQUEST_BYTE_TRIGGER_DENOMINATOR: u64 = 5;

/// Configuration for the default compactor implementation.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Compaction triggers when `last_input_tokens >= auto_compact_threshold`.
    pub auto_compact_threshold: u64,
    /// Provider request-size cap in bytes, when known.
    ///
    /// `None` disables the byte-aware trigger (pre-incident behavior). When
    /// set, compaction also triggers when the estimated serialized request
    /// size crosses [`CompactionConfig::request_byte_trigger_threshold`]
    /// (4/5 of this cap). The token trigger alone missed the 2026-07-29
    /// production incident: providers cap requests in bytes, and byte-heavy /
    /// token-light content (inline media) crosses the byte cap first, after
    /// which every turn fails terminally with `request_too_large`.
    pub max_request_bytes: Option<u64>,
    /// Number of recent complete turns to retain after compaction.
    pub recent_turn_budget: usize,
    /// Maximum tokens for the compaction summary LLM response.
    pub max_summary_tokens: u32,
    /// Minimum session-scoped LLM boundaries between ordinary consecutive
    /// compactions. Built-in compaction bypasses this cost guard when the
    /// current history or exact provider request has already crossed a live
    /// capacity threshold; cadence cannot veto recovery.
    pub min_turns_between_compactions: u32,
    /// Recent transcript rewrites whose bodies the session graph keeps after
    /// a compaction; older ones are retired. Zero is treated as one: the
    /// newest rewrite is always retained.
    pub history_retained_rewrites: usize,
}

impl CompactionConfig {
    /// The typed retention bound for [`Self::history_retained_rewrites`].
    #[must_use]
    pub fn transcript_history_retention(&self) -> crate::TranscriptHistoryRetention {
        crate::TranscriptHistoryRetention::from_count(self.history_retained_rewrites.max(1))
            .unwrap_or_default()
    }
}

impl CompactionConfig {
    /// Effective byte-trigger threshold: 4/5 of `max_request_bytes`, or
    /// `None` when no request-size cap is configured.
    ///
    /// This is the one owner of the byte-trigger arithmetic so every
    /// compactor evaluates the same fraction of the same cap.
    pub fn request_byte_trigger_threshold(&self) -> Option<u64> {
        self.max_request_bytes.map(|cap| {
            cap.saturating_mul(REQUEST_BYTE_TRIGGER_NUMERATOR) / REQUEST_BYTE_TRIGGER_DENOMINATOR
        })
    }
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact_threshold: 100_000,
            max_request_bytes: None,
            recent_turn_budget: 4,
            max_summary_tokens: 4096,
            min_turns_between_compactions: 3,
            history_retained_rewrites: crate::TranscriptHistoryRetention::DEFAULT_RETAINED_REWRITES,
        }
    }
}

/// Serialized source bytes a model-derived summarization budget allows per
/// token of the summarizing model's input window: half the transcript
/// estimator's four bytes per token, so a source that tokenizes densely
/// (code, JSON, non-Latin scripts) still fits the window.
const SUMMARIZATION_SOURCE_BYTES_PER_TOKEN: u64 = 2;
/// Fraction of the compaction threshold available to the source when the
/// summarizing model declares no context window. The threshold is normally
/// four-fifths of a context window; a quarter of it leaves room for JSON
/// escaping, the compaction prompt and the summary response, even when one
/// tool result jumps from a healthy request past the window in a single
/// boundary.
const THRESHOLD_SUMMARIZATION_SOURCE_DENOMINATOR: u64 = 4;
/// The smallest summarization source budget.
const MIN_SUMMARIZATION_SOURCE_BYTES: u64 = 4 * 1024;

/// Serialized-byte budget for the transcript a compactor sends to the model
/// that writes the compaction summary.
///
/// This type is the one owner of the summarization source arithmetic. The
/// agent's compaction seam derives it from the summarizing client's
/// registry-owned limits ([`Self::for_summary_model`]); for a model that
/// declares no context window a compactor falls back to its compaction
/// threshold ([`Self::from_compaction_threshold`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummarizationSourceBudget {
    bytes: u64,
}

impl SummarizationSourceBudget {
    /// The budget for a summary written by `model`: the input side of its
    /// context window (the window less the `max_summary_tokens` output
    /// reserve, capped by a separately declared input ceiling) at two
    /// serialized bytes per token, less the `prompt` sent after the source,
    /// and at least 4 KiB. `None` when `model` declares no context window.
    #[must_use]
    pub fn for_summary_model(
        model: &crate::ModelProfileWitness,
        max_summary_tokens: u32,
        prompt: &str,
    ) -> Option<Self> {
        let window = u64::from(model.context_window()?);
        let mut input_tokens = window.saturating_sub(u64::from(max_summary_tokens));
        if let Some(ceiling) = model.max_input_tokens() {
            input_tokens = input_tokens.min(u64::from(ceiling));
        }
        let prompt_bytes = u64::try_from(prompt.len()).unwrap_or(u64::MAX);
        Some(Self::at_least_minimum(
            input_tokens
                .saturating_mul(SUMMARIZATION_SOURCE_BYTES_PER_TOKEN)
                .saturating_sub(prompt_bytes),
        ))
    }

    /// The budget when the summarizing model declares no context window: a
    /// quarter of the compaction `threshold`, at least 4 KiB.
    #[must_use]
    pub fn from_compaction_threshold(threshold: u64) -> Self {
        Self::at_least_minimum(threshold.div_ceil(THRESHOLD_SUMMARIZATION_SOURCE_DENOMINATOR))
    }

    fn at_least_minimum(bytes: u64) -> Self {
        Self {
            bytes: bytes.max(MIN_SUMMARIZATION_SOURCE_BYTES),
        }
    }

    /// The budget in serialized bytes of the source messages.
    #[must_use]
    pub fn bytes(self) -> u64 {
        self.bytes
    }
}

/// How much of the history being compacted a summarization source carries.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SummarizationSourceBound {
    /// Every message, nothing cut to fit the budget.
    Whole,
    /// Every message, with the texts too large for the budget cut around an
    /// omission marker the summarizing model sees.
    Trimmed {
        /// The last kind of text cut to fit.
        through: SummarizationTrimStage,
    },
    /// Only the newest `kept` of the history's `of` user requests; everything
    /// else is omitted.
    RequestsOnly {
        /// User requests the source carries.
        kept: usize,
        /// User requests in the history being compacted.
        of: usize,
    },
}

/// The kinds of text a summarization source is trimmed through, in order:
/// tool results first, user requests last.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SummarizationTrimStage {
    /// Tool results.
    ToolResults,
    /// System prompts and assistant text, with oversized tool-call arguments
    /// replaced by their size.
    Context,
    /// User requests.
    UserRequests,
}

/// The transcript a compactor sends to the summarizing model, and how much of
/// the history it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct SummarizationSource {
    /// The source messages. The compaction prompt is sent after them.
    pub messages: Vec<Message>,
    /// How much of the history `messages` carries.
    pub bound: SummarizationSourceBound,
}

impl SummarizationSource {
    /// A source that carries the whole history.
    #[must_use]
    pub fn whole(messages: Vec<Message>) -> Self {
        Self {
            messages,
            bound: SummarizationSourceBound::Whole,
        }
    }
}

/// Provider-agnostic compaction strategy.
///
/// Determines when to compact and how to rebuild the history after summarization.
pub trait Compactor: Send + Sync {
    /// Check whether compaction should run given the current context.
    fn should_compact(&self, ctx: &CompactionContext) -> bool;

    /// Effective hard request-body cap for post-compaction fit checks.
    ///
    /// Custom compactors inherit the provider witness. Implementations with a
    /// stricter host-configured cap override this so trigger and fit semantics
    /// use the same authority.
    fn request_byte_cap(&self, pressure: ProviderRequestPressure) -> Option<u64> {
        pressure.max_bytes
    }

    /// How many recent transcript rewrites keep their bodies in the session
    /// graph after a compaction commits; older ones are retired (commits and
    /// digests kept). Defaults to
    /// [`crate::TranscriptHistoryRetention::DEFAULT_RETAINED_REWRITES`].
    fn transcript_history_retention(&self) -> crate::TranscriptHistoryRetention {
        crate::TranscriptHistoryRetention::default()
    }

    /// Return the prompt to send to the LLM for summarization.
    fn compaction_prompt(&self) -> &str;

    /// Maximum tokens the summarization response may consume.
    fn max_summary_tokens(&self) -> u32;

    /// Prepare the source for the summarization LLM call.
    ///
    /// Called before sending the history to the LLM for summarization.
    /// Implementations may strip content that is not suitable for the
    /// summarization pass (e.g. base64-encoded images), and should drop
    /// superseded versions of typed keyed system prompts
    /// ([`crate::types::materialize_latest_system_prompt_versions`]).
    ///
    /// `budget` is the serialized size the model that writes the summary can
    /// take for its source ([`SummarizationSourceBudget::for_summary_model`]);
    /// `None` when that model declares no context window. A source larger
    /// than the budget makes the summary request exceed the model's window.
    /// The returned source states how much of the history it carries; the
    /// compaction completion reports it.
    fn prepare_for_summarization(
        &self,
        messages: &[Message],
        budget: Option<SummarizationSourceBudget>,
    ) -> SummarizationSource;

    /// Rebuild history while carrying the exact provider request pressure that
    /// triggered compaction, when available.
    ///
    /// The default preserves the original custom-compactor contract. Built-in
    /// compaction uses the pressure witness to ensure its exact retained tail
    /// cannot itself recreate the oversized request it is meant to repair.
    fn rebuild_history_under_pressure(
        &self,
        messages: &[Message],
        summary: &str,
        _pressure: Option<ProviderRequestPressure>,
    ) -> CompactionResult {
        self.rebuild_history(messages, summary)
    }

    /// Rebuild the session history from a summary and current messages.
    ///
    /// The implementation should:
    /// 1. Preserve every unkeyed and latest-version `Message::System` verbatim
    ///    and in relative source order; discard superseded keyed versions.
    /// 2. Inject a summary message.
    /// 3. Retain recent complete turns per `recent_turn_budget`.
    /// 4. Return retained source messages as `retained`, with their offsets in
    ///    both the full source slice and rebuilt history.
    /// 5. Return everything else as `discarded`, with each message's offset in
    ///    the full `messages` slice.
    ///
    /// `retained` and `discarded` must partition the source transcript. The
    /// `summary` must identify the one injected message at the rebuilt position
    /// of the first discarded source row: every retained source row before that
    /// boundary remains before the summary, and every retained source row after
    /// it remains after the summary. Its typed role and text must match the
    /// exact summary supplied to this method. At least one source message must
    /// be discarded, and the rebuild must not grow the transcript.
    fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult;
}

/// Borrowed view of the compaction inputs handed to a [`CompactionCurator`].
///
/// This is exactly the data the agent-loop compaction flow already holds when
/// it would otherwise run the summarization LLM call: the full current
/// transcript (including every ordered System message and any prior typed
/// compaction-summary user message), the last observed input token count, and
/// the session-scoped boundary index at which compaction runs.
#[derive(Debug, Clone, Copy)]
pub struct CompactionWindow<'a> {
    /// Full current transcript messages.
    pub messages: &'a [Message],
    /// Input token count from the last LLM response.
    pub last_input_tokens: u64,
    /// Session-scoped pre-LLM boundary index at which compaction runs.
    pub session_boundary_index: u64,
}

/// Validated non-empty summary text produced by a [`CompactionCurator`].
///
/// The fallible constructor is the only way to mint a value, so a curated
/// summary can never smuggle an empty string past the typed contract and
/// re-create the empty-summary failure mode downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratedCompactionSummary(String);

impl CuratedCompactionSummary {
    /// Construct from summary text; whitespace-only text is rejected.
    pub fn new(text: impl Into<String>) -> Result<Self, CompactionCuratorError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(CompactionCuratorError::EmptySummary);
        }
        Ok(Self(text))
    }

    /// The summary text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume into the owned summary text.
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Errors from a host-supplied compaction curator.
#[derive(Debug, thiserror::Error)]
pub enum CompactionCuratorError {
    /// The curator produced an empty summary, so there is nothing to commit.
    #[error("curator produced an empty compaction summary")]
    EmptySummary,
    /// The curator failed to produce a summary.
    #[error("curator failed to produce a compaction summary: {0}")]
    Failed(String),
}

/// Host-supplied compaction summary curation.
///
/// When configured on the agent, the compaction flow asks the curator to
/// produce the summary text INSTEAD of running the summarization LLM call
/// (`prepare_for_summarization` and the client `stream_response` call are
/// skipped entirely; the recorded summary usage is zero).
///
/// The compaction TRIGGER stays machine-emitted (`CheckCompaction`) and gated
/// by [`Compactor::should_compact`]; the curator substitutes summary CONTENT
/// production only. There is no LLM fallback: a failing curator surfaces a
/// typed `CompactionFailed` event and the original history is preserved.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait CompactionCurator: Send + Sync {
    /// Produce the compaction summary for the supplied window.
    async fn curate_summary(
        &self,
        window: CompactionWindow<'_>,
    ) -> Result<CuratedCompactionSummary, CompactionCuratorError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn curated_compaction_summary_rejects_empty_text() {
        assert!(matches!(
            CuratedCompactionSummary::new(""),
            Err(CompactionCuratorError::EmptySummary)
        ));
        assert!(matches!(
            CuratedCompactionSummary::new("   \n\t"),
            Err(CompactionCuratorError::EmptySummary)
        ));
    }

    #[test]
    fn curated_compaction_summary_round_trips_text() {
        let summary = CuratedCompactionSummary::new("curated summary").unwrap();
        assert_eq!(summary.as_str(), "curated summary");
        assert_eq!(summary.into_string(), "curated summary");
    }

    fn summary_model(
        context_window: Option<u32>,
        max_input_tokens: Option<u32>,
    ) -> crate::ModelProfileWitness {
        let mut config = crate::Config::default();
        config.models.custom.insert(
            "summary-budget-model".to_string(),
            crate::config::CustomModelConfig {
                provider: crate::Provider::OpenAI,
                display_name: None,
                context_window,
                max_input_tokens,
                max_output_tokens: Some(8_192),
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            },
        );
        crate::ModelRegistry::from_config(
            &config,
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .unwrap()
        .profile_witness_for_provider(crate::Provider::OpenAI, "summary-budget-model")
        .unwrap()
    }

    #[test]
    fn summarization_source_budget_is_two_bytes_per_token_of_the_input_window() {
        // The window less the summary's output reserve, less the prompt.
        let budget = SummarizationSourceBudget::for_summary_model(
            &summary_model(Some(200_000), None),
            4_096,
            "summarize",
        )
        .unwrap();
        assert_eq!(budget.bytes(), (200_000 - 4_096) * 2 - 9);

        // A separate input ceiling caps the input side.
        let budget = SummarizationSourceBudget::for_summary_model(
            &summary_model(Some(200_000), Some(50_000)),
            4_096,
            "",
        )
        .unwrap();
        assert_eq!(budget.bytes(), 50_000 * 2);

        // An output reserve that fills the window leaves the 4 KiB floor.
        let budget = SummarizationSourceBudget::for_summary_model(
            &summary_model(Some(4_000), None),
            4_096,
            "",
        )
        .unwrap();
        assert_eq!(budget.bytes(), 4 * 1024);

        // A model without a declared window has no model budget.
        assert_eq!(
            SummarizationSourceBudget::for_summary_model(&summary_model(None, None), 4_096, ""),
            None
        );
    }

    #[test]
    fn summarization_source_budget_from_the_threshold_is_a_quarter_with_a_floor() {
        assert_eq!(
            SummarizationSourceBudget::from_compaction_threshold(160_000).bytes(),
            40_000
        );
        assert_eq!(
            SummarizationSourceBudget::from_compaction_threshold(1_000).bytes(),
            4 * 1024
        );
    }

    #[test]
    fn request_byte_trigger_threshold_is_four_fifths_of_the_cap() {
        let config = CompactionConfig {
            max_request_bytes: Some(10_000_000),
            ..CompactionConfig::default()
        };
        assert_eq!(config.request_byte_trigger_threshold(), Some(8_000_000));

        // Default None keeps the byte trigger disabled (pre-incident behavior).
        assert_eq!(
            CompactionConfig::default().request_byte_trigger_threshold(),
            None
        );
    }

    #[test]
    fn provider_and_configured_request_caps_compose_by_minimum() {
        let pressure = ProviderRequestPressure::new(1, Some(8_000_000));
        assert_eq!(pressure.effective_cap(Some(10_000_000)), Some(8_000_000));
        assert_eq!(pressure.effective_cap(Some(6_000_000)), Some(6_000_000));
        assert_eq!(
            pressure.trigger_threshold(Some(10_000_000)),
            Some(6_400_000)
        );
        assert_eq!(pressure.trigger_threshold(Some(6_000_000)), Some(4_800_000));
        assert_eq!(
            ProviderRequestPressure::new(1, None).effective_cap(Some(6_000_000)),
            Some(6_000_000)
        );
        assert_eq!(
            ProviderRequestPressure::new(1, Some(8_000_000)).effective_cap(None),
            Some(8_000_000)
        );
    }

    #[test]
    fn byte_only_constructor_does_not_claim_provider_token_truth() {
        let byte_only = ProviderRequestPressure::new(2_400, None);
        assert_eq!(byte_only.provider_issued_input_tokens, None);

        let counted = byte_only.with_provider_issued_input_tokens(731);
        assert_eq!(counted.provider_issued_input_tokens, Some(731));
    }
}

//! Agent compaction — runs context compaction during the agent loop.
//!
//! Called from the agent state machine when a compactor is configured and
//! the threshold is met.

use crate::Session;
use crate::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionCurator, CompactionDiscard,
    CompactionRetained, CompactionSummary, CompactionWindow, Compactor,
    SESSION_COMPACTION_CADENCE_KEY, SessionCompactionCadence,
};
use crate::event::AgentEvent;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::types::{AssistantBlock, Message, Usage};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Errors that can occur during compaction.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    /// The LLM call for summarization failed.
    #[error("compaction LLM call failed: {0}")]
    LlmFailed(#[from] crate::error::AgentError),

    /// The LLM returned an empty summary.
    #[error("LLM returned empty summary")]
    EmptySummary,

    /// The host-supplied compaction curator failed to produce a summary.
    /// There is no LLM fallback: the original history is preserved.
    #[error("compaction curator failed: {0}")]
    CuratorFailed(#[from] crate::compact::CompactionCuratorError),

    /// Failed to estimate token count (serialization error).
    #[error("token estimation failed: {0}")]
    EstimationFailed(String),

    /// The compactor returned discard provenance that does not identify the
    /// same ordered messages in the full pre-compaction transcript.
    #[error("invalid compaction rebuild: {0}")]
    InvalidRebuild(String),
}

fn checked_offset(kind: &str, offset: u64, upper_bound: usize) -> Result<usize, CompactionError> {
    let offset = usize::try_from(offset).map_err(|_| {
        CompactionError::InvalidRebuild(format!("{kind} offset {offset} does not fit this target"))
    })?;
    if offset >= upper_bound {
        return Err(CompactionError::InvalidRebuild(format!(
            "{kind} offset {offset} is outside the {upper_bound}-message transcript"
        )));
    }
    Ok(offset)
}

fn validate_compaction_rebuild(
    messages: &[Message],
    rebuilt: &[Message],
    summary: &CompactionSummary,
    summary_text: &str,
    retained: &[CompactionRetained],
    discarded: &[CompactionDiscard],
) -> Result<(), CompactionError> {
    #[derive(Clone, Copy)]
    enum SourceDisposition {
        Retained,
        Discarded,
    }

    if discarded.is_empty() {
        return Err(CompactionError::InvalidRebuild(
            "compaction must discard at least one source message".to_string(),
        ));
    }
    let mut source_dispositions = vec![None; messages.len()];
    let mut previous_discard_offset = None;
    for discard in discarded {
        if previous_discard_offset.is_some_and(|previous| previous >= discard.source_offset) {
            return Err(CompactionError::InvalidRebuild(format!(
                "discard source offsets must be strictly increasing, got {} after {:?}",
                discard.source_offset, previous_discard_offset
            )));
        }
        let source_offset =
            checked_offset("discard source", discard.source_offset, messages.len())?;
        let source_message = &messages[source_offset];
        if source_message != &discard.message {
            return Err(CompactionError::InvalidRebuild(format!(
                "discard at source offset {} does not match the source transcript message",
                discard.source_offset
            )));
        }
        if source_dispositions[source_offset]
            .replace(SourceDisposition::Discarded)
            .is_some()
        {
            return Err(CompactionError::InvalidRebuild(format!(
                "source offset {} has more than one compaction disposition",
                discard.source_offset
            )));
        }
        previous_discard_offset = Some(discard.source_offset);
    }

    let mut rebuilt_owners = vec![None; rebuilt.len()];
    let mut previous_source_offset = None;
    let mut previous_rebuilt_offset = None;
    for retention in retained {
        if previous_source_offset.is_some_and(|previous| previous >= retention.source_offset) {
            return Err(CompactionError::InvalidRebuild(format!(
                "retained source offsets must be strictly increasing, got {} after {:?}",
                retention.source_offset, previous_source_offset
            )));
        }
        if previous_rebuilt_offset.is_some_and(|previous| previous >= retention.rebuilt_offset) {
            return Err(CompactionError::InvalidRebuild(format!(
                "retained rebuilt offsets must be strictly increasing, got {} after {:?}",
                retention.rebuilt_offset, previous_rebuilt_offset
            )));
        }
        let source_offset =
            checked_offset("retained source", retention.source_offset, messages.len())?;
        let rebuilt_offset =
            checked_offset("retained rebuilt", retention.rebuilt_offset, rebuilt.len())?;
        if messages[source_offset] != retention.message {
            return Err(CompactionError::InvalidRebuild(format!(
                "retention at source offset {} does not match the source transcript message",
                retention.source_offset
            )));
        }
        if matches!(
            &retention.message,
            Message::User(user) if user.transcript_role.is_compaction_summary()
        ) {
            return Err(CompactionError::InvalidRebuild(format!(
                "prior compaction summary at source offset {} must be discarded, not retained",
                retention.source_offset
            )));
        }
        if rebuilt[rebuilt_offset] != retention.message {
            return Err(CompactionError::InvalidRebuild(format!(
                "retention from source offset {} does not match rebuilt offset {}",
                retention.source_offset, retention.rebuilt_offset
            )));
        }
        if source_dispositions[source_offset]
            .replace(SourceDisposition::Retained)
            .is_some()
        {
            return Err(CompactionError::InvalidRebuild(format!(
                "source offset {} has more than one compaction disposition",
                retention.source_offset
            )));
        }
        if rebuilt_owners[rebuilt_offset]
            .replace(retention.source_offset)
            .is_some()
        {
            return Err(CompactionError::InvalidRebuild(format!(
                "rebuilt offset {} is claimed by more than one retained source message",
                retention.rebuilt_offset
            )));
        }
        previous_source_offset = Some(retention.source_offset);
        previous_rebuilt_offset = Some(retention.rebuilt_offset);
    }

    if let Some(missing_offset) = source_dispositions.iter().position(Option::is_none) {
        return Err(CompactionError::InvalidRebuild(format!(
            "source offset {missing_offset} is neither retained nor discarded"
        )));
    }
    if rebuilt.len() > messages.len() {
        return Err(CompactionError::InvalidRebuild(format!(
            "compaction must not grow the transcript from {} to {} messages",
            messages.len(),
            rebuilt.len()
        )));
    }

    let summary_offset = checked_offset("summary rebuilt", summary.rebuilt_offset, rebuilt.len())?;
    if rebuilt[summary_offset] != summary.message {
        return Err(CompactionError::InvalidRebuild(format!(
            "summary mapping does not match rebuilt offset {}",
            summary.rebuilt_offset
        )));
    }
    let first_discarded_source_offset = discarded[0].source_offset;
    let expected_summary_offset = retained
        .iter()
        .take_while(|retention| retention.source_offset < first_discarded_source_offset)
        .count();
    if summary_offset != expected_summary_offset {
        return Err(CompactionError::InvalidRebuild(format!(
            "summary must replace the first discarded source position at rebuilt offset {expected_summary_offset}, found {summary_offset}"
        )));
    }
    for (source_offset, message) in messages.iter().enumerate() {
        if !matches!(message, Message::System(_)) {
            continue;
        }
        let source_offset = u64::try_from(source_offset).map_err(|_| {
            CompactionError::InvalidRebuild(
                "source System offset exceeds the compaction mapping range".to_string(),
            )
        })?;
        if !retained
            .iter()
            .any(|retention| retention.source_offset == source_offset)
        {
            return Err(CompactionError::InvalidRebuild(format!(
                "ordered System message at source offset {source_offset} must be retained verbatim"
            )));
        }
    }
    let Message::User(summary_user) = &summary.message else {
        return Err(CompactionError::InvalidRebuild(
            "inserted compaction summary must be a user message".to_string(),
        ));
    };
    if !summary_user.transcript_role.is_compaction_summary() {
        return Err(CompactionError::InvalidRebuild(
            "inserted compaction summary must carry the typed compaction-summary role".to_string(),
        ));
    }
    let expected_summary_text = format!("{COMPACTION_SUMMARY_PREFIX}{summary_text}");
    if summary_user.content != crate::types::ContentBlock::text_vec(expected_summary_text)
        || summary_user.render_metadata.is_some()
        || !summary_user.identity.is_empty()
    {
        return Err(CompactionError::InvalidRebuild(
            "inserted compaction summary content does not exactly match the produced summary"
                .to_string(),
        ));
    }

    let unmapped_rebuilt = rebuilt_owners
        .iter()
        .enumerate()
        .filter_map(|(offset, owner)| owner.is_none().then_some(offset))
        .collect::<Vec<_>>();
    if unmapped_rebuilt.as_slice() != [summary_offset] {
        return Err(CompactionError::InvalidRebuild(format!(
            "rebuilt transcript must contain only the mapped summary as an inserted message, found unmapped offsets {unmapped_rebuilt:?}"
        )));
    }
    Ok(())
}

/// Approximate token cost per image block.
///
/// Anthropic charges ~1600 tokens for a standard image regardless of
/// resolution. Using a fixed estimate avoids counting raw base64 bytes
/// (which inflate the estimate by ~200x).
const IMAGE_TOKEN_ESTIMATE: u64 = 1_600;

/// Safety factor (numerator/denominator = 1.2×) applied to summed content
/// bytes when estimating the serialized request size.
///
/// The exact request size is only known at provider request-build time (each
/// provider crate shapes its own body), and re-serializing multi-megabyte
/// inline media on every pre-LLM boundary would double the hot-path cost the
/// estimator exists to avoid. Content lengths are therefore summed directly
/// and inflated by 20% to cover the JSON envelope (keys, quoting, string
/// escaping) plus request components outside the transcript (tool schemas,
/// provider parameters). Base64 payloads never need JSON escaping, so on the
/// media-dominated transcripts this estimate protects against (2026-07-29
/// household incident: inline media crossed Anthropic's request-size cap and
/// every turn failed with `request_too_large`) the raw sum is within a few
/// percent of the wire size and the factor is pure margin.
const REQUEST_BYTE_SAFETY_NUMERATOR: u64 = 6;
const REQUEST_BYTE_SAFETY_DENOMINATOR: u64 = 5;

/// Combined single-walk estimate of transcript pressure in both units the
/// runtime triggers on: tokens (model context window) and serialized request
/// bytes (provider request-size cap).
struct TranscriptPressure {
    tokens: u64,
    /// Safety-adjusted serialized request size estimate.
    request_bytes: u64,
}

fn estimate_inline_video_tokens(data: &str) -> u64 {
    let len = data.len() as u64;
    if len > 0 { (len / 4).max(1) } else { 0 }
}

fn estimate_video_duration_tokens(duration_ms: u64) -> u64 {
    // Rough default-resolution Gemini heuristic, scaled by caller-provided duration.
    duration_ms.saturating_mul(300).div_ceil(1000)
}

/// Estimate a content-block slice in both trigger units, returning
/// `(tokens, content_bytes)`.
///
/// Tokens use fixed image/video heuristics instead of counting raw base64
/// payload bytes (which would massively overcount); text blocks use
/// `text_projection().len() / 4`. Content bytes count what actually rides the
/// wire — inline media payloads at full base64 length (the byte-heavy /
/// token-light content class from the 2026-07-29 request_too_large incident)
/// and text projections for everything else. Blob-backed media count only
/// their reference projection: the payload a later hydration pass inlines is
/// not visible here without an async blob-store read.
fn estimate_content_blocks(blocks: &[crate::types::ContentBlock]) -> (u64, u64) {
    let mut tokens: u64 = 0;
    let mut content_bytes: u64 = 0;
    for block in blocks {
        match block {
            crate::types::ContentBlock::Image {
                data: crate::types::ImageData::Inline { data },
                ..
            } => {
                tokens += IMAGE_TOKEN_ESTIMATE;
                content_bytes += data.len() as u64;
            }
            crate::types::ContentBlock::Image { .. } => {
                tokens += IMAGE_TOKEN_ESTIMATE;
                content_bytes += block.text_projection().len() as u64;
            }
            crate::types::ContentBlock::Video {
                duration_ms,
                data: crate::types::VideoData::Inline { data },
                ..
            } => {
                tokens += estimate_inline_video_tokens(data)
                    .max(estimate_video_duration_tokens(*duration_ms));
                content_bytes += data.len() as u64;
            }
            crate::types::ContentBlock::Video {
                duration_ms,
                data: crate::types::VideoData::Uri { uri },
                ..
            } => {
                tokens += estimate_video_duration_tokens(*duration_ms);
                content_bytes += uri.len() as u64;
            }
            _ => {
                let len = block.text_projection().len() as u64;
                tokens += if len > 0 { (len / 4).max(1) } else { 0 };
                content_bytes += len;
            }
        }
    }
    (tokens, content_bytes)
}

/// Estimate transcript pressure in both trigger units with one walk.
///
/// Token and request-byte estimates share the same traversal (and the same
/// per-message JSON serialization for assistant/system messages) so adding
/// the byte estimate did not add a second full serialization pass to the
/// per-boundary compaction check.
fn estimate_transcript_pressure(
    messages: &[Message],
) -> Result<TranscriptPressure, CompactionError> {
    let mut tokens: u64 = 0;
    let mut content_bytes: u64 = 0;
    for msg in messages {
        match msg {
            Message::User(u) => {
                let (block_tokens, block_bytes) = estimate_content_blocks(&u.content);
                tokens += block_tokens;
                content_bytes += block_bytes;
            }
            Message::ToolResults { results, .. } => {
                for r in results {
                    let (block_tokens, block_bytes) = estimate_content_blocks(&r.content);
                    tokens += block_tokens;
                    content_bytes += block_bytes;
                }
            }
            // System notices can carry image/video content blocks inside
            // `Comms`/`ExternalEvent` payloads (e.g. a peer message relaying a
            // blob-backed image inlined for the model). Estimate those blocks
            // with the same fixed image/video heuristic as user/tool content;
            // counting the serialized JSON would treat inline base64 as text
            // and massively overcount, forcing spurious compaction.
            Message::SystemNotice(notice) => {
                if let Some(body) = notice.body.as_deref() {
                    let len = body.len() as u64;
                    tokens += if len > 0 { (len / 4).max(1) } else { 0 };
                    content_bytes += len;
                }
                for block in &notice.blocks {
                    match block {
                        crate::types::SystemNoticeBlock::Comms { content, .. }
                        | crate::types::SystemNoticeBlock::ExternalEvent { content, .. } => {
                            let (block_tokens, block_bytes) = estimate_content_blocks(content);
                            tokens += block_tokens;
                            content_bytes += block_bytes;
                        }
                        other => {
                            let json = serde_json::to_string(other)
                                .map_err(|e| CompactionError::EstimationFailed(e.to_string()))?;
                            tokens += json.len() as u64 / 4;
                            content_bytes += json.len() as u64;
                        }
                    }
                }
            }
            // For assistant/system messages, serialize to JSON (no image blocks).
            other => {
                let json = serde_json::to_string(other)
                    .map_err(|e| CompactionError::EstimationFailed(e.to_string()))?;
                tokens += json.len() as u64 / 4;
                content_bytes += json.len() as u64;
            }
        }
    }
    Ok(TranscriptPressure {
        tokens,
        request_bytes: content_bytes.saturating_mul(REQUEST_BYTE_SAFETY_NUMERATOR)
            / REQUEST_BYTE_SAFETY_DENOMINATOR,
    })
}

/// Estimate token count from message history.
///
/// Text content uses `json_bytes / 4` as a rough heuristic.
/// Image blocks use a fixed per-image estimate instead of serializing
/// the base64 payload (which would massively overcount).
pub fn estimate_tokens(messages: &[Message]) -> Result<u64, CompactionError> {
    Ok(estimate_transcript_pressure(messages)?.tokens)
}

/// Estimate the serialized request size in bytes for this message history.
///
/// This is the byte-trigger counterpart of [`estimate_tokens`]: providers
/// reject oversized requests in bytes (`request_too_large`), so the
/// compaction check needs the transcript measured in the unit the provider
/// enforces. The estimate sums content byte lengths — inline media at full
/// base64 length — and applies the documented 1.2× envelope safety factor.
pub fn estimate_request_bytes(messages: &[Message]) -> Result<u64, CompactionError> {
    Ok(estimate_transcript_pressure(messages)?.request_bytes)
}

/// Build a `CompactionContext` from current agent state.
///
/// Falls back to 0 estimated tokens if serialization fails (non-fatal for
/// context building — the should_compact check will use last_input_tokens).
pub fn build_compaction_context(
    messages: &[Message],
    last_input_tokens: u64,
    provider_request_pressure: Option<crate::ProviderRequestPressure>,
    last_compaction_boundary_index: Option<u64>,
    session_boundary_index: u64,
) -> CompactionContext {
    let (estimated_history_tokens, estimated_request_bytes) =
        match estimate_transcript_pressure(messages) {
            Ok(pressure) => (pressure.tokens, pressure.request_bytes),
            Err(err) => {
                tracing::warn!("failed to estimate history pressure for compaction context: {err}");
                (0, 0)
            }
        };

    CompactionContext {
        last_input_tokens,
        message_count: messages.len(),
        estimated_history_tokens,
        estimated_request_bytes,
        provider_request_pressure,
        last_compaction_boundary_index,
        session_boundary_index,
    }
}

/// Best-effort count of prior LLM boundaries for older sessions that do not
/// yet carry explicit cadence metadata.
fn infer_session_boundary_index(messages: &[Message]) -> u64 {
    messages
        .iter()
        .filter(|message| matches!(message, Message::BlockAssistant(_)))
        .count() as u64
}

/// Error establishing the session compaction cadence at agent-build ingress.
#[derive(Debug, thiserror::Error)]
pub enum CompactionCadenceError {
    /// The persisted cadence metadata is present but does not deserialize.
    /// Silently re-inferring here would launder a corrupt persisted cadence
    /// into a fresh transcript-derived count, masking the corruption.
    #[error("persisted session compaction cadence metadata is corrupt: {0}")]
    CorruptMetadata(serde_json::Error),
    /// The one-time migration cadence could not be persisted, which would
    /// leave the transcript-derived count alive as a parallel owner.
    #[error("failed to persist migrated session compaction cadence metadata: {0}")]
    PersistFailed(serde_json::Error),
}

/// Establish the session compaction cadence at agent-build ingress.
///
/// Session metadata under [`SESSION_COMPACTION_CADENCE_KEY`] is the single
/// typed owner of cadence truth:
/// - **Present**: parsed fail-closed. Corrupt metadata is a typed build
///   fault, never a trigger for silent re-inference.
/// - **Absent** (pre-migration session): the cadence is inferred from
///   transcript history exactly once and immediately persisted, so the
///   transcript-derived count never survives as a shadow owner past ingress.
pub fn load_compaction_cadence(
    session: &mut Session,
) -> Result<SessionCompactionCadence, CompactionCadenceError> {
    match session.metadata().get(SESSION_COMPACTION_CADENCE_KEY) {
        Some(value) => serde_json::from_value::<SessionCompactionCadence>(value.clone())
            .map_err(CompactionCadenceError::CorruptMetadata),
        None => {
            let cadence = SessionCompactionCadence {
                session_boundary_index: infer_session_boundary_index(session.messages()),
                last_compaction_boundary_index: None,
                last_compaction_attempt_boundary_index: None,
            };
            persist_compaction_cadence(session, &cadence)
                .map_err(CompactionCadenceError::PersistFailed)?;
            Ok(cadence)
        }
    }
}

/// Persist compaction cadence so reused/resumed sessions preserve their
/// session-scoped compaction behavior.
pub fn persist_compaction_cadence(
    session: &mut Session,
    cadence: &SessionCompactionCadence,
) -> Result<(), serde_json::Error> {
    let value = serde_json::to_value(cadence)?;
    session.set_metadata(SESSION_COMPACTION_CADENCE_KEY, value);
    Ok(())
}

/// Run the compaction flow.
///
/// 1. Emit CompactionStarted
/// 2. Produce the summary: a configured curator substitutes summary content
///    directly (no summarization LLM call, zero summary usage); otherwise
///    call the LLM with the compaction prompt
/// 3. On failure: emit CompactionFailed, return error without mutating session
///    (a failing curator never falls back to the LLM path)
/// 4. Rebuild history via compactor
/// 5. Return a typed outcome; the caller commits it and emits CompactionCompleted
pub async fn run_compaction<C>(
    client: &C,
    compactor: &Arc<dyn Compactor>,
    curator: Option<&Arc<dyn CompactionCurator>>,
    window: CompactionWindow<'_>,
    parent_revision: String,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
    event_tap: &crate::event_tap::EventTap,
) -> Result<CompactionOutcome, CompactionError>
where
    C: crate::agent::AgentLlmClient + ?Sized,
{
    let CompactionWindow {
        messages,
        last_input_tokens,
        session_boundary_index,
    } = window;
    let estimated = estimate_tokens(messages)?;
    let message_count = messages.len();
    let mut event_stream_open = true;

    // 1. Emit CompactionStarted
    if event_stream_open
        && !crate::event_tap::tap_emit(
            event_tap,
            event_tx.as_ref(),
            AgentEvent::CompactionStarted {
                input_tokens: last_input_tokens,
                estimated_history_tokens: estimated,
                message_count,
            },
        )
        .await
    {
        event_stream_open = false;
        tracing::warn!("compaction event stream receiver dropped before CompactionStarted");
    }

    // 2/3. Produce the summary. A configured curator owns summary content
    // production outright: the summarization LLM call is skipped and a
    // curator failure is terminal for this compaction attempt (no fallback).
    let (summary_text, summary_usage) = if let Some(curator) = curator {
        let curator_window = CompactionWindow {
            messages,
            last_input_tokens,
            session_boundary_index,
        };
        match curator.curate_summary(curator_window).await {
            Ok(summary) => (summary.into_string(), Usage::default()),
            Err(e) => {
                if event_stream_open
                    && !crate::event_tap::tap_emit(
                        event_tap,
                        event_tx.as_ref(),
                        AgentEvent::CompactionFailed {
                            reason: crate::event::CompactionFailureReason::curator_failed(
                                e.to_string(),
                            ),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        "compaction event stream receiver dropped before CompactionFailed"
                    );
                }
                return Err(CompactionError::CuratorFailed(e));
            }
        }
    } else {
        // Build the compaction prompt messages
        let compaction_prompt = compactor.compaction_prompt();
        let max_summary_tokens = compactor.max_summary_tokens();

        let mut compaction_messages = compactor.prepare_for_summarization(messages);
        compaction_messages.push(Message::User(crate::types::UserMessage::text(
            compaction_prompt.to_string(),
        )));

        // Call LLM with empty tools, max_summary_tokens
        let llm_result = client
            .stream_response(&compaction_messages, &[], max_summary_tokens, None, None)
            .await;

        match llm_result {
            Ok(result) => {
                // Extract summary text from response blocks
                let mut summary = String::new();
                for block in result.blocks() {
                    if let AssistantBlock::Text { text, .. } = block {
                        summary.push_str(text);
                    }
                }
                if summary.is_empty() {
                    if event_stream_open
                        && !crate::event_tap::tap_emit(
                            event_tap,
                            event_tx.as_ref(),
                            AgentEvent::CompactionFailed {
                                reason: crate::event::CompactionFailureReason::EmptySummary,
                            },
                        )
                        .await
                    {
                        tracing::warn!(
                            "compaction event stream receiver dropped before CompactionFailed"
                        );
                    }
                    return Err(CompactionError::EmptySummary);
                }
                (summary, result.usage().clone())
            }
            Err(e) => {
                if event_stream_open
                    && !crate::event_tap::tap_emit(
                        event_tap,
                        event_tx.as_ref(),
                        AgentEvent::CompactionFailed {
                            reason: crate::event::CompactionFailureReason::llm_failed(&e),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        "compaction event stream receiver dropped before CompactionFailed"
                    );
                }
                return Err(CompactionError::LlmFailed(e));
            }
        }
    };

    // 4. Rebuild history. Validation below requires every ordinary System row
    // to remain verbatim at its mapped relative position.
    let result = compactor.rebuild_history(messages, &summary_text);
    if let Err(error) = validate_compaction_rebuild(
        messages,
        &result.messages,
        &result.summary,
        &summary_text,
        &result.retained,
        &result.discarded,
    ) {
        if event_stream_open
            && !crate::event_tap::tap_emit(
                event_tap,
                event_tx.as_ref(),
                AgentEvent::CompactionFailed {
                    reason: crate::event::CompactionFailureReason::transcript_rewrite_failed(
                        error.to_string(),
                    ),
                },
            )
            .await
        {
            tracing::warn!(
                "compaction event stream receiver dropped before invalid-rebuild CompactionFailed"
            );
        }
        return Err(error);
    }
    let rewrite_authority =
        ValidatedCompactionRewrite::from_validated(parent_revision, messages, &result.messages)?;
    let messages_after = result.messages.len();

    Ok(CompactionOutcome {
        new_messages: result.messages,
        discarded: result.discarded,
        summary_usage,
        session_boundary_index,
        messages_before: message_count,
        messages_after,
        rewrite_authority,
    })
}

/// Opaque proof that [`validate_compaction_rebuild`] accepted the exact rebuilt
/// transcript. Only this module can mint it; Session consumes it to select the
/// typed compaction rewrite semantic.
#[derive(Debug)]
pub(crate) struct ValidatedCompactionRewrite {
    parent_revision: String,
    revision: String,
    messages_before: usize,
    messages_after: usize,
}

impl ValidatedCompactionRewrite {
    /// Mint the token from the caller's already-proved parent revision plus
    /// one fresh digest of the rebuilt transcript (new content — the one
    /// hash a compaction genuinely owes). `parent_revision` MUST be the
    /// format-2 digest of exactly `messages`; the production caller serves
    /// it from the session accumulator, which is contractually byte-identical
    /// to `transcript_messages_digest(messages)` (pinned by
    /// `canonicalize_messages_for_digest_is_element_wise`), and the debug
    /// cross-check below keeps that binding evidence-backed without counting
    /// against the digest budget.
    fn from_validated(
        parent_revision: String,
        messages: &[Message],
        rebuilt: &[Message],
    ) -> Result<Self, CompactionError> {
        #[cfg(debug_assertions)]
        {
            let recomputed = crate::session::transcript_messages_digest_uncounted(messages)
                .map_err(|error| CompactionError::InvalidRebuild(error.to_string()))?;
            assert_eq!(
                recomputed, parent_revision,
                "compaction authority minted with a parent revision that is not \
                 the digest of the exact pre-compaction transcript"
            );
        }
        let revision = crate::session::transcript_messages_digest(rebuilt)
            .map_err(|error| CompactionError::InvalidRebuild(error.to_string()))?;
        Ok(Self {
            parent_revision,
            revision,
            messages_before: messages.len(),
            messages_after: rebuilt.len(),
        })
    }

    /// Whether the token's parent side binds this exact live transcript:
    /// digest and both length facts. The rebuilt side deliberately binds at
    /// the commit seam instead, where its one required digest is computed —
    /// see `Session::replace_messages_for_compaction_internal`.
    pub(crate) fn authorizes_parent_digest(
        &self,
        parent_revision: &str,
        messages_before: usize,
        messages_after: usize,
    ) -> bool {
        self.messages_before == messages_before
            && self.messages_after == messages_after
            && self.parent_revision == parent_revision
    }

    /// Whether the validated rebuild is content-identical to its parent.
    /// Answered from the token's own two digests — the mint bound them to
    /// the exact accepted pair, so no re-hash is needed to ask.
    pub(crate) fn is_no_op(&self) -> bool {
        self.parent_revision == self.revision
    }

    /// The parent-side digest the token was minted over.
    pub(crate) fn parent_revision(&self) -> &str {
        &self.parent_revision
    }

    /// The rebuilt-side digest the token was minted over.
    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn authorizes_commit(&self, commit: &crate::TranscriptRewriteCommit) -> bool {
        let (start, end) = commit.selection.bounds();
        commit.selection.semantic() == crate::TranscriptRewriteSemantic::Compaction
            && start == 0
            && end == self.messages_before
            && commit.messages_before == self.messages_before
            && commit.messages_after == self.messages_after
            && commit.parent_revision == self.parent_revision
            && commit.revision == self.revision
            && commit.original_span_digest == self.parent_revision
            && commit.replacement_digest == self.revision
    }
}

#[cfg(test)]
impl ValidatedCompactionRewrite {
    pub(crate) fn for_test(
        messages: &[Message],
        rebuilt: &[Message],
    ) -> Result<Self, CompactionError> {
        let parent_revision = crate::session::transcript_messages_digest(messages)
            .map_err(|error| CompactionError::InvalidRebuild(error.to_string()))?;
        Self::from_validated(parent_revision, messages, rebuilt)
    }
}

/// Result of a successful compaction.
pub struct CompactionOutcome {
    /// New session messages to replace current history.
    pub new_messages: Vec<Message>,
    /// Messages that were discarded (for future memory indexing).
    pub discarded: Vec<CompactionDiscard>,
    /// Usage from the summary LLM call.
    pub summary_usage: Usage,
    /// Session boundary index at which compaction occurred.
    pub session_boundary_index: u64,
    /// Number of messages before compaction.
    pub messages_before: usize,
    /// Number of messages after compaction.
    pub messages_after: usize,
    /// Authority for the exact validated transcript rewrite semantic.
    pub(crate) rewrite_authority: ValidatedCompactionRewrite,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::{
        AssistantBlock, BlockAssistantMessage, ContentBlock, StopReason, ToolResult, Usage,
        UserMessage, VideoData,
    };

    fn valid_summary(summary_text: &str) -> (Message, CompactionSummary) {
        let message = Message::User(UserMessage::compaction_summary(format!(
            "{COMPACTION_SUMMARY_PREFIX}{summary_text}"
        )));
        let mapping = CompactionSummary::new(0, message.clone());
        (message, mapping)
    }

    #[test]
    fn load_compaction_cadence_infers_boundary_count_from_existing_history() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("first")));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "ok".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
        session.push(Message::User(UserMessage::text("second")));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "done".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
        session.record_usage(Usage::default());

        let cadence = load_compaction_cadence(&mut session).unwrap();
        assert_eq!(cadence.session_boundary_index, 2);
        assert_eq!(cadence.last_compaction_boundary_index, None);
        // The one-time migration must persist immediately so the
        // transcript-derived count never survives as a shadow owner.
        let persisted: SessionCompactionCadence = serde_json::from_value(
            session
                .metadata()
                .get(SESSION_COMPACTION_CADENCE_KEY)
                .expect("migrated cadence must be persisted at ingress")
                .clone(),
        )
        .unwrap();
        assert_eq!(persisted, cadence);
    }

    #[test]
    fn persisted_compaction_cadence_round_trips_through_session_metadata() {
        let mut session = Session::new();
        let cadence = SessionCompactionCadence {
            session_boundary_index: 7,
            last_compaction_boundary_index: Some(4),
            last_compaction_attempt_boundary_index: Some(6),
        };
        persist_compaction_cadence(&mut session, &cadence).unwrap();

        assert_eq!(load_compaction_cadence(&mut session).unwrap(), cadence);
    }

    #[test]
    fn corrupt_persisted_compaction_cadence_fails_closed() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("first")));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "ok".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
        session.set_metadata(
            SESSION_COMPACTION_CADENCE_KEY,
            serde_json::json!({"session_boundary_index": "not-a-number"}),
        );

        // Corrupt persisted cadence is a typed fault, never silently
        // re-inferred from the transcript.
        let err = load_compaction_cadence(&mut session)
            .expect_err("corrupt cadence metadata must fail closed");
        assert!(matches!(err, CompactionCadenceError::CorruptMetadata(_)));
    }

    #[test]
    fn estimate_tokens_uses_video_heuristic_for_user_and_tool_results() {
        let messages = vec![
            Message::User(UserMessage::with_blocks(vec![ContentBlock::Video {
                media_type: "video/mp4".to_string(),
                duration_ms: 8_000,
                data: VideoData::Inline {
                    data: "A".repeat(8_000),
                },
            }])),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tool-1".to_string(),
                vec![ContentBlock::Video {
                    media_type: "video/webm".to_string(),
                    duration_ms: 4_000,
                    data: VideoData::Inline {
                        data: "B".repeat(4_000),
                    },
                }],
                false,
            )]),
        ];

        assert_eq!(estimate_tokens(&messages).unwrap(), 3_600);
    }

    #[test]
    fn estimate_tokens_uses_image_heuristic_for_system_notice_comms_blocks() {
        use crate::types::{
            ImageData, SystemNoticeBlock, SystemNoticeDirection, SystemNoticeKind,
            SystemNoticeMessage,
        };
        // A relayed peer message carrying a large inline image. Counting the
        // base64 payload as serialized JSON text would massively overcount and
        // force spurious compaction; the estimate must use the fixed image
        // heuristic just like user/tool content blocks.
        let huge_base64 = "A".repeat(4_000_000);
        let messages = vec![Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::Comms,
            None,
            SystemNoticeBlock::Comms {
                kind: crate::types::CommsNoticeKind::Message,
                direction: SystemNoticeDirection::Incoming,
                peer: None,
                sender_taint: None,
                request_id: None,
                intent: None,
                status: None,
                summary: None,
                payload: None,
                content: vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline { data: huge_base64 },
                }],
            },
        ))];

        // One inline image => IMAGE_TOKEN_ESTIMATE, not ~1M tokens from base64.
        assert_eq!(estimate_tokens(&messages).unwrap(), IMAGE_TOKEN_ESTIMATE);
    }

    #[test]
    fn estimate_request_bytes_counts_inline_media_at_full_payload_length() {
        use crate::types::ImageData;
        // The 2026-07-29 incident class: byte-heavy, token-light. The token
        // estimate must keep its fixed per-image heuristic while the byte
        // estimate counts the full base64 payload (plus the 1.2x envelope
        // safety factor), because the provider request cap is enforced on
        // exactly those bytes.
        let payload = "A".repeat(4_000_000);
        let messages = vec![Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline { data: payload },
            },
        ]))];

        assert_eq!(estimate_tokens(&messages).unwrap(), IMAGE_TOKEN_ESTIMATE);
        assert_eq!(
            estimate_request_bytes(&messages).unwrap(),
            4_000_000 * REQUEST_BYTE_SAFETY_NUMERATOR / REQUEST_BYTE_SAFETY_DENOMINATOR
        );
    }

    #[test]
    fn build_compaction_context_populates_request_byte_estimate() {
        let messages = vec![Message::User(UserMessage::text("hello bytes"))];
        let ctx = build_compaction_context(&messages, 42, None, None, 7);
        assert_eq!(
            ctx.estimated_request_bytes,
            estimate_request_bytes(&messages).unwrap(),
            "the loop-facing context must carry the same byte estimate the estimator produces"
        );
        assert!(ctx.estimated_request_bytes > 0);
    }

    #[test]
    fn compaction_rebuild_rejects_noop_without_an_injected_summary() {
        let source = vec![Message::User(UserMessage::text("unchanged"))];
        let retained = vec![CompactionRetained::new(0, 0, source[0].clone())];
        let (_, summary) = valid_summary("summary");

        let error =
            validate_compaction_rebuild(&source, &source, &summary, "summary", &retained, &[])
                .expect_err("a no-op rebuild must not be accepted as compaction");

        assert!(matches!(error, CompactionError::InvalidRebuild(_)));
        assert!(error.to_string().contains("discard at least one"));
    }

    #[test]
    fn compaction_rebuild_rejects_source_claimed_as_retained_and_discarded() {
        let first = Message::User(UserMessage::text("first"));
        let second = Message::User(UserMessage::text("second"));
        let source = vec![first.clone(), second.clone()];
        let (summary_message, summary) = valid_summary("summary");
        let rebuilt = vec![summary_message, first.clone(), second.clone()];
        let retained = vec![
            CompactionRetained::new(0, 1, first.clone()),
            CompactionRetained::new(1, 2, second),
        ];
        let discarded = vec![CompactionDiscard::new(0, first)];

        let error = validate_compaction_rebuild(
            &source, &rebuilt, &summary, "summary", &retained, &discarded,
        )
        .expect_err("one source offset cannot be both retained and discarded");

        assert!(matches!(error, CompactionError::InvalidRebuild(_)));
        assert!(
            error
                .to_string()
                .contains("more than one compaction disposition")
        );
    }

    #[test]
    fn compaction_rebuild_rejects_duplicate_and_out_of_order_discard_offsets() {
        let first = Message::User(UserMessage::text("first"));
        let second = Message::User(UserMessage::text("second"));
        let source = vec![first.clone(), second.clone()];
        let (summary_message, summary) = valid_summary("summary");
        let rebuilt = vec![summary_message];

        let duplicate = vec![
            CompactionDiscard::new(0, first.clone()),
            CompactionDiscard::new(0, first.clone()),
        ];
        let duplicate_error =
            validate_compaction_rebuild(&source, &rebuilt, &summary, "summary", &[], &duplicate)
                .expect_err("duplicate source offsets must be rejected");
        assert!(matches!(
            duplicate_error,
            CompactionError::InvalidRebuild(_)
        ));
        assert!(duplicate_error.to_string().contains("strictly increasing"));

        let out_of_order = vec![
            CompactionDiscard::new(1, second),
            CompactionDiscard::new(0, first),
        ];
        let order_error =
            validate_compaction_rebuild(&source, &rebuilt, &summary, "summary", &[], &out_of_order)
                .expect_err("out-of-order source offsets must be rejected");
        assert!(matches!(order_error, CompactionError::InvalidRebuild(_)));
        assert!(order_error.to_string().contains("strictly increasing"));
    }

    #[test]
    fn compaction_rebuild_uses_offsets_to_disambiguate_duplicate_messages() {
        let duplicate = Message::User(UserMessage::text("same value"));
        let source = vec![duplicate.clone(), duplicate.clone()];
        let (summary_message, summary) = valid_summary("summary");
        let rebuilt = vec![summary_message, duplicate.clone()];
        let retained = vec![CompactionRetained::new(1, 1, duplicate.clone())];
        let discarded = vec![CompactionDiscard::new(0, duplicate)];

        validate_compaction_rebuild(
            &source, &rebuilt, &summary, "summary", &retained, &discarded,
        )
        .expect("explicit source and rebuilt offsets make duplicate provenance exact");
    }

    #[test]
    fn compaction_rebuild_rejects_discarding_mid_thread_system() {
        let old = Message::User(UserMessage::text("old turn"));
        let current = Message::System(crate::types::SystemMessage::new("current prompt"));
        let recent = Message::User(UserMessage::text("recent turn"));
        let source = vec![old.clone(), current.clone(), recent.clone()];
        let (summary_message, summary) = valid_summary("summary");
        let rebuilt = vec![summary_message, recent.clone()];
        let retained = vec![CompactionRetained::new(2, 1, recent)];
        let discarded = vec![
            CompactionDiscard::new(0, old),
            CompactionDiscard::new(1, current),
        ];

        let error = validate_compaction_rebuild(
            &source, &rebuilt, &summary, "summary", &retained, &discarded,
        )
        .expect_err("every ordered System must survive compaction");

        assert!(error.to_string().contains("must be retained verbatim"));
    }

    #[test]
    fn compaction_rebuild_rejects_summary_inside_retained_system_prefix() {
        let system_a = Message::System(crate::types::SystemMessage::new("system A"));
        let system_b = Message::System(crate::types::SystemMessage::new("system B"));
        let old = Message::User(UserMessage::text("old turn"));
        let source = vec![system_a.clone(), system_b.clone(), old.clone()];
        let (summary_message, _) = valid_summary("summary");
        let summary = CompactionSummary::new(1, summary_message.clone());
        let rebuilt = vec![system_a.clone(), summary_message, system_b.clone()];
        let retained = vec![
            CompactionRetained::new(0, 0, system_a),
            CompactionRetained::new(1, 2, system_b),
        ];
        let discarded = vec![CompactionDiscard::new(2, old)];

        let error = validate_compaction_rebuild(
            &source, &rebuilt, &summary, "summary", &retained, &discarded,
        )
        .expect_err("summary must not split the exact retained source prefix");

        assert!(
            error
                .to_string()
                .contains("replace the first discarded source position")
        );
    }

    #[test]
    fn compaction_rebuild_rejects_arbitrary_unmapped_message() {
        let source_message = Message::User(UserMessage::text("source"));
        let source = vec![source_message.clone()];
        let arbitrary = Message::User(UserMessage::text("not the produced summary"));
        let rebuilt = vec![arbitrary.clone()];
        let claimed_summary = CompactionSummary::new(0, arbitrary);
        let discarded = vec![CompactionDiscard::new(0, source_message)];

        let error = validate_compaction_rebuild(
            &source,
            &rebuilt,
            &claimed_summary,
            "summary",
            &[],
            &discarded,
        )
        .expect_err("an arbitrary unmapped user message must not pass as a compaction summary");

        assert!(error.to_string().contains("typed compaction-summary role"));
    }

    #[test]
    fn compaction_rebuild_rejects_summary_with_wrong_content() {
        let source_message = Message::User(UserMessage::text("source"));
        let source = vec![source_message.clone()];
        let wrong = Message::User(UserMessage::compaction_summary(format!(
            "{COMPACTION_SUMMARY_PREFIX}different"
        )));
        let rebuilt = vec![wrong.clone()];
        let claimed_summary = CompactionSummary::new(0, wrong);
        let discarded = vec![CompactionDiscard::new(0, source_message)];

        let error = validate_compaction_rebuild(
            &source,
            &rebuilt,
            &claimed_summary,
            "summary",
            &[],
            &discarded,
        )
        .expect_err("summary content must match the text produced for this attempt");

        assert!(
            error
                .to_string()
                .contains("does not exactly match the produced summary")
        );
    }
}

//! Agent compaction — runs context compaction during the agent loop.
//!
//! Called from the agent state machine when a compactor is configured and
//! the threshold is met.

use crate::Session;
use crate::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionCurator, CompactionDiscard,
    CompactionResult, CompactionRetained, CompactionSummary, CompactionWindow, Compactor,
    SESSION_COMPACTION_CADENCE_KEY, SessionCompactionCadence,
};
use crate::event::AgentEvent;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::types::{AssistantBlock, Message, StopReason, SystemMessage, TurnUsage, Usage};
use sha2::{Digest, Sha256};
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

/// Normalize the compaction summary call's usage, reporting any identity
/// dispute for routing rather than swallowing it.
///
/// Absent accounting still fails this call closed, and deliberately so: unlike
/// the model turn, a refused compaction does not terminalize anything the
/// caller has read. `Agent::maybe_compact` consumes the outcome as
/// `if let Ok(outcome)`, so the failure preserves the untouched history and the
/// run continues. The two sites therefore share a doctrine, not a verdict: each
/// fault may only terminalize what it actually invalidates, and here that is
/// one optional rewrite rather than a completed turn.
///
/// An identity dispute never refuses: the counters exist and are internally
/// consistent, so they are charged exactly as reported and the dispute is
/// returned for the caller to route.
fn validated_compaction_summary_usage(
    usage: Usage,
    active_provider: crate::Provider,
    active_model: &str,
) -> Result<
    (
        TurnUsage,
        Option<crate::provider_evidence::DisputedTurnUsageAccountingIdentity>,
    ),
    CompactionError,
> {
    let turn_usage = TurnUsage::try_from_usage(usage).map_err(|error| {
        CompactionError::LlmFailed(crate::error::AgentError::llm(
            active_provider.as_str(),
            crate::error::LlmFailureReason::ProviderError(
                crate::error::LlmProviderError::non_retryable(
                    crate::error::LlmProviderErrorKind::IncompleteResponse,
                    serde_json::json!({
                        "reason": "normalized_provider_accounting_unavailable",
                        "model": active_model,
                    }),
                ),
            ),
            error.to_string(),
        ))
    })?;
    let dispute = match super::classify_provider_turn_usage_identity(
        &turn_usage,
        active_provider,
        active_model,
    ) {
        super::TurnUsageIdentityVerdict::Agreed => None,
        super::TurnUsageIdentityVerdict::Disputed(dispute) => Some(dispute),
    };
    Ok((turn_usage, dispute))
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

/// What a rebuilt transcript inserts or changes besides its summary.
#[derive(Debug, Clone, Copy)]
enum RebuildInsertions<'a> {
    /// A compactor's own rebuild: only the summary is new, and every ordinary
    /// System row, operator-instruction entry rows and instruction activation
    /// rows included, stays verbatim.
    Summary,
    /// The rebuild after the instruction-row fold. With `folds_entries` every
    /// operator-instruction entry row is taken out, and the fold's snapshot
    /// (none for an empty set) is the last row of the leading System prefix,
    /// before the summary. Each instruction activation row whose body the
    /// session-document authority retires (`retirement`) holds the exact
    /// retired form of its source row; every other retained row stays
    /// verbatim.
    Fold {
        folds_entries: bool,
        snapshot: Option<&'a SystemMessage>,
        retirement: Option<&'a crate::session::ActivationRetirement>,
    },
}

fn validate_compaction_rebuild(
    messages: &[Message],
    rebuilt: &[Message],
    summary: &CompactionSummary,
    summary_text: &str,
    retained: &[CompactionRetained],
    discarded: &[CompactionDiscard],
) -> Result<(), CompactionError> {
    validate_compaction_rebuild_with(
        messages,
        rebuilt,
        summary,
        summary_text,
        retained,
        discarded,
        RebuildInsertions::Summary,
    )
}

fn validate_compaction_rebuild_with(
    messages: &[Message],
    rebuilt: &[Message],
    summary: &CompactionSummary,
    summary_text: &str,
    retained: &[CompactionRetained],
    discarded: &[CompactionDiscard],
    insertions: RebuildInsertions<'_>,
) -> Result<(), CompactionError> {
    #[derive(Clone, Copy)]
    enum SourceDisposition {
        Retained,
        Discarded,
    }

    let superseded_system_prompts = crate::types::superseded_system_prompt_offsets(messages);
    let (folds_entries, snapshot, retirement) = match insertions {
        RebuildInsertions::Fold {
            folds_entries,
            snapshot,
            retirement,
        } => (folds_entries, snapshot, retirement),
        RebuildInsertions::Summary => (false, None, None),
    };
    // Each authorized retirement must retire a superseded body of its own
    // source row, exactly: never a key's newest row, never a row that is not
    // a live instruction activation row, never with another header or body
    // length.
    if let Some(retirement) = retirement {
        let superseded = crate::session::superseded_activation_rows(messages);
        for source_offset in retirement.source_offsets() {
            let Some(Message::System(source)) = messages.get(source_offset) else {
                return Err(CompactionError::InvalidRebuild(format!(
                    "a retired instruction body names source offset {source_offset}, which is not a System row"
                )));
            };
            let Some(identity) = source.instruction_activation.as_ref() else {
                return Err(CompactionError::InvalidRebuild(format!(
                    "a retired instruction body names source offset {source_offset}, which is not an instruction activation row"
                )));
            };
            if !superseded.contains(&(
                identity.revision.namespace.clone(),
                identity.revision.key.clone(),
                identity.activation_id.clone(),
            )) {
                return Err(CompactionError::InvalidRebuild(format!(
                    "the instruction activation at source offset {source_offset} is its key's newest row and keeps its body"
                )));
            }
            if crate::session::retired_activation_row(source).as_ref()
                != retirement.retired_row(source_offset)
            {
                return Err(CompactionError::InvalidRebuild(format!(
                    "the retired instruction body at source offset {source_offset} is not the exact retired form of its source row"
                )));
            }
        }
    }
    let folded_entry = |source_offset: u64| {
        folds_entries
            && usize::try_from(source_offset)
                .ok()
                .and_then(|offset| messages.get(offset))
                .is_some_and(crate::session::is_operator_instruction_entry)
    };

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
        // A retained row is verbatim, unless its body is authorized for
        // retirement: then it is exactly the retired form of its source row.
        let rebuilt_matches =
            match retirement.and_then(|retirement| retirement.retired_row(source_offset)) {
                Some(retired) => {
                    matches!(&rebuilt[rebuilt_offset], Message::System(row) if row == retired)
                }
                None => rebuilt[rebuilt_offset] == retention.message,
            };
        if !rebuilt_matches {
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
    if let Some(retirement) = retirement
        && let Some(discarded_offset) = retirement.source_offsets().find(|source_offset| {
            !matches!(
                source_dispositions.get(*source_offset),
                Some(Some(SourceDisposition::Retained))
            )
        })
    {
        return Err(CompactionError::InvalidRebuild(format!(
            "the instruction activation at source offset {discarded_offset} has its body retired, so it must be retained"
        )));
    }
    // The shrink rule counts the rows other than operator-instruction
    // entries: the fold's snapshot is neutral in it, and so are the entry
    // rows the fold takes out. A compactor's own rebuild keeps every entry
    // row, so for it the count is its plain row count.
    let source_rows = crate::session::rows_besides_operator_instruction_entries(messages);
    let rebuilt_rows = crate::session::rows_besides_operator_instruction_entries(rebuilt);
    if rebuilt_rows > source_rows {
        return Err(CompactionError::InvalidRebuild(format!(
            "compaction must not grow the transcript from {source_rows} to {rebuilt_rows} messages (operator instruction entries aside)"
        )));
    }

    let summary_offset = checked_offset("summary rebuilt", summary.rebuilt_offset, rebuilt.len())?;
    if rebuilt[summary_offset] != summary.message {
        return Err(CompactionError::InvalidRebuild(format!(
            "summary mapping does not match rebuilt offset {}",
            summary.rebuilt_offset
        )));
    }
    // The summary takes the place of the first row the compactor folded;
    // the entry rows the fold takes out are not the compactor's, and the
    // fold's snapshot sits before the summary.
    let first_discarded_source_offset = discarded
        .iter()
        .map(|discard| discard.source_offset)
        .find(|source_offset| !folded_entry(*source_offset))
        .ok_or_else(|| {
            CompactionError::InvalidRebuild(
                "compaction must discard at least one source message besides operator \
                 instruction entries"
                    .to_string(),
            )
        })?;
    let expected_summary_offset = retained
        .iter()
        .take_while(|retention| retention.source_offset < first_discarded_source_offset)
        .count()
        + usize::from(snapshot.is_some());
    if summary_offset != expected_summary_offset {
        return Err(CompactionError::InvalidRebuild(format!(
            "summary must replace the first discarded source position at rebuilt offset {expected_summary_offset}, found {summary_offset}"
        )));
    }
    for (source_offset, message) in messages.iter().enumerate() {
        if !matches!(message, Message::System(_)) {
            continue;
        }
        let must_retain = !superseded_system_prompts.contains(&source_offset);
        let source_offset = u64::try_from(source_offset).map_err(|_| {
            CompactionError::InvalidRebuild(
                "source System offset exceeds the compaction mapping range".to_string(),
            )
        })?;
        let was_retained = retained
            .iter()
            .any(|retention| retention.source_offset == source_offset);
        if folded_entry(source_offset) {
            if was_retained {
                return Err(CompactionError::InvalidRebuild(format!(
                    "operator instruction entry at source offset {source_offset} must be folded, not retained"
                )));
            }
            continue;
        }
        if must_retain && !was_retained {
            return Err(CompactionError::InvalidRebuild(format!(
                "active ordered System message at source offset {source_offset} must be retained verbatim"
            )));
        }
        if !must_retain && was_retained {
            return Err(CompactionError::InvalidRebuild(format!(
                "superseded versioned System message at source offset {source_offset} must be discarded"
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
    let expected_unmapped = match snapshot {
        None => vec![summary_offset],
        Some(snapshot) => {
            // The snapshot is the last row of the leading System prefix.
            let leading_system_rows = rebuilt
                .iter()
                .take_while(|message| matches!(message, Message::System(_)))
                .count();
            let snapshot_offset = leading_system_rows.checked_sub(1).ok_or_else(|| {
                CompactionError::InvalidRebuild(
                    "operator instruction snapshot must follow the leading System prefix"
                        .to_string(),
                )
            })?;
            if !matches!(&rebuilt[snapshot_offset], Message::System(row) if row == snapshot) {
                return Err(CompactionError::InvalidRebuild(format!(
                    "rebuilt offset {snapshot_offset} must be the authorized operator instruction snapshot"
                )));
            }
            vec![snapshot_offset, summary_offset]
        }
    };
    if unmapped_rebuilt != expected_unmapped {
        return Err(CompactionError::InvalidRebuild(match snapshot {
            None => format!(
                "rebuilt transcript must contain only the mapped summary as an inserted message, found unmapped offsets {unmapped_rebuilt:?}"
            ),
            Some(_) => format!(
                "rebuilt transcript must contain only the mapped summary and the operator instruction snapshot as inserted messages, found unmapped offsets {unmapped_rebuilt:?}"
            ),
        }));
    }
    Ok(())
}

/// The instruction-row fold of a validated rebuild, as the session-document
/// authority authorized it. With an operator-instruction fold, every entry
/// row the compactor retained (as it retains every System row) joins the
/// discarded rows, and the fold's snapshot (none for an empty set) becomes
/// the last row of the leading System prefix, before the summary; retained
/// and summary offsets move with their rows. With an activation-body
/// retirement, every retained activation row whose body it retires takes its
/// retired form in place. The folded rebuild is validated again in its
/// folded form; without either the rebuild is returned as it is.
fn fold_instruction_rows(
    messages: &[Message],
    result: crate::compact::CompactionResult,
    summary_text: &str,
    fold: Option<&crate::session::InstructionFold>,
    retirement: Option<&crate::session::ActivationRetirement>,
) -> Result<crate::compact::CompactionResult, CompactionError> {
    if fold.is_none() && retirement.is_none() {
        return Ok(result);
    }
    let crate::compact::CompactionResult {
        messages: rebuilt,
        summary,
        retained,
        mut discarded,
    } = result;
    let summary_offset = usize::try_from(summary.rebuilt_offset).map_err(|_| {
        CompactionError::InvalidRebuild("summary offset does not fit this target".to_string())
    })?;
    let mut retained = retained.into_iter().peekable();
    let mut folded = Vec::with_capacity(rebuilt.len() + 1);
    let mut folded_retained = Vec::with_capacity(retained.len());
    let mut folded_summary_offset = None;
    for (offset, message) in rebuilt.into_iter().enumerate() {
        if offset == summary_offset {
            folded_summary_offset = Some(folded.len());
            folded.push(message);
            continue;
        }
        let Some(retention) = retained
            .next_if(|retention| usize::try_from(retention.rebuilt_offset).ok() == Some(offset))
        else {
            return Err(CompactionError::InvalidRebuild(format!(
                "rebuilt offset {offset} has no retained source message"
            )));
        };
        if fold.is_some() && crate::session::is_operator_instruction_entry(&message) {
            discarded.push(CompactionDiscard::new(
                retention.source_offset,
                retention.message,
            ));
            continue;
        }
        let retired = usize::try_from(retention.source_offset)
            .ok()
            .and_then(|source_offset| {
                retirement.and_then(|retirement| retirement.retired_row(source_offset))
            });
        let message = match retired {
            Some(retired) => Message::System(retired.clone()),
            None => message,
        };
        folded_retained.push(CompactionRetained::new(
            retention.source_offset,
            u64::try_from(folded.len()).unwrap_or(u64::MAX),
            retention.message,
        ));
        folded.push(message);
    }
    let mut summary_offset = folded_summary_offset.ok_or_else(|| {
        CompactionError::InvalidRebuild("summary offset is outside the rebuilt transcript".into())
    })?;
    discarded.sort_by_key(|discard| discard.source_offset);
    let snapshot = fold.and_then(crate::session::InstructionFold::snapshot);
    if let Some(snapshot) = snapshot {
        let at = folded
            .iter()
            .take_while(|message| matches!(message, Message::System(_)))
            .count();
        folded.insert(at, Message::System(snapshot.clone()));
        let at_offset = u64::try_from(at).unwrap_or(u64::MAX);
        for retention in &mut folded_retained {
            if retention.rebuilt_offset >= at_offset {
                retention.rebuilt_offset += 1;
            }
        }
        if summary_offset >= at {
            summary_offset += 1;
        }
    }
    let folded = crate::compact::CompactionResult {
        messages: folded,
        summary: CompactionSummary::new(
            u64::try_from(summary_offset).unwrap_or(u64::MAX),
            summary.message,
        ),
        retained: folded_retained,
        discarded,
    };
    validate_compaction_rebuild_with(
        messages,
        &folded.messages,
        &folded.summary,
        summary_text,
        &folded.retained,
        &folded.discarded,
        RebuildInsertions::Fold {
            folds_entries: fold.is_some(),
            snapshot,
            retirement,
        },
    )?;
    Ok(folded)
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
/// production incident: inline media crossed Anthropic's request-size cap and
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
                        // A runtime notice's typed content is estimated as
                        // other notice content is, its other fields as text.
                        crate::types::SystemNoticeBlock::RuntimeNotice {
                            category,
                            detail,
                            payload,
                            content,
                        } if !content.is_empty() => {
                            let (block_tokens, block_bytes) = estimate_content_blocks(content);
                            let fields = serde_json::to_string(
                                &crate::types::SystemNoticeBlock::RuntimeNotice {
                                    category: category.clone(),
                                    detail: detail.clone(),
                                    payload: payload.clone(),
                                    content: Vec::new(),
                                },
                            )
                            .map_err(|e| CompactionError::EstimationFailed(e.to_string()))?;
                            tokens += block_tokens + fields.len() as u64 / 4;
                            content_bytes += block_bytes + fields.len() as u64;
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
    request_context_budget: Option<crate::ContextBudgetFact>,
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
        request_context_budget,
        provider_request_pressure,
        last_compaction_boundary_index,
        session_boundary_index,
        last_rewrite_history_tokens: None,
    }
}

/// The history estimate `build_compaction_context` reports for `messages`,
/// or `None` when it cannot be measured.
pub(crate) fn estimated_history_tokens(messages: &[Message]) -> Option<u64> {
    estimate_transcript_pressure(messages)
        .ok()
        .map(|pressure| pressure.tokens)
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
                last_rewrite_history_tokens: None,
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
/// 3. If the summary request itself exceeds provider capacity, use a
///    deterministic mechanical handoff for ordinary canonical history. A
///    source containing unmeasured voice dialogue instead preserves the source
///    when a mechanical rebuild would discard its unsummarized details.
///    Other failures emit CompactionFailed and preserve the original session
///    (a failing curator never falls back to either path).
/// 4. Rebuild history via compactor
/// 5. Return a typed outcome; the caller commits it and emits CompactionCompleted
const MECHANICAL_CAPACITY_SUMMARY: &str = "The summarization request exceeded the active provider capacity. Older transcript rows were mechanically compacted so the session can continue. A durable transcript or memory store may retain the discarded detail when configured; re-establish any critical working detail before relying on it.";

fn is_compaction_capacity_error(error: &crate::error::AgentError) -> bool {
    matches!(
        error,
        crate::error::AgentError::Llm {
            reason: crate::error::LlmFailureReason::ContextExceeded { .. },
            ..
        } | crate::error::AgentError::Llm {
            reason: crate::error::LlmFailureReason::ProviderError(crate::error::LlmProviderError {
                kind: crate::error::LlmProviderErrorKind::RequestTooLarge,
                ..
            }),
            ..
        }
    )
}

pub(crate) struct CompactionInvocation<'a> {
    pub(crate) authorization: Option<crate::LlmRequestAuthorization>,
    /// The run's budget: a summary call waiting for an owner change never
    /// waits past it.
    pub(crate) budget: Option<&'a crate::budget::Budget>,
    /// Exact model-facing projection. Superseded prompt versions remain in
    /// the durable `window` for rewrite validation but never enter forecasts
    /// or the summarization request. Operator-instruction entry rows are in
    /// it as the model reads them; no summarizer reads them
    /// (`without_operator_instruction_entries`), and no summarizer reads an
    /// activation body this attempt retires
    /// (`crate::session::with_retired_activation_bodies`).
    pub(crate) model_messages: &'a [Message],
    /// Unmeasured dialogue must reach a generated summary or remain intact
    /// when a mechanical rebuild would discard its unsummarized details.
    pub(crate) observation_source: CompactionObservationSource,
    pub(crate) window: CompactionWindow<'a>,
    pub(crate) request_pressure: Option<crate::ProviderRequestPressure>,
    pub(crate) parent_revision: String,
    /// Source budget of the client this attempt summarizes on, from its
    /// registry-owned limits; `None` when that model declares no context
    /// window.
    pub(crate) summarization_budget: Option<crate::compact::SummarizationSourceBudget>,
    /// The summary request's per-call provider params: `None` lets the
    /// client use its own, and a provider-native tool policy that disables
    /// native tools passes the session's tag with them stripped.
    pub(crate) provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
    /// Where the summary request's owner-change waits are reported, for a
    /// host that installed an operation refusal observer.
    pub(crate) wait_report: Option<crate::authorization::WaitReport>,
    /// What this attempt folds while a run is in flight
    /// ([`compaction_scope`]); `model_messages` is the projection of the rows
    /// the summary is written from ([`compaction_source_floor`]).
    pub(crate) scope: CompactionScope,
    /// The whole model projection at this boundary, which
    /// `CompactionStarted` and `CompactionCompleted` report on whatever the
    /// scope.
    pub(crate) projection: &'a [Message],
    /// The calls the open run's earlier folds took out
    /// ([`open_run_folded_calls`]); used only when this attempt folds the
    /// run's rounds.
    pub(crate) earlier_folded_calls: FoldedCalls,
    /// The operator-instruction fold the session-document authority
    /// authorized for this attempt (`Session::authorize_instruction_fold`);
    /// `None` for a session without operator-instruction facts.
    pub(crate) instruction_fold: Option<crate::session::InstructionFold>,
    /// The superseded instruction activation bodies the session-document
    /// authority authorized this attempt to retire
    /// (`Session::authorize_activation_body_retirement`); `None` when there
    /// is none to retire. No summarizer reads a body it retires.
    pub(crate) activation_retirement: Option<crate::session::ActivationRetirement>,
}

/// What a compaction folds, given the run in flight.
///
/// A compaction must not fold the open run's request or its newest round:
/// the model would see only a summary of a call it just made, or of the
/// request it is answering, and could act on it again, and a side-effecting
/// tool would act twice. So inside a run, after its first response, the
/// rows before the run are folded first; only when nothing is left there
/// does a compaction fold the run's own completed rounds, and then it keeps
/// the run's request and newest round verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactionScope {
    /// No run in flight, or a run before its first response: the whole
    /// transcript, as before.
    Whole,
    /// Only the rows before the open run (`messages[..floor]`), through the
    /// compactor; the run's own rows follow verbatim.
    BeforeOpenRun { floor: usize },
    /// Nothing before the open run is left to fold. The run's completed
    /// rounds (`messages[first_response..newest_round]`, System rows aside)
    /// and every earlier compaction summary are folded into the one summary,
    /// mechanically, whatever the compactor. Everything else stays verbatim:
    /// the rows before the run, the run's request (`messages[floor..
    /// first_response]`) and its newest round (`messages[newest_round..]`:
    /// the inputs that led to it, its tool calls and their results).
    OpenRunRounds {
        first_response: usize,
        newest_round: usize,
    },
}

/// Whether `message` is an input row a run's or a round's request is made
/// of: a user row that is not a compaction summary (a request, injected
/// context, a typed user append) or a system notice (a peer, flow, external
/// or job input, or a notice the run's boundary added).
fn is_request_input_row(message: &Message) -> bool {
    match message {
        Message::User(user) => !user.transcript_role.is_compaction_summary(),
        Message::SystemNotice(_) => true,
        Message::System(_) | Message::BlockAssistant(_) | Message::ToolResults { .. } => false,
    }
}

/// `index` moved back over the input rows right before it, and over the
/// operator-instruction entry rows among them: a run start appends its entry
/// after a pending tail or before the run's own input, so an entry belongs
/// to the request rows of the run it starts.
fn back_over_inputs(messages: &[Message], mut index: usize) -> usize {
    while index > 0
        && (is_request_input_row(&messages[index - 1])
            || crate::session::is_operator_instruction_entry(&messages[index - 1]))
    {
        index -= 1;
    }
    index
}

/// The first assistant row of the run `run_id`.
fn first_run_response(messages: &[Message], run_id: &crate::lifecycle::RunId) -> Option<usize> {
    messages.iter().position(|message| {
        matches!(
            message,
            Message::BlockAssistant(assistant) if assistant.identity.run_id.as_ref() == Some(run_id)
        )
    })
}

/// The first row of the run `run_id` while it is in flight: the start of
/// the input rows (request, injected context, typed appends, notices) before
/// its first assistant row, with the operator-instruction entry its run
/// start applied. The walk stops at any other System row, a response, a
/// tool-results row or a compaction summary. `None` before the run's first
/// assistant row exists, so a run's first boundary compacts as it always
/// did. Derived from the transcript at each boundary (assistant rows carry
/// the run id), so a resumed run and a rewrite earlier in the same run need
/// no remapping. An earlier run's trailing notices or unanswered prompt
/// right before the run count as the run's own, which only keeps them
/// verbatim.
pub(crate) fn open_run_floor(
    messages: &[Message],
    run_id: &crate::lifecycle::RunId,
) -> Option<usize> {
    first_run_response(messages, run_id).map(|first| back_over_inputs(messages, first))
}

/// The first row of the run's newest round: its newest assistant row with
/// tool calls, after the input rows that led to it (a steer, a notice). A
/// tool can append run-stamped assistant rows after its results (generated
/// images, callback blocks); those belong to the round they follow, so the
/// round starts at its call. A run with no call yet starts it at its newest
/// row.
fn newest_run_round(messages: &[Message], run_id: &crate::lifecycle::RunId) -> Option<usize> {
    let run_row = |message: &Message, with_calls: bool| {
        matches!(
            message,
            Message::BlockAssistant(assistant)
                if assistant.identity.run_id.as_ref() == Some(run_id)
                    && (!with_calls || assistant.has_tool_calls())
        )
    };
    messages
        .iter()
        .rposition(|message| run_row(message, true))
        .or_else(|| messages.iter().rposition(|message| run_row(message, false)))
        .map(|newest| back_over_inputs(messages, newest))
}

/// What a compaction at this boundary folds, with `run_id` the run in
/// flight; `None` when nothing can be folded without touching the open
/// run's request or newest round, so the attempt is skipped before its
/// summary call.
pub(crate) fn compaction_scope(
    messages: &[Message],
    run_id: Option<&crate::lifecycle::RunId>,
) -> Option<CompactionScope> {
    let Some(run_id) = run_id else {
        return Some(CompactionScope::Whole);
    };
    let Some(floor) = open_run_floor(messages, run_id) else {
        return Some(CompactionScope::Whole);
    };
    if prefix_has_foldable_turn(&messages[..floor]) {
        return Some(CompactionScope::BeforeOpenRun { floor });
    }
    let first_response = first_run_response(messages, run_id)?;
    let newest_round = newest_run_round(messages, run_id)?;
    // A fold must take at least two rows out to shrink the transcript (the
    // summary takes one back).
    let fold = open_run_rounds_fold(messages, first_response, newest_round);
    let folded_rows = messages
        .iter()
        .enumerate()
        .filter(|(offset, message)| fold(*offset, message))
        .count();
    (newest_round > first_response
        && prefix_has_foldable_turn(&messages[first_response..newest_round])
        && folded_rows >= 2)
        .then_some(CompactionScope::OpenRunRounds {
            first_response,
            newest_round,
        })
}

/// Which rows a fold of the open run's completed rounds takes out: the
/// rounds (`messages[first_response..newest_round]`, System rows aside),
/// every earlier compaction summary, and superseded System versions.
fn open_run_rounds_fold(
    messages: &[Message],
    first_response: usize,
    newest_round: usize,
) -> impl Fn(usize, &Message) -> bool {
    let superseded = crate::types::superseded_system_prompt_offsets(messages);
    move |offset: usize, message: &Message| match message {
        Message::System(_) => superseded.contains(&offset),
        Message::User(user) if user.transcript_role.is_compaction_summary() => true,
        _ => (first_response..newest_round).contains(&offset),
    }
}

/// The rows of the transcript the summary of `scope` is written from, when
/// they are not all of it: only the rows before the open run when that is
/// what folds. A fold of the run's own completed rounds is written from the
/// whole transcript, so the summary sees where the run stands; it replaces
/// only the completed rounds.
pub(crate) fn compaction_source_floor(scope: CompactionScope) -> Option<usize> {
    match scope {
        CompactionScope::BeforeOpenRun { floor } => Some(floor),
        CompactionScope::Whole | CompactionScope::OpenRunRounds { .. } => None,
    }
}

/// The fixed line a summary of the open run's own rounds ends with, after a
/// blank line: the calls those rounds made, and that they ran.
const FOLDED_CALLS_LEAD: &str = "The current run already made these tool calls in its earlier \
     rounds; their results are summarized above.";
/// Calls that ran: listed after this, as `name (id)` separated by `, `.
const FOLDED_CALLS_RAN: &str = " They ran, so do not make them again: ";
/// Calls that ran and failed: the model may still decide to retry one.
const FOLDED_CALLS_FAILED: &str =
    " They ran and failed, so make one again only if it is still needed: ";

/// Session metadata: the calls the folds of the open run took out of the
/// transcript, for the run's later folds to keep naming. Written with each
/// fold of the run's rounds, in the same prepared session value as the
/// rewrite, so it commits, rolls back and persists with it; read only for
/// the run it names. Never read back from summary text, which a model may
/// have written or copied.
pub(crate) const SESSION_OPEN_RUN_FOLDED_CALLS_KEY: &str = "open_run_folded_calls";

/// One tool call a fold of the open run's rounds took out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FoldedCall {
    name: String,
    id: String,
    failed: bool,
}

/// How many of the open run's folded calls the fixed line names one by one:
/// the newest. Older ones are counted by tool name, so the line and the
/// stored list stay bounded however long the run.
const MAX_NAMED_FOLDED_CALLS: usize = 200;

/// The calls the open run's folds took out: the newest
/// [`MAX_NAMED_FOLDED_CALLS`], each named, in order, and how many older ones
/// each tool made.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FoldedCalls {
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    earlier_by_tool: std::collections::BTreeMap<String, u64>,
    calls: Vec<FoldedCall>,
}

impl FoldedCalls {
    /// Append `call`, moving the oldest named call into the counts once more
    /// than [`MAX_NAMED_FOLDED_CALLS`] are named.
    fn push(&mut self, call: FoldedCall) {
        self.calls.push(call);
        if self.calls.len() > MAX_NAMED_FOLDED_CALLS {
            let oldest = self.calls.remove(0);
            *self.earlier_by_tool.entry(oldest.name).or_default() += 1;
        }
    }

    fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.earlier_by_tool.is_empty()
    }
}

/// The folded-call list of one run, as stored under
/// [`SESSION_OPEN_RUN_FOLDED_CALLS_KEY`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct OpenRunFoldedCalls {
    run_id: crate::lifecycle::RunId,
    #[serde(flatten)]
    folded: FoldedCalls,
}

/// The calls earlier folds of the run `run_id` took out, from the session's
/// metadata; none when the stored list names another run, or is absent or
/// unreadable.
pub(crate) fn open_run_folded_calls(
    session: &Session,
    run_id: &crate::lifecycle::RunId,
) -> FoldedCalls {
    session
        .metadata()
        .get(SESSION_OPEN_RUN_FOLDED_CALLS_KEY)
        .cloned()
        .and_then(|value| serde_json::from_value::<OpenRunFoldedCalls>(value).ok())
        .filter(|stored| &stored.run_id == run_id)
        .map(|stored| stored.folded)
        .unwrap_or_default()
}

/// Store `folded` as the folded-call list of the run `run_id`.
pub(crate) fn persist_open_run_folded_calls(
    session: &mut Session,
    run_id: &crate::lifecycle::RunId,
    folded: &FoldedCalls,
) -> Result<(), serde_json::Error> {
    let value = serde_json::to_value(OpenRunFoldedCalls {
        run_id: run_id.clone(),
        folded: folded.clone(),
    })?;
    session.set_metadata(SESSION_OPEN_RUN_FOLDED_CALLS_KEY, value);
    Ok(())
}

/// The calls a fold of the open run's rounds names: `earlier`, those the
/// run's earlier folds took out, then the calls of the rounds it folds now
/// (`messages[first_response..newest_round]`). Each call is paired with the
/// tool-results row of its own round (the first after its assistant row),
/// and is failed when its result there was an error. Call ids are matched
/// within that row only, since some providers reuse ids in every round.
fn folded_run_calls(
    earlier: &FoldedCalls,
    messages: &[Message],
    first_response: usize,
    newest_round: usize,
) -> FoldedCalls {
    let folded = messages
        .get(first_response..newest_round)
        .unwrap_or_default();
    let mut calls = earlier.clone();
    for (offset, message) in folded.iter().enumerate() {
        let Message::BlockAssistant(assistant) = message else {
            continue;
        };
        // The round's results come before the next response.
        let round_results = folded[offset + 1..]
            .iter()
            .take_while(|later| !matches!(later, Message::BlockAssistant(_)))
            .find_map(|later| match later {
                Message::ToolResults { results, .. } => Some(results.as_slice()),
                _ => None,
            });
        for block in &assistant.blocks {
            let AssistantBlock::ToolUse { id, name, .. } = block else {
                continue;
            };
            let failed = round_results
                .and_then(|results| results.iter().find(|result| &result.tool_use_id == id))
                .is_some_and(|result| result.is_error);
            calls.push(FoldedCall {
                name: name.clone(),
                id: id.clone(),
                failed,
            });
        }
    }
    calls
}

/// The fixed line naming `folded`: its named calls, those whose result was
/// an error listed apart as ran and failed, then the older calls counted by
/// tool name. `None` when there is no call to name.
fn folded_calls_line(folded: &FoldedCalls) -> Option<String> {
    if folded.is_empty() {
        return None;
    }
    let calls = &folded.calls;
    let list = |failed: bool| {
        calls
            .iter()
            .filter(|call| call.failed == failed)
            .map(|call| format!("{} ({})", call.name, call.id))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut line = format!("\n\n{FOLDED_CALLS_LEAD}");
    let ran = list(false);
    if !ran.is_empty() {
        line.push_str(FOLDED_CALLS_RAN);
        line.push_str(&ran);
        line.push('.');
    }
    let failed = list(true);
    if !failed.is_empty() {
        line.push_str(FOLDED_CALLS_FAILED);
        line.push_str(&failed);
        line.push('.');
    }
    if !folded.earlier_by_tool.is_empty() {
        let mut by_tool: Vec<_> = folded.earlier_by_tool.iter().collect();
        by_tool.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
        let total: u64 = by_tool.iter().map(|(_, count)| **count).sum();
        let counts = by_tool
            .iter()
            .map(|(name, count)| format!("{name} x{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        line.push_str(&format!(
            " Before those, it already made {total} earlier calls: {counts}."
        ));
    }
    Some(line)
}

/// Fold the open run's completed rounds (`messages[first_response..
/// newest_round]`, System rows aside) and every earlier compaction summary
/// and superseded System version into one summary at the first folded
/// position; keep every other row verbatim.
fn rebuild_open_run_rounds(
    messages: &[Message],
    first_response: usize,
    newest_round: usize,
    summary_text: &str,
) -> CompactionResult {
    let fold = open_run_rounds_fold(messages, first_response, newest_round);
    let summary_message = Message::User(crate::types::UserMessage::compaction_summary(format!(
        "{COMPACTION_SUMMARY_PREFIX}{summary_text}"
    )));
    let mut rebuilt = Vec::with_capacity(messages.len());
    let mut retained = Vec::new();
    let mut discarded = Vec::new();
    let mut summary = None;
    let as_offset = |offset: usize| u64::try_from(offset).unwrap_or(u64::MAX);
    for (offset, message) in messages.iter().enumerate() {
        if fold(offset, message) {
            if summary.is_none() {
                summary = Some(CompactionSummary::new(
                    as_offset(rebuilt.len()),
                    summary_message.clone(),
                ));
                rebuilt.push(summary_message.clone());
            }
            discarded.push(CompactionDiscard::new(as_offset(offset), message.clone()));
        } else {
            retained.push(CompactionRetained::new(
                as_offset(offset),
                as_offset(rebuilt.len()),
                message.clone(),
            ));
            rebuilt.push(message.clone());
        }
    }
    let summary = summary.unwrap_or_else(|| {
        let mapping = CompactionSummary::new(as_offset(rebuilt.len()), summary_message.clone());
        rebuilt.push(summary_message);
        mapping
    });
    CompactionResult {
        messages: rebuilt,
        summary,
        retained,
        discarded,
    }
}

/// The least a bounded tool result of the open run keeps of its text.
const MIN_BOUNDED_RUN_RESULT_BYTES: usize = 4 * 1024;

/// The model request `request` with the open run's oversized tool results
/// bounded, when the request (`encoded_bytes`) exceeds `max_bytes` and the
/// run's own results are what can give way. `None` when nothing is bounded.
///
/// A compaction never folds the open run's newest round, so a run whose own
/// tool results are larger than the provider accepts would fail every
/// request. For this request only, each result in a tool-results row of the
/// run (after the run's floor in the request, whose rows keep their
/// identity) that answers a call the run made keeps its identity
/// (`tool_use_id`, `is_error`) and the head of its text, with a marker
/// naming the call, so the model still sees every call it made and that its
/// result was cut. An earlier run's result is never bounded, even under a
/// reused call id. The stored transcript keeps every result whole.
pub(crate) fn bound_open_run_tool_results_for_request(
    request: &[Message],
    run_id: &crate::lifecycle::RunId,
    encoded_bytes: u64,
    max_bytes: u64,
) -> Option<Vec<Message>> {
    if encoded_bytes <= max_bytes {
        return None;
    }
    let floor = open_run_floor(request, run_id)?;
    let run_calls: std::collections::BTreeMap<&str, &str> = request[floor..]
        .iter()
        .filter_map(|message| match message {
            Message::BlockAssistant(assistant) => Some(assistant.blocks.iter()),
            _ => None,
        })
        .flatten()
        .filter_map(|block| match block {
            AssistantBlock::ToolUse { id, name, .. } => Some((id.as_str(), name.as_str())),
            _ => None,
        })
        .collect();
    let content_bytes = |result: &crate::types::ToolResult| {
        serde_json::to_vec(&result.content).map_or(usize::MAX, |encoded| encoded.len())
    };
    let run_results = request[floor..]
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results.iter()),
            _ => None,
        })
        .flatten()
        .filter(|result| run_calls.contains_key(result.tool_use_id.as_str()))
        .collect::<Vec<_>>();
    if run_results.is_empty() {
        return None;
    }
    let run_bytes = run_results
        .iter()
        .map(|result| content_bytes(result))
        .fold(0usize, usize::saturating_add);
    let encoded = usize::try_from(encoded_bytes).unwrap_or(usize::MAX);
    let cap = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    // What the rest of the request leaves, with a tenth kept for the markers
    // and the provider's escaping of the heads.
    let other_bytes = encoded.saturating_sub(run_bytes);
    let room = cap.saturating_sub(other_bytes) / 10 * 9;
    let share = (room / run_results.len()).max(MIN_BOUNDED_RUN_RESULT_BYTES);
    let mut bounded_any = false;
    let bounded = request
        .iter()
        .enumerate()
        .map(|(offset, message)| {
            let Message::ToolResults {
                results,
                created_at,
            } = message
            else {
                return message.clone();
            };
            if offset < floor {
                return message.clone();
            }
            let results = results
                .iter()
                .map(|result| {
                    let Some(name) = run_calls.get(result.tool_use_id.as_str()) else {
                        return result.clone();
                    };
                    if content_bytes(result) <= share {
                        return result.clone();
                    }
                    bounded_any = true;
                    bounded_run_tool_result(result, name, share)
                })
                .collect();
            Message::ToolResults {
                results,
                created_at: *created_at,
            }
        })
        .collect::<Vec<_>>();
    bounded_any.then_some(bounded)
}

/// `result` with its content cut to the first `max_text_bytes` bytes of its
/// text projection (media as placeholders) and a model-visible marker naming
/// the tool call and how much of its result this request shows.
fn bounded_run_tool_result(
    result: &crate::types::ToolResult,
    tool_name: &str,
    max_text_bytes: usize,
) -> crate::types::ToolResult {
    let text = result
        .content
        .iter()
        .map(crate::types::ContentBlock::text_projection)
        .collect::<Vec<_>>()
        .join("\n");
    let mut kept = max_text_bytes.min(text.len());
    while !text.is_char_boundary(kept) {
        kept -= 1;
    }
    let mut bounded = result.clone();
    bounded.content = vec![
        crate::types::ContentBlock::Text {
            text: text.get(..kept).unwrap_or_default().to_string(),
        },
        crate::types::ContentBlock::Text {
            text: format!(
                "[result of tool call {tool_name} ({id}) shortened for this request: it shows \
                 the first {kept} of {total} bytes of its text, media as placeholders; the \
                 call already ran and its full result is kept in the session]",
                id = result.tool_use_id,
                total = text.len(),
            ),
        },
    ];
    bounded
}

/// Whether `rows` hold anything a rewrite can fold: a response (an
/// assistant or tool-results row). System rows, a prior summary and a
/// request without a response leave nothing to summarize.
pub(crate) fn prefix_has_foldable_turn(rows: &[Message]) -> bool {
    rows.iter().any(|message| {
        matches!(
            message,
            Message::BlockAssistant(_) | Message::ToolResults { .. }
        )
    })
}

/// Exact canonical unmeasured transcript subset incorporated by this attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactionObservationSource {
    digest: [u8; 32],
    has_observations: bool,
}

impl CompactionObservationSource {
    pub(crate) fn from_session(session: &Session) -> Result<Self, serde_json::Error> {
        let observations = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::BlockAssistant(assistant) => Some(&assistant.blocks),
                _ => None,
            })
            .flatten()
            .filter(|block| {
                matches!(
                    block,
                    crate::AssistantBlock::Transcript {
                        source: crate::types::TranscriptSource::SpokenUnmeasured,
                        ..
                    }
                )
            })
            .collect::<Vec<_>>();
        Self::from_observations(&observations)
    }

    fn from_observations(
        observations: &[&crate::AssistantBlock],
    ) -> Result<Self, serde_json::Error> {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.compaction-observation-source.v1\0");
        digest.update(serde_json::to_vec(observations)?);
        Ok(Self {
            digest: digest.finalize().into(),
            has_observations: !observations.is_empty(),
        })
    }
}

/// A summary call that came back but did not produce an accepted provider
/// summary: the provider stopped it, its text was empty, or the rebuild was
/// rejected. The provider did that work, so the caller charges it and records
/// its request row although nothing of it is committed.
#[derive(Debug)]
pub(crate) struct UncommittedSummaryCall {
    pub(crate) usage: TurnUsage,
    pub(crate) identity_dispute:
        Option<crate::provider_evidence::DisputedTurnUsageAccountingIdentity>,
}

/// One compaction attempt: its result, and the summary call it did not
/// commit, reported on success (the mechanical fallback) and failure alike.
pub(crate) struct CompactionAttempt {
    pub(crate) result: Result<CompactionOutcome, CompactionError>,
    pub(crate) uncommitted_call: Option<UncommittedSummaryCall>,
}

pub(crate) async fn run_compaction<C>(
    client: &C,
    compactor: &Arc<dyn Compactor>,
    curator: Option<&Arc<dyn CompactionCurator>>,
    invocation: CompactionInvocation<'_>,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
    event_tap: &crate::event_tap::EventTap,
) -> CompactionAttempt
where
    C: crate::agent::AgentLlmClient + ?Sized,
{
    let mut uncommitted_call = None;
    let result = compact_history(
        client,
        compactor,
        curator,
        invocation,
        event_tx,
        event_tap,
        &mut uncommitted_call,
    )
    .await;
    CompactionAttempt {
        result,
        uncommitted_call,
    }
}

/// `messages` without its operator-instruction entry rows: what a
/// summarizer reads. Borrowed when it holds none.
fn without_operator_instruction_entries(messages: &[Message]) -> std::borrow::Cow<'_, [Message]> {
    if messages
        .iter()
        .any(crate::session::is_operator_instruction_entry)
    {
        std::borrow::Cow::Owned(
            messages
                .iter()
                .filter(|message| !crate::session::is_operator_instruction_entry(message))
                .cloned()
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(messages)
    }
}

async fn compact_history<C>(
    client: &C,
    compactor: &Arc<dyn Compactor>,
    curator: Option<&Arc<dyn CompactionCurator>>,
    invocation: CompactionInvocation<'_>,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
    event_tap: &crate::event_tap::EventTap,
    uncommitted_call: &mut Option<UncommittedSummaryCall>,
) -> Result<CompactionOutcome, CompactionError>
where
    C: crate::agent::AgentLlmClient + ?Sized,
{
    let CompactionInvocation {
        authorization,
        budget,
        model_messages,
        observation_source,
        window,
        request_pressure,
        parent_revision,
        summarization_budget,
        provider_params,
        wait_report,
        scope,
        projection,
        earlier_folded_calls,
        instruction_fold,
        activation_retirement,
    } = invocation;
    let CompactionWindow {
        messages,
        last_input_tokens,
        session_boundary_index,
    } = window;
    // Reported over the whole projection: with a run in flight
    // `model_messages` holds only the rows the summary is written from.
    let estimated = estimate_tokens(projection)?;
    let message_count = projection.len();
    let mut event_stream_open = true;
    // No summarizer reads an operator-instruction entry row, a delta or a
    // snapshot: a summary is a user row, so a body it paraphrased would
    // outlive the entry that superseded or revoked it. The fold carries what
    // is in effect into its snapshot instead. Likewise no summarizer reads
    // an instruction activation body this compaction retires: it reads the
    // retired header, so a summary cannot restate a superseded body. Both
    // are done once, here, before the host curator and the compactor's own
    // preparation, which a host implements and cannot be relied on to do
    // it; the counts above keep the rows the model reads.
    let without_entries = without_operator_instruction_entries(model_messages);
    let summarizer_input = crate::session::with_retired_activation_bodies(
        &without_entries,
        activation_retirement.as_ref(),
    );

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
    // Routed by the caller, which owns the session identity this dispute
    // belongs to. A summary call charged under a foreign identity is still
    // charged: only attribution is in question.
    let mut summary_usage_identity_dispute = None;
    let (summary_text, summary_usage, summary_source) = if let Some(curator) = curator {
        let curator_window = CompactionWindow {
            messages: &summarizer_input,
            last_input_tokens,
            session_boundary_index,
        };
        match curator.curate_summary(curator_window).await {
            Ok(summary) => (
                summary.into_string(),
                TurnUsage::host_declared(
                    crate::Provider::Other,
                    "host-compaction-curator",
                    Usage::default(),
                ),
                CompactionSummarySource::HostCurator,
            ),
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

        let crate::compact::SummarizationSource {
            messages: mut compaction_messages,
            bound: source_bound,
        } = compactor.prepare_for_summarization(&summarizer_input, summarization_budget);
        compaction_messages.push(Message::User(crate::types::UserMessage::text(
            compaction_prompt.to_string(),
        )));

        // Call LLM with empty tools, max_summary_tokens. A summary refused
        // because an owner change is still in progress on the work's own
        // publication waits for it (bounded) and asks again under the same
        // operation; any other refusal fails this compaction as before.
        let mut owner_change_churn = crate::authorization::OwnerChangeChurn::reporting(wait_report);
        let llm_result = loop {
            let result = client
                .stream_response_authorized(
                    &compaction_messages,
                    &[],
                    max_summary_tokens,
                    None,
                    provider_params.as_ref(),
                    authorization.clone(),
                )
                .await;
            if let Err(error) = &result
                && error.operation_refusal().is_some_and(|refusal| {
                    refusal.kind() == crate::OperationRefusalKind::ReprepareRequired
                })
                && let Some(request) = authorization.as_ref()
                && let Some(wait) = owner_change_churn
                    .wait_for_settled_change(
                        request.work_authorization().authorization().as_ref(),
                        budget.and_then(crate::budget::Budget::remaining_duration),
                        "compaction_summary",
                        Some(request.operation_id()),
                    )
                    .await
                && wait.settled()
            {
                continue;
            }
            break result;
        };

        // Preserve accounting as soon as the provider returns. Rejecting the
        // text or its rebuild cannot undo that call. Keep a validation error
        // for the ordinary summary path without inventing a chargeable receipt.
        let llm_result = llm_result.map(|result| {
            let usage = validated_compaction_summary_usage(
                result.usage().clone(),
                client.provider(),
                client.model(),
            );
            *uncommitted_call =
                usage
                    .as_ref()
                    .ok()
                    .map(|(usage, identity_dispute)| UncommittedSummaryCall {
                        usage: usage.clone(),
                        identity_dispute: identity_dispute.clone(),
                    });
            (result, usage)
        });

        // A summary the provider stopped because the model context window
        // filled is truncated. It joins the capacity fallback below instead
        // of being committed. A refused or content-filtered summary fails this
        // compaction: its text must never become the session summary, so the
        // history stays as it is and the run goes on. Either call did real
        // provider work, so it is handed back as the uncommitted call, which
        // the caller charges whatever this attempt's result.
        let llm_result = match llm_result {
            Ok((result, _))
                if matches!(
                    result.stop_reason(),
                    StopReason::ContextWindowExceeded
                        | StopReason::Refusal
                        | StopReason::ContentFilter
                ) =>
            {
                if result.stop_reason() == StopReason::ContextWindowExceeded {
                    Err(crate::error::AgentError::llm_context_window_stop(
                        client.provider().as_str(),
                        None,
                        uncommitted_call
                            .as_ref()
                            .map(|call| call.usage.normalized_total_tokens()),
                    ))
                } else {
                    Err(crate::error::AgentError::llm(
                        client.provider().as_str(),
                        crate::error::LlmFailureReason::ProviderError(
                            crate::error::LlmProviderError::non_retryable(
                                crate::error::LlmProviderErrorKind::ContentFiltered,
                                serde_json::json!({ "stop_reason": result.stop_reason() }),
                            ),
                        ),
                        "the provider refused the compaction summary",
                    ))
                }
            }
            other => other,
        };

        match llm_result {
            Ok((result, usage)) => {
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
                let (usage, dispute) = usage?;
                summary_usage_identity_dispute = dispute;
                (
                    summary,
                    usage,
                    CompactionSummarySource::ProviderCall { source_bound },
                )
            }
            Err(e) if is_compaction_capacity_error(&e) && !observation_source.has_observations => {
                tracing::warn!(
                    error = %e,
                    message_count,
                    estimated_history_tokens = estimated,
                    "compaction summary request exceeded provider capacity; using mechanical progress fallback"
                );
                // No provider request backs the mechanical summary. A call the
                // provider stopped at the context window first is the
                // uncommitted call, charged by the caller on its own.
                (
                    MECHANICAL_CAPACITY_SUMMARY.to_string(),
                    TurnUsage::host_declared(
                        crate::Provider::Other,
                        "mechanical-compaction-fallback",
                        Usage::default(),
                    ),
                    CompactionSummarySource::MechanicalFallback,
                )
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
    // to remain verbatim at its mapped relative position. With a run in
    // flight only the rows before it are rebuilt by the compactor, and the
    // run's own rows follow verbatim, each retained at its exact offsets;
    // when nothing before the run is left, the run's completed rounds are
    // folded mechanically and its request and newest round kept, whatever
    // the compactor. That summary ends naming the calls the run's earlier
    // rounds made: those this fold takes out and those the run's earlier
    // folds took out, from the run's stored list (never from summary text).
    let folded_calls = match scope {
        CompactionScope::OpenRunRounds {
            first_response,
            newest_round,
        } => Some(folded_run_calls(
            &earlier_folded_calls,
            messages,
            first_response,
            newest_round,
        )),
        CompactionScope::Whole | CompactionScope::BeforeOpenRun { .. } => None,
    };
    let summary_text = match folded_calls.as_ref().and_then(folded_calls_line) {
        Some(line) => format!("{summary_text}{line}"),
        None => summary_text,
    };
    let result = match scope {
        CompactionScope::OpenRunRounds {
            first_response,
            newest_round,
        } => rebuild_open_run_rounds(messages, first_response, newest_round, &summary_text),
        CompactionScope::BeforeOpenRun { floor } => {
            debug_assert!(floor <= messages.len(), "the floor lies in the transcript");
            let floor = floor.min(messages.len());
            let mut result = compactor.rebuild_history_under_pressure(
                &messages[..floor],
                &summary_text,
                request_pressure,
            );
            let rebuilt_floor = result.messages.len();
            for (index, message) in messages[floor..].iter().enumerate() {
                result.retained.push(CompactionRetained::new(
                    u64::try_from(floor + index).unwrap_or(u64::MAX),
                    u64::try_from(rebuilt_floor + index).unwrap_or(u64::MAX),
                    message.clone(),
                ));
                result.messages.push(message.clone());
            }
            result
        }
        CompactionScope::Whole => {
            compactor.rebuild_history_under_pressure(messages, &summary_text, request_pressure)
        }
    };
    // 5. Validate the compactor's rebuild, then fold the instruction rows:
    // the compactor keeps the operator-instruction entry rows and every
    // activation row verbatim, as it keeps every System row; the fold takes
    // the entry rows out, inserts the authorized snapshot, and puts each
    // activation row whose body the authority retires in its retired form.
    let validated = validate_compaction_rebuild(
        messages,
        &result.messages,
        &result.summary,
        &summary_text,
        &result.retained,
        &result.discarded,
    )
    .and_then(|()| {
        fold_instruction_rows(
            messages,
            result,
            &summary_text,
            instruction_fold.as_ref(),
            activation_retirement.as_ref(),
        )
    });
    let result = match validated {
        Ok(result) => result,
        Err(error) => {
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
    };
    let rewrite_authority = ValidatedCompactionRewrite::from_validated(
        parent_revision,
        messages,
        &result.messages,
        observation_source,
    )?;
    let messages_after = result.messages.len();

    // The validated provider outcome now owns this receipt. A failed rewrite
    // or a mechanical fallback leaves it uncommitted for the caller to charge.
    if matches!(summary_source, CompactionSummarySource::ProviderCall { .. }) {
        *uncommitted_call = None;
    }

    Ok(CompactionOutcome {
        new_messages: result.messages,
        discarded: result.discarded,
        summary_usage,
        summary_source,
        summary_usage_identity_dispute,
        session_boundary_index,
        messages_before: message_count,
        messages_after,
        rewrite_authority,
        folded_run_calls: folded_calls,
        instruction_fold,
        activation_retirement,
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
    observation_source: CompactionObservationSource,
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
        observation_source: CompactionObservationSource,
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
            observation_source,
        })
    }

    pub(crate) fn authorizes_observations(&self, source: &CompactionObservationSource) -> bool {
        &self.observation_source == source
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

#[cfg(any(test, feature = "test-support"))]
impl ValidatedCompactionRewrite {
    pub(crate) fn for_test(
        messages: &[Message],
        rebuilt: &[Message],
    ) -> Result<Self, CompactionError> {
        let parent_revision = crate::session::transcript_messages_digest(messages)
            .map_err(|error| CompactionError::InvalidRebuild(error.to_string()))?;
        Self::from_validated(
            parent_revision,
            messages,
            rebuilt,
            CompactionObservationSource::from_observations(&[])
                .map_err(|error| CompactionError::InvalidRebuild(error.to_string()))?,
        )
    }
}

/// Who produced a compaction summary.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompactionSummarySource {
    /// The summarization LLM call: a real provider request.
    ProviderCall {
        /// How much of the history the call's source carried.
        source_bound: crate::compact::SummarizationSourceBound,
    },
    /// A configured host curator; no provider request was made.
    HostCurator,
    /// The mechanical capacity fallback; no provider request backs the
    /// summary. When the provider answered first and stopped at the model
    /// context window, that call is still charged to the session, apart from
    /// this outcome.
    MechanicalFallback,
}

/// Result of a successful compaction.
pub struct CompactionOutcome {
    /// New session messages to replace current history.
    pub new_messages: Vec<Message>,
    /// Messages that were discarded (for future memory indexing).
    pub discarded: Vec<CompactionDiscard>,
    /// Usage from the summary LLM call.
    pub summary_usage: TurnUsage,
    /// Who produced the summary. Only [`CompactionSummarySource::ProviderCall`]
    /// is backed by a provider request; the others carry zero usage.
    pub summary_source: CompactionSummarySource,
    /// Set when the summary call's accounting named a provider/model other
    /// than the one requested. The usage above is charged exactly as reported;
    /// the caller owns routing this dispute onto the session's event stream.
    pub summary_usage_identity_dispute:
        Option<crate::provider_evidence::DisputedTurnUsageAccountingIdentity>,
    /// Session boundary index at which compaction occurred.
    pub session_boundary_index: u64,
    /// Number of messages before compaction.
    pub messages_before: usize,
    /// Number of messages after compaction.
    pub messages_after: usize,
    /// Authority for the exact validated transcript rewrite semantic.
    pub(crate) rewrite_authority: ValidatedCompactionRewrite,
    /// The open run's folded-call list after this fold, when it folded the
    /// run's rounds: the caller stores it with the rewrite
    /// ([`persist_open_run_folded_calls`]).
    pub(crate) folded_run_calls: Option<FoldedCalls>,
    /// The operator-instruction fold whose snapshot the rewrite inserted:
    /// the caller records it in the same prepared session value as the
    /// rewrite (`Session::record_instruction_fold`).
    pub(crate) instruction_fold: Option<crate::session::InstructionFold>,
    /// The activation bodies the rewrite retired: the caller records the
    /// envelope-5 marker in the same prepared session value as the rewrite
    /// (`Session::record_activation_body_retirement`).
    pub(crate) activation_retirement: Option<crate::session::ActivationRetirement>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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

    fn unmeasured_voice(message: &Message, expected: &str) -> bool {
        matches!(message, Message::BlockAssistant(assistant) if assistant.blocks.iter().any(|block| matches!(block,
            AssistantBlock::Transcript { text, source: crate::types::TranscriptSource::SpokenUnmeasured, .. }
                if text.contains(expected)
        )))
    }

    fn session_with_unmeasured_voice() -> Session {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("Discuss the project.")));
        retain_voice_observation(&mut session, "first", "The blueprints need review.");
        session.push(Message::User(UserMessage::text(
            "Follow up on what we discussed.",
        )));
        assert_eq!(
            session
                .messages()
                .iter()
                .filter(|message| unmeasured_voice(message, ""))
                .count(),
            1
        );
        session
    }

    fn retain_voice_observation(session: &mut Session, item: &str, text: &str) {
        let channel = crate::LiveChannelId::new("observation-context");
        let interaction = crate::InteractionId::new();
        session
            .admit_live_assistant_playback_target(&channel, interaction, item, item, 0)
            .unwrap();
        session.append_realtime_transcript_event(
            crate::RealtimeTranscriptEvent::AssistantUnmeasuredSnapshotCommitted {
                channel_id: channel.to_string(),
                interaction_id: interaction,
                response_id: item.into(),
                item_id: item.into(),
                content_index: 0,
                text: text.into(),
                evidence: crate::LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(
                    text.into(),
                ),
            },
        );
        session
            .resolve_live_assistant_playback_target(&channel, interaction, item, item, 0)
            .unwrap();
    }

    #[test]
    fn ordinary_text_context_includes_unmeasured_voice_after_reload() {
        let session = session_with_unmeasured_voice();
        let encoded = serde_json::to_vec(&session).unwrap();
        let reloaded: Session = serde_json::from_slice(&encoded).unwrap();
        let projected = reloaded.messages_for_model_boundary().unwrap();
        assert_eq!(
            reloaded.messages().len(),
            3,
            "observed dialogue is canonical without claiming playback"
        );
        assert_eq!(projected.len(), 3);
        assert!(unmeasured_voice(
            &projected[1],
            "The blueprints need review."
        ));
        assert_eq!(projected[0], reloaded.messages()[0]);
        assert_eq!(projected[2], reloaded.messages()[2]);
        assert_eq!(projected, session.messages_for_model_boundary().unwrap());
        let mut ordinary = Session::new();
        ordinary.push(Message::User(UserMessage::text("Plain text.")));
        assert_eq!(
            ordinary.messages_for_model_boundary().unwrap(),
            ordinary.messages()
        );
    }

    struct ObservationCompactor;

    impl Compactor for ObservationCompactor {
        fn should_compact(&self, _: &CompactionContext) -> bool {
            true
        }
        fn compaction_prompt(&self) -> &'static str {
            "Summarize context, retaining uncertainty."
        }
        fn max_summary_tokens(&self) -> u32 {
            100
        }

        fn prepare_for_summarization(
            &self,
            messages: &[Message],
            _budget: Option<crate::compact::SummarizationSourceBudget>,
        ) -> crate::compact::SummarizationSource {
            crate::compact::SummarizationSource::whole(
                crate::types::materialize_latest_system_prompt_versions(messages),
            )
        }
        fn rebuild_history(
            &self,
            messages: &[Message],
            summary: &str,
        ) -> crate::compact::CompactionResult {
            let (message, mapping) = valid_summary(summary);
            crate::compact::CompactionResult {
                messages: vec![message],
                summary: mapping,
                retained: Vec::new(),
                discarded: messages
                    .iter()
                    .enumerate()
                    .map(|(offset, message)| CompactionDiscard::new(offset as u64, message.clone()))
                    .collect(),
            }
        }
    }

    struct ObservationCurator;

    #[async_trait::async_trait]
    impl CompactionCurator for ObservationCurator {
        async fn curate_summary(
            &self,
            window: CompactionWindow<'_>,
        ) -> Result<crate::compact::CuratedCompactionSummary, crate::compact::CompactionCuratorError>
        {
            assert!(
                window
                    .messages
                    .iter()
                    .any(|message| unmeasured_voice(message, "The blueprints need review."))
            );
            crate::compact::CuratedCompactionSummary::new(
                "Observed voice discussion concerned blueprint review; playback remains UNMEASURED.",
            )
        }
    }

    struct UnusedSummaryClient;

    #[async_trait::async_trait]
    impl crate::agent::AgentLlmClient for UnusedSummaryClient {
        async fn stream_response(
            &self,
            _: &[Message],
            _: &[Arc<crate::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&crate::ProviderParamsOverride>,
        ) -> Result<crate::LlmStreamResult, crate::error::AgentError> {
            Err(crate::error::AgentError::InternalError(
                "curator must own summary production".into(),
            ))
        }
        fn provider(&self) -> crate::Provider {
            crate::Provider::OpenAI
        }
        fn model(&self) -> &'static str {
            "test-observation-summary"
        }
    }

    struct ObservationSummaryClient {
        expect_observation: bool,
        capacity_failure: bool,
    }

    #[async_trait::async_trait]
    impl crate::agent::AgentLlmClient for ObservationSummaryClient {
        async fn stream_response(
            &self,
            messages: &[Message],
            tools: &[Arc<crate::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&crate::ProviderParamsOverride>,
        ) -> Result<crate::LlmStreamResult, crate::error::AgentError> {
            assert!(tools.is_empty());
            assert_eq!(
                messages
                    .iter()
                    .any(|message| unmeasured_voice(message, "The blueprints need review.")),
                self.expect_observation
            );
            if self.capacity_failure {
                return Err(crate::error::AgentError::llm(
                    "openai",
                    crate::error::LlmFailureReason::ProviderError(
                        crate::error::LlmProviderError::non_retryable(
                            crate::error::LlmProviderErrorKind::RequestTooLarge,
                            serde_json::json!({}),
                        ),
                    ),
                    "summary request exceeded capacity",
                ));
            }
            Ok(crate::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "Blueprint review was discussed in observed speech; playback UNMEASURED."
                        .into(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage {
                    input_tokens: 10,
                    output_tokens: 3,
                    provider_accounting: Some(crate::ProviderTokenAccounting::openai(
                        "test-observation-summary",
                        10,
                    )),
                    ..Usage::default()
                },
            ))
        }
        fn provider(&self) -> crate::Provider {
            crate::Provider::OpenAI
        }
        fn model(&self) -> &'static str {
            "test-observation-summary"
        }
    }

    async fn compact_observed_source<C: crate::agent::AgentLlmClient>(
        session: &Session,
        client: &C,
    ) -> Result<CompactionOutcome, CompactionError> {
        attempt_observed_source(session, client).await.result
    }

    async fn attempt_observed_source<C: crate::agent::AgentLlmClient>(
        session: &Session,
        client: &C,
    ) -> CompactionAttempt {
        let compactor: Arc<dyn Compactor> = Arc::new(ObservationCompactor);
        attempt_source_with_compactor(session, client, &compactor).await
    }

    async fn attempt_source_with_compactor<C: crate::agent::AgentLlmClient>(
        session: &Session,
        client: &C,
        compactor: &Arc<dyn Compactor>,
    ) -> CompactionAttempt {
        let model_messages = session.messages_for_model_boundary().unwrap();
        run_compaction(
            client,
            compactor,
            None,
            CompactionInvocation {
                authorization: None,
                budget: None,
                model_messages: &model_messages,
                observation_source: CompactionObservationSource::from_session(session).unwrap(),
                window: CompactionWindow {
                    messages: session.messages(),
                    last_input_tokens: 200,
                    session_boundary_index: 4,
                },
                request_pressure: None,
                parent_revision: session.transcript_revision().unwrap(),
                summarization_budget: None,
                provider_params: None,
                wait_report: None,
                scope: CompactionScope::Whole,
                projection: &model_messages,
                earlier_folded_calls: FoldedCalls::default(),
                instruction_fold: None,
                activation_retirement: None,
            },
            &None,
            &crate::event_tap::new_event_tap(),
        )
        .await
    }

    struct ReturnedSummaryClient {
        text: &'static str,
        stop_reason: StopReason,
        usage: Usage,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl ReturnedSummaryClient {
        fn new(text: &'static str, stop_reason: StopReason) -> Self {
            Self {
                text,
                stop_reason,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 3,
                    provider_accounting: Some(crate::ProviderTokenAccounting::openai(
                        "test-receipt-summary",
                        10,
                    )),
                    ..Usage::default()
                },
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::agent::AgentLlmClient for ReturnedSummaryClient {
        async fn stream_response(
            &self,
            _: &[Message],
            tools: &[Arc<crate::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&crate::ProviderParamsOverride>,
        ) -> Result<crate::LlmStreamResult, crate::error::AgentError> {
            assert!(tools.is_empty());
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let blocks = if self.text.is_empty() {
                Vec::new()
            } else {
                vec![AssistantBlock::Text {
                    text: self.text.into(),
                    meta: None,
                }]
            };
            Ok(crate::LlmStreamResult::new(
                blocks,
                self.stop_reason,
                self.usage.clone(),
            ))
        }

        fn provider(&self) -> crate::Provider {
            crate::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            "test-receipt-summary"
        }
    }

    async fn assert_empty_summary_preserves_provider_usage(stop_reason: StopReason) {
        let session = session_with_unmeasured_voice();
        let before = serde_json::to_vec(&session).unwrap();
        let mut client = ReturnedSummaryClient::new("", stop_reason);
        if stop_reason == StopReason::MaxTokens {
            client.usage.reasoning_tokens = Some(3);
        }
        let attempt = attempt_observed_source(&session, &client).await;
        assert!(matches!(attempt.result, Err(CompactionError::EmptySummary)));
        let call = attempt
            .uncommitted_call
            .expect("an empty summary still consumed one provider call");
        assert_eq!(call.usage.into_inner(), client.usage, "{stop_reason:?}");
        assert!(call.identity_dispute.is_none());
        assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);
    }

    #[tokio::test]
    async fn empty_end_turn_summary_preserves_provider_usage() {
        assert_empty_summary_preserves_provider_usage(StopReason::EndTurn).await;
    }

    #[tokio::test]
    async fn empty_max_tokens_summary_preserves_provider_usage() {
        assert_empty_summary_preserves_provider_usage(StopReason::MaxTokens).await;
    }

    #[tokio::test]
    async fn empty_summary_preserves_normalized_usage_and_identity_dispute() {
        let session = session_with_unmeasured_voice();
        let mut client = ReturnedSummaryClient::new("", StopReason::EndTurn);
        client.usage = Usage {
            input_tokens: 5,
            output_tokens: 3,
            cache_read_tokens: Some(7),
            cache_creation_tokens: Some(2),
            provider_accounting: Some(crate::ProviderTokenAccounting::anthropic(
                "reported-summary-model",
                5,
                7,
                2,
            )),
            ..Usage::default()
        };
        let attempt = attempt_observed_source(&session, &client).await;
        assert!(matches!(attempt.result, Err(CompactionError::EmptySummary)));
        let call = attempt
            .uncommitted_call
            .expect("reported work is preserved");
        assert_eq!(call.usage.presented_tokens(), 14);
        assert_eq!(call.usage.normalized_total_tokens(), 17);
        assert_eq!(call.usage.into_inner(), client.usage);
        let dispute = call.identity_dispute.expect("attribution remains disputed");
        assert_eq!(dispute.active_provider, crate::Provider::OpenAI);
        assert_eq!(dispute.active_model, "test-receipt-summary");
        assert_eq!(dispute.reported_provider, crate::Provider::Anthropic);
        assert_eq!(dispute.reported_model, "reported-summary-model");
        assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    struct InvalidSummaryRebuild;

    impl Compactor for InvalidSummaryRebuild {
        fn should_compact(&self, context: &CompactionContext) -> bool {
            ObservationCompactor.should_compact(context)
        }

        fn compaction_prompt(&self) -> &'static str {
            ObservationCompactor.compaction_prompt()
        }

        fn max_summary_tokens(&self) -> u32 {
            ObservationCompactor.max_summary_tokens()
        }

        fn prepare_for_summarization(
            &self,
            messages: &[Message],
            budget: Option<crate::compact::SummarizationSourceBudget>,
        ) -> crate::compact::SummarizationSource {
            ObservationCompactor.prepare_for_summarization(messages, budget)
        }

        fn rebuild_history(
            &self,
            messages: &[Message],
            summary: &str,
        ) -> crate::compact::CompactionResult {
            let mut result = ObservationCompactor.rebuild_history(messages, summary);
            result.discarded.pop();
            result
        }
    }

    #[tokio::test]
    async fn rejected_summary_rebuild_preserves_provider_usage() {
        let session = session_with_unmeasured_voice();
        let before = serde_json::to_vec(&session).unwrap();
        let client = ReturnedSummaryClient::new("A valid summary.", StopReason::EndTurn);
        let compactor: Arc<dyn Compactor> = Arc::new(InvalidSummaryRebuild);
        let attempt = attempt_source_with_compactor(&session, &client, &compactor).await;
        assert!(matches!(
            attempt.result,
            Err(CompactionError::InvalidRebuild(_))
        ));
        let call = attempt
            .uncommitted_call
            .expect("rejecting a rebuild does not undo provider work");
        assert_eq!(call.usage.into_inner(), client.usage);
        assert!(call.identity_dispute.is_none());
        assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);
    }

    #[tokio::test]
    async fn successful_summary_exposes_only_committed_provider_usage() {
        let session = session_with_unmeasured_voice();
        let client = ReturnedSummaryClient::new("A valid summary.", StopReason::EndTurn);
        let attempt = attempt_observed_source(&session, &client).await;
        let outcome = attempt.result.expect("valid summary and rebuild");
        assert!(matches!(
            outcome.summary_source,
            CompactionSummarySource::ProviderCall { .. }
        ));
        assert_eq!(outcome.summary_usage.into_inner(), client.usage);
        assert!(
            attempt.uncommitted_call.is_none(),
            "the call must not be charged twice"
        );
        assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalid_summary_accounting_does_not_fabricate_a_provider_receipt() {
        for text in ["", "A valid summary."] {
            let session = session_with_unmeasured_voice();
            let before = serde_json::to_vec(&session).unwrap();
            let mut client = ReturnedSummaryClient::new(text, StopReason::EndTurn);
            client.usage.provider_accounting = None;
            let attempt = attempt_observed_source(&session, &client).await;
            if text.is_empty() {
                assert!(matches!(attempt.result, Err(CompactionError::EmptySummary)));
            } else {
                assert!(matches!(attempt.result, Err(CompactionError::LlmFailed(_))));
            }
            assert!(attempt.uncommitted_call.is_none());
            assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(serde_json::to_vec(&session).unwrap(), before);
        }
    }

    #[tokio::test]
    async fn stale_compaction_cannot_consume_later_unmeasured_transcript() {
        let mut session = session_with_unmeasured_voice();
        let revision = session.transcript_revision().unwrap();
        let outcome = compact_observed_source(
            &session,
            &ObservationSummaryClient {
                expect_observation: true,
                capacity_failure: false,
            },
        )
        .await
        .unwrap();
        retain_voice_observation(&mut session, "late", "A later voice-only decision.");
        assert_ne!(session.transcript_revision().unwrap(), revision);
        let before = serde_json::to_vec(&session).unwrap();
        assert!(
            session
                .replace_messages_for_compaction_internal(
                    outcome.new_messages,
                    &outcome.rewrite_authority,
                )
                .is_err(),
            "the prior canonical authority cannot cover newly observed dialogue"
        );
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);
        assert_eq!(
            session
                .messages()
                .iter()
                .filter(|message| unmeasured_voice(message, ""))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn compaction_llm_receives_observed_dialogue_without_tools() {
        let mut session = session_with_unmeasured_voice();
        let outcome = compact_observed_source(
            &session,
            &ObservationSummaryClient {
                expect_observation: true,
                capacity_failure: false,
            },
        )
        .await
        .unwrap();
        session
            .replace_messages_for_compaction_internal(
                outcome.new_messages,
                &outcome.rewrite_authority,
            )
            .unwrap()
            .expect("validated compaction");
        let reloaded: Session =
            serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
        assert!(
            matches!(&reloaded.messages_for_model_boundary().unwrap()[0], Message::User(user)
                if user.text_content().contains("Blueprint review")
                    && user.text_content().contains("UNMEASURED"))
        );
    }

    #[tokio::test]
    async fn capacity_fallback_cannot_erase_unsummarized_voice_observations() {
        let session = session_with_unmeasured_voice();
        let before = serde_json::to_vec(&session).unwrap();
        assert!(matches!(
            compact_observed_source(
                &session,
                &ObservationSummaryClient {
                    expect_observation: true,
                    capacity_failure: true,
                }
            )
            .await,
            Err(CompactionError::LlmFailed(_))
        ));
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);
        let mut ordinary = Session::new();
        ordinary.push(Message::User(UserMessage::text("One")));
        ordinary.push(Message::User(UserMessage::text("Two")));
        let outcome = compact_observed_source(
            &ordinary,
            &ObservationSummaryClient {
                expect_observation: false,
                capacity_failure: true,
            },
        )
        .await
        .expect("ordinary mechanical capacity fallback remains available");
        assert!(matches!(&outcome.new_messages[0], Message::User(user)
                if user.text_content().contains(MECHANICAL_CAPACITY_SUMMARY)));
    }

    /// A summary call the provider stopped because the model context window
    /// filled.
    struct ContextWindowStoppedSummaryClient;

    #[async_trait::async_trait]
    impl crate::agent::AgentLlmClient for ContextWindowStoppedSummaryClient {
        async fn stream_response(
            &self,
            _: &[Message],
            tools: &[Arc<crate::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&crate::ProviderParamsOverride>,
        ) -> Result<crate::LlmStreamResult, crate::error::AgentError> {
            assert!(tools.is_empty());
            Ok(crate::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "Blueprint review was discussed and the".into(),
                    meta: None,
                }],
                StopReason::ContextWindowExceeded,
                Usage {
                    input_tokens: 10,
                    output_tokens: 3,
                    provider_accounting: Some(crate::ProviderTokenAccounting::openai(
                        "test-observation-summary",
                        10,
                    )),
                    ..Usage::default()
                },
            ))
        }
        fn provider(&self) -> crate::Provider {
            crate::Provider::OpenAI
        }
        fn model(&self) -> &'static str {
            "test-observation-summary"
        }
    }

    #[tokio::test]
    async fn context_window_stopped_summary_joins_the_capacity_fallback() {
        let mut ordinary = Session::new();
        ordinary.push(Message::User(UserMessage::text("One")));
        ordinary.push(Message::User(UserMessage::text("Two")));
        let attempt = attempt_observed_source(&ordinary, &ContextWindowStoppedSummaryClient).await;
        let outcome = attempt
            .result
            .expect("a truncated summary takes the mechanical capacity fallback");
        assert!(matches!(&outcome.new_messages[0], Message::User(user)
                if user.text_content().contains(MECHANICAL_CAPACITY_SUMMARY)));
        assert_eq!(
            outcome.summary_source,
            CompactionSummarySource::MechanicalFallback
        );
        assert_eq!(outcome.summary_usage.output_tokens, 0);
        // The provider answered, so its work stays charged, apart from the
        // mechanical summary.
        assert_eq!(
            attempt
                .uncommitted_call
                .map(|call| call.usage.output_tokens),
            Some(3)
        );

        // With unsummarized voice observations the fallback is closed, as for
        // a refused summary request: the source is preserved, and the stopped
        // call is still charged.
        let session = session_with_unmeasured_voice();
        let before = serde_json::to_vec(&session).unwrap();
        let attempt = attempt_observed_source(&session, &ContextWindowStoppedSummaryClient).await;
        assert!(matches!(
            attempt.result,
            Err(CompactionError::LlmFailed(crate::error::AgentError::Llm {
                reason: crate::error::LlmFailureReason::ContextExceeded { .. },
                ..
            }))
        ));
        assert_eq!(
            attempt
                .uncommitted_call
                .map(|call| call.usage.output_tokens),
            Some(3)
        );
        assert_eq!(serde_json::to_vec(&session).unwrap(), before);
    }

    /// A summary call that stopped with a refusal or a content filter.
    struct RefusedSummaryClient {
        stop_reason: StopReason,
    }

    #[async_trait::async_trait]
    impl crate::agent::AgentLlmClient for RefusedSummaryClient {
        async fn stream_response(
            &self,
            _: &[Message],
            _: &[Arc<crate::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&crate::ProviderParamsOverride>,
        ) -> Result<crate::LlmStreamResult, crate::error::AgentError> {
            Ok(crate::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "I can't help with summarizing this conversation.".into(),
                    meta: None,
                }],
                self.stop_reason,
                Usage {
                    input_tokens: 10,
                    output_tokens: 3,
                    provider_accounting: Some(crate::ProviderTokenAccounting::openai(
                        "test-observation-summary",
                        10,
                    )),
                    ..Usage::default()
                },
            ))
        }
        fn provider(&self) -> crate::Provider {
            crate::Provider::OpenAI
        }
        fn model(&self) -> &'static str {
            "test-observation-summary"
        }
    }

    /// The refusal text never becomes the session summary: the compaction
    /// fails as a content-filtered LLM failure and the history is untouched.
    #[tokio::test]
    async fn refused_summary_fails_the_compaction_and_keeps_the_history() {
        for stop_reason in [StopReason::Refusal, StopReason::ContentFilter] {
            let mut session = Session::new();
            session.push(Message::User(UserMessage::text("One")));
            session.push(Message::User(UserMessage::text("Two")));
            let before = serde_json::to_vec(&session).unwrap();
            let attempt =
                attempt_observed_source(&session, &RefusedSummaryClient { stop_reason }).await;
            let outcome = attempt.result;
            assert!(
                matches!(
                    &outcome,
                    Err(CompactionError::LlmFailed(crate::error::AgentError::Llm {
                        reason: crate::error::LlmFailureReason::ProviderError(provider_error),
                        ..
                    })) if provider_error.kind
                        == crate::error::LlmProviderErrorKind::ContentFiltered
                ),
                "{stop_reason:?}: {:?}",
                outcome.as_ref().err()
            );
            assert_eq!(
                serde_json::to_vec(&session).unwrap(),
                before,
                "{stop_reason:?}"
            );
            // The refused call did real provider work: it is handed back to
            // be charged although nothing of it is committed.
            assert_eq!(
                attempt
                    .uncommitted_call
                    .map(|call| call.usage.output_tokens),
                Some(3),
                "{stop_reason:?}"
            );
        }
    }

    #[tokio::test]
    async fn compaction_integrates_voice_observations_before_rebase_and_reload() {
        let mut session = session_with_unmeasured_voice();
        let model_messages = session.messages_for_model_boundary().unwrap();
        let compactor: Arc<dyn Compactor> = Arc::new(ObservationCompactor);
        let curator: Arc<dyn CompactionCurator> = Arc::new(ObservationCurator);
        let attempt = run_compaction(
            &UnusedSummaryClient,
            &compactor,
            Some(&curator),
            CompactionInvocation {
                authorization: None,
                budget: None,
                model_messages: &model_messages,
                observation_source: CompactionObservationSource::from_session(&session).unwrap(),
                window: CompactionWindow {
                    messages: session.messages(),
                    last_input_tokens: 200,
                    session_boundary_index: 4,
                },
                request_pressure: None,
                parent_revision: session.transcript_revision().unwrap(),
                summarization_budget: None,
                provider_params: None,
                wait_report: None,
                scope: CompactionScope::Whole,
                projection: &model_messages,
                earlier_folded_calls: FoldedCalls::default(),
                instruction_fold: None,
                activation_retirement: None,
            },
            &None,
            &crate::event_tap::new_event_tap(),
        )
        .await;
        assert!(
            attempt.uncommitted_call.is_none(),
            "the curator made no provider call"
        );
        let outcome = attempt.result.unwrap();
        assert_eq!(outcome.summary_source, CompactionSummarySource::HostCurator);
        assert_eq!(outcome.summary_usage.normalized_total_tokens(), 0);
        assert!(
            outcome
                .discarded
                .iter()
                .all(|discard| discard.source_offset < session.messages().len() as u64),
            "observed transcript discard offsets refer to actual canonical rows"
        );
        session
            .replace_messages_for_compaction_internal(
                outcome.new_messages,
                &outcome.rewrite_authority,
            )
            .unwrap()
            .expect("generated compaction commits");
        assert_eq!(
            session
                .messages()
                .iter()
                .filter(|message| unmeasured_voice(message, ""))
                .count(),
            0,
            "raw observations retire only after their source reached the summary producer"
        );
        let reloaded: Session =
            serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
        let projected = reloaded.messages_for_model_boundary().unwrap();
        assert_eq!(projected.len(), 1);
        assert!(matches!(&projected[0], Message::User(user)
            if user.transcript_role.is_compaction_summary()
                && user.text_content().contains("blueprint review")
                && user.text_content().contains("UNMEASURED")));
        assert_eq!(projected, reloaded.messages_for_model_boundary().unwrap());
    }

    /// These two cases used to fail the compaction closed. They are now
    /// reported disputes: the summary call happened and its counters are
    /// internally consistent, so they are charged exactly as reported and the
    /// contested attribution is handed back for routing. Refusing here would
    /// discard a summary the provider already produced and paid for over a
    /// disagreement about a name.
    fn assert_compaction_usage_identity_disputed(
        usage: Usage,
        expected_reported_provider: crate::Provider,
        expected_reported_model: &str,
    ) {
        let (turn_usage, dispute) =
            validated_compaction_summary_usage(usage, crate::Provider::OpenAI, "active-model")
                .expect("a contested identity must not fail the compaction");
        let dispute = dispute.expect("the disagreement must be reported, not swallowed");
        assert_eq!(dispute.active_provider, crate::Provider::OpenAI);
        assert_eq!(dispute.active_model, "active-model");
        assert_eq!(dispute.reported_provider, expected_reported_provider);
        assert_eq!(
            dispute.reported_model, expected_reported_model,
            "the reported identity must survive verbatim"
        );
        assert_eq!(
            turn_usage.presented_tokens(),
            5,
            "the disputed call is still charged exactly what it reported"
        );
    }

    #[test]
    fn compaction_summary_usage_disputes_wrong_provider() {
        assert_compaction_usage_identity_disputed(
            Usage {
                input_tokens: 5,
                output_tokens: 1,
                provider_accounting: Some(crate::ProviderTokenAccounting::anthropic(
                    "active-model",
                    5,
                    0,
                    0,
                )),
                ..Usage::default()
            },
            crate::Provider::Anthropic,
            "active-model",
        );
    }

    #[test]
    fn compaction_summary_usage_disputes_same_provider_wrong_model() {
        assert_compaction_usage_identity_disputed(
            Usage {
                input_tokens: 5,
                output_tokens: 1,
                provider_accounting: Some(crate::ProviderTokenAccounting::openai("wrong-model", 5)),
                ..Usage::default()
            },
            crate::Provider::OpenAI,
            "wrong-model",
        );
    }

    /// Absent accounting still fails the compaction closed, and that is a
    /// different verdict for a reason: a refused compaction preserves the
    /// untouched history and the run continues (`Agent::maybe_compact`
    /// consumes this as `if let Ok(outcome)`), so nothing the caller has read
    /// is terminalized. The model turn cannot say the same, which is why it
    /// degrades instead.
    #[test]
    fn compaction_summary_usage_still_refuses_absent_accounting() {
        let error = validated_compaction_summary_usage(
            Usage {
                input_tokens: 5,
                output_tokens: 1,
                ..Usage::default()
            },
            crate::Provider::OpenAI,
            "active-model",
        )
        .expect_err("a compaction summary with no accounting is refused");
        assert!(matches!(
            error,
            CompactionError::LlmFailed(crate::error::AgentError::Llm {
                reason: crate::error::LlmFailureReason::ProviderError(ref provider_error),
                ..
            }) if provider_error.kind == crate::error::LlmProviderErrorKind::IncompleteResponse
        ));
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
        session.record_cumulative_usage(Usage::default());

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
            last_rewrite_history_tokens: Some(61_234),
        };
        persist_compaction_cadence(&mut session, &cadence).unwrap();

        assert_eq!(load_compaction_cadence(&mut session).unwrap(), cadence);
    }

    /// Cadence metadata written before 0.9.4 has no rewrite evidence: it reads
    /// as `None` (no evidence, so a history crossing still bypasses the
    /// cadence), and a cadence without evidence writes no field for an older
    /// binary to meet.
    #[test]
    fn compaction_cadence_without_rewrite_evidence_reads_and_writes_as_before() {
        let legacy = serde_json::json!({
            "session_boundary_index": 7,
            "last_compaction_boundary_index": 4,
            "last_compaction_attempt_boundary_index": 6
        });
        let cadence: SessionCompactionCadence = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(cadence.last_rewrite_history_tokens, None);
        assert_eq!(serde_json::to_value(&cadence).unwrap(), legacy);
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
                delivery_id: None,
                in_reply_to: None,
            },
        ))];

        // One inline image => IMAGE_TOKEN_ESTIMATE, not ~1M tokens from base64.
        assert_eq!(estimate_tokens(&messages).unwrap(), IMAGE_TOKEN_ESTIMATE);
    }

    #[test]
    fn estimate_tokens_uses_image_heuristic_for_runtime_notice_content() {
        use crate::types::{ImageData, SystemNoticeBlock, SystemNoticeKind, SystemNoticeMessage};
        let notice = |content: Vec<ContentBlock>| {
            Message::SystemNotice(SystemNoticeMessage::with_block(
                SystemNoticeKind::Generic,
                None,
                SystemNoticeBlock::RuntimeNotice {
                    category: "consent_attempt".to_string(),
                    detail: None,
                    payload: None,
                    content,
                },
            ))
        };
        let fields = estimate_tokens(&[notice(Vec::new())]).unwrap();
        // One large inline image: the fixed image heuristic plus the same
        // field estimate, never ~1M tokens of base64.
        let with_image = notice(vec![ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: ImageData::Inline {
                data: "A".repeat(4_000_000),
            },
        }]);
        assert_eq!(
            estimate_tokens(&[with_image]).unwrap(),
            IMAGE_TOKEN_ESTIMATE + fields
        );
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
        let ctx = build_compaction_context(&messages, 42, None, None, None, 7);
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
    fn compaction_retains_assistant_message_ids_exactly_and_rejects_tampering() {
        let occurrence = crate::types::AssistantMessageId::mint();
        let mut kept = crate::types::BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "kept answer".to_string(),
                meta: None,
            }],
            crate::types::StopReason::EndTurn,
        );
        kept.assistant_message_id = Some(occurrence);
        let discarded_row = Message::User(UserMessage::text("old question"));
        let kept = Message::BlockAssistant(kept);
        let source = vec![discarded_row.clone(), kept.clone()];
        let (summary_message, summary) = valid_summary("summary");
        let discarded = vec![CompactionDiscard::new(0, discarded_row)];

        let rebuilt = vec![summary_message.clone(), kept.clone()];
        let retained = vec![CompactionRetained::new(1, 1, kept.clone())];
        validate_compaction_rebuild(
            &source, &rebuilt, &summary, "summary", &retained, &discarded,
        )
        .expect("a retained row with its exact id is a valid rebuild");

        // A rebuild that re-mints, strips, or swaps the retained id is not the
        // same occurrence and must be refused.
        for tampered_id in [None, Some(crate::types::AssistantMessageId::mint())] {
            let mut tampered = kept.clone();
            if let Message::BlockAssistant(assistant) = &mut tampered {
                assistant.assistant_message_id = tampered_id;
            }
            let rebuilt = vec![summary_message.clone(), tampered.clone()];
            let retained = vec![CompactionRetained::new(1, 1, tampered)];
            let error = validate_compaction_rebuild(
                &source, &rebuilt, &summary, "summary", &retained, &discarded,
            )
            .expect_err("a retained row whose id changed is rejected");
            assert!(matches!(error, CompactionError::InvalidRebuild(_)));
        }
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
    fn compaction_rebuild_rejects_discarding_instruction_activation() {
        let body = "durable instruction";
        let mut activation = crate::types::SystemMessage::new(body);
        activation.instruction_activation = Some(crate::types::InstructionActivationIdentity {
            activation_id: crate::types::InstructionActivationId::new("activation-1").unwrap(),
            revision: crate::types::InstructionRevisionRef {
                namespace: crate::types::InstructionNamespace::new("app.example").unwrap(),
                key: crate::types::InstructionKey::new("primary").unwrap(),
                revision_id: crate::types::InstructionRevisionId::new("revision-1").unwrap(),
                content_sha256: crate::types::InstructionContentDigest::for_body(body),
            },
            supersedes: None,
            origin_session_id: crate::types::SessionId::new(),
            render_version: crate::INSTRUCTION_ACTIVATION_RENDER_VERSION_V1,
        });
        let activation = Message::System(activation);
        let old = Message::User(UserMessage::text("old turn"));
        let recent = Message::User(UserMessage::text("recent turn"));
        let source = vec![old.clone(), activation.clone(), recent.clone()];
        let (summary_message, summary) = valid_summary("summary");
        let rebuilt = vec![summary_message, recent.clone()];
        let retained = vec![CompactionRetained::new(2, 1, recent)];
        let discarded = vec![
            CompactionDiscard::new(0, old),
            CompactionDiscard::new(1, activation),
        ];

        let error = validate_compaction_rebuild(
            &source, &rebuilt, &summary, "summary", &retained, &discarded,
        )
        .expect_err("a chronological instruction activation must survive compaction");

        assert!(error.to_string().contains("must be retained verbatim"));
    }

    fn activation_request(
        activation: &str,
        supersedes: Option<&str>,
        body: &str,
    ) -> crate::InstructionActivationRequest {
        crate::InstructionActivationRequest {
            revision: crate::types::InstructionRevisionRef {
                namespace: crate::types::InstructionNamespace::new("app.skills").unwrap(),
                key: crate::types::InstructionKey::new("calendar").unwrap(),
                revision_id: crate::types::InstructionRevisionId::new(format!("rev-{activation}"))
                    .unwrap(),
                content_sha256: crate::types::InstructionContentDigest::for_body(body),
            },
            activation_id: crate::types::InstructionActivationId::new(activation).unwrap(),
            expectation: supersedes.map_or(
                crate::InstructionActivationExpectation::Absent,
                |previous| {
                    crate::InstructionActivationExpectation::Effective(
                        crate::types::InstructionActivationId::new(previous).unwrap(),
                    )
                },
            ),
            supersedes: supersedes
                .map(|previous| crate::types::InstructionActivationId::new(previous).unwrap()),
            body: body.to_string(),
        }
    }

    /// `[prompt, A1, turn, A2, turn]`, A2 superseding A1, and a compactor's
    /// rebuild of it that keeps every System row and the last turn.
    fn superseded_activation_rebuild() -> (Vec<Message>, crate::compact::CompactionResult) {
        let mut session = Session::new();
        session.append_system_message("prompt");
        session
            .activate_instruction(activation_request("act-1", None, "first body"))
            .unwrap();
        session.push(Message::User(UserMessage::text("first turn")));
        session
            .activate_instruction(activation_request("act-2", Some("act-1"), "second body"))
            .unwrap();
        session.push(Message::User(UserMessage::text("second turn")));
        let source = session.messages().to_vec();
        let (summary_message, _) = valid_summary("summary");
        let rebuilt = vec![
            source[0].clone(),
            source[1].clone(),
            summary_message.clone(),
            source[3].clone(),
            source[4].clone(),
        ];
        let result = crate::compact::CompactionResult {
            messages: rebuilt,
            summary: CompactionSummary::new(2, summary_message),
            retained: vec![
                CompactionRetained::new(0, 0, source[0].clone()),
                CompactionRetained::new(1, 1, source[1].clone()),
                CompactionRetained::new(3, 3, source[3].clone()),
                CompactionRetained::new(4, 4, source[4].clone()),
            ],
            discarded: vec![CompactionDiscard::new(2, source[2].clone())],
        };
        (source, result)
    }

    fn system_row(message: &Message) -> &crate::types::SystemMessage {
        let Message::System(system) = message else {
            panic!("a System row: {message:?}");
        };
        system
    }

    /// The fold puts exactly the authorized retired form in place of a
    /// superseded row, and refuses a retirement the session-document
    /// authority did not authorize: one naming a key's newest row, one with
    /// an inexact retired form, one naming a row that is not an activation
    /// row, and a rebuild that retires a row the set does not name.
    #[test]
    fn the_fold_refuses_an_unauthorized_or_newest_retirement() {
        let (source, result) = superseded_activation_rebuild();
        validate_compaction_rebuild(
            &source,
            &result.messages,
            &result.summary,
            "summary",
            &result.retained,
            &result.discarded,
        )
        .expect("the compactor's rebuild keeps every row verbatim");
        let retire = |offset: usize| {
            crate::session::retired_activation_row(system_row(&source[offset]))
                .expect("a live activation row has a retired form")
        };

        let authorized =
            crate::session::ActivationRetirement::from_rows([(1, retire(1))].into_iter().collect());
        let folded =
            fold_instruction_rows(&source, result.clone(), "summary", None, Some(&authorized))
                .expect("the authorized retirement folds");
        assert_eq!(folded.messages[1], Message::System(retire(1)));
        assert_eq!(
            folded.messages[3], source[3],
            "the newest row keeps its body"
        );

        let newest =
            crate::session::ActivationRetirement::from_rows([(3, retire(3))].into_iter().collect());
        let error = fold_instruction_rows(&source, result.clone(), "summary", None, Some(&newest))
            .expect_err("a key's newest row keeps its body");
        assert!(
            error.to_string().contains("is its key's newest row"),
            "{error}"
        );

        let mut inexact = retire(1);
        inexact.retired_instruction_body = Some(crate::types::RetiredInstructionBody {
            render_version: crate::RETIRED_INSTRUCTION_RENDER_VERSION_V1,
            body_bytes: 1,
        });
        let inexact =
            crate::session::ActivationRetirement::from_rows([(1, inexact)].into_iter().collect());
        let error = fold_instruction_rows(&source, result.clone(), "summary", None, Some(&inexact))
            .expect_err("an inexact retired form is refused");
        assert!(
            error.to_string().contains("is not the exact retired form"),
            "{error}"
        );

        let not_an_activation =
            crate::session::ActivationRetirement::from_rows([(0, retire(1))].into_iter().collect());
        let error = fold_instruction_rows(
            &source,
            result.clone(),
            "summary",
            None,
            Some(&not_an_activation),
        )
        .expect_err("a row that is not an activation row has no body to retire");
        assert!(
            error
                .to_string()
                .contains("not an instruction activation row"),
            "{error}"
        );

        let mut unauthorized = result;
        unauthorized.messages[1] = Message::System(retire(1));
        let nothing =
            crate::session::ActivationRetirement::from_rows(std::collections::BTreeMap::new());
        let error = validate_compaction_rebuild_with(
            &source,
            &unauthorized.messages,
            &unauthorized.summary,
            "summary",
            &unauthorized.retained,
            &unauthorized.discarded,
            RebuildInsertions::Fold {
                folds_entries: false,
                snapshot: None,
                retirement: Some(&nothing),
            },
        )
        .expect_err("a retirement the authority did not authorize is refused");
        assert!(
            error.to_string().contains("does not match rebuilt offset"),
            "{error}"
        );
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

    fn call_row(run_id: Option<&crate::lifecycle::RunId>, call_id: &str) -> Message {
        let mut call = BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: call_id.into(),
                name: "list_records".into(),
                args: serde_json::value::RawValue::from_string("{}".into()).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
        );
        call.identity.run_id = run_id.cloned();
        Message::BlockAssistant(call)
    }

    fn answer_row(run_id: Option<&crate::lifecycle::RunId>, text: &str) -> Message {
        let mut answer = BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: text.into(),
                meta: None,
            }],
            StopReason::EndTurn,
        );
        answer.identity.run_id = run_id.cloned();
        Message::BlockAssistant(answer)
    }

    fn result_row(call_id: &str, bytes: usize) -> Message {
        Message::tool_results(vec![ToolResult::new(
            call_id.into(),
            "r".repeat(bytes),
            false,
        )])
    }

    fn failed_result_row(call_id: &str) -> Message {
        Message::tool_results(vec![ToolResult::new(
            call_id.into(),
            "the call failed".into(),
            true,
        )])
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn notice(kind: crate::types::SystemNoticeKind, body: &str) -> Message {
        Message::SystemNotice(crate::types::SystemNoticeMessage::new(kind, body))
    }

    fn summary_row(text: &str) -> Message {
        Message::User(UserMessage::compaction_summary(format!(
            "{COMPACTION_SUMMARY_PREFIX}{text}"
        )))
    }

    /// An earlier run, then the open run: its request and one tool round.
    fn run_rows(run_id: &crate::lifecycle::RunId, result_bytes: usize) -> Vec<Message> {
        vec![
            user("an earlier request"),
            answer_row(None, "an earlier answer"),
            user("list every record"),
            call_row(Some(run_id), "tc_1"),
            result_row("tc_1", result_bytes),
        ]
    }

    /// The open run's floor is the start of the input rows before its first
    /// response, notices included: a peer, flow, external or job input
    /// arrives as a system notice, and a run's boundary may add one before
    /// the run's first response. The walk stops at a response, a
    /// tool-results row, a System row or a summary.
    #[test]
    fn the_open_run_floor_is_its_request_before_its_first_response() {
        use crate::types::SystemNoticeKind;
        let run = crate::lifecycle::RunId::new();
        let r = Some(&run);
        let cases: Vec<(&str, Vec<Message>, Option<usize>)> = vec![
            ("a prompt", run_rows(&run, 10), Some(2)),
            (
                "a peer-triggered run",
                vec![
                    user("a"),
                    answer_row(None, "a'"),
                    notice(SystemNoticeKind::Comms, "peer message"),
                    call_row(r, "tc_1"),
                    result_row("tc_1", 10),
                ],
                Some(2),
            ),
            (
                "a run whose first boundary added a notice",
                vec![
                    user("a"),
                    answer_row(None, "a'"),
                    Message::User(UserMessage::injected_context("ctx")),
                    user("prompt"),
                    notice(SystemNoticeKind::ToolScope, "tools changed"),
                    call_row(r, "tc_1"),
                    result_row("tc_1", 10),
                ],
                Some(2),
            ),
            (
                "a fork seed",
                vec![
                    answer_row(None, "the parent's run"),
                    Message::User(UserMessage::injected_context("planned context")),
                    notice(SystemNoticeKind::Generic, "fork boundary"),
                    user("prompt"),
                    call_row(r, "tc_1"),
                ],
                Some(1),
            ),
            (
                "after a summary",
                vec![
                    Message::System(crate::types::SystemMessage::new("sys")),
                    summary_row("earlier"),
                    Message::User(UserMessage::injected_context("ctx")),
                    user("prompt"),
                    call_row(r, "tc_1"),
                ],
                Some(2),
            ),
            (
                "an earlier run's unanswered prompt is kept with the run",
                vec![
                    answer_row(None, "prev"),
                    user("orphan"),
                    user("prompt"),
                    call_row(r, "tc_1"),
                ],
                Some(1),
            ),
            (
                "a run with no response yet",
                vec![user("a"), answer_row(None, "a'"), user("prompt")],
                None,
            ),
        ];
        for (case, rows, floor) in cases {
            assert_eq!(open_run_floor(&rows, &run), floor, "{case}");
        }
    }

    /// The scope a compaction folds: everything without a run in flight or
    /// before its first response; the rows before the run while they hold
    /// a turn; then the run's completed rounds, keeping its request and its
    /// newest round (with the inputs that led to it); and nothing at all
    /// when the run has no completed round.
    #[test]
    fn a_compaction_folds_before_the_run_first_then_its_completed_rounds() {
        use crate::types::SystemNoticeKind;
        let run = crate::lifecycle::RunId::new();
        let r = Some(&run);
        let sys = Message::System(crate::types::SystemMessage::new("sys"));
        assert_eq!(
            compaction_scope(&run_rows(&run, 10), None),
            Some(CompactionScope::Whole),
            "no run in flight"
        );
        assert_eq!(
            compaction_scope(&[user("a"), answer_row(None, "a'"), user("p")], Some(&run)),
            Some(CompactionScope::Whole),
            "a run before its first response"
        );
        assert_eq!(
            compaction_scope(&run_rows(&run, 10), Some(&run)),
            Some(CompactionScope::BeforeOpenRun { floor: 2 }),
            "the rows before the run hold a turn"
        );
        assert_eq!(
            compaction_scope(
                &[
                    sys.clone(),
                    user("p"),
                    call_row(r, "tc_1"),
                    result_row("tc_1", 10)
                ],
                Some(&run)
            ),
            None,
            "only the run's newest round: nothing to fold"
        );
        assert_eq!(
            compaction_scope(
                &[
                    sys.clone(),
                    summary_row("earlier"),
                    user("p"),
                    call_row(r, "tc_1"),
                    result_row("tc_1", 10),
                    notice(SystemNoticeKind::Comms, "a steer"),
                    call_row(r, "tc_2"),
                    result_row("tc_2", 10),
                ],
                Some(&run)
            ),
            Some(CompactionScope::OpenRunRounds {
                first_response: 3,
                newest_round: 5,
            }),
            "the completed round folds; the steer stays with the newest round"
        );
        assert_eq!(
            compaction_scope(
                &[
                    sys.clone(),
                    user("p"),
                    call_row(r, "tc_1"),
                    result_row("tc_1", 10),
                    answer_row(r, "an image the tool appended"),
                ],
                Some(&run)
            ),
            None,
            "a row a tool appended after its result belongs to its round: still the only one"
        );
        assert_eq!(
            compaction_scope(
                &[
                    sys.clone(),
                    user("p"),
                    call_row(r, "tc_1"),
                    result_row("tc_1", 10),
                    call_row(r, "tc_2"),
                    result_row("tc_2", 10),
                    answer_row(r, "an image the tool appended"),
                ],
                Some(&run)
            ),
            Some(CompactionScope::OpenRunRounds {
                first_response: 2,
                newest_round: 4,
            }),
            "the newest round starts at its call, the appended row after it"
        );
        assert_eq!(
            compaction_scope(
                &[
                    sys,
                    user("p"),
                    answer_row(r, "a first thought"),
                    user("a steer"),
                    call_row(r, "tc_1"),
                    result_row("tc_1", 10),
                ],
                Some(&run)
            ),
            None,
            "one row to fold would not shrink the transcript"
        );
    }

    fn call_row_named(run_id: &crate::lifecycle::RunId, call_id: &str, name: &str) -> Message {
        let mut call = BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: call_id.into(),
                name: name.into(),
                args: serde_json::value::RawValue::from_string("{}".into()).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
        );
        call.identity.run_id = Some(run_id.clone());
        Message::BlockAssistant(call)
    }

    fn named(calls: Vec<FoldedCall>) -> FoldedCalls {
        let mut folded = FoldedCalls::default();
        for call in calls {
            folded.push(call);
        }
        folded
    }

    /// A run that folded more calls than the line names one by one: the
    /// newest 200 stay named (a failed one among them still listed apart),
    /// and the older ones are counted by tool name, in the line and in the
    /// stored list.
    #[test]
    fn a_long_runs_folded_calls_are_bounded() {
        let mut folded = FoldedCalls::default();
        for index in 0..600 {
            folded.push(folded_call("read_file", &format!("r{index}"), false));
        }
        for index in 0..212 {
            folded.push(folded_call("search", &format!("s{index}"), false));
        }
        for index in 0..199 {
            folded.push(folded_call("read_file", &format!("n{index}"), false));
        }
        folded.push(folded_call("send_email", "last", true));
        assert_eq!(folded.calls.len(), 200, "the newest 200 are named");
        assert_eq!(folded.calls[199], folded_call("send_email", "last", true));
        assert_eq!(
            folded.earlier_by_tool,
            [("read_file".to_string(), 600), ("search".to_string(), 212)]
                .into_iter()
                .collect()
        );
        let line = folded_calls_line(&folded).expect("the calls are named");
        assert!(
            line.contains("only if it is still needed: send_email (last).")
                && line.contains("it already made 812 earlier calls: read_file x600, search x212."),
            "{line}"
        );
        assert!(!line.contains("(r0)"), "an older call is only counted");
        assert!(
            line.len() < 16 * 1024,
            "the line stays bounded: {}",
            line.len()
        );
    }

    fn folded_call(name: &str, id: &str, failed: bool) -> FoldedCall {
        FoldedCall {
            name: name.into(),
            id: id.into(),
            failed,
        }
    }

    fn folded(name: &str, id: &str, failed: bool) -> FoldedCall {
        FoldedCall {
            name: name.into(),
            id: id.into(),
            failed,
        }
    }

    /// A later fold of the run's rounds keeps naming the calls the earlier
    /// folds took out, from the run's stored list, and adds those it folds
    /// now. A call whose result was an error is named as ran and failed, so
    /// the model may still decide to make it again.
    #[test]
    fn a_later_fold_keeps_naming_the_calls_an_earlier_fold_took_out() {
        let run = crate::lifecycle::RunId::new();
        let r = Some(&run);
        let sys = Message::System(crate::types::SystemMessage::new("sys"));
        let first = vec![
            sys.clone(),
            user("p"),
            call_row(r, "tc_1"),
            result_row("tc_1", 10),
            call_row(r, "tc_2"),
            result_row("tc_2", 10),
        ];
        let after_first = folded_run_calls(&FoldedCalls::default(), &first, 2, 4);
        assert_eq!(
            after_first.calls,
            vec![folded("list_records", "tc_1", false)]
        );
        let second = vec![
            sys,
            summary_row("summary"),
            user("p"),
            call_row(r, "tc_2"),
            failed_result_row("tc_2"),
            call_row(r, "tc_3"),
            result_row("tc_3", 10),
        ];
        let after_second = folded_run_calls(&after_first, &second, 3, 5);
        assert_eq!(
            after_second.calls,
            vec![
                folded("list_records", "tc_1", false),
                folded("list_records", "tc_2", true),
            ]
        );
        let line = folded_calls_line(&after_second).expect("the calls are named");
        assert!(
            line.contains("do not make them again: list_records (tc_1).")
                && line.contains("only if it is still needed: list_records (tc_2)."),
            "{line}"
        );
    }

    /// The list is never read from summary text: a summary a model wrote,
    /// copying an earlier run's fixed sentence, names nothing the next fold
    /// repeats. Only the stored list of the open run counts, and a list
    /// stored for another run is ignored.
    #[test]
    fn a_model_written_copy_of_the_fixed_line_names_nothing() {
        let run = crate::lifecycle::RunId::new();
        let r = Some(&run);
        let copied = format!("summary\n\n{FOLDED_CALLS_LEAD}{FOLDED_CALLS_RAN}send_report (x9).");
        let messages = vec![
            Message::System(crate::types::SystemMessage::new("sys")),
            summary_row(&copied),
            user("p"),
            call_row(r, "tc_1"),
            result_row("tc_1", 10),
            call_row(r, "tc_2"),
            result_row("tc_2", 10),
        ];
        let mut session = Session::new();
        persist_open_run_folded_calls(
            &mut session,
            &crate::lifecycle::RunId::new(),
            &named(vec![folded("send_report", "x9", false)]),
        )
        .unwrap();
        let earlier = open_run_folded_calls(&session, &run);
        assert_eq!(
            earlier,
            FoldedCalls::default(),
            "another run's list is ignored"
        );
        let line = folded_calls_line(&folded_run_calls(&earlier, &messages, 3, 5))
            .expect("the folded call is named");
        assert!(!line.contains("x9"), "{line}");
        assert!(line.contains("list_records (tc_1)"), "{line}");
    }

    /// The run's stored list survives the session's persisted form, as it
    /// must across a restart that resumes the run.
    #[test]
    fn the_folded_call_list_survives_a_restart() {
        let run = crate::lifecycle::RunId::new();
        let mut session = Session::new();
        let calls = named(vec![
            folded("list_records", "tc_1", false),
            folded("send_email", "tc_2", true),
        ]);
        persist_open_run_folded_calls(&mut session, &run, &calls).unwrap();
        let restored =
            Session::from_persisted_bytes(&session.to_persisted_bytes().unwrap()).unwrap();
        assert_eq!(open_run_folded_calls(&restored, &run), calls);
    }

    /// A provider that numbers call ids per response gives every round's
    /// first call the same id. Two folds that take out `list_records (fc_0)`
    /// and then `send_email (fc_0)` name both; and within one fold each
    /// call's outcome is read from its own round, so a failed `fc_0` does
    /// not mark a later, successful `fc_0` as failed.
    #[test]
    fn reused_call_ids_are_kept_apart_by_round() {
        let run = crate::lifecycle::RunId::new();
        let sys = Message::System(crate::types::SystemMessage::new("sys"));
        let first = vec![
            sys.clone(),
            user("p"),
            call_row_named(&run, "fc_0", "list_records"),
            result_row("fc_0", 10),
            call_row_named(&run, "fc_0", "send_email"),
            result_row("fc_0", 10),
        ];
        let after_first = folded_run_calls(&FoldedCalls::default(), &first, 2, 4);
        let second = vec![
            sys.clone(),
            summary_row("summary"),
            user("p"),
            call_row_named(&run, "fc_0", "send_email"),
            result_row("fc_0", 10),
            call_row_named(&run, "fc_0", "read_inbox"),
            result_row("fc_0", 10),
        ];
        assert_eq!(
            folded_run_calls(&after_first, &second, 3, 5).calls,
            vec![
                folded("list_records", "fc_0", false),
                folded("send_email", "fc_0", false),
            ],
            "both fc_0 calls are named"
        );

        let one_fold = vec![
            sys,
            user("p"),
            call_row_named(&run, "fc_0", "list_records"),
            failed_result_row("fc_0"),
            call_row_named(&run, "fc_0", "send_email"),
            result_row("fc_0", 10),
            call_row_named(&run, "fc_0", "read_inbox"),
            result_row("fc_0", 10),
        ];
        let calls = folded_run_calls(&FoldedCalls::default(), &one_fold, 2, 6);
        assert_eq!(
            calls.calls,
            vec![
                folded("list_records", "fc_0", true),
                folded("send_email", "fc_0", false),
            ]
        );
        let line = folded_calls_line(&calls).expect("the calls are named");
        assert!(
            line.contains("do not make them again: send_email (fc_0).")
                && line.contains("only if it is still needed: list_records (fc_0)."),
            "{line}"
        );
    }

    /// Folding the run's completed rounds keeps the System rows, the run's
    /// request and its newest round verbatim, replaces the earlier summary
    /// and the completed round with the one summary, and passes the
    /// rebuild's validation. The summary names the calls it folded and says
    /// they ran.
    #[test]
    fn folding_the_runs_completed_rounds_keeps_its_request_and_newest_round() {
        let run = crate::lifecycle::RunId::new();
        let r = Some(&run);
        let messages = vec![
            Message::System(crate::types::SystemMessage::new("sys")),
            summary_row("earlier"),
            user("p"),
            call_row(r, "tc_1"),
            result_row("tc_1", 10),
            call_row(r, "tc_2"),
            result_row("tc_2", 10),
        ];
        let note = folded_calls_line(&folded_run_calls(&FoldedCalls::default(), &messages, 3, 5))
            .expect("the folded call is named");
        assert!(
            note.contains("do not make them again: list_records (tc_1)."),
            "{note}"
        );
        let summary_text = format!("summary{note}");
        let result = rebuild_open_run_rounds(&messages, 3, 5, &summary_text);
        assert_eq!(
            result.messages,
            vec![
                messages[0].clone(),
                result.summary.message.clone(),
                messages[2].clone(),
                messages[5].clone(),
                messages[6].clone(),
            ]
        );
        let Message::User(summary) = &result.summary.message else {
            panic!("the summary is a user row");
        };
        assert_eq!(
            summary.text_content(),
            format!("{COMPACTION_SUMMARY_PREFIX}{summary_text}")
        );
        validate_compaction_rebuild(
            &messages,
            &result.messages,
            &result.summary,
            &summary_text,
            &result.retained,
            &result.discarded,
        )
        .expect("the rebuild is valid");
    }

    /// The open run's own result alone exceeds the provider cap. For that
    /// request only, the result keeps its identity and the head of its text
    /// with a marker naming the call; every other row is unchanged, and a
    /// request within the cap is left alone.
    #[test]
    fn an_over_cap_open_run_result_is_bounded_for_the_request_only() {
        let run_id = crate::lifecycle::RunId::new();
        let request = run_rows(&run_id, 600_000);

        assert!(
            bound_open_run_tool_results_for_request(&request, &run_id, 90_000, 100_000).is_none(),
            "a request within the cap is sent as composed"
        );
        let bounded = bound_open_run_tool_results_for_request(&request, &run_id, 700_000, 100_000)
            .expect("the over-cap request is bounded");

        assert_eq!(bounded.len(), request.len());
        for offset in 0..4 {
            assert_eq!(
                bounded[offset], request[offset],
                "row {offset} is unchanged"
            );
        }
        let Message::ToolResults {
            results,
            created_at,
        } = &bounded[4]
        else {
            panic!("the run's result row stays a tool-results row");
        };
        let Message::ToolResults {
            created_at: source_created_at,
            ..
        } = &request[4]
        else {
            panic!("the source row is a tool-results row");
        };
        assert_eq!(created_at, source_created_at);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].tool_use_id, "tc_1");
        assert!(!results[0].is_error);
        let Some(ContentBlock::Text { text: head }) = results[0].content.first() else {
            panic!("a bounded result starts with its text head");
        };
        assert!(
            head.len() < 100_000 && head.len() >= 4 * 1024,
            "the head fits the cap: {}",
            head.len()
        );
        let Some(ContentBlock::Text { text: marker }) = results[0].content.last() else {
            panic!("a bounded result ends with its marker");
        };
        assert!(
            marker.contains("list_records") && marker.contains("tc_1"),
            "the marker names the call: {marker}"
        );
        assert!(
            marker.contains("600000"),
            "the marker states the full size: {marker}"
        );

        assert!(
            bound_open_run_tool_results_for_request(
                &request,
                &crate::lifecycle::RunId::new(),
                700_000,
                100_000
            )
            .is_none(),
            "another run's results are never bounded"
        );
    }

    /// An earlier run and the open run both used the call id `tc_1`, as some
    /// providers and adapters reuse ids across turns. Only the open run's
    /// result is bounded.
    #[test]
    fn a_reused_call_id_bounds_only_the_open_runs_result() {
        let earlier = crate::lifecycle::RunId::new();
        let run = crate::lifecycle::RunId::new();
        let request = vec![
            user("an earlier request"),
            call_row(Some(&earlier), "tc_1"),
            result_row("tc_1", 300_000),
            answer_row(Some(&earlier), "an earlier answer"),
            user("list every record"),
            call_row(Some(&run), "tc_1"),
            result_row("tc_1", 300_000),
        ];
        let bounded = bound_open_run_tool_results_for_request(&request, &run, 700_000, 100_000)
            .expect("the over-cap request is bounded");
        assert_eq!(
            bounded[2], request[2],
            "the earlier run's result is untouched"
        );
        assert_ne!(bounded[6], request[6], "the open run's result is bounded");
    }
}

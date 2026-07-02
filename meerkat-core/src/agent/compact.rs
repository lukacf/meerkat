//! Agent compaction — runs context compaction during the agent loop.
//!
//! Called from the agent state machine when a compactor is configured and
//! the threshold is met.

use crate::Session;
use crate::compact::{
    CompactionContext, Compactor, SESSION_COMPACTION_CADENCE_KEY, SessionCompactionCadence,
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

    /// Failed to estimate token count (serialization error).
    #[error("token estimation failed: {0}")]
    EstimationFailed(String),
}

/// Approximate token cost per image block.
///
/// Anthropic charges ~1600 tokens for a standard image regardless of
/// resolution. Using a fixed estimate avoids counting raw base64 bytes
/// (which inflate the estimate by ~200x).
const IMAGE_TOKEN_ESTIMATE: u64 = 1_600;

fn estimate_inline_video_tokens(data: &str) -> u64 {
    let len = data.len() as u64;
    if len > 0 { (len / 4).max(1) } else { 0 }
}

fn estimate_video_duration_tokens(duration_ms: u64) -> u64 {
    // Rough default-resolution Gemini heuristic, scaled by caller-provided duration.
    duration_ms.saturating_mul(300).div_ceil(1000)
}

/// Estimate token count from message history.
///
/// Text content uses `json_bytes / 4` as a rough heuristic.
/// Image blocks use a fixed per-image estimate instead of serializing
/// the base64 payload (which would massively overcount).
/// Estimate tokens for a content-block slice, using fixed image/video
/// heuristics instead of counting raw base64 payload bytes (which would
/// massively overcount). Text blocks use `text_projection().len() / 4`.
fn estimate_content_block_tokens(blocks: &[crate::types::ContentBlock]) -> u64 {
    let mut tokens: u64 = 0;
    for block in blocks {
        match block {
            crate::types::ContentBlock::Image { .. } => {
                tokens += IMAGE_TOKEN_ESTIMATE;
            }
            crate::types::ContentBlock::Video {
                duration_ms,
                data: crate::types::VideoData::Inline { data },
                ..
            } => {
                tokens += estimate_inline_video_tokens(data)
                    .max(estimate_video_duration_tokens(*duration_ms));
            }
            crate::types::ContentBlock::Video { duration_ms, .. } => {
                tokens += estimate_video_duration_tokens(*duration_ms);
            }
            _ => {
                let len = block.text_projection().len() as u64;
                tokens += if len > 0 { (len / 4).max(1) } else { 0 };
            }
        }
    }
    tokens
}

pub fn estimate_tokens(messages: &[Message]) -> Result<u64, CompactionError> {
    let mut tokens: u64 = 0;
    for msg in messages {
        match msg {
            Message::User(u) => {
                tokens += estimate_content_block_tokens(&u.content);
            }
            Message::ToolResults { results, .. } => {
                for r in results {
                    tokens += estimate_content_block_tokens(&r.content);
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
                }
                for block in &notice.blocks {
                    match block {
                        crate::types::SystemNoticeBlock::Comms { content, .. }
                        | crate::types::SystemNoticeBlock::ExternalEvent { content, .. } => {
                            tokens += estimate_content_block_tokens(content);
                        }
                        other => {
                            let json = serde_json::to_string(other)
                                .map_err(|e| CompactionError::EstimationFailed(e.to_string()))?;
                            tokens += json.len() as u64 / 4;
                        }
                    }
                }
            }
            // For assistant/system messages, serialize to JSON (no image blocks).
            other => {
                let json = serde_json::to_string(other)
                    .map_err(|e| CompactionError::EstimationFailed(e.to_string()))?;
                tokens += json.len() as u64 / 4;
            }
        }
    }
    Ok(tokens)
}

/// Build a `CompactionContext` from current agent state.
///
/// Falls back to 0 estimated tokens if serialization fails (non-fatal for
/// context building — the should_compact check will use last_input_tokens).
pub fn build_compaction_context(
    messages: &[Message],
    last_input_tokens: u64,
    last_compaction_boundary_index: Option<u64>,
    session_boundary_index: u64,
) -> CompactionContext {
    let estimated_history_tokens = match estimate_tokens(messages) {
        Ok(tokens) => tokens,
        Err(err) => {
            tracing::warn!("failed to estimate history tokens for compaction context: {err}");
            0
        }
    };

    CompactionContext {
        last_input_tokens,
        message_count: messages.len(),
        estimated_history_tokens,
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
/// 2. Call LLM with compaction prompt
/// 3. On failure: emit CompactionFailed, return error without mutating session
/// 4. Rebuild history via compactor
/// 5. Return a typed outcome; the caller commits it and emits CompactionCompleted
pub async fn run_compaction<C>(
    client: &C,
    compactor: &Arc<dyn Compactor>,
    messages: &[Message],
    last_input_tokens: u64,
    session_boundary_index: u64,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
    event_tap: &crate::event_tap::EventTap,
) -> Result<CompactionOutcome, CompactionError>
where
    C: crate::agent::AgentLlmClient + ?Sized,
{
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

    // 2. Build the compaction prompt messages
    let compaction_prompt = compactor.compaction_prompt();
    let max_summary_tokens = compactor.max_summary_tokens();

    let mut compaction_messages = compactor.prepare_for_summarization(messages);
    compaction_messages.push(Message::User(crate::types::UserMessage::text(
        compaction_prompt.to_string(),
    )));

    // 3. Call LLM with empty tools, max_summary_tokens
    let llm_result = client
        .stream_response(&compaction_messages, &[], max_summary_tokens, None, None)
        .await;

    let (summary_text, summary_usage) = match llm_result {
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
                tracing::warn!("compaction event stream receiver dropped before CompactionFailed");
            }
            return Err(CompactionError::LlmFailed(e));
        }
    };

    // 4. Rebuild history — extract system prompt from messages directly
    let result = compactor.rebuild_history(messages, &summary_text);
    let messages_after = result.messages.len();

    Ok(CompactionOutcome {
        new_messages: result.messages,
        discarded: result.discarded,
        summary_usage,
        session_boundary_index,
        messages_before: message_count,
        messages_after,
    })
}

/// Result of a successful compaction.
pub struct CompactionOutcome {
    /// New session messages to replace current history.
    pub new_messages: Vec<Message>,
    /// Messages that were discarded (for future memory indexing).
    pub discarded: Vec<Message>,
    /// Usage from the summary LLM call.
    pub summary_usage: Usage,
    /// Session boundary index at which compaction occurred.
    pub session_boundary_index: u64,
    /// Number of messages before compaction.
    pub messages_before: usize,
    /// Number of messages after compaction.
    pub messages_after: usize,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::{
        AssistantBlock, BlockAssistantMessage, ContentBlock, StopReason, ToolResult, Usage,
        UserMessage, VideoData,
    };

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
}

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
pub fn estimate_tokens(messages: &[Message]) -> Result<u64, CompactionError> {
    let mut tokens: u64 = 0;
    for msg in messages {
        match msg {
            Message::User(u) => {
                for block in &u.content {
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
                        _ => {
                            let len = block.text_projection().len() as u64;
                            tokens += if len > 0 { (len / 4).max(1) } else { 0 };
                        }
                    }
                }
            }
            Message::ToolResults { results, .. } => {
                for r in results {
                    for block in &r.content {
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
                            _ => {
                                let len = block.text_projection().len() as u64;
                                tokens += if len > 0 { (len / 4).max(1) } else { 0 };
                            }
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

/// Load persisted compaction cadence from session metadata, falling back to
/// an explicit zero cadence when metadata is absent.
pub fn load_compaction_cadence(session: &Session) -> SessionCompactionCadence {
    session
        .metadata()
        .get(SESSION_COMPACTION_CADENCE_KEY)
        .and_then(|value| serde_json::from_value::<SessionCompactionCadence>(value.clone()).ok())
        .unwrap_or(SessionCompactionCadence {
            session_boundary_index: 0,
            last_compaction_boundary_index: None,
        })
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
                            error: "LLM returned empty summary".to_string(),
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
                        error: e.to_string(),
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
    let discarded = annotate_compaction_discards(messages, result.discarded);
    let messages_after = result.messages.len();

    Ok(CompactionOutcome {
        new_messages: result.messages,
        discarded,
        summary_usage,
        session_boundary_index,
        messages_before: message_count,
        messages_after,
    })
}

/// A message discarded by compaction together with the session turn that
/// originally produced it. The source turn is owned by the transcript order;
/// compaction timing is intentionally not used as a fallback.
#[derive(Debug, Clone)]
pub struct CompactionDiscard {
    pub message: Message,
    pub source_turn: Option<u32>,
}

fn transcript_source_turns(messages: &[Message]) -> Vec<Option<u32>> {
    let mut current_turn = 0u32;
    messages
        .iter()
        .map(|message| match message {
            Message::User(_) => Some(current_turn),
            Message::Assistant(_) | Message::BlockAssistant(_) => {
                let source_turn = Some(current_turn);
                current_turn = current_turn.saturating_add(1);
                source_turn
            }
            Message::System(_) | Message::SystemNotice(_) | Message::ToolResults { .. } => None,
        })
        .collect()
}

fn annotate_compaction_discards(
    messages: &[Message],
    discarded: Vec<Message>,
) -> Vec<CompactionDiscard> {
    let source_turns = transcript_source_turns(messages);
    let message_keys = messages
        .iter()
        .map(serde_json::to_vec)
        .map(Result::ok)
        .collect::<Vec<_>>();
    let mut consumed = vec![false; messages.len()];
    discarded
        .into_iter()
        .map(|message| {
            let source_turn = serde_json::to_vec(&message)
                .ok()
                .and_then(|discard_key| {
                    message_keys.iter().enumerate().find_map(|(index, key)| {
                        (!consumed[index] && key.as_ref() == Some(&discard_key)).then_some(index)
                    })
                })
                .map(|index| {
                    consumed[index] = true;
                    source_turns[index]
                })
                .unwrap_or(None);
            CompactionDiscard {
                message,
                source_turn,
            }
        })
        .collect()
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
    fn load_compaction_cadence_does_not_infer_boundary_count_from_existing_history() {
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

        let cadence = load_compaction_cadence(&session);
        assert_eq!(cadence.session_boundary_index, 0);
        assert_eq!(cadence.last_compaction_boundary_index, None);
    }

    #[test]
    fn compaction_discard_annotation_preserves_source_turn() {
        let first = Message::User(UserMessage::text("first"));
        let first_reply = Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "first reply".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        ));
        let second = Message::User(UserMessage::text("second"));
        let transcript = vec![first.clone(), first_reply, second.clone()];

        let discarded = annotate_compaction_discards(&transcript, vec![first, second]);

        assert_eq!(discarded[0].source_turn, Some(0));
        assert_eq!(discarded[1].source_turn, Some(1));
    }

    #[test]
    fn persisted_compaction_cadence_round_trips_through_session_metadata() {
        let mut session = Session::new();
        let cadence = SessionCompactionCadence {
            session_boundary_index: 7,
            last_compaction_boundary_index: Some(4),
        };
        persist_compaction_cadence(&mut session, &cadence).unwrap();

        assert_eq!(load_compaction_cadence(&session), cadence);
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
}

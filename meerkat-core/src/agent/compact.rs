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
                        _ => {
                            tokens += block.text_projection().len() as u64 / 4;
                        }
                    }
                }
            }
            Message::ToolResults { results } => {
                for r in results {
                    for block in &r.content {
                        match block {
                            crate::types::ContentBlock::Image { .. } => {
                                tokens += IMAGE_TOKEN_ESTIMATE;
                            }
                            _ => {
                                tokens += block.text_projection().len() as u64 / 4;
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

/// Best-effort count of prior LLM boundaries for older sessions that do not
/// yet carry explicit cadence metadata.
fn infer_session_boundary_index(messages: &[Message]) -> u64 {
    messages
        .iter()
        .filter(|message| matches!(message, Message::BlockAssistant(_)))
        .count() as u64
}

/// Load persisted compaction cadence from session metadata, falling back to
/// transcript-derived history for pre-migration sessions.
pub fn load_compaction_cadence(session: &Session) -> SessionCompactionCadence {
    session
        .metadata()
        .get(SESSION_COMPACTION_CADENCE_KEY)
        .and_then(|value| serde_json::from_value::<SessionCompactionCadence>(value.clone()).ok())
        .unwrap_or_else(|| SessionCompactionCadence {
            session_boundary_index: infer_session_boundary_index(session.messages()),
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
/// 5. Emit CompactionCompleted
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
    let messages_after = result.messages.len();

    // 5. Emit CompactionCompleted
    if event_stream_open
        && !crate::event_tap::tap_emit(
            event_tap,
            event_tx.as_ref(),
            AgentEvent::CompactionCompleted {
                summary_tokens: summary_usage.output_tokens,
                messages_before: message_count,
                messages_after,
            },
        )
        .await
    {
        tracing::warn!("compaction event stream receiver dropped before CompactionCompleted");
    }

    Ok(CompactionOutcome {
        new_messages: result.messages,
        discarded: result.discarded,
        summary_usage,
        session_boundary_index,
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::{AssistantBlock, BlockAssistantMessage, StopReason, Usage, UserMessage};

    #[test]
    fn load_compaction_cadence_infers_boundary_count_from_existing_history() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("first")));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "ok".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
        }));
        session.push(Message::User(UserMessage::text("second")));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "done".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
        }));
        session.record_usage(Usage::default());

        let cadence = load_compaction_cadence(&session);
        assert_eq!(cadence.session_boundary_index, 2);
        assert_eq!(cadence.last_compaction_boundary_index, None);
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
}

//! Agent compaction — runs context compaction during the agent loop.
//!
//! Called from the agent state machine when a compactor is configured and
//! the threshold is met.

use crate::compact::{CompactionContext, Compactor};
use crate::event::AgentEvent;
use crate::event_tap::{EventTap, tap_emit};
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

/// Estimate token count from message history (JSON bytes / 4).
///
/// Returns an error if the messages cannot be serialized, rather than
/// silently returning 0.
pub fn estimate_tokens(messages: &[Message]) -> Result<u64, CompactionError> {
    let json = serde_json::to_string(messages)
        .map_err(|e| CompactionError::EstimationFailed(e.to_string()))?;
    Ok(json.len() as u64 / 4)
}

/// Build a `CompactionContext` from current agent state.
///
/// Falls back to 0 estimated tokens if serialization fails (non-fatal for
/// context building — the should_compact check will use last_input_tokens).
pub fn build_compaction_context(
    messages: &[Message],
    last_input_tokens: u64,
    last_compaction_turn: Option<u32>,
    current_turn: u32,
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
        last_compaction_turn,
        current_turn,
    }
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
    current_turn: u32,
    event_tap: &EventTap,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
) -> Result<CompactionOutcome, CompactionError>
where
    C: crate::agent::AgentLlmClient + ?Sized,
{
    let estimated = estimate_tokens(messages)?;
    let message_count = messages.len();
    let mut event_stream_open = true;

    // 1. Emit CompactionStarted
    if event_stream_open {
        if !tap_emit(
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
    }

    // 2. Build the compaction prompt messages
    let compaction_prompt = compactor.compaction_prompt();
    let max_summary_tokens = compactor.max_summary_tokens();

    let mut compaction_messages = messages.to_vec();
    compaction_messages.push(Message::User(crate::types::UserMessage {
        content: compaction_prompt.to_string(),
    }));

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
                if event_stream_open {
                    if !tap_emit(
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
                }
                return Err(CompactionError::EmptySummary);
            }
            (summary, result.usage().clone())
        }
        Err(e) => {
            if event_stream_open {
                if !tap_emit(
                    event_tap,
                    event_tx.as_ref(),
                    AgentEvent::CompactionFailed {
                        error: e.to_string(),
                    },
                )
                .await
                {
                    tracing::warn!(
                        "compaction event stream receiver dropped before CompactionFailed"
                    );
                }
            }
            return Err(CompactionError::LlmFailed(e));
        }
    };

    // 4. Rebuild history — extract system prompt from messages directly
    let result = compactor.rebuild_history(messages, &summary_text);
    let messages_after = result.messages.len();

    // 5. Emit CompactionCompleted
    if event_stream_open {
        if !tap_emit(
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
    }

    Ok(CompactionOutcome {
        new_messages: result.messages,
        discarded: result.discarded,
        summary_usage,
        current_turn,
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
    /// Turn at which compaction occurred.
    pub current_turn: u32,
}

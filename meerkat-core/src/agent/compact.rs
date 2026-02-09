//! Agent compaction — runs context compaction during the agent loop.
//!
//! Called from the agent state machine when a compactor is configured and
//! the threshold is met.

use crate::compact::{CompactionContext, Compactor};
use crate::event::AgentEvent;
use crate::types::{AssistantBlock, Message, Usage};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Estimate token count from message history (JSON bytes / 4).
pub fn estimate_tokens(messages: &[Message]) -> u64 {
    let json_bytes = serde_json::to_string(messages)
        .map(|s| s.len())
        .unwrap_or(0);
    json_bytes as u64 / 4
}

/// Build a `CompactionContext` from current agent state.
pub fn build_compaction_context(
    messages: &[Message],
    last_input_tokens: u64,
    last_compaction_turn: Option<u32>,
    current_turn: u32,
) -> CompactionContext {
    CompactionContext {
        last_input_tokens,
        message_count: messages.len(),
        estimated_history_tokens: estimate_tokens(messages),
        last_compaction_turn,
        current_turn,
    }
}

/// Run the compaction flow.
///
/// 1. Emit CompactionStarted
/// 2. Call LLM with compaction prompt
/// 3. On failure: emit CompactionFailed, return without mutating session
/// 4. Rebuild history via compactor
/// 5. Replace session messages
/// 6. Emit CompactionCompleted
pub async fn run_compaction<C>(
    client: &C,
    compactor: &Arc<dyn Compactor>,
    messages: &[Message],
    last_input_tokens: u64,
    current_turn: u32,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
) -> Result<CompactionOutcome, ()>
where
    C: crate::agent::AgentLlmClient + ?Sized,
{
    let estimated = estimate_tokens(messages);
    let message_count = messages.len();

    // 1. Emit CompactionStarted
    if let Some(tx) = event_tx {
        let _ = tx
            .send(AgentEvent::CompactionStarted {
                input_tokens: last_input_tokens,
                estimated_history_tokens: estimated,
                message_count,
            })
            .await;
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

    let summary = match llm_result {
        Ok(result) => {
            // Extract summary text from response blocks
            let mut summary = String::new();
            for block in result.blocks() {
                if let AssistantBlock::Text { text, .. } = block {
                    summary.push_str(text);
                }
            }
            if summary.is_empty() {
                // Empty summary — treat as failure
                if let Some(tx) = event_tx {
                    let _ = tx
                        .send(AgentEvent::CompactionFailed {
                            error: "LLM returned empty summary".to_string(),
                        })
                        .await;
                }
                return Err(());
            }
            (summary, result.usage().clone())
        }
        Err(e) => {
            // 5. On failure: emit CompactionFailed, return without mutating
            if let Some(tx) = event_tx {
                let _ = tx
                    .send(AgentEvent::CompactionFailed {
                        error: e.to_string(),
                    })
                    .await;
            }
            return Err(());
        }
    };

    let (summary_text, summary_usage) = summary;

    // 6. Detect system prompt
    let system_prompt = messages.iter().find_map(|m| match m {
        Message::System(s) => Some(s.content.as_str()),
        _ => None,
    });

    // 7. Rebuild history
    let result = compactor.rebuild_history(system_prompt, messages, &summary_text);
    let messages_after = result.messages.len();

    // 8. Emit CompactionCompleted
    if let Some(tx) = event_tx {
        let _ = tx
            .send(AgentEvent::CompactionCompleted {
                summary_tokens: summary_usage.output_tokens,
                messages_before: message_count,
                messages_after,
            })
            .await;
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

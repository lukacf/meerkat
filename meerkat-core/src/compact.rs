//! Compactor trait — provider-agnostic context compaction.
//!
//! The `Compactor` trait defines how and when to compact (summarize) the
//! conversation history to reclaim context window space. Implementations
//! live in `meerkat-session` (behind the `session-compaction` feature).

use crate::types::Message;

/// Context provided to `Compactor::should_compact` for trigger decisions.
#[derive(Debug, Clone)]
pub struct CompactionContext {
    /// Input token count from the last LLM response.
    pub last_input_tokens: u64,
    /// Total number of messages in the session.
    pub message_count: usize,
    /// Estimated history tokens (JSON bytes / 4).
    pub estimated_history_tokens: u64,
    /// Turn number when compaction last occurred, if ever.
    pub last_compaction_turn: Option<u32>,
    /// The current turn number (0-indexed).
    pub current_turn: u32,
}

/// Result of a compaction rebuild.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The rebuilt message history (summary + retained recent turns).
    pub messages: Vec<Message>,
    /// Messages that were removed from history (for future memory indexing).
    pub discarded: Vec<Message>,
}

/// Configuration for the default compactor implementation.
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Compaction triggers when `last_input_tokens >= auto_compact_threshold`.
    pub auto_compact_threshold: u64,
    /// Number of recent complete turns to retain after compaction.
    pub recent_turn_budget: usize,
    /// Maximum tokens for the compaction summary LLM response.
    pub max_summary_tokens: u32,
    /// Minimum turns between consecutive compactions (loop guard).
    pub min_turns_between_compactions: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact_threshold: 100_000,
            recent_turn_budget: 4,
            max_summary_tokens: 4096,
            min_turns_between_compactions: 3,
        }
    }
}

/// Provider-agnostic compaction strategy.
///
/// Determines when to compact and how to rebuild the history after summarization.
pub trait Compactor: Send + Sync {
    /// Check whether compaction should run given the current context.
    fn should_compact(&self, ctx: &CompactionContext) -> bool;

    /// Return the prompt to send to the LLM for summarization.
    fn compaction_prompt(&self) -> &str;

    /// Maximum tokens the summarization response may consume.
    fn max_summary_tokens(&self) -> u32;

    /// Rebuild the session history from a summary and current messages.
    ///
    /// The system prompt is extracted from `messages` directly (the first
    /// `Message::System` if present). No dual source of truth.
    ///
    /// The implementation should:
    /// 1. Preserve any `Message::System` verbatim.
    /// 2. Inject a summary message.
    /// 3. Retain recent complete turns per `recent_turn_budget`.
    /// 4. Return everything else as `discarded`.
    fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult;
}

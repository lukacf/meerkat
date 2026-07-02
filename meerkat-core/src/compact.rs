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
    /// Session-scoped pre-LLM boundary index used by the cadence guard.
    ///
    /// This is the latest successful compaction boundary or failed compaction
    /// attempt boundary, whichever is newer.
    pub last_compaction_boundary_index: Option<u64>,
    /// Current session-scoped pre-LLM boundary index.
    pub session_boundary_index: u64,
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
    /// Minimum session-scoped LLM boundaries between consecutive compactions.
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

    /// Prepare messages for the summarization LLM call.
    ///
    /// Called before sending the history to the LLM for summarization.
    /// Implementations may strip content that is not suitable for the
    /// summarization pass (e.g. base64-encoded images).
    ///
    /// The default implementation returns an unmodified clone.
    fn prepare_for_summarization(&self, messages: &[Message]) -> Vec<Message> {
        messages.to_vec()
    }

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

/// Borrowed view of the compaction inputs handed to a [`CompactionCurator`].
///
/// This is exactly the data the agent-loop compaction flow already holds when
/// it would otherwise run the summarization LLM call: the full current
/// transcript (including the system prompt and any prior typed
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
}

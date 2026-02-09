//! DefaultCompactor — provider-agnostic context compaction implementation.
//!
//! Gated behind the `session-compaction` feature.

use meerkat_core::compact::{CompactionConfig, CompactionContext, CompactionResult, Compactor};
use meerkat_core::types::Message;

/// Summarization prompt sent to the LLM with the current history.
const COMPACTION_PROMPT: &str = "\
You are performing a CONTEXT COMPACTION. Your job is to create a handoff summary so work can continue seamlessly.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences discovered
- What remains to be done (clear next steps)
- Any critical data, file paths, examples, or references needed to continue
- Tool call patterns that worked or failed

Be concise and structured. Prioritize information the next context needs to act, not narrate.";

/// Prefix injected before the summary in the rebuilt history.
const SUMMARY_PREFIX: &str = "\
[Context compacted] A previous context produced the following summary of work so far. \
The current tool and session state is preserved. Use this summary to continue without \
duplicating work:\n\n";

/// Default compaction strategy implementation.
pub struct DefaultCompactor {
    config: CompactionConfig,
}

impl DefaultCompactor {
    /// Create a new compactor with the given configuration.
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }
}

impl Compactor for DefaultCompactor {
    fn should_compact(&self, ctx: &CompactionContext) -> bool {
        // Never compact on the first turn
        if ctx.current_turn == 0 {
            return false;
        }

        // Loop guard: enforce minimum turns between compactions.
        // Use saturating_sub to prevent underflow when last_compaction_turn
        // comes from an earlier run and current_turn has reset.
        if let Some(last) = ctx.last_compaction_turn {
            if ctx.current_turn.saturating_sub(last) < self.config.min_turns_between_compactions {
                return false;
            }
        }

        // Trigger on either threshold
        ctx.last_input_tokens >= self.config.auto_compact_threshold
            || ctx.estimated_history_tokens >= self.config.auto_compact_threshold
    }

    fn compaction_prompt(&self) -> &str {
        COMPACTION_PROMPT
    }

    fn max_summary_tokens(&self) -> u32 {
        self.config.max_summary_tokens
    }

    fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
        let mut rebuilt = Vec::new();
        let mut discarded = Vec::new();

        // 1. Preserve system prompt (extracted from messages, single source of truth)
        if let Some(Message::System(sys)) = messages.first() {
            rebuilt.push(Message::System(sys.clone()));
        }

        // 2. Inject summary as a user message
        let summary_content = format!("{}{}", SUMMARY_PREFIX, summary);
        rebuilt.push(Message::User(meerkat_core::types::UserMessage {
            content: summary_content,
        }));

        // 3. Identify recent complete turns to retain
        // A "turn" is User → BlockAssistant → ToolResults sequence.
        // We work backward from the end to find `recent_turn_budget` turns.
        let non_system_start = messages
            .iter()
            .position(|m| !matches!(m, Message::System(_)))
            .unwrap_or(0);
        let history = &messages[non_system_start..];

        // Find turn boundaries (each User message starts a turn)
        let mut turn_starts: Vec<usize> = Vec::new();
        for (i, msg) in history.iter().enumerate() {
            if matches!(msg, Message::User(_)) {
                turn_starts.push(i);
            }
        }

        let retain_from = if self.config.recent_turn_budget == 0 {
            // Retain nothing — discard all history
            history.len()
        } else if turn_starts.len() > self.config.recent_turn_budget {
            let idx = turn_starts.len() - self.config.recent_turn_budget;
            turn_starts[idx]
        } else {
            0
        };

        // Everything before retain_from goes to discarded
        for msg in &history[..retain_from] {
            discarded.push(msg.clone());
        }

        // Everything from retain_from goes to rebuilt
        for msg in &history[retain_from..] {
            rebuilt.push(msg.clone());
        }

        CompactionResult {
            messages: rebuilt,
            discarded,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::{SystemMessage, UserMessage};

    fn make_config() -> CompactionConfig {
        CompactionConfig {
            auto_compact_threshold: 100_000,
            recent_turn_budget: 2,
            max_summary_tokens: 4096,
            min_turns_between_compactions: 3,
        }
    }

    #[test]
    fn test_should_compact_first_turn_never() {
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 200_000,
            message_count: 100,
            estimated_history_tokens: 200_000,
            last_compaction_turn: None,
            current_turn: 0,
        };
        assert!(!c.should_compact(&ctx));
    }

    #[test]
    fn test_should_compact_loop_guard() {
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 200_000,
            message_count: 100,
            estimated_history_tokens: 200_000,
            last_compaction_turn: Some(5),
            current_turn: 7, // Only 2 turns since last compaction, threshold is 3
        };
        assert!(!c.should_compact(&ctx));
    }

    #[test]
    fn test_should_compact_dual_threshold() {
        let c = DefaultCompactor::new(make_config());

        // Trigger via input tokens
        let ctx = CompactionContext {
            last_input_tokens: 100_000,
            message_count: 50,
            estimated_history_tokens: 50_000,
            last_compaction_turn: None,
            current_turn: 5,
        };
        assert!(c.should_compact(&ctx));

        // Trigger via history tokens
        let ctx2 = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 50,
            estimated_history_tokens: 100_000,
            last_compaction_turn: None,
            current_turn: 5,
        };
        assert!(c.should_compact(&ctx2));
    }

    #[test]
    fn test_rebuild_preserves_system_prompt() {
        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::System(SystemMessage {
                content: "system".to_string(),
            }),
            Message::User(UserMessage {
                content: "turn1".to_string(),
            }),
            Message::User(UserMessage {
                content: "turn2".to_string(),
            }),
            Message::User(UserMessage {
                content: "turn3".to_string(),
            }),
        ];
        let result = c.rebuild_history(&messages, "summary text");
        assert!(matches!(&result.messages[0], Message::System(s) if s.content == "system"));
    }

    #[test]
    fn test_rebuild_keeps_recent_turns_not_just_user() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage {
                content: "turn1".to_string(),
            }),
            Message::User(UserMessage {
                content: "turn2".to_string(),
            }),
            Message::User(UserMessage {
                content: "turn3".to_string(),
            }),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // Summary + last 1 turn (turn3)
        assert_eq!(result.messages.len(), 2); // summary + turn3
        assert_eq!(result.discarded.len(), 2); // turn1, turn2
    }

    #[test]
    fn test_rebuild_respects_turn_budget() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 2,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage {
                content: "t1".to_string(),
            }),
            Message::User(UserMessage {
                content: "t2".to_string(),
            }),
            Message::User(UserMessage {
                content: "t3".to_string(),
            }),
            Message::User(UserMessage {
                content: "t4".to_string(),
            }),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // summary + last 2 turns (t3, t4)
        assert_eq!(result.messages.len(), 3);
        assert_eq!(result.discarded.len(), 2); // t1, t2
    }

    #[test]
    fn test_rebuild_budget_larger_than_history_keeps_all_turns() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 10,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage {
                content: "t1".to_string(),
            }),
            Message::User(UserMessage {
                content: "t2".to_string(),
            }),
            Message::User(UserMessage {
                content: "t3".to_string(),
            }),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // Summary + all original turns (budget exceeds available turns)
        assert_eq!(result.messages.len(), 4);
        assert_eq!(result.discarded.len(), 0);
    }

    #[test]
    fn test_rebuild_discarded_messages_in_order() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage {
                content: "a".to_string(),
            }),
            Message::User(UserMessage {
                content: "b".to_string(),
            }),
            Message::User(UserMessage {
                content: "c".to_string(),
            }),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // Discarded should be in original order: a, b
        assert_eq!(result.discarded.len(), 2);
        if let Message::User(u) = &result.discarded[0] {
            assert_eq!(u.content, "a");
        }
        if let Message::User(u) = &result.discarded[1] {
            assert_eq!(u.content, "b");
        }
    }

    #[test]
    fn test_rebuild_zero_budget_discards_all() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 0,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage {
                content: "a".to_string(),
            }),
            Message::User(UserMessage {
                content: "b".to_string(),
            }),
            Message::User(UserMessage {
                content: "c".to_string(),
            }),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // Only the summary message should remain
        assert_eq!(result.messages.len(), 1);
        // All original messages should be discarded
        assert_eq!(result.discarded.len(), 3);
    }

    #[test]
    fn test_rebuild_with_block_assistant_and_tool_results() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, ToolResult};
        use serde_json::value::RawValue;

        let args_raw = RawValue::from_string(r#"{"city":"Tokyo"}"#.to_string()).unwrap();

        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });

        // Simulate a realistic conversation:
        // Turn 1: User → BlockAssistant(tool call) → ToolResults → BlockAssistant(text)
        // Turn 2: User → BlockAssistant(text)
        let messages = vec![
            Message::System(SystemMessage {
                content: "You are helpful.".to_string(),
            }),
            // Turn 1
            Message::User(UserMessage {
                content: "What is the weather?".to_string(),
            }),
            Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::ToolUse {
                    id: "tc_1".to_string(),
                    name: "get_weather".to_string(),
                    args: args_raw,
                    meta: None,
                }],
                stop_reason: StopReason::ToolUse,
            }),
            Message::ToolResults {
                results: vec![ToolResult {
                    tool_use_id: "tc_1".to_string(),
                    content: "Sunny, 25C".to_string(),
                    is_error: false,
                }],
            },
            Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "It's sunny in Tokyo!".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
            }),
            // Turn 2
            Message::User(UserMessage {
                content: "Thanks!".to_string(),
            }),
            Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "You're welcome!".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
            }),
        ];

        let result = c.rebuild_history(&messages, "Summary of weather conversation");

        // System prompt + summary + last turn (User "Thanks!" + BlockAssistant "You're welcome!")
        assert_eq!(result.messages.len(), 4); // system + summary + user + assistant
        assert!(matches!(&result.messages[0], Message::System(_)));

        // Discarded: turn 1 (User + BlockAssistant + ToolResults + BlockAssistant = 4 messages)
        assert_eq!(result.discarded.len(), 4);
    }
}

//! DefaultCompactor — provider-agnostic context compaction implementation.
//!
//! Gated behind the `session-compaction` feature.

use meerkat_core::compact::{CompactionConfig, CompactionContext, CompactionResult, Compactor};
use meerkat_core::types::{ContentBlock, Message};

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

/// Replace image blocks with text placeholders for compaction.
/// Includes source_path when available so agents can re-read via view_image.
fn strip_images_for_compaction(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Image { media_type, .. } => {
                // NOTE: source_path is intentionally NOT included in the placeholder.
                // It is internal metadata and must not leak through transcript history
                // into wire/API surfaces. The agent can re-read images via view_image
                // if the source_path was set on the original ContentBlock.
                ContentBlock::Text {
                    text: format!("[image: {media_type}]"),
                }
            }
            other => other.clone(),
        })
        .collect()
}

/// Strip images from all messages in a history, replacing them with text placeholders.
///
/// Applies `strip_images_for_compaction` to `UserMessage.content` and
/// `ToolResult.content` blocks. Other message types pass through unchanged.
fn strip_images_from_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|msg| match msg {
            Message::User(user) => {
                let content = strip_images_for_compaction(&user.content);
                Message::User(meerkat_core::types::UserMessage::with_blocks(content))
            }
            Message::ToolResults { results } => {
                let results = results
                    .iter()
                    .map(|r| {
                        let content = strip_images_for_compaction(&r.content);
                        meerkat_core::types::ToolResult::with_blocks(
                            r.tool_use_id.clone(),
                            content,
                            r.is_error,
                        )
                    })
                    .collect();
                Message::ToolResults { results }
            }
            other => other.clone(),
        })
        .collect()
}

impl Compactor for DefaultCompactor {
    fn should_compact(&self, ctx: &CompactionContext) -> bool {
        // Never compact on the first-ever session LLM boundary.
        if ctx.session_boundary_index == 0 {
            return false;
        }

        // Loop guard: enforce minimum session-scoped boundaries between
        // compactions. Session boundary indices do not reset across runs.
        if let Some(last) = ctx.last_compaction_boundary_index
            && ctx.session_boundary_index.saturating_sub(last)
                < u64::from(self.config.min_turns_between_compactions)
        {
            return false;
        }

        // Trigger on either threshold
        ctx.last_input_tokens >= self.config.auto_compact_threshold
            || ctx.estimated_history_tokens >= self.config.auto_compact_threshold
    }

    fn prepare_for_summarization(&self, messages: &[Message]) -> Vec<Message> {
        strip_images_from_messages(messages)
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
        let summary_content = format!("{SUMMARY_PREFIX}{summary}");
        rebuilt.push(Message::User(meerkat_core::types::UserMessage::text(
            summary_content,
        )));

        // 3. Identify recent complete turns to retain
        // A "turn" is User -> BlockAssistant -> ToolResults sequence.
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
            // Retain nothing -- discard all history
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
            last_compaction_boundary_index: None,
            session_boundary_index: 0,
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
            last_compaction_boundary_index: Some(5),
            session_boundary_index: 7, // Only 2 boundaries since last compaction, threshold is 3
        };
        assert!(!c.should_compact(&ctx));
    }

    #[test]
    fn test_should_compact_follow_up_run_boundary_zero_no_longer_special() {
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 200_000,
            message_count: 100,
            estimated_history_tokens: 200_000,
            last_compaction_boundary_index: None,
            session_boundary_index: 1,
        };
        assert!(c.should_compact(&ctx));
    }

    #[test]
    fn test_should_compact_dual_threshold() {
        let c = DefaultCompactor::new(make_config());

        // Trigger via input tokens
        let ctx = CompactionContext {
            last_input_tokens: 100_000,
            message_count: 50,
            estimated_history_tokens: 50_000,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
        };
        assert!(c.should_compact(&ctx));

        // Trigger via history tokens
        let ctx2 = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 50,
            estimated_history_tokens: 100_000,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
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
            Message::User(UserMessage::text("turn1")),
            Message::User(UserMessage::text("turn2")),
            Message::User(UserMessage::text("turn3")),
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
            Message::User(UserMessage::text("turn1")),
            Message::User(UserMessage::text("turn2")),
            Message::User(UserMessage::text("turn3")),
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
            Message::User(UserMessage::text("t1")),
            Message::User(UserMessage::text("t2")),
            Message::User(UserMessage::text("t3")),
            Message::User(UserMessage::text("t4")),
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
            Message::User(UserMessage::text("t1")),
            Message::User(UserMessage::text("t2")),
            Message::User(UserMessage::text("t3")),
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
            Message::User(UserMessage::text("a")),
            Message::User(UserMessage::text("b")),
            Message::User(UserMessage::text("c")),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // Discarded should be in original order: a, b
        assert_eq!(result.discarded.len(), 2);
        if let Message::User(u) = &result.discarded[0] {
            assert_eq!(u.text_content(), "a");
        }
        if let Message::User(u) = &result.discarded[1] {
            assert_eq!(u.text_content(), "b");
        }
    }

    #[test]
    fn test_rebuild_zero_budget_discards_all() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 0,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage::text("a")),
            Message::User(UserMessage::text("b")),
            Message::User(UserMessage::text("c")),
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
        // Turn 1: User -> BlockAssistant(tool call) -> ToolResults -> BlockAssistant(text)
        // Turn 2: User -> BlockAssistant(text)
        let messages = vec![
            Message::System(SystemMessage {
                content: "You are helpful.".to_string(),
            }),
            // Turn 1
            Message::User(UserMessage::text("What is the weather?")),
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
                results: vec![ToolResult::new(
                    "tc_1".to_string(),
                    "Sunny, 25C".to_string(),
                    false,
                )],
            },
            Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "It's sunny in Tokyo!".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
            }),
            // Turn 2
            Message::User(UserMessage::text("Thanks!")),
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

    #[test]
    fn compaction_strips_images_preserves_text() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "base64data".to_string(),
                source_path: None,
            },
            ContentBlock::Text {
                text: "world".to_string(),
            },
        ];
        let result = strip_images_for_compaction(&blocks);
        assert_eq!(result.len(), 3);
        assert!(matches!(&result[0], ContentBlock::Text { text } if text == "hello"));
        assert!(matches!(&result[1], ContentBlock::Text { text } if text == "[image: image/png]"));
        assert!(matches!(&result[2], ContentBlock::Text { text } if text == "world"));
    }

    #[test]
    fn compaction_image_placeholder_excludes_source_path() {
        // source_path must NOT appear in the placeholder — it's internal metadata
        // that would leak filesystem paths through transcript history APIs.
        let blocks = vec![ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "base64data".to_string(),
            source_path: Some("/tmp/x.png".to_string()),
        }];
        let result = strip_images_for_compaction(&blocks);
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ContentBlock::Text { text } if text == "[image: image/png]"));
        // Verify source_path is NOT in the output
        if let ContentBlock::Text { text } = &result[0] {
            assert!(
                !text.contains("/tmp/x.png"),
                "source_path must not leak into placeholder"
            );
        }
    }

    #[test]
    fn compaction_text_only_unchanged() {
        let blocks = vec![
            ContentBlock::Text {
                text: "one".to_string(),
            },
            ContentBlock::Text {
                text: "two".to_string(),
            },
        ];
        let result = strip_images_for_compaction(&blocks);
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0], ContentBlock::Text { text } if text == "one"));
        assert!(matches!(&result[1], ContentBlock::Text { text } if text == "two"));
    }

    #[test]
    fn prepare_for_summarization_strips_user_and_tool_images() {
        use meerkat_core::types::ToolResult;

        let c = DefaultCompactor::new(make_config());

        let messages = vec![
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "Look at this".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/jpeg".to_string(),
                    data: "bigdata".to_string(),
                    source_path: Some("/tmp/photo.jpg".to_string()),
                },
            ])),
            Message::ToolResults {
                results: vec![ToolResult::with_blocks(
                    "tc_1".to_string(),
                    vec![
                        ContentBlock::Text {
                            text: "screenshot captured".to_string(),
                        },
                        ContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: "screenshotdata".to_string(),
                            source_path: None,
                        },
                    ],
                    false,
                )],
            },
        ];

        let prepared = c.prepare_for_summarization(&messages);
        assert_eq!(prepared.len(), 2);

        // User message: text preserved, image replaced
        if let Message::User(u) = &prepared[0] {
            assert_eq!(u.content.len(), 2);
            assert!(matches!(&u.content[0], ContentBlock::Text { text } if text == "Look at this"));
            assert!(
                matches!(&u.content[1], ContentBlock::Text { text } if text == "[image: image/jpeg]")
            );
        } else {
            panic!("expected User message");
        }

        // Tool result: text preserved, image replaced
        if let Message::ToolResults { results } = &prepared[1] {
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].content.len(), 2);
            assert!(
                matches!(&results[0].content[0], ContentBlock::Text { text } if text == "screenshot captured")
            );
            assert!(
                matches!(&results[0].content[1], ContentBlock::Text { text } if text == "[image: image/png]")
            );
        } else {
            panic!("expected ToolResults message");
        }
    }
}

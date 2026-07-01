//! DefaultCompactor — provider-agnostic context compaction implementation.
//!
//! Gated behind the `session-compaction` feature.

use meerkat_core::compact::{CompactionConfig, CompactionContext, CompactionResult, Compactor};
use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, ContentBlock, Message};

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

/// Project media blocks to text placeholders for the summarization LLM input.
///
/// This projection is intentionally text-only so summarization remains compatible
/// with providers/models that cannot consume every retained media payload. It
/// must not be used for rebuilt active history, which preserves typed media/blob
/// references verbatim.
fn project_media_for_summarization(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Image { media_type, .. } => ContentBlock::Text {
                text: format!("[image: {media_type}]"),
            },
            ContentBlock::Video { media_type, .. } => ContentBlock::Text {
                text: format!("[video: {media_type}]"),
            },
            other => other.clone(),
        })
        .collect()
}

/// Strip reasoning blocks from assistant messages for compaction.
///
/// Reasoning blocks contain provider-specific encrypted content that is
/// only valid within the original API session. Replaying them into a
/// compaction call (a fresh API request) causes provider failures.
/// The visible reasoning text is already captured in `Text` blocks or
/// is internal-only; the summarizer does not need it.
fn project_assistant_blocks_for_summarization(blocks: &[AssistantBlock]) -> Vec<AssistantBlock> {
    blocks
        .iter()
        .filter(|b| !matches!(b, AssistantBlock::Reasoning { .. }))
        .cloned()
        .collect()
}

/// Project media from all messages in a history for the summary request.
///
/// Applies `project_media_for_summarization` to `UserMessage.content` and
/// `ToolResult.content` blocks. Strips reasoning blocks from assistant
/// messages (encrypted content is session-scoped and cannot be replayed).
/// Drops assistant messages that become empty after projection.
fn project_messages_for_summarization(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|msg| match msg {
            Message::User(user) => {
                let content = project_media_for_summarization(&user.content);
                let mut user = user.clone();
                user.content = content;
                Some(Message::User(user))
            }
            Message::BlockAssistant(assistant) => {
                let blocks = project_assistant_blocks_for_summarization(&assistant.blocks);
                if blocks.is_empty() {
                    None
                } else {
                    Some(Message::BlockAssistant(BlockAssistantMessage {
                        blocks,
                        stop_reason: assistant.stop_reason,
                        identity: meerkat_core::types::TranscriptMessageIdentity::default(),
                        created_at: assistant.created_at,
                    }))
                }
            }
            Message::ToolResults {
                results,
                created_at,
            } => {
                let results = results
                    .iter()
                    .map(|r| {
                        let content = project_media_for_summarization(&r.content);
                        meerkat_core::types::ToolResult::with_blocks(
                            r.tool_use_id.clone(),
                            content,
                            r.is_error,
                        )
                    })
                    .collect();
                Some(Message::ToolResults {
                    results,
                    created_at: *created_at,
                })
            }
            other => Some(other.clone()),
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

        // Trigger on either threshold. `last_input_tokens` is the
        // authoritative provider-reported cost of the last LLM call;
        // `estimated_history_tokens` is the fallback used when the provider
        // never reports usage (voice-only sessions can run for hours
        // without an agent-loop turn, so the fallback is what keeps history
        // bounded). Both paths are traced so operators can see which branch
        // fired in production.
        let input_trigger = ctx.last_input_tokens >= self.config.auto_compact_threshold;
        let history_trigger = ctx.estimated_history_tokens >= self.config.auto_compact_threshold;
        if input_trigger || history_trigger {
            tracing::trace!(
                input_tokens = ctx.last_input_tokens,
                estimated_history_tokens = ctx.estimated_history_tokens,
                threshold = self.config.auto_compact_threshold,
                branch = if input_trigger {
                    "last_input_tokens"
                } else {
                    "estimated_history_tokens_fallback"
                },
                "compaction trigger fired",
            );
        }
        input_trigger || history_trigger
    }

    fn prepare_for_summarization(&self, messages: &[Message]) -> Vec<Message> {
        project_messages_for_summarization(messages)
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

        // 2. Inject summary as a user message carrying the typed compaction-
        //    summary transcript role so the transcript-continuity save-guard
        //    recognizes the rebuilt-transcript boundary from a typed field, not
        //    the rendered `[Context compacted]` prefix.
        let summary_content = format!("{SUMMARY_PREFIX}{summary}");
        rebuilt.push(Message::User(
            meerkat_core::types::UserMessage::compaction_summary(summary_content),
        ));

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

        // Everything from retain_from remains active history. Summary LLM input
        // uses a separate text projection; retained messages keep typed
        // media/blob references verbatim so compaction does not lose context.
        rebuilt.extend(history[retain_from..].iter().cloned());

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
    use meerkat_core::BlobId;
    use meerkat_core::types::{ImageData, SystemMessage, UserMessage, VideoData};

    fn make_config() -> CompactionConfig {
        CompactionConfig {
            auto_compact_threshold: 100_000,
            recent_turn_budget: 2,
            max_summary_tokens: 4096,
            min_turns_between_compactions: 3,
        }
    }

    fn inline_image_block(media_type: &str, data: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: media_type.to_string(),
            data: ImageData::Inline {
                data: data.to_string(),
            },
        }
    }

    fn blob_image_block(media_type: &str, blob_id: &str) -> ContentBlock {
        ContentBlock::Image {
            media_type: media_type.to_string(),
            data: ImageData::Blob {
                blob_id: BlobId::new(blob_id),
            },
        }
    }

    fn inline_video_block(media_type: &str, duration_ms: u64, data: &str) -> ContentBlock {
        ContentBlock::Video {
            media_type: media_type.to_string(),
            duration_ms,
            data: VideoData::Inline {
                data: data.to_string(),
            },
        }
    }

    fn assert_blob_image(block: &ContentBlock, expected_media_type: &str, expected_blob_id: &str) {
        match block {
            ContentBlock::Image {
                media_type,
                data: ImageData::Blob { blob_id },
            } => {
                assert_eq!(media_type, expected_media_type);
                assert_eq!(blob_id.as_str(), expected_blob_id);
            }
            other => panic!("expected blob image block, got {other:?}"),
        }
    }

    fn assert_inline_image(block: &ContentBlock, expected_media_type: &str, expected_data: &str) {
        match block {
            ContentBlock::Image {
                media_type,
                data: ImageData::Inline { data },
            } => {
                assert_eq!(media_type, expected_media_type);
                assert_eq!(data, expected_data);
            }
            other => panic!("expected inline image block, got {other:?}"),
        }
    }

    fn assert_inline_video(
        block: &ContentBlock,
        expected_media_type: &str,
        expected_duration_ms: u64,
        expected_data: &str,
    ) {
        match block {
            ContentBlock::Video {
                media_type,
                duration_ms,
                data: VideoData::Inline { data },
            } => {
                assert_eq!(media_type, expected_media_type);
                assert_eq!(*duration_ms, expected_duration_ms);
                assert_eq!(data, expected_data);
            }
            other => panic!("expected inline video block, got {other:?}"),
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
    fn test_voice_only_session_compacts_via_estimated_history_fallback() {
        // Voice-only sessions never pump the agent loop, so
        // `last_input_tokens` stays at zero. The
        // `estimated_history_tokens` path is the fallback that keeps a
        // long-running voice session from growing unbounded. This locks
        // in that the compactor fires on the fallback branch alone.
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 0,
            message_count: 200,
            estimated_history_tokens: 150_000,
            last_compaction_boundary_index: None,
            session_boundary_index: 42,
        };
        assert!(
            c.should_compact(&ctx),
            "voice-only session must compact via estimated_history_tokens \
             when last_input_tokens is zero",
        );
    }

    #[test]
    fn test_should_not_compact_when_neither_threshold_met() {
        // Regression guard for the Item 6 trace instrumentation: if neither
        // the input-tokens nor the estimated-history branch exceeds the
        // configured threshold, should_compact must still return false even
        // though the tracing span is absent.
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 20,
            estimated_history_tokens: 50_000,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
        };
        assert!(!c.should_compact(&ctx));
    }

    #[test]
    fn test_rebuild_preserves_system_prompt() {
        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::System(SystemMessage::new("system")),
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
            Message::System(SystemMessage::new("You are helpful.")),
            // Turn 1
            Message::User(UserMessage::text("What is the weather?")),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tc_1".to_string(),
                    name: "get_weather".to_string(),
                    args: args_raw,
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::new(
                "tc_1".to_string(),
                "Sunny, 25C".to_string(),
                false,
            )]),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "It's sunny in Tokyo!".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            )),
            // Turn 2
            Message::User(UserMessage::text("Thanks!")),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "You're welcome!".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            )),
        ];

        let result = c.rebuild_history(&messages, "Summary of weather conversation");

        // System prompt + summary + last turn (User "Thanks!" + BlockAssistant "You're welcome!")
        assert_eq!(result.messages.len(), 4); // system + summary + user + assistant
        assert!(matches!(&result.messages[0], Message::System(_)));

        // Discarded: turn 1 (User + BlockAssistant + ToolResults + BlockAssistant = 4 messages)
        assert_eq!(result.discarded.len(), 4);
    }

    #[test]
    fn summary_projection_replaces_media_preserves_text() {
        let blocks = vec![
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            inline_image_block("image/png", "base64data"),
            inline_video_block("video/mp4", 5_000, "videodata"),
            ContentBlock::Text {
                text: "world".to_string(),
            },
        ];
        let result = project_media_for_summarization(&blocks);
        assert_eq!(result.len(), 4);
        assert!(matches!(&result[0], ContentBlock::Text { text } if text == "hello"));
        assert!(matches!(&result[1], ContentBlock::Text { text } if text == "[image: image/png]"));
        assert!(matches!(&result[2], ContentBlock::Text { text } if text == "[video: video/mp4]"));
        assert!(matches!(&result[3], ContentBlock::Text { text } if text == "world"));
    }

    #[test]
    fn compaction_image_placeholder_excludes_source_path() {
        // source_path must NOT appear in the placeholder — it's internal metadata
        // that would leak filesystem paths through transcript history APIs.
        let blocks = vec![inline_image_block("image/png", "base64data")];
        let result = project_media_for_summarization(&blocks);
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
        let result = project_media_for_summarization(&blocks);
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0], ContentBlock::Text { text } if text == "one"));
        assert!(matches!(&result[1], ContentBlock::Text { text } if text == "two"));
    }

    #[test]
    fn prepare_for_summarization_projects_user_and_tool_media() {
        use meerkat_core::types::ToolResult;

        let c = DefaultCompactor::new(make_config());

        let messages = vec![
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "Look at this".to_string(),
                },
                inline_image_block("image/jpeg", "bigdata"),
                inline_video_block("video/mp4", 5_000, "video"),
            ])),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tc_1".to_string(),
                vec![
                    ContentBlock::Text {
                        text: "screenshot captured".to_string(),
                    },
                    inline_image_block("image/png", "screenshotdata"),
                    inline_video_block("video/webm", 7_000, "toolvideo"),
                ],
                false,
            )]),
        ];

        let prepared = c.prepare_for_summarization(&messages);
        assert_eq!(prepared.len(), 2);

        // User message: text preserved, image replaced
        if let Message::User(u) = &prepared[0] {
            assert_eq!(u.content.len(), 3);
            assert!(matches!(&u.content[0], ContentBlock::Text { text } if text == "Look at this"));
            assert!(
                matches!(&u.content[1], ContentBlock::Text { text } if text == "[image: image/jpeg]")
            );
            assert!(
                matches!(&u.content[2], ContentBlock::Text { text } if text == "[video: video/mp4]")
            );
        } else {
            panic!("expected User message");
        }

        // Tool result: text preserved, media replaced
        if let Message::ToolResults { results, .. } = &prepared[1] {
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].content.len(), 3);
            assert!(
                matches!(&results[0].content[0], ContentBlock::Text { text } if text == "screenshot captured")
            );
            assert!(
                matches!(&results[0].content[1], ContentBlock::Text { text } if text == "[image: image/png]")
            );
            assert!(
                matches!(&results[0].content[2], ContentBlock::Text { text } if text == "[video: video/webm]")
            );
        } else {
            panic!("expected ToolResults message");
        }
    }

    #[test]
    fn prepare_for_summarization_projects_media_without_mutating_source_history() {
        use meerkat_core::types::ToolResult;

        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "keep source typed".to_string(),
                },
                blob_image_block("image/png", "sha256:source-image"),
                inline_video_block("video/webm", 3_000, "source-video"),
            ])),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tool_1".to_string(),
                vec![
                    ContentBlock::Text {
                        text: "tool media".to_string(),
                    },
                    inline_image_block("image/jpeg", "tool-image"),
                ],
                true,
            )]),
        ];

        let prepared = c.prepare_for_summarization(&messages);

        match &prepared[0] {
            Message::User(user) => {
                assert_eq!(user.content.len(), 3);
                assert!(
                    matches!(&user.content[1], ContentBlock::Text { text } if text == "[image: image/png]")
                );
                assert!(
                    matches!(&user.content[2], ContentBlock::Text { text } if text == "[video: video/webm]")
                );
            }
            other => panic!("expected projected user message, got {other:?}"),
        }

        match &prepared[1] {
            Message::ToolResults { results, .. } => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].tool_use_id, "tool_1");
                assert!(results[0].is_error);
                assert!(
                    matches!(&results[0].content[1], ContentBlock::Text { text } if text == "[image: image/jpeg]")
                );
            }
            other => panic!("expected projected tool results, got {other:?}"),
        }

        match &messages[0] {
            Message::User(user) => {
                assert_blob_image(&user.content[1], "image/png", "sha256:source-image");
                assert_inline_video(&user.content[2], "video/webm", 3_000, "source-video");
            }
            other => panic!("expected original user message, got {other:?}"),
        }
        match &messages[1] {
            Message::ToolResults { results, .. } => {
                assert_inline_image(&results[0].content[1], "image/jpeg", "tool-image");
            }
            other => panic!("expected original tool results, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_history_preserves_videos_from_retained_turns() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });

        let messages = vec![
            Message::User(UserMessage::text("old text turn")),
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "latest with video".to_string(),
                },
                inline_video_block("video/mp4", 5_000, "video-data"),
            ])),
        ];

        let result = c.rebuild_history(&messages, "summary");

        assert_eq!(result.messages.len(), 2, "summary + retained turn");
        let retained = result.messages.last().expect("retained turn");
        match retained {
            Message::User(user) => {
                assert_eq!(user.content.len(), 2);
                assert!(matches!(
                    &user.content[0],
                    ContentBlock::Text { text } if text == "latest with video"
                ));
                assert_inline_video(&user.content[1], "video/mp4", 5_000, "video-data");
            }
            other => panic!("expected retained user turn, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_history_preserves_blob_images_from_retained_turns() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });

        let messages = vec![
            Message::User(UserMessage::text("old text turn")),
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "latest with image".to_string(),
                },
                blob_image_block("image/png", "sha256:test"),
            ])),
        ];

        let result = c.rebuild_history(&messages, "summary");

        assert_eq!(result.messages.len(), 2, "summary + retained turn");
        let retained = result.messages.last().expect("retained turn");
        match retained {
            Message::User(user) => {
                assert_eq!(user.content.len(), 2);
                assert!(matches!(
                    &user.content[0],
                    ContentBlock::Text { text } if text == "latest with image"
                ));
                assert_blob_image(&user.content[1], "image/png", "sha256:test");
            }
            other => panic!("expected retained user turn, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_history_preserves_tool_result_images_from_retained_turns() {
        use meerkat_core::types::ToolResult;

        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });

        let messages = vec![
            Message::User(UserMessage::text("old turn")),
            Message::User(UserMessage::text("latest turn")),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tool_1".to_string(),
                vec![
                    ContentBlock::Text {
                        text: "saw this".to_string(),
                    },
                    inline_image_block("image/jpeg", "abc"),
                ],
                false,
            )]),
        ];

        let result = c.rebuild_history(&messages, "summary");

        assert_eq!(
            result.messages.len(),
            3,
            "summary + retained user + tool results"
        );
        match &result.messages[2] {
            Message::ToolResults { results, .. } => {
                assert_eq!(results.len(), 1);
                assert_inline_image(&results[0].content[1], "image/jpeg", "abc");
            }
            other => panic!("expected retained tool results, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_history_retained_multimodal_shape_survives_json_roundtrip() {
        use meerkat_core::types::ToolResult;

        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });

        let messages = vec![
            Message::System(SystemMessage::new("system")),
            Message::User(UserMessage::text("discarded turn")),
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "latest media".to_string(),
                },
                blob_image_block("image/png", "sha256:latest-image"),
                inline_video_block("video/mp4", 8_000, "latest-video"),
            ])),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tool_2".to_string(),
                vec![
                    ContentBlock::Text {
                        text: "tool image".to_string(),
                    },
                    inline_image_block("image/jpeg", "tool-image"),
                ],
                false,
            )]),
        ];

        let result = c.rebuild_history(&messages, "summary");
        let json = serde_json::to_string(&result.messages).expect("serialize rebuilt transcript");
        let round_tripped: Vec<Message> =
            serde_json::from_str(&json).expect("deserialize rebuilt transcript");

        assert_eq!(
            round_tripped.len(),
            4,
            "system + summary + retained user + retained tool results"
        );

        match &round_tripped[2] {
            Message::User(user) => {
                assert_eq!(user.content.len(), 3);
                assert!(matches!(
                    &user.content[0],
                    ContentBlock::Text { text } if text == "latest media"
                ));
                assert_blob_image(&user.content[1], "image/png", "sha256:latest-image");
                assert_inline_video(&user.content[2], "video/mp4", 8_000, "latest-video");
            }
            other => panic!("expected retained user message after roundtrip, got {other:?}"),
        }

        match &round_tripped[3] {
            Message::ToolResults { results, .. } => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].tool_use_id, "tool_2");
                assert_inline_image(&results[0].content[1], "image/jpeg", "tool-image");
            }
            other => panic!("expected retained tool results after roundtrip, got {other:?}"),
        }

        assert!(
            !json.contains("[image:") && !json.contains("[video:"),
            "retained transcript JSON must keep typed media blocks, not summary placeholders: {json}"
        );
    }

    #[test]
    fn prepare_for_summarization_strips_reasoning_blocks() {
        use meerkat_core::types::{ProviderMeta, StopReason};

        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::User(UserMessage::text("Hello".to_string())),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![
                    AssistantBlock::Reasoning {
                        text: "Let me think".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_1".to_string(),
                            encrypted_content: Some("enc_data".to_string()),
                            phase: None,
                            response_id: None,
                        })),
                    },
                    AssistantBlock::Text {
                        text: "Here is my answer".to_string(),
                        meta: None,
                    },
                ],
                StopReason::EndTurn,
            )),
        ];

        let prepared = c.prepare_for_summarization(&messages);
        assert_eq!(prepared.len(), 2);

        if let Message::BlockAssistant(a) = &prepared[1] {
            assert_eq!(a.blocks.len(), 1);
            assert!(
                matches!(&a.blocks[0], AssistantBlock::Text { text, .. } if text == "Here is my answer")
            );
        } else {
            panic!("expected BlockAssistant message");
        }
    }

    #[test]
    fn prepare_for_summarization_drops_reasoning_only_assistant() {
        use meerkat_core::types::{ProviderMeta, StopReason};

        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::User(UserMessage::text("First".to_string())),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Reasoning {
                    text: String::new(),
                    meta: Some(Box::new(ProviderMeta::OpenAi {
                        id: "rs_orphan".to_string(),
                        encrypted_content: Some("enc".to_string()),
                        phase: None,
                        response_id: None,
                    })),
                }],
                StopReason::EndTurn,
            )),
            Message::User(UserMessage::text("Second".to_string())),
        ];

        let prepared = c.prepare_for_summarization(&messages);
        assert_eq!(
            prepared.len(),
            2,
            "reasoning-only assistant should be dropped"
        );
        assert!(matches!(&prepared[0], Message::User(u) if u.text_content() == "First"));
        assert!(matches!(&prepared[1], Message::User(u) if u.text_content() == "Second"));
    }
}

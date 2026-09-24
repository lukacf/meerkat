//! DefaultCompactor — provider-agnostic context compaction implementation.
//!
//! Gated behind the `session-compaction` feature.

use meerkat_core::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionConfig, CompactionContext, CompactionResult,
    CompactionSummary, Compactor,
};
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, Message,
    materialize_latest_system_prompt_versions, superseded_system_prompt_offsets,
};

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

/// Fraction of the normal compaction threshold available to the source
/// excerpt sent to the summarization model.
///
/// The threshold is normally 4/5 of the model context window. Capping the raw
/// UTF-8 excerpt at 1/4 of that threshold leaves room for JSON escaping, the
/// compaction prompt, and the summary response even when a single tool result
/// jumps from a healthy request to beyond the context window in one boundary.
const SUMMARIZATION_SOURCE_BUDGET_DENOMINATOR: u64 = 4;
const RETAINED_HISTORY_BUDGET_DENOMINATOR: u64 = 2;
const MIN_SUMMARIZATION_SOURCE_BUDGET_BYTES: u64 = 4 * 1024;
const OVERSIZED_SOURCE_PREFIX: &str = "[Bounded compaction source excerpt. The current projected transcript is larger than the safe summarization window. Its middle is intentionally omitted.]\n";
const OVERSIZED_SOURCE_GAP: &str =
    "\n[... middle of projected transcript omitted for provider capacity ...]\n";
const OVERSIZED_SOURCE_SUFFIX: &str = "\n[End of bounded excerpt. The active rewrite retains only recent turns that fit its safety budget.]";

/// Default compaction strategy implementation.
pub struct DefaultCompactor {
    config: CompactionConfig,
}

impl DefaultCompactor {
    /// Create a new compactor with the given configuration.
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    fn summarization_source_budget_bytes(&self) -> usize {
        let budget = self
            .config
            .auto_compact_threshold
            .div_ceil(SUMMARIZATION_SOURCE_BUDGET_DENOMINATOR)
            .max(MIN_SUMMARIZATION_SOURCE_BUDGET_BYTES);
        usize::try_from(budget).unwrap_or(usize::MAX)
    }

    fn retained_history_budget_bytes(
        &self,
        pressure: Option<meerkat_core::ProviderRequestPressure>,
    ) -> usize {
        let mut budget = self
            .config
            .auto_compact_threshold
            .div_ceil(RETAINED_HISTORY_BUDGET_DENOMINATOR)
            .max(MIN_SUMMARIZATION_SOURCE_BUDGET_BYTES);
        if let Some(pressure) = pressure
            && let Some(request_cap) = pressure.effective_cap(self.config.max_request_bytes)
        {
            // Retained canonical rows are lowered and JSON-escaped again by
            // the provider adapter. One quarter of the exact request cap
            // leaves room for tools, provider parameters, the summary row,
            // and escaping expansion.
            budget = budget.min(request_cap.div_ceil(4).max(1));
        }
        usize::try_from(budget).unwrap_or(usize::MAX)
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

/// Collapse an oversized typed projection to one bounded textual excerpt.
///
/// Keeping the ordinary typed projection when it fits preserves the richest
/// provider input. The fallback uses one user message so truncating in the
/// middle of an assistant/tool exchange cannot create an invalid provider
/// message sequence. The original session messages are never mutated.
fn bound_summarization_projection(messages: Vec<Message>, budget: usize) -> Vec<Message> {
    let serialized = match serde_json::to_string(&messages) {
        Ok(serialized) => serialized,
        Err(error) => {
            return vec![Message::User(meerkat_core::types::UserMessage::text(
                format!(
                    "[Compaction source projection could not be serialized: {error}. Continue with a mechanical handoff summary.]"
                ),
            ))];
        }
    };
    if serialized.len() <= budget {
        return messages;
    }

    tracing::warn!(
        projected_bytes = serialized.len(),
        budget_bytes = budget,
        "bounding oversized compaction summarization source"
    );
    let framing_bytes = OVERSIZED_SOURCE_PREFIX
        .len()
        .saturating_add(OVERSIZED_SOURCE_GAP.len())
        .saturating_add(OVERSIZED_SOURCE_SUFFIX.len());
    let excerpt_budget = budget.saturating_sub(framing_bytes);
    let head_budget = excerpt_budget / 2;
    let tail_budget = excerpt_budget.saturating_sub(head_budget);
    let mut head_end = head_budget.min(serialized.len());
    while head_end > 0 && !serialized.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = serialized.len().saturating_sub(tail_budget);
    while tail_start < serialized.len() && !serialized.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let bounded = format!(
        "{OVERSIZED_SOURCE_PREFIX}{}{OVERSIZED_SOURCE_GAP}{}{OVERSIZED_SOURCE_SUFFIX}",
        &serialized[..head_end],
        &serialized[tail_start..]
    );
    vec![Message::User(meerkat_core::types::UserMessage::text(
        bounded,
    ))]
}

impl Compactor for DefaultCompactor {
    fn request_byte_cap(&self, pressure: meerkat_core::ProviderRequestPressure) -> Option<u64> {
        pressure.effective_cap(self.config.max_request_bytes)
    }

    fn should_compact(&self, ctx: &CompactionContext) -> bool {
        // Never compact on the first-ever session LLM boundary.
        if ctx.session_boundary_index == 0 {
            return false;
        }

        // Trigger on any crossing threshold. `last_input_tokens` is the
        // authoritative provider-reported cost of the last LLM call;
        // `estimated_history_tokens` is the fallback used when the provider
        // never reports usage (voice-only sessions can run for hours
        // without an agent-loop turn, so the fallback is what keeps history
        // bounded). The byte trigger measures the transcript in the unit
        // providers actually enforce: the 2026-07-29 household incident grew
        // a byte-heavy/token-light transcript past Anthropic's request-size
        // cap and failed every turn with `request_too_large` while both
        // token triggers stayed far below threshold. All paths are traced so
        // operators can see which branch fired in production.
        let input_trigger = ctx.last_input_tokens >= self.config.auto_compact_threshold;
        let history_trigger = ctx.estimated_history_tokens >= self.config.auto_compact_threshold;
        // Whole-request token forecast, in the same unit the threshold is
        // derived from (4/5 of the model context window). Both token measures
        // above are smaller than the request that window has to hold:
        // `last_input_tokens` is a provider count that already includes tool
        // schemas and framing but lags this boundary by whatever it appended,
        // and `estimated_history_tokens` is current but measures the transcript
        // alone - no tool definitions, no payload a later blob hydration
        // inlines. A request that crosses only because of those components was
        // invisible to the trigger and visible to the provider.
        let request_forecast_tokens = ctx
            .request_context_budget
            .as_ref()
            .map(|budget| budget.effective_input_tokens());
        let forecast_trigger = request_forecast_tokens
            .is_some_and(|tokens| tokens >= self.config.auto_compact_threshold);
        let (request_bytes, byte_threshold, request_measurement) =
            match ctx.provider_request_pressure {
                Some(pressure) => (
                    pressure.encoded_bytes,
                    pressure.trigger_threshold(self.config.max_request_bytes),
                    "provider_lowered_exact",
                ),
                None => (
                    ctx.estimated_request_bytes,
                    self.config.request_byte_trigger_threshold(),
                    "transcript_estimate",
                ),
            };
        let byte_trigger = byte_threshold.is_some_and(|threshold| request_bytes >= threshold);

        // Cadence is a cost policy, not recovery authority. It may suppress a
        // provider-reported high-water mark while the current transcript is
        // still below both live pressure thresholds, but it must never veto a
        // history or exact-byte crossing. One large tool result can make the
        // very next request impossible, including when the previous
        // compaction happened fewer than `min_turns_between_compactions`
        // boundaries ago.
        //
        // The whole-request forecast is deliberately NOT capacity recovery.
        // Part of what it measures - tool definitions and the output reserve -
        // survives compaction untouched, so a forecast crossing that cadence
        // could not veto would compact on every boundary forever without
        // reducing the crossing.
        let requires_capacity_recovery = history_trigger || byte_trigger;
        if !requires_capacity_recovery
            && let Some(last) = ctx.last_compaction_boundary_index
            && ctx.session_boundary_index.saturating_sub(last)
                < u64::from(self.config.min_turns_between_compactions)
        {
            return false;
        }
        if input_trigger || history_trigger || byte_trigger || forecast_trigger {
            tracing::trace!(
                input_tokens = ctx.last_input_tokens,
                estimated_history_tokens = ctx.estimated_history_tokens,
                request_forecast_tokens,
                request_forecast_provenance = ctx
                    .request_context_budget
                    .as_ref()
                    .map(|budget| budget.estimate_provenance)
                    .map(|provenance| format!("{provenance:?}")),
                estimated_request_bytes = ctx.estimated_request_bytes,
                provider_request_bytes = ctx
                    .provider_request_pressure
                    .map(|pressure| pressure.encoded_bytes),
                threshold = self.config.auto_compact_threshold,
                byte_threshold,
                request_measurement,
                branch = if input_trigger {
                    "last_input_tokens"
                } else if history_trigger {
                    "estimated_history_tokens_fallback"
                } else if byte_trigger && ctx.provider_request_pressure.is_some() {
                    "provider_request_bytes"
                } else if byte_trigger {
                    "estimated_request_bytes"
                } else {
                    "request_context_budget_forecast"
                },
                "compaction trigger fired",
            );
        }
        input_trigger || history_trigger || byte_trigger || forecast_trigger
    }

    fn prepare_for_summarization(&self, messages: &[Message]) -> Vec<Message> {
        let messages = materialize_latest_system_prompt_versions(messages);
        bound_summarization_projection(
            project_messages_for_summarization(&messages),
            self.summarization_source_budget_bytes(),
        )
    }

    fn compaction_prompt(&self) -> &str {
        COMPACTION_PROMPT
    }

    fn max_summary_tokens(&self) -> u32 {
        self.config.max_summary_tokens
    }

    fn rebuild_history_under_pressure(
        &self,
        messages: &[Message],
        summary: &str,
        pressure: Option<meerkat_core::ProviderRequestPressure>,
    ) -> CompactionResult {
        let superseded_system_prompts = superseded_system_prompt_offsets(messages);
        let mut rebuilt = Vec::new();
        let mut retained = Vec::new();
        let mut discarded = Vec::new();
        let summary_content = format!("{COMPACTION_SUMMARY_PREFIX}{summary}");
        let summary_message = Message::User(meerkat_core::types::UserMessage::compaction_summary(
            summary_content,
        ));

        // Identify recent complete turns to retain.
        // A "turn" is User -> BlockAssistant -> ToolResults sequence.
        // We work backward from the end to find `recent_turn_budget` turns.
        // Find turn boundaries. Only a CONVERSATIONAL user message starts a
        // turn: a prior compaction summary is a runtime boundary marker (and
        // is discarded wholesale at the next compaction), and injected-context
        // messages are host-attached ambient context delivered immediately
        // BEFORE their turn's user message — counting either as a turn start
        // would dilute the retained-turn budget. A retained turn is glued to
        // the contiguous injected-context run preceding it, so a kept user
        // message never loses the ambient context the model responded with.
        let mut turn_starts: Vec<usize> = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if matches!(msg, Message::User(u) if u.transcript_role.is_conversational()) {
                let mut start = i;
                while start > 0
                    && matches!(
                        &messages[start - 1],
                        Message::User(u) if u.transcript_role.is_injected_context()
                    )
                {
                    start -= 1;
                }
                turn_starts.push(start);
            }
        }

        // A successful compaction must summarize and remove real source
        // content on every pass. A prior summary/non-turn prefix is discarded,
        // but it cannot be the only discarded content: replacing that prefix
        // with an identical new summary would be a no-op and would never
        // advance the rewrite chain. The turn budget is therefore a maximum,
        // and at least the oldest live turn is summarized on every pass.
        let mut retain_turn_count = if self.config.recent_turn_budget == 0 {
            0
        } else {
            self.config
                .recent_turn_budget
                .min(turn_starts.len().saturating_sub(1))
        };
        let retention_budget = self.retained_history_budget_bytes(pressure);
        let retain_from = loop {
            let candidate = if retain_turn_count == 0 {
                messages.len()
            } else {
                turn_starts[turn_starts.len() - retain_turn_count]
            };
            let retained_bytes = messages
                .iter()
                .enumerate()
                .filter(|(source_offset, message)| {
                    !superseded_system_prompts.contains(source_offset)
                        && (matches!(message, Message::System(_)) || *source_offset >= candidate)
                })
                .try_fold(0usize, |total, (_, message)| {
                    serde_json::to_vec(message)
                        .ok()
                        .and_then(|encoded| total.checked_add(encoded.len()))
                })
                .unwrap_or(usize::MAX);
            if retained_bytes <= retention_budget || retain_turn_count == 0 {
                break candidate;
            }
            retain_turn_count -= 1;
        };

        // Unkeyed System rows and the latest version for each prompt key are
        // ordinary ordered events and remain exact. Superseded keyed versions
        // are historical rows: compaction discards them from the active body,
        // while the typed rewrite graph keeps their prior revision reachable.
        // Insert the summary where the first discarded source row stood.
        let first_discarded_source_offset = messages
            .iter()
            .enumerate()
            .find_map(|(source_offset, message)| {
                (superseded_system_prompts.contains(&source_offset)
                    || (!matches!(message, Message::System(_)) && source_offset < retain_from))
                    .then_some(source_offset)
            })
            .unwrap_or(messages.len());
        let mut summary_mapping = None;
        for (source_offset, message) in messages.iter().enumerate() {
            if source_offset == first_discarded_source_offset {
                summary_mapping = Some(CompactionSummary::new(
                    u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                    summary_message.clone(),
                ));
                rebuilt.push(summary_message.clone());
            }
            let retain = !superseded_system_prompts.contains(&source_offset)
                && (matches!(message, Message::System(_)) || source_offset >= retain_from);
            if retain {
                retained.push(meerkat_core::compact::CompactionRetained::new(
                    u64::try_from(source_offset).unwrap_or(u64::MAX),
                    u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                    message.clone(),
                ));
                rebuilt.push(message.clone());
                continue;
            }
            discarded.push(meerkat_core::compact::CompactionDiscard::new(
                u64::try_from(source_offset).unwrap_or(u64::MAX),
                message.clone(),
            ));
        }
        let summary_mapping = summary_mapping.unwrap_or_else(|| {
            let mapping = CompactionSummary::new(
                u64::try_from(rebuilt.len()).unwrap_or(u64::MAX),
                summary_message.clone(),
            );
            rebuilt.push(summary_message);
            mapping
        });

        CompactionResult {
            messages: rebuilt,
            summary: summary_mapping,
            retained,
            discarded,
        }
    }

    fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
        self.rebuild_history_under_pressure(messages, summary, None)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::BlobId;
    use meerkat_core::types::{
        ImageData, SystemMessage, SystemPromptKey, SystemPromptVersion,
        SystemPromptVersionIdentity, UserMessage, VideoData,
    };

    fn make_config() -> CompactionConfig {
        CompactionConfig {
            auto_compact_threshold: 100_000,
            max_request_bytes: None,
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
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
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
            estimated_history_tokens: 50_000,
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: Some(5),
            session_boundary_index: 7, // Only 2 boundaries since last compaction, threshold is 3
        };
        assert!(!c.should_compact(&ctx));
    }

    #[test]
    fn history_capacity_crossing_bypasses_loop_guard() {
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 100,
            estimated_history_tokens: 100_000,
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: Some(5),
            session_boundary_index: 6,
        };
        assert!(
            c.should_compact(&ctx),
            "an already-oversized history must recover even immediately after a prior compaction"
        );
    }

    #[test]
    fn test_should_compact_follow_up_run_boundary_zero_no_longer_special() {
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 200_000,
            message_count: 100,
            estimated_history_tokens: 200_000,
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
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
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
        };
        assert!(c.should_compact(&ctx));

        // Trigger via history tokens
        let ctx2 = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 50,
            estimated_history_tokens: 100_000,
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
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
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
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
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
        };
        assert!(!c.should_compact(&ctx));
    }

    #[test]
    fn test_byte_trigger_fires_below_token_threshold_on_byte_heavy_transcript() {
        // The 2026-07-29 incident shape: inline media dominates request BYTES
        // while both token measures sit far below the token threshold. The
        // byte trigger must fire at 4/5 of the configured cap.
        let c = DefaultCompactor::new(CompactionConfig {
            max_request_bytes: Some(9_000_000),
            ..make_config()
        });
        let ctx = CompactionContext {
            last_input_tokens: 10_000,
            message_count: 40,
            estimated_history_tokens: 12_000,
            estimated_request_bytes: 7_200_000, // exactly 4/5 of the 9 MB cap
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
        };
        assert!(
            c.should_compact(&ctx),
            "byte trigger must fire on a byte-heavy transcript before any token threshold"
        );

        // One byte below the 4/5 threshold must not fire.
        let below = CompactionContext {
            estimated_request_bytes: 7_199_999,
            ..ctx
        };
        assert!(!c.should_compact(&below));
    }

    #[test]
    fn test_token_trigger_unchanged_when_byte_trigger_unset() {
        // With max_request_bytes unset (the default), even an absurd byte
        // estimate must not fire — pre-incident behavior is preserved.
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 50,
            estimated_history_tokens: 50_000,
            estimated_request_bytes: u64::MAX,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
        };
        assert!(
            !c.should_compact(&ctx),
            "an unset byte cap must leave the trigger decision to the token thresholds"
        );

        // And the token trigger still fires exactly as before.
        let token_crossing = CompactionContext {
            last_input_tokens: 100_000,
            ..ctx
        };
        assert!(c.should_compact(&token_crossing));
    }

    #[test]
    fn exact_provider_witness_supplies_the_dynamic_active_cap() {
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 1,
            message_count: 2,
            estimated_history_tokens: 1,
            // The transcript estimate cannot see a cold-resumed blob payload.
            estimated_request_bytes: 128,
            request_context_budget: None,
            provider_request_pressure: Some(meerkat_core::ProviderRequestPressure::new(
                7_200_000,
                Some(9_000_000),
            )),
            last_compaction_boundary_index: Some(5),
            session_boundary_index: 6,
        };
        assert!(
            c.should_compact(&ctx),
            "the active provider's exact lowered body and cap must override both the blind transcript estimate and cadence guard"
        );
    }

    /// Model profile witness for a custom model with an exact declared window.
    ///
    /// The arithmetic is scale-free, so a small window keeps the synthetic
    /// transcript cheap. Production shape it stands in for: a 1,050,000-token
    /// window, a 840,000-token trigger, and a tool-heavy mob member.
    fn windowed_profile_witness(window_tokens: u32) -> meerkat_core::ModelProfileWitness {
        const MODEL: &str = "compaction-forecast-test-model";
        let mut config = meerkat_core::Config::default();
        config.models.custom.insert(
            MODEL.to_string(),
            meerkat_core::config::CustomModelConfig {
                provider: meerkat_core::Provider::OpenAI,
                display_name: None,
                context_window: Some(window_tokens),
                max_input_tokens: None,
                max_output_tokens: Some(2_048),
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            },
        );
        let registry =
            meerkat_core::ModelRegistry::from_config(&config, meerkat_models::canonical())
                .expect("custom-model test registry");
        registry
            .profile_witness_for_provider(meerkat_core::Provider::OpenAI, MODEL)
            .expect("custom model must mint an exact profile witness")
    }

    fn forecast_test_tools(count: usize) -> Vec<std::sync::Arc<meerkat_core::ToolDef>> {
        (0..count)
            .map(|index| {
                std::sync::Arc::new(meerkat_core::ToolDef::new(
                    format!("forecast_tool_{index}"),
                    "a tool whose schema rides every single request in full",
                    serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "what to look up" },
                            "limit": { "type": "integer", "description": "row cap" }
                        },
                        "required": ["query"]
                    }),
                ))
            })
            .collect()
    }

    /// The trigger-side accounting pin: a request whose true token cost crosses
    /// the threshold while the transcript estimate alone stays under it must
    /// compact. Tool definitions ride every request in full and the provider
    /// counts them; before the whole-request forecast reached the trigger, they
    /// were invisible to it, so a strand could keep growing past the threshold
    /// the model window was supposed to protect.
    #[test]
    fn whole_request_forecast_fires_when_the_transcript_estimate_stays_below_threshold() {
        let threshold = 8_000;
        let compactor = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: threshold,
            ..make_config()
        });
        let profile = windowed_profile_witness(10_000);
        let tools = forecast_test_tools(24);
        // Just under the threshold on the transcript axis alone.
        let messages = vec![Message::User(UserMessage::text("t".repeat(31_600)))];

        let transcript_only =
            meerkat_core::agent::compact::estimate_tokens(&messages).expect("transcript estimate");
        assert!(
            transcript_only < threshold,
            "the synthetic transcript must stay below the threshold on its own: {transcript_only}"
        );

        let budget =
            meerkat_core::context_budget_fact_for_messages(&messages, &tools, 2_048, &profile)
                .expect("declared window must classify");
        assert!(
            budget.estimated_tool_tokens > 0,
            "the visible tool set must contribute measured tokens"
        );
        assert!(
            budget.effective_input_tokens() >= threshold,
            "the whole request must cross the threshold the transcript alone does not: {}",
            budget.effective_input_tokens()
        );

        let ctx = meerkat_core::agent::compact::build_compaction_context(
            &messages,
            0,
            Some(budget),
            None,
            None,
            5,
        );
        assert!(
            ctx.estimated_history_tokens < threshold,
            "the legacy history estimate must not be what fires here"
        );
        assert!(
            compactor.should_compact(&ctx),
            "a request whose measured cost crosses the threshold must compact before the window"
        );

        // With no declared window the forecast is absent, and the decision falls
        // back to exactly the pre-existing measures.
        let without_forecast = CompactionContext {
            request_context_budget: None,
            ..ctx
        };
        assert!(
            !compactor.should_compact(&without_forecast),
            "an unavailable forecast must leave the trigger decision unchanged"
        );
    }

    /// An exact provider-issued input count inside the forecast is authoritative:
    /// it must not be replaced by, or averaged with, the local estimate.
    #[test]
    fn exact_provider_issued_token_count_drives_the_forecast_trigger() {
        let threshold = 8_000;
        let compactor = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: threshold,
            ..make_config()
        });
        let profile = windowed_profile_witness(10_000);
        let messages = vec![Message::User(UserMessage::text("small transcript"))];

        let budget = meerkat_core::context_budget_fact_for_provider_request(
            &messages,
            &[],
            2_048,
            &profile,
            meerkat_core::ProviderRequestPressure::new(4_096, None)
                .with_provider_issued_input_tokens(8_100),
        )
        .expect("declared window must classify");

        let ctx = meerkat_core::agent::compact::build_compaction_context(
            &messages,
            0,
            Some(budget),
            None,
            None,
            5,
        );
        assert!(
            ctx.estimated_history_tokens < threshold,
            "the transcript is deliberately tiny on both legacy measures"
        );
        assert!(
            compactor.should_compact(&ctx),
            "an exact provider count above the threshold must fire the trigger"
        );
    }

    /// The forecast crossing is a cost-guarded trigger, not capacity recovery.
    /// Tool definitions and the output reserve survive compaction untouched, so
    /// a forecast crossing that cadence could not veto would compact on every
    /// boundary without ever clearing the crossing.
    #[test]
    fn forecast_crossing_still_respects_the_cadence_guard() {
        let threshold = 8_000;
        let compactor = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: threshold,
            ..make_config()
        });
        let profile = windowed_profile_witness(10_000);
        let tools = forecast_test_tools(24);
        let messages = vec![Message::User(UserMessage::text("t".repeat(31_600)))];
        let budget =
            meerkat_core::context_budget_fact_for_messages(&messages, &tools, 2_048, &profile)
                .expect("declared window must classify");

        let ctx = meerkat_core::agent::compact::build_compaction_context(
            &messages,
            0,
            Some(budget),
            None,
            Some(5),
            6,
        );
        assert!(
            !compactor.should_compact(&ctx),
            "a forecast-only crossing must not bypass the cost guard"
        );
    }

    #[test]
    fn test_both_triggers_set_first_crossing_wins() {
        // With both thresholds armed, whichever crosses first fires — the
        // triggers are independent, not conjunctive.
        let c = DefaultCompactor::new(CompactionConfig {
            max_request_bytes: Some(9_000_000),
            ..make_config()
        });
        let neither = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 50,
            estimated_history_tokens: 50_000,
            estimated_request_bytes: 1_000_000,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: None,
            session_boundary_index: 5,
        };
        assert!(!c.should_compact(&neither));

        let bytes_first = CompactionContext {
            estimated_request_bytes: 8_000_000,
            ..neither.clone()
        };
        assert!(
            c.should_compact(&bytes_first),
            "byte crossing alone must fire when both triggers are armed"
        );

        let tokens_first = CompactionContext {
            last_input_tokens: 100_000,
            ..neither
        };
        assert!(
            c.should_compact(&tokens_first),
            "token crossing alone must fire when both triggers are armed"
        );
    }

    #[test]
    fn rebuild_preserves_ordered_system_message() {
        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::System(SystemMessage::new("system")),
            Message::User(UserMessage::text("turn1")),
            Message::User(UserMessage::text("turn2")),
            Message::User(UserMessage::text("turn3")),
        ];
        let result = c.rebuild_history(&messages, "summary text");
        assert!(matches!(&result.messages[0], Message::System(s) if s.content == "system"));
        assert_eq!(result.summary.rebuilt_offset, 1);
        assert_eq!(result.messages[1], result.summary.message);
        assert_eq!(result.discarded.len(), 1);
        assert_eq!(result.discarded[0].source_offset, 1);
        assert!(matches!(
            &result.discarded[0].message,
            Message::User(u) if u.text_content() == "turn1"
        ));
        assert_eq!(result.retained[0].source_offset, 0);
        assert_eq!(result.retained[0].rebuilt_offset, 0);
    }

    #[test]
    fn compaction_selects_latest_versioned_system_prompt_and_discards_prior_versions() {
        let c = DefaultCompactor::new(make_config());
        let key = SystemPromptKey::new("primary").expect("prompt key");
        let mut first = SystemMessage::new("version one");
        first.prompt_version = Some(SystemPromptVersionIdentity {
            key: key.clone(),
            version: SystemPromptVersion::INITIAL,
        });
        let mut second = SystemMessage::new("version two");
        second.prompt_version = Some(SystemPromptVersionIdentity {
            key,
            version: SystemPromptVersion::new(2).expect("version two"),
        });
        let messages = vec![
            Message::System(first),
            Message::System(second),
            Message::User(UserMessage::text("old turn")),
            Message::User(UserMessage::text("recent turn")),
        ];

        let summary_input = c.prepare_for_summarization(&messages);
        assert!(summary_input.iter().all(
            |message| !matches!(message, Message::System(system) if system.content == "version one")
        ));
        assert!(summary_input.iter().any(
            |message| matches!(message, Message::System(system) if system.content == "version two")
        ));

        let result = c.rebuild_history(&messages, "summary");
        assert!(result.discarded.iter().any(|discard| {
            discard.source_offset == 0
                && matches!(&discard.message, Message::System(system) if system.content == "version one")
        }));
        assert!(result.retained.iter().any(|retention| {
            retention.source_offset == 1
                && matches!(&retention.message, Message::System(system) if system.content == "version two")
        }));
    }

    #[test]
    fn rebuild_preserves_mid_thread_system_in_order_across_compaction() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });
        let messages = vec![
            Message::System(SystemMessage::new("initial prompt")),
            Message::User(UserMessage::text("turn1")),
            Message::System(SystemMessage::new("mid-thread instruction")),
            Message::User(UserMessage::text("turn2")),
            Message::User(UserMessage::text("turn3")),
        ];

        let result = c.rebuild_history(&messages, "summary");

        assert!(matches!(
            &result.messages[0],
            Message::System(system) if system.content == "initial prompt"
        ));
        assert_eq!(result.messages[1], result.summary.message);
        assert!(matches!(
            &result.messages[2],
            Message::System(system) if system.content == "mid-thread instruction"
        ));
        assert!(
            result
                .retained
                .iter()
                .any(|retention| retention.source_offset == 2 && retention.rebuilt_offset == 2),
            "the mid-thread System message must remain an exact retained row"
        );
        assert!(
            result
                .discarded
                .iter()
                .all(|discard| !matches!(discard.message, Message::System(_))),
            "compaction may never discard an ordered System message"
        );
    }

    #[test]
    fn rebuild_places_summary_after_complete_retained_instruction_prefix() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });
        let messages = vec![
            Message::System(SystemMessage::new("system A")),
            Message::System(SystemMessage::new("system B")),
            Message::User(UserMessage::text("old turn")),
            Message::User(UserMessage::text("recent turn")),
        ];

        let result = c.rebuild_history(&messages, "summary");

        assert!(matches!(
            &result.messages[0],
            Message::System(system) if system.content == "system A"
        ));
        assert!(matches!(
            &result.messages[1],
            Message::System(system) if system.content == "system B"
        ));
        assert_eq!(result.summary.rebuilt_offset, 2);
        assert_eq!(result.messages[2], result.summary.message);
        assert_eq!(
            result
                .retained
                .iter()
                .map(|retention| (retention.source_offset, retention.rebuilt_offset))
                .collect::<Vec<_>>(),
            vec![(0, 0), (1, 1), (3, 3)]
        );
        assert_eq!(result.discarded.len(), 1);
        assert_eq!(result.discarded[0].source_offset, 2);
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
    fn test_rebuild_below_turn_budget_still_discards_oldest_turn() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 4,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage::text("turn1")),
            Message::User(UserMessage::text("turn2")),
        ];

        let result = c.rebuild_history(&messages, "summary");

        assert_eq!(result.messages.len(), messages.len());
        assert_eq!(result.discarded.len(), 1);
        assert_eq!(result.discarded[0].source_offset, 0);
        assert!(matches!(
            &result.messages[0],
            Message::User(user) if user.transcript_role.is_compaction_summary()
        ));
        assert!(matches!(
            &result.messages[1],
            Message::User(user) if user.text_content() == "turn2"
        ));
    }

    #[test]
    fn test_rebuild_below_turn_budget_discards_prior_summary_and_oldest_live_turn() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 4,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage::compaction_summary("old summary")),
            Message::User(UserMessage::text("turn1")),
            Message::User(UserMessage::text("turn2")),
        ];

        let result = c.rebuild_history(&messages, "summary");

        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.discarded.len(), 2);
        assert!(matches!(
            &result.discarded[0].message,
            Message::User(user) if user.transcript_role.is_compaction_summary()
        ));
        assert!(matches!(
            &result.discarded[1].message,
            Message::User(user) if user.text_content() == "turn1"
        ));
        assert!(matches!(
            &result.messages[1],
            Message::User(user) if user.text_content() == "turn2"
        ));
        assert!(result.retained.iter().all(|retention| !matches!(
            &retention.message,
            Message::User(user) if user.transcript_role.is_compaction_summary()
        )));
    }

    #[test]
    fn test_rebuild_injected_context_does_not_start_or_dilute_turns() {
        // Two real turns, each preceded by an injected-context run. With a
        // budget of 1, only the LAST conversational turn is retained — glued
        // to its injected-context messages — and the injected messages never
        // count as turns of their own.
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage::injected_context("ambient a")),
            Message::User(UserMessage::text("turn1")),
            Message::User(UserMessage::injected_context("ambient b1")),
            Message::User(UserMessage::injected_context("ambient b2")),
            Message::User(UserMessage::text("turn2")),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // Summary + [ambient b1, ambient b2, turn2] retained.
        assert_eq!(result.messages.len(), 4);
        assert!(matches!(
            &result.messages[1],
            Message::User(u) if u.transcript_role.is_injected_context()
                && u.text_content() == "ambient b1"
        ));
        assert!(matches!(
            &result.messages[3],
            Message::User(u) if u.text_content() == "turn2"
        ));
        // Discarded: [ambient a, turn1].
        assert_eq!(result.discarded.len(), 2);
    }

    #[test]
    fn test_rebuild_prior_summary_does_not_count_as_turn() {
        // A prior compaction-summary message is a runtime boundary marker,
        // not a conversational turn: with budget 1 it must be discarded while
        // the single real turn is retained.
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage::compaction_summary("[Context compacted] old")),
            Message::User(UserMessage::text("turn1")),
            Message::User(UserMessage::text("turn2")),
        ];
        let result = c.rebuild_history(&messages, "summary");
        // If the prior summary counted as a turn, retain_from would land on
        // turn1 and keep it; with summary excluded only turn2 survives.
        assert_eq!(result.messages.len(), 2, "new summary + retained turn2");
        assert!(matches!(
            &result.messages[1],
            Message::User(u) if u.text_content() == "turn2"
        ));
        assert_eq!(result.discarded.len(), 2, "prior summary + turn1 discarded");
        assert!(matches!(
            &result.discarded[0].message,
            Message::User(u) if u.transcript_role.is_compaction_summary()
        ));
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
    fn test_rebuild_budget_larger_than_history_still_replaces_oldest_turn() {
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
        // A successful compaction never retains every source and appends a
        // summary. The budget is a maximum, so the oldest turn is summarized
        // and the remaining two stay live.
        assert_eq!(result.messages.len(), 3);
        assert_eq!(result.discarded.len(), 1);
        assert!(matches!(
            &result.discarded[0].message,
            Message::User(user) if user.text_content() == "t1"
        ));
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
        if let Message::User(u) = &result.discarded[0].message {
            assert_eq!(u.text_content(), "a");
        }
        if let Message::User(u) = &result.discarded[1].message {
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
    fn rebuild_does_not_retain_a_single_turn_larger_than_the_recovery_budget() {
        use meerkat_core::types::ToolResult;

        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 100_000,
            recent_turn_budget: 4,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage::text("older turn")),
            Message::User(UserMessage::text("inspect the large result")),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tc_oversized".to_string(),
                vec![ContentBlock::Text {
                    text: "x".repeat(200_000),
                }],
                false,
            )]),
        ];

        let result = c.rebuild_history(&messages, "bounded summary");
        assert_eq!(
            result.messages.len(),
            1,
            "an individually oversized newest turn must be summarized, not retained verbatim"
        );
        assert_eq!(result.discarded.len(), messages.len());
        assert!(matches!(
            &result.messages[0],
            Message::User(user) if user.transcript_role.is_compaction_summary()
        ));
    }

    #[test]
    fn rebuild_uses_exact_provider_cap_to_bound_retained_tail() {
        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 1_000_000,
            recent_turn_budget: 4,
            ..make_config()
        });
        let messages = vec![
            Message::User(UserMessage::text("older turn")),
            Message::User(UserMessage::text("x".repeat(100_000))),
        ];
        let pressure = meerkat_core::ProviderRequestPressure::new(120_000, Some(40_000));

        let result = c.rebuild_history_under_pressure(&messages, "bounded summary", Some(pressure));
        assert_eq!(
            result.messages.len(),
            1,
            "the exact provider byte cap must prevent the oversized newest turn from surviving the rescue rewrite"
        );
        assert_eq!(result.discarded.len(), messages.len());
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
    fn prepare_for_summarization_bounds_single_jump_past_context_window() {
        use meerkat_core::types::ToolResult;

        let config = CompactionConfig {
            // Production scaling uses 4/5 of the active model context. A
            // 100k threshold gives this test a 25k raw source budget.
            auto_compact_threshold: 100_000,
            ..make_config()
        };
        let c = DefaultCompactor::new(config);
        let oversized = "{\"dense\":\"value\"}".repeat(100_000);
        let messages = vec![
            Message::User(UserMessage::text("inspect the large result")),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tc_oversized".to_string(),
                vec![ContentBlock::Text { text: oversized }],
                false,
            )]),
        ];

        let prepared = c.prepare_for_summarization(&messages);
        assert_eq!(prepared.len(), 1);
        let Message::User(excerpt) = &prepared[0] else {
            panic!("oversized projection must become one provider-legal user excerpt");
        };
        let excerpt = excerpt.text_content();
        assert!(excerpt.contains("Bounded compaction source excerpt"));
        assert!(excerpt.contains("intentionally omitted"));
        assert!(excerpt.contains("middle of projected transcript omitted"));
        assert!(excerpt.contains("End of bounded excerpt"));
        assert!(
            excerpt.len() <= c.summarization_source_budget_bytes(),
            "bounded source was {} bytes for a {} byte budget",
            excerpt.len(),
            c.summarization_source_budget_bytes()
        );
        assert!(
            serde_json::to_vec(&prepared).unwrap().len()
                < 2 * c.summarization_source_budget_bytes(),
            "JSON escaping must retain at least half the model-context safety margin"
        );
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
    fn discarded_prior_compaction_summary_is_not_reindexed() {
        use meerkat_core::types::{MemoryIndexExclusion, MemoryIndexableContent};

        // Ask 3 regression: when a SECOND compaction discards the previous
        // compaction summary (a projection of already-indexed history), the
        // typed indexability decision carried to the memory store must be the
        // CompactionSummary exclusion — never Indexable. Re-indexing the
        // summary would promote the projection to source content.
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });

        // Transcript as rebuilt by a prior compaction: summary boundary
        // followed by later conversation turns.
        let first_pass = c.rebuild_history(
            &[
                Message::User(UserMessage::text("original turn")),
                Message::User(UserMessage::text("second turn")),
            ],
            "summary of original work",
        );
        let mut messages = first_pass.messages;
        messages.push(Message::User(UserMessage::text("post-compaction turn 1")));
        messages.push(Message::User(UserMessage::text("post-compaction turn 2")));

        let result = c.rebuild_history(&messages, "summary of everything");

        // The prior summary is discarded together with the older turns.
        let discarded_summary = result
            .discarded
            .iter()
            .find(|discard| {
                matches!(
                    &discard.message,
                    Message::User(user) if user.transcript_role.is_compaction_summary()
                )
            })
            .expect("prior compaction summary must be in the discard set");
        assert_eq!(
            discarded_summary.message.indexable_content(),
            MemoryIndexableContent::Excluded(MemoryIndexExclusion::CompactionSummary),
            "discarded prior summary must carry the typed exclusion, not re-index"
        );

        // Ordinary discarded conversation stays indexable.
        let discarded_turn = result
            .discarded
            .iter()
            .find(|discard| {
                matches!(
                    &discard.message,
                    Message::User(user) if user.transcript_role.is_conversational()
                )
            })
            .expect("a conversational turn is also discarded");
        assert!(
            discarded_turn.message.indexable_content().is_indexable(),
            "conversational discards remain indexable"
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

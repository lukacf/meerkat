//! DefaultCompactor — provider-agnostic context compaction implementation.
//!
//! Gated behind the `session-compaction` feature.

use meerkat_core::compact::{
    COMPACTION_SUMMARY_PREFIX, CompactionConfig, CompactionContext, CompactionResult,
    CompactionSummary, Compactor, SummarizationSource, SummarizationSourceBound,
    SummarizationSourceBudget,
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

Images and videos shown as bracketed placeholders have their content omitted from this summary request. Their absence here says nothing about what the agent could see. Do not infer the agent's ability to see them from these placeholders, and do not invent their contents; keep what the surrounding recorded text says about them and any references needed to act on them.

Be concise and structured. Prioritize information the next context needs to act, not narrate.";

const RETAINED_HISTORY_BUDGET_DENOMINATOR: u64 = 2;
const MIN_RETAINED_HISTORY_BUDGET_BYTES: u64 = 4 * 1024;
/// The smallest a trimmed text is cut to while bounding a summarization
/// source; below it, the next role is trimmed instead.
const MIN_TRIMMED_TEXT_BYTES: usize = 512;
const TRIMMED_TEXT_MARKER: &str = "\n[... trimmed for the compaction source ...]\n";

/// Default compaction strategy implementation.
pub struct DefaultCompactor {
    config: CompactionConfig,
}

impl DefaultCompactor {
    /// Create a new compactor with the given configuration.
    pub fn new(config: CompactionConfig) -> Self {
        Self { config }
    }

    /// The serialized-byte budget of the summarization source: `budget`, the
    /// summarizing model's, or this compactor's threshold rule when that
    /// model declares no context window.
    fn summarization_source_budget_bytes(
        &self,
        budget: Option<SummarizationSourceBudget>,
    ) -> usize {
        let budget = budget.unwrap_or_else(|| {
            SummarizationSourceBudget::from_compaction_threshold(self.config.auto_compact_threshold)
        });
        usize::try_from(budget.bytes()).unwrap_or(usize::MAX)
    }

    fn retained_history_budget_bytes(
        &self,
        pressure: Option<meerkat_core::ProviderRequestPressure>,
    ) -> usize {
        let mut budget = self
            .config
            .auto_compact_threshold
            .div_ceil(RETAINED_HISTORY_BUDGET_DENOMINATOR)
            .max(MIN_RETAINED_HISTORY_BUDGET_BYTES);
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

    /// Request bytes of one blob-backed image when the exact provider request
    /// has crossed the byte trigger: the measured body beyond the serialized
    /// rows, shared evenly by the blob images. Zero below the trigger and
    /// without an exact witness.
    ///
    /// A blob image is a reference in its row and a full payload in the
    /// hydrated request. Above the trigger the payloads dominate the body, so
    /// the share is their size within the few kilobytes tool definitions and
    /// framing add.
    fn crossing_blob_image_bytes(
        &self,
        messages: &[Message],
        pressure: Option<meerkat_core::ProviderRequestPressure>,
    ) -> usize {
        let Some(pressure) = pressure.filter(|pressure| {
            pressure
                .trigger_threshold(self.config.max_request_bytes)
                .is_some_and(|threshold| pressure.encoded_bytes >= threshold)
        }) else {
            return 0;
        };
        let images = messages.iter().map(blob_images).sum::<usize>();
        let Some(images) = u64::try_from(images).ok().filter(|images| *images > 0) else {
            return 0;
        };
        let rows = messages
            .iter()
            .filter_map(|message| serde_json::to_vec(message).ok())
            .fold(0u64, |total, encoded| {
                total.saturating_add(u64::try_from(encoded.len()).unwrap_or(u64::MAX))
            });
        usize::try_from(pressure.encoded_bytes.saturating_sub(rows) / images).unwrap_or(usize::MAX)
    }
}

/// Blob-backed image blocks in one message: references in the row, full
/// payloads once the request is hydrated.
fn blob_images(message: &Message) -> usize {
    fn count(blocks: &[ContentBlock]) -> usize {
        blocks
            .iter()
            .filter(|block| {
                matches!(
                    block,
                    ContentBlock::Image {
                        data: meerkat_core::types::ImageData::Blob { .. },
                        ..
                    }
                )
            })
            .count()
    }
    match message {
        Message::User(user) => count(&user.content),
        Message::ToolResults { results, .. } => {
            results.iter().map(|result| count(&result.content)).sum()
        }
        Message::SystemNotice(notice) => notice
            .blocks
            .iter()
            .map(|block| match block {
                meerkat_core::types::SystemNoticeBlock::Comms { content, .. }
                | meerkat_core::types::SystemNoticeBlock::ExternalEvent { content, .. }
                | meerkat_core::types::SystemNoticeBlock::RuntimeNotice { content, .. } => {
                    count(content)
                }
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

/// Project media blocks to text placeholders for the summarization LLM input.
///
/// This projection is intentionally text-only so summarization remains compatible
/// with providers/models that cannot consume every retained media payload. It
/// must not be used for rebuilt active history, which preserves typed media/blob
/// references verbatim.
///
/// A placeholder names the media type and says only that the content is
/// omitted from this summary request. It says nothing about what the agent
/// could see, and `COMPACTION_PROMPT` tells the summarizer so.
fn project_media_for_summarization(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Image { media_type, .. } => ContentBlock::Text {
                text: format!("[image ({media_type}) omitted from this summary request]"),
            },
            ContentBlock::Video { media_type, .. } => ContentBlock::Text {
                text: format!("[video ({media_type}) omitted from this summary request]"),
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
                        // Summarizer input only: lineage and occurrence
                        // identity are not dialogue.
                        assistant_message_id: None,
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

/// What a bounding pass trims, in order: large tool results go first, then
/// the other context (System rows, assistant text and tool arguments), and
/// user rows only last.
#[derive(Clone, Copy, Debug)]
enum TrimRole {
    ToolResults,
    Context,
    User,
}

impl TrimRole {
    fn stage(self) -> meerkat_core::compact::SummarizationTrimStage {
        match self {
            Self::ToolResults => meerkat_core::compact::SummarizationTrimStage::ToolResults,
            Self::Context => meerkat_core::compact::SummarizationTrimStage::Context,
            Self::User => meerkat_core::compact::SummarizationTrimStage::UserRequests,
        }
    }
}

fn serialized_len(messages: &[Message]) -> Result<usize, serde_json::Error> {
    serde_json::to_vec(messages).map(|encoded| encoded.len())
}

/// Cut `text` to at most `cap` bytes, keeping its head and tail around an
/// omission marker.
fn trim_text(text: &mut String, cap: usize) {
    if text.len() <= cap {
        return;
    }
    let keep = cap.saturating_sub(TRIMMED_TEXT_MARKER.len());
    let mut head_end = keep / 2;
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len().saturating_sub(keep - keep / 2);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    *text = format!(
        "{}{TRIMMED_TEXT_MARKER}{}",
        &text[..head_end],
        &text[tail_start..]
    );
}

fn trim_blocks(blocks: &mut [ContentBlock], cap: usize) {
    for block in blocks {
        if let ContentBlock::Text { text } = block {
            trim_text(text, cap);
        }
    }
}

/// Trim every text of `role` in `messages` to at most `cap` bytes. Message
/// and block structure is kept, so the source stays a provider-legal
/// sequence.
fn trim_role(messages: &mut [Message], role: TrimRole, cap: usize) {
    for message in messages {
        match (role, message) {
            (TrimRole::ToolResults, Message::ToolResults { results, .. }) => {
                for result in results {
                    trim_blocks(&mut result.content, cap);
                }
            }
            (TrimRole::Context, Message::System(system)) => trim_text(&mut system.content, cap),
            (TrimRole::Context, Message::BlockAssistant(assistant)) => {
                for block in &mut assistant.blocks {
                    match block {
                        AssistantBlock::Text { text, .. }
                        | AssistantBlock::Transcript { text, .. } => trim_text(text, cap),
                        AssistantBlock::ToolUse { args, .. } if args.get().len() > cap => {
                            let omitted = serde_json::json!({
                                "arguments_trimmed_for_compaction_source_bytes": args.get().len()
                            });
                            if let Ok(replacement) =
                                serde_json::value::RawValue::from_string(omitted.to_string())
                            {
                                *args = replacement;
                            }
                        }
                        _ => {}
                    }
                }
            }
            (TrimRole::User, Message::User(user)) => trim_blocks(&mut user.content, cap),
            _ => {}
        }
    }
}

/// The largest text `role` holds in `messages`, in bytes.
fn largest_text(messages: &[Message], role: TrimRole) -> usize {
    let blocks = |blocks: &[ContentBlock]| {
        blocks
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => text.len(),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    };
    messages
        .iter()
        .map(|message| match (role, message) {
            (TrimRole::ToolResults, Message::ToolResults { results, .. }) => results
                .iter()
                .map(|result| blocks(&result.content))
                .max()
                .unwrap_or(0),
            (TrimRole::Context, Message::System(system)) => system.content.len(),
            (TrimRole::Context, Message::BlockAssistant(assistant)) => assistant
                .blocks
                .iter()
                .map(|block| match block {
                    AssistantBlock::Text { text, .. } | AssistantBlock::Transcript { text, .. } => {
                        text.len()
                    }
                    AssistantBlock::ToolUse { args, .. } => args.get().len(),
                    _ => 0,
                })
                .max()
                .unwrap_or(0),
            (TrimRole::User, Message::User(user)) => blocks(&user.content),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

/// The note opening the last-resort source, naming how many of the
/// history's user requests the source carries.
fn requests_only_note(kept: usize, of: usize) -> String {
    format!(
        "[Bounded compaction source. The projected transcript is larger than the safe summarization window even with every tool result and message trimmed. Only the newest {kept} of the {of} user requests are kept, in their original order; everything else is omitted.]\n"
    )
}

/// The user requests of `messages`, newest first.
fn user_requests_newest_first(
    messages: &[Message],
) -> impl Iterator<Item = &meerkat_core::types::UserMessage> {
    messages.iter().rev().filter_map(|message| match message {
        Message::User(user) if !user.transcript_role.is_compaction_summary() => Some(user),
        _ => None,
    })
}

/// The last-resort source: only the user requests, newest first while the
/// source's final serialized size (envelope and JSON escaping included)
/// stays within `budget`, in transcript order. The newest request is always
/// kept, cut further if it alone does not fit.
fn requests_only_source(messages: &[Message], budget: usize) -> SummarizationSource {
    let of = user_requests_newest_first(messages).count();
    let source = |newest_first: &[String]| {
        let lines: String = newest_first.iter().rev().map(String::as_str).collect();
        vec![Message::User(meerkat_core::types::UserMessage::text(
            format!("{}{lines}", requests_only_note(newest_first.len(), of)),
        ))]
    };
    let fits = |candidate: &[Message]| serialized_len(candidate).is_ok_and(|len| len <= budget);
    // The exact source that was measured is the one returned: a rebuilt
    // message carries a new timestamp, whose serialized width varies.
    let mut kept: Vec<String> = Vec::new();
    let mut checked = source(&kept);
    for user in user_requests_newest_first(messages) {
        let mut text = user.text_content();
        trim_text(&mut text, MIN_TRIMMED_TEXT_BYTES);
        let mut candidate = kept.clone();
        candidate.push(format!("- {text}\n"));
        let built = source(&candidate);
        if fits(&built) {
            kept = candidate;
            checked = built;
            continue;
        }
        if kept.is_empty() {
            // The newest request alone does not fit: cut it until it does
            // (down to the omission marker).
            let mut cap = MIN_TRIMMED_TEXT_BYTES;
            loop {
                cap /= 2;
                trim_text(&mut text, cap);
                let newest = vec![format!("- {text}\n")];
                let built = source(&newest);
                if fits(&built) || cap <= TRIMMED_TEXT_MARKER.len() {
                    kept = newest;
                    checked = built;
                    break;
                }
            }
        }
        break;
    }
    SummarizationSource {
        messages: checked,
        bound: SummarizationSourceBound::RequestsOnly {
            kept: kept.len(),
            of,
        },
    }
}

/// Bound an oversized typed projection to `budget` serialized bytes, by
/// role: large tool results are trimmed first, then the other context, and
/// user rows only last, so every user request (including one in the middle
/// of the transcript) still reaches the summarizer. Messages are never
/// dropped from the typed projection; only a source that cannot fit even
/// fully trimmed falls back to the user requests alone. The original
/// session messages are never mutated.
fn bound_summarization_projection(messages: Vec<Message>, budget: usize) -> SummarizationSource {
    let projected = serialized_len(&messages);
    // Unreachable for in-memory messages: every `Message` serializes
    // infallibly (string-keyed maps, raw JSON that was validated when it was
    // built, non-finite floats written as null). The branch is defence in
    // depth: debug builds flag it, and a release build degrades to a note
    // that carries none of the user requests, which `RequestsOnly { kept: 0 }`
    // states exactly.
    debug_assert!(
        projected.is_ok(),
        "a compaction source projection always serializes"
    );
    let projected_bytes = match projected {
        Ok(projected_bytes) => projected_bytes,
        Err(error) => {
            return SummarizationSource {
                messages: vec![Message::User(meerkat_core::types::UserMessage::text(
                    format!(
                        "[Compaction source projection could not be serialized: {error}. Continue with a mechanical handoff summary.]"
                    ),
                ))],
                bound: SummarizationSourceBound::RequestsOnly {
                    kept: 0,
                    of: user_requests_newest_first(&messages).count(),
                },
            };
        }
    };
    if projected_bytes <= budget {
        return SummarizationSource::whole(messages);
    }
    let user_rows = messages
        .iter()
        .filter(|message| matches!(message, Message::User(_)))
        .count();
    let mut bounded = messages;
    for role in [TrimRole::ToolResults, TrimRole::Context, TrimRole::User] {
        let mut cap = largest_text(&bounded, role);
        while cap > MIN_TRIMMED_TEXT_BYTES {
            cap = (cap / 2).max(MIN_TRIMMED_TEXT_BYTES);
            trim_role(&mut bounded, role, cap);
            if let Ok(bounded_bytes) = serialized_len(&bounded)
                && bounded_bytes <= budget
            {
                tracing::warn!(
                    projected_bytes,
                    bounded_bytes,
                    budget_bytes = budget,
                    messages = bounded.len(),
                    user_rows,
                    trimmed_through = ?role,
                    "bounded oversized compaction summarization source by trimming"
                );
                return SummarizationSource {
                    messages: bounded,
                    bound: SummarizationSourceBound::Trimmed {
                        through: role.stage(),
                    },
                };
            }
        }
    }
    tracing::warn!(
        projected_bytes,
        budget_bytes = budget,
        messages = bounded.len(),
        user_rows,
        "compaction summarization source exceeds its budget even fully trimmed; summarizing the user requests only"
    );
    requests_only_source(&bounded, budget)
}

impl Compactor for DefaultCompactor {
    fn transcript_history_retention(&self) -> meerkat_core::TranscriptHistoryRetention {
        self.config.transcript_history_retention()
    }

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
        // providers actually enforce: the 2026-07-29 production incident grew
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
        // The projected input of this request: the provider's own count for
        // the last answered request plus the transcript estimate of only the
        // rows appended since. `last_input_tokens` lags by exactly those rows
        // (one tool result can be most of a window), and the two estimates
        // above count text bytes, which under-reads dense content such as
        // JSON listings; the projection keeps the exact count for everything
        // the provider counted and estimates only the growth.
        let projection = ctx.projected_input_tokens;
        let projection_trigger = projection.is_some_and(|projection| {
            projection.projected_tokens() >= self.config.auto_compact_threshold
        });
        // A request that did not fit: the provider refused it for the model's
        // context (it reported no usage, so no measure above moved since it
        // was sent), or it carried the open run's own tool results shortened
        // for that request only (the transcript keeps them whole). Either way,
        // without compacting, the next request carries the same content again.
        // The agent shows the fact only at a check whose compaction covers the
        // whole transcript.
        let context_refused = ctx.last_context_refusal_boundary_index.is_some();

        // Cadence is a cost policy, not recovery authority. It may suppress a
        // provider-reported high-water mark while the current transcript is
        // still below both live pressure thresholds, but it must never veto a
        // crossing that compacting can recover. One large tool result can make
        // the very next request impossible, including when the previous
        // compaction happened fewer than `min_turns_between_compactions`
        // boundaries ago.
        //
        // The whole-request forecast is deliberately NOT capacity recovery.
        // Part of what it measures - tool definitions and the output reserve -
        // survives compaction untouched, so a forecast crossing that cadence
        // could not veto would compact on every boundary forever without
        // reducing the crossing.
        //
        // A history crossing is recoverable only while the last rewrite got
        // the history under the threshold. A rewrite keeps the System rows,
        // the in-flight request and the recent turns, and adds a summary; when
        // what it left is still at or above the threshold, that floor crosses
        // on its own, and compacting again pays for a summary that recovers
        // nothing. Such a crossing waits for the cadence like the forecast,
        // unless the request no longer fits the model window and what arrived
        // since that rewrite is at least the overage: that growth is the only
        // content a rewrite can still remove. Without a request budget fact
        // (a model that declares no context window, or a request the budget
        // could not measure) there is no overage, so the escape never applies:
        // with the floor over the threshold every history crossing waits for
        // the cadence however large the growth, and only the request-byte
        // crossing still bypasses it.
        //
        // The byte crossing keeps its bypass. On the exact path the rebuilt
        // request must shrink and fit its cap, or the turn fails as
        // irreducible. On the estimated path a rewrite keeps the System rows,
        // the summary and at most two retention budgets of verbatim rows, so
        // its bytes stay under a request-byte threshold measured in megabytes
        // unless the System rows alone approach the provider's request cap,
        // which also puts the history far over the token threshold and the
        // request over the model window.
        //
        // A projection crossing is recoverable when the growth since the last
        // answered request made it: that request presented under the
        // threshold, and what was appended since is what a rewrite removes.
        // When the anchor alone is at or over the threshold (the answered
        // request already held the newest round a rewrite keeps), the
        // projection is a provider-reported high-water mark like
        // `last_input_tokens`, and waits for the cadence. A committed rewrite
        // drops the anchor, so the next answer re-anchors with what the
        // rewrite left. The floor rule of a history crossing holds too: when
        // the last rewrite left the history at or above the threshold, a
        // growth crossing waits for the cadence unless that growth covers the
        // window overage.
        //
        // A request that did not fit is recovery by definition, so it bypasses
        // the threshold and the cadence. The attempt it causes clears it,
        // committed or not, so one refusal or bound costs one attempt.
        let history_floor_crosses = ctx
            .last_rewrite_history_tokens
            .is_some_and(|left| left >= self.config.auto_compact_threshold);
        let removable_growth_covers_overage = ctx
            .request_context_budget
            .as_ref()
            .filter(|budget| budget.state != meerkat_core::ContextBudgetState::Within)
            .zip(ctx.last_rewrite_history_tokens)
            .is_some_and(|(budget, left)| {
                ctx.estimated_history_tokens.saturating_sub(left) >= budget.overage_tokens
            });
        let history_recovers =
            history_trigger && (!history_floor_crosses || removable_growth_covers_overage);
        if history_trigger
            && !history_recovers
            && ctx
                .last_compaction_boundary_index
                .is_some_and(|last| ctx.session_boundary_index == last.saturating_add(1))
        {
            tracing::warn!(
                last_rewrite_history_tokens = ctx.last_rewrite_history_tokens,
                estimated_history_tokens = ctx.estimated_history_tokens,
                threshold = self.config.auto_compact_threshold,
                min_turns_between_compactions = self.config.min_turns_between_compactions,
                "compaction cannot bring the history under its threshold: the last rewrite left \
                 the history at or above it, so the System rows, the kept request, the recent \
                 turns and the summary cross it on their own; history crossings now wait for the \
                 compaction cadence. Raise auto_compact_threshold above that floor, omit it so it \
                 derives from the model window, or shrink the System rows"
            );
        }
        let projection_recovers = projection_trigger
            && projection.is_some_and(|projection| {
                projection.anchor_tokens < self.config.auto_compact_threshold
            })
            && (!history_floor_crosses || removable_growth_covers_overage);
        let requires_capacity_recovery =
            history_recovers || byte_trigger || projection_recovers || context_refused;
        if !requires_capacity_recovery
            && let Some(last) = ctx.last_compaction_boundary_index
            && ctx.session_boundary_index.saturating_sub(last)
                < u64::from(self.config.min_turns_between_compactions)
        {
            return false;
        }
        let fires = input_trigger
            || history_trigger
            || byte_trigger
            || forecast_trigger
            || projection_trigger
            || context_refused;
        if fires {
            tracing::trace!(
                input_tokens = ctx.last_input_tokens,
                estimated_history_tokens = ctx.estimated_history_tokens,
                projected_input_tokens = projection.map(|projection| projection.projected_tokens()),
                projection_anchor_tokens = projection.map(|projection| projection.anchor_tokens),
                last_context_refusal_boundary_index = ctx.last_context_refusal_boundary_index,
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
                branch = if context_refused {
                    "context_refusal"
                } else if input_trigger {
                    "last_input_tokens"
                } else if history_trigger {
                    "estimated_history_tokens_fallback"
                } else if byte_trigger && ctx.provider_request_pressure.is_some() {
                    "provider_request_bytes"
                } else if byte_trigger {
                    "estimated_request_bytes"
                } else if projection_trigger {
                    "projected_input_tokens"
                } else {
                    "request_context_budget_forecast"
                },
                "compaction trigger fired",
            );
        }
        fires
    }

    fn prepare_for_summarization(
        &self,
        messages: &[Message],
        budget: Option<SummarizationSourceBudget>,
    ) -> SummarizationSource {
        let messages = materialize_latest_system_prompt_versions(messages);
        bound_summarization_projection(
            project_messages_for_summarization(&messages),
            self.summarization_source_budget_bytes(budget),
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
        // The retention budget bounds the verbatim turns only. System rows
        // are kept whatever their size and are not part of what the rewrite
        // can shrink, so they never count against it: counting them let a
        // System-heavy session retain no turn at all.
        let retention_budget = self.retained_history_budget_bytes(pressure);
        // When this rewrite answers a request-byte crossing, a blob image
        // costs its hydrated payload, as an inline one already does. Counted
        // as a reference, every recent image turn fits the budget, the rewrite
        // keeps the payloads that crossed, and the next turn crosses again.
        let blob_image_bytes = self.crossing_blob_image_bytes(messages, pressure);
        let turn_bytes = |range: std::ops::Range<usize>| {
            messages[range]
                .iter()
                .filter(|message| !matches!(message, Message::System(_)))
                .try_fold(0usize, |total, message| {
                    serde_json::to_vec(message).ok().and_then(|encoded| {
                        total
                            .checked_add(encoded.len())?
                            .checked_add(blob_images(message).checked_mul(blob_image_bytes)?)
                    })
                })
                .unwrap_or(usize::MAX)
        };
        let retain_from = loop {
            let candidate = if retain_turn_count == 0 {
                messages.len()
            } else {
                turn_starts[turn_starts.len() - retain_turn_count]
            };
            if turn_bytes(candidate..messages.len()) <= retention_budget || retain_turn_count == 0 {
                break candidate;
            }
            retain_turn_count -= 1;
        };
        // The in-flight turn's request bundle (its conversational user row
        // plus the injected context glued before it) stays verbatim even
        // when the whole turn does not fit: a mid-turn rewrite must not drop
        // the request the turn is answering. Only the rest of that turn is
        // summarized. A bundle that alone exceeds the retention budget
        // (a large request, or a short one behind large injected context)
        // cannot be resent as is; it is summarized instead, and the
        // summarizer source keeps user rows to the last. A newest turn that
        // already has its final answer (a response with no tool call) is
        // not in flight: inside a run the agent hands over only the rows
        // before that run, whose newest turn is an earlier, answered one. It
        // is kept whole or folded whole, never left as a request that reads
        // as pending.
        let newest_turn_answered = turn_starts.last().is_some_and(|start| {
            messages[*start..].iter().any(|message| {
                matches!(message, Message::BlockAssistant(assistant) if !assistant.has_tool_calls())
            })
        });
        let pinned_request = turn_starts
            .last()
            .copied()
            .filter(|start| *start < retain_from && !newest_turn_answered)
            .and_then(|start| {
                let request = (start..messages.len()).find(|offset| {
                    matches!(
                        &messages[*offset],
                        Message::User(user) if user.transcript_role.is_conversational()
                    )
                })?;
                (turn_bytes(start..request + 1) <= retention_budget).then_some(start..request + 1)
            });
        let keep = |source_offset: usize, message: &Message| {
            !superseded_system_prompts.contains(&source_offset)
                && (matches!(message, Message::System(_))
                    || source_offset >= retain_from
                    || pinned_request
                        .as_ref()
                        .is_some_and(|request| request.contains(&source_offset)))
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
                (!keep(source_offset, message)).then_some(source_offset)
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
            if keep(source_offset, message) {
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

        if retain_turn_count == 0 {
            tracing::warn!(
                source_messages = messages.len(),
                retained = retained.len(),
                discarded = discarded.len(),
                retained_turns = 0,
                in_flight_request_kept = pinned_request.is_some(),
                retention_budget_bytes = retention_budget,
                "compaction retained no complete turn"
            );
        }
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
            history_retained_rewrites: 4,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
        };
        assert!(
            c.should_compact(&ctx),
            "an already-oversized history must recover even immediately after a prior compaction"
        );
    }

    /// A history crossing is capacity recovery only while compacting can get
    /// under the threshold. When the last rewrite already left the history at
    /// or above it, the floor crosses on its own: the crossing waits for the
    /// cadence like an ordinary trigger instead of paying for a summary at
    /// every boundary, and compaction still runs once the cadence allows it.
    #[test]
    fn history_crossing_waits_for_the_cadence_when_the_last_rewrite_left_it_over() {
        let c = DefaultCompactor::new(make_config());
        let just_after = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 100,
            estimated_history_tokens: 105_000,
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: Some(5),
            session_boundary_index: 6,
            last_rewrite_history_tokens: Some(101_000),
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
        };
        assert!(
            !c.should_compact(&just_after),
            "a rewrite that left the history over the threshold cannot recover it at the next boundary"
        );
        let after_cadence = CompactionContext {
            session_boundary_index: 8,
            ..just_after
        };
        assert!(
            c.should_compact(&after_cadence),
            "the crossing still compacts once the cadence allows it"
        );
    }

    /// Control: a rewrite that got the history under the threshold keeps the
    /// bypass, so new content that crosses again is recovered at once.
    #[test]
    fn history_crossing_bypasses_the_cadence_when_the_last_rewrite_got_under() {
        let c = DefaultCompactor::new(make_config());
        let ctx = CompactionContext {
            last_input_tokens: 50_000,
            message_count: 100,
            estimated_history_tokens: 105_000,
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: Some(5),
            session_boundary_index: 6,
            last_rewrite_history_tokens: Some(40_000),
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
        };
        assert!(c.should_compact(&ctx));
    }

    /// A context whose measures stay under the 100k threshold of
    /// `make_config` except, possibly, the projection of this request's input:
    /// `anchor` presented tokens of the last answered request plus `appended`
    /// estimated tokens of the rows since, one boundary after the last
    /// compaction unless `boundary` says otherwise. No last input tokens, as
    /// for an agent restored from its session store.
    fn projected_context(anchor: u64, appended: u64, boundary: u64) -> CompactionContext {
        CompactionContext {
            last_input_tokens: 0,
            message_count: 10,
            estimated_history_tokens: 40_000,
            estimated_request_bytes: 0,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: Some(5),
            session_boundary_index: boundary,
            last_rewrite_history_tokens: None,
            projected_input_tokens: Some(meerkat_core::InputTokenProjection {
                anchor_tokens: anchor,
                appended_tokens: appended,
            }),
            last_context_refusal_boundary_index: None,
        }
    }

    /// The reported shape: the last answered request presented 90k tokens,
    /// under the threshold, and what was appended since (one tool result)
    /// projects the next request over it. That growth is what a rewrite can
    /// remove, so the crossing is capacity recovery: the cadence cannot veto
    /// it, one boundary after the last compaction included.
    #[test]
    fn projection_crossing_from_growth_bypasses_the_cadence() {
        let c = DefaultCompactor::new(make_config());
        assert!(c.should_compact(&projected_context(90_000, 15_000, 6)));
        assert!(
            c.should_compact(&projected_context(90_000, 10_000, 6)),
            "the threshold itself is a crossing"
        );
        assert!(
            !c.should_compact(&projected_context(90_000, 9_999, 6)),
            "a projection under the threshold does not fire"
        );
    }

    /// When the anchor alone is at or over the threshold (the request a
    /// rewrite cannot shrink below, its newest round included, was already
    /// answered), the projection is a provider-reported high-water mark the
    /// cadence may hold back, as the last input tokens are: compacting again
    /// at once could not get under it.
    #[test]
    fn projection_from_the_anchor_alone_waits_for_the_cadence() {
        let c = DefaultCompactor::new(make_config());
        assert!(
            !c.should_compact(&projected_context(101_000, 1_000, 6)),
            "one boundary after the last compaction the cadence holds"
        );
        assert!(
            c.should_compact(&projected_context(101_000, 1_000, 8)),
            "once the cadence allows it the projection fires"
        );
        let first = CompactionContext {
            last_compaction_boundary_index: None,
            ..projected_context(101_000, 0, 1)
        };
        assert!(
            c.should_compact(&first),
            "a restored agent's first request compacts on its persisted anchor"
        );
    }

    /// The floor rule of a history crossing holds for a projection crossing
    /// too: when the last rewrite left the history at or above the threshold,
    /// compacting again cannot get under it, so a crossing the growth made
    /// waits for the cadence instead of paying for a summary at every
    /// boundary. A rewrite that got under the threshold keeps the bypass.
    #[test]
    fn projection_crossing_waits_for_the_cadence_when_the_last_rewrite_left_the_history_over() {
        let c = DefaultCompactor::new(make_config());
        let floor_over = CompactionContext {
            last_rewrite_history_tokens: Some(100_000),
            ..projected_context(90_000, 15_000, 6)
        };
        assert!(
            !c.should_compact(&floor_over),
            "one boundary after a rewrite that left the history over the threshold the cadence holds"
        );
        let after_cadence = CompactionContext {
            session_boundary_index: 8,
            ..floor_over
        };
        assert!(
            c.should_compact(&after_cadence),
            "once the cadence allows it the projection fires"
        );
        let floor_under = CompactionContext {
            last_rewrite_history_tokens: Some(99_999),
            ..projected_context(90_000, 15_000, 6)
        };
        assert!(
            c.should_compact(&floor_under),
            "a rewrite that got under the threshold keeps the bypass"
        );
    }

    /// Without an anchor (before the first answered request, after a
    /// rewrite, after a model or provider change, or for a session last
    /// written before the anchor existed) the existing triggers decide alone.
    #[test]
    fn no_anchor_keeps_the_existing_triggers() {
        let c = DefaultCompactor::new(make_config());
        let unanchored = CompactionContext {
            projected_input_tokens: None,
            ..projected_context(90_000, 15_000, 6)
        };
        assert!(!c.should_compact(&unanchored));
        let history = CompactionContext {
            estimated_history_tokens: 100_000,
            ..unanchored
        };
        assert!(
            c.should_compact(&history),
            "a history crossing still recovers at once"
        );
    }

    /// A request the provider refused for context forces one capacity
    /// compaction at the next check, whatever the measures and the cadence
    /// say: the refused request reported no usage, so no measure moved, and
    /// without the refusal fact the next run sends the same request again.
    #[test]
    fn a_context_refusal_forces_one_capacity_compaction() {
        let c = DefaultCompactor::new(make_config());
        let quiet = CompactionContext {
            projected_input_tokens: None,
            ..projected_context(10_000, 0, 6)
        };
        assert!(!c.should_compact(&quiet));
        let refused = CompactionContext {
            last_context_refusal_boundary_index: Some(5),
            ..quiet
        };
        assert!(
            c.should_compact(&refused),
            "the refusal bypasses the thresholds and the cadence"
        );
        let first_ever = CompactionContext {
            last_compaction_boundary_index: None,
            session_boundary_index: 0,
            ..refused
        };
        assert!(
            !c.should_compact(&first_ever),
            "the first-ever boundary still never compacts"
        );
    }

    /// With the floor over the threshold, the bypass returns only when the
    /// request no longer fits the model window and what arrived since the
    /// last rewrite covers the overage: that growth is the only content a
    /// rewrite can still remove. Without such growth the cadence holds.
    #[test]
    fn floor_over_threshold_bypasses_only_for_growth_that_covers_the_window_overage() {
        let threshold = 6_000;
        let compactor = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: threshold,
            ..make_config()
        });
        let profile = windowed_profile_witness(10_000);
        let messages = vec![Message::User(UserMessage::text("t".repeat(80_000)))];
        let budget =
            meerkat_core::context_budget_fact_for_messages(&messages, &[], 2_048, &profile)
                .expect("declared window must classify");
        assert_ne!(
            budget.state,
            meerkat_core::ContextBudgetState::Within,
            "the request must be projected beyond the window"
        );
        let overage = budget.overage_tokens;
        assert!(overage > 0);
        let ctx = meerkat_core::agent::compact::build_compaction_context(
            &messages,
            0,
            None,
            Some(budget),
            None,
            Some(5),
            6,
        );
        let history = ctx.estimated_history_tokens;
        assert!(
            history.saturating_sub(overage) >= threshold,
            "every case below must keep the floor over the threshold"
        );

        let no_growth = CompactionContext {
            last_rewrite_history_tokens: Some(history),
            ..ctx.clone()
        };
        assert!(
            !compactor.should_compact(&no_growth),
            "nothing removable arrived since the last rewrite: the cadence holds"
        );
        let short_of_overage = CompactionContext {
            last_rewrite_history_tokens: Some(history - overage + 1),
            ..ctx.clone()
        };
        assert!(
            !compactor.should_compact(&short_of_overage),
            "growth below the overage cannot make the request fit"
        );
        let covers_overage = CompactionContext {
            last_rewrite_history_tokens: Some(history - overage),
            ..ctx
        };
        assert!(
            compactor.should_compact(&covers_overage),
            "growth that covers the overage is removable: the bypass fires"
        );
    }

    /// The byte crossing keeps its bypass whatever the last rewrite left.
    #[test]
    fn byte_crossing_keeps_its_bypass_after_a_rewrite_that_left_the_history_over() {
        let c = DefaultCompactor::new(CompactionConfig {
            max_request_bytes: Some(9_000_000),
            ..make_config()
        });
        let ctx = CompactionContext {
            last_input_tokens: 0,
            message_count: 100,
            estimated_history_tokens: 105_000,
            estimated_request_bytes: 8_000_000,
            request_context_budget: None,
            provider_request_pressure: None,
            last_compaction_boundary_index: Some(5),
            session_boundary_index: 6,
            last_rewrite_history_tokens: Some(101_000),
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
        };
        assert!(c.should_compact(&ctx));
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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
            None,
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
            None,
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
            None,
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
            last_rewrite_history_tokens: None,
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
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

        let summary_input = c.prepare_for_summarization(&messages, None).messages;
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

    /// The rows before an open run, as the agent hands them over inside the
    /// run: one exchange, already answered. Nothing is in flight, so no
    /// request is pinned: the exchange folds whole and the rebuild shrinks.
    #[test]
    fn an_answered_newest_turn_folds_whole() {
        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::User(UserMessage::text("hi")),
            Message::BlockAssistant(meerkat_core::types::BlockAssistantMessage::new(
                vec![meerkat_core::types::AssistantBlock::Text {
                    text: "hello".to_string(),
                    meta: None,
                }],
                meerkat_core::types::StopReason::EndTurn,
            )),
        ];
        let result = c.rebuild_history(&messages, "summary");
        assert_eq!(result.messages.len(), 1, "only the summary is left");
        assert_eq!(result.discarded.len(), 2, "the exchange folds whole");
    }

    /// An answered newest turn too large for the retention is folded whole:
    /// its request is not kept verbatim after the summary, where it would
    /// read as pending and could be acted on again.
    #[test]
    fn an_answered_newest_turn_over_the_retention_leaves_no_pending_request() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, ToolResult};

        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 100_000,
            recent_turn_budget: 4,
            ..make_config()
        });
        let text = |text: &str| {
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: text.to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
            ))
        };
        let messages = vec![
            Message::User(UserMessage::text("a")),
            text("a answered"),
            Message::User(UserMessage::text("b")),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tc_b".to_string(),
                    name: "inspect".to_string(),
                    args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tc_b".to_string(),
                vec![ContentBlock::Text {
                    text: "x".repeat(200_000),
                }],
                false,
            )]),
            text("b answered"),
        ];
        let result = c.rebuild_history(&messages, "summary");
        assert!(
            !result.messages.iter().any(
                |message| matches!(message, Message::User(user) if user.text_content() == "b")
            ),
            "the answered request is not left verbatim: {:?}",
            result.messages
        );
    }

    /// This is the compactor alone. The agent hands it a whole transcript,
    /// open request included, only at a run's first boundary, before the
    /// run's first response; inside a run, after its first response, it
    /// hands over only the rows before the run, whose newest turn is
    /// answered, and folds the run's own completed rounds itself
    /// (`agent::compact::compaction_scope`).
    #[test]
    fn test_rebuild_zero_budget_keeps_only_the_in_flight_request() {
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
        // No complete turn is retained; the in-flight request stays verbatim.
        assert_eq!(result.messages.len(), 2);
        assert!(matches!(
            &result.messages[1],
            Message::User(user) if user.text_content() == "c"
        ));
        assert_eq!(result.discarded.len(), 2);
    }

    /// This is the compactor alone. The agent hands it a whole transcript,
    /// open request included, only at a run's first boundary, before the
    /// run's first response; inside a run, after its first response, it
    /// hands over only the rows before the run, whose newest turn is
    /// answered, and folds the run's own completed rounds itself
    /// (`agent::compact::compaction_scope`).
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
            2,
            "an individually oversized newest turn is summarized; only its request is retained verbatim"
        );
        assert_eq!(result.discarded.len(), messages.len() - 1);
        assert!(matches!(
            &result.messages[0],
            Message::User(user) if user.transcript_role.is_compaction_summary()
        ));
        assert!(matches!(
            &result.messages[1],
            Message::User(user) if user.text_content() == "inspect the large result"
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

    const CAMERA_PROVIDER_CAP: u64 = 9_000_000; // approximate_request_byte_cap(Anthropic)

    /// A conversational turn whose tool result carries two images, inline or
    /// blob-backed (the form compaction sees: the agent externalizes inline
    /// media before it compacts).
    fn camera_turn(round: usize, inline_base64_bytes: Option<usize>) -> Vec<Message> {
        let image = |camera: &str| match inline_base64_bytes {
            Some(bytes) => inline_image_block("image/jpeg", &"A".repeat(bytes)),
            None => blob_image_block("image/jpeg", &format!("{camera}-{round}")),
        };
        vec![
            Message::User(UserMessage::text(format!(
                "check the cameras, round {round}"
            ))),
            Message::tool_results(vec![meerkat_core::types::ToolResult::with_blocks(
                format!("tc_{round}"),
                vec![image("cam-a"), image("cam-b")],
                false,
            )]),
        ]
    }

    fn camera_blob_images(messages: &[Message]) -> u64 {
        messages
            .iter()
            .map(|message| match message {
                Message::ToolResults { results, .. } => results
                    .iter()
                    .flat_map(|result| result.content.iter())
                    .filter(|block| {
                        matches!(
                            block,
                            ContentBlock::Image {
                                data: ImageData::Blob { .. },
                                ..
                            }
                        )
                    })
                    .count() as u64,
                _ => 0,
            })
            .sum()
    }

    /// The provider-lowered request: the canonical rows plus every blob
    /// payload the execution hydration pass inlines.
    fn camera_request_bytes(messages: &[Message], blob_payload_bytes: u64) -> u64 {
        let rows: u64 = messages
            .iter()
            .map(|message| serde_json::to_vec(message).unwrap().len() as u64)
            .sum();
        rows + camera_blob_images(messages) * blob_payload_bytes
    }

    fn camera_context(request_bytes: u64, boundary: u64) -> CompactionContext {
        CompactionContext {
            last_input_tokens: 20_000,
            message_count: 12,
            estimated_history_tokens: 20_000,
            estimated_request_bytes: 4_096,
            request_context_budget: None,
            provider_request_pressure: Some(meerkat_core::ProviderRequestPressure::new(
                request_bytes,
                Some(CAMERA_PROVIDER_CAP),
            )),
            last_compaction_boundary_index: Some(boundary - 1),
            session_boundary_index: boundary,
            last_rewrite_history_tokens: Some(10_000),
            projected_input_tokens: None,
            last_context_refusal_boundary_index: None,
        }
    }

    fn camera_compactor() -> DefaultCompactor {
        DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 160_000,
            recent_turn_budget: 4,
            ..make_config()
        })
    }

    /// Every turn adds a tool result with two large camera images. A rewrite
    /// that answers the request-byte crossing must shed the image turns whose
    /// payloads crossed, as it does for inline media. It used to count a blob
    /// image as its reference, keep the four newest image turns, and let the
    /// next turn cross again: the session compacted on every turn.
    #[test]
    fn a_byte_triggered_rewrite_sheds_blob_backed_image_turns() {
        const IMAGE_BASE64_BYTES: u64 = 800_000; // a ~600 KB JPEG per snapshot
        let c = camera_compactor();
        let mut messages = vec![Message::System(SystemMessage::new("You watch the house."))];
        for round in 1..=5 {
            messages.extend(camera_turn(round, None));
        }
        let before = camera_request_bytes(&messages, IMAGE_BASE64_BYTES);
        assert!(
            c.should_compact(&camera_context(before, 10)),
            "ten images cross 4/5 of the cap one boundary after the last compaction"
        );
        let pressure =
            meerkat_core::ProviderRequestPressure::new(before, Some(CAMERA_PROVIDER_CAP));
        let rewrite = c.rebuild_history_under_pressure(&messages, "summary", Some(pressure));
        let mut next = rewrite.messages.clone();
        next.extend(camera_turn(6, None));
        let next_bytes = camera_request_bytes(&next, IMAGE_BASE64_BYTES);
        assert!(
            !c.should_compact(&camera_context(next_bytes, 12)),
            "the next turn compacts again: the rewrite kept {} blob images, and the next \
             request carries {next_bytes} bytes",
            camera_blob_images(&rewrite.messages)
        );

        // Control: the same images inline are shed the same way.
        let mut inline = vec![Message::System(SystemMessage::new("You watch the house."))];
        for round in 1..=5 {
            inline.extend(camera_turn(round, Some(IMAGE_BASE64_BYTES as usize)));
        }
        let inline_bytes = camera_request_bytes(&inline, IMAGE_BASE64_BYTES);
        let inline_pressure =
            meerkat_core::ProviderRequestPressure::new(inline_bytes, Some(CAMERA_PROVIDER_CAP));
        let mut inline_next = c
            .rebuild_history_under_pressure(&inline, "summary", Some(inline_pressure))
            .messages;
        inline_next.extend(camera_turn(6, Some(IMAGE_BASE64_BYTES as usize)));
        assert!(!c.should_compact(&camera_context(
            camera_request_bytes(&inline_next, IMAGE_BASE64_BYTES),
            12
        )));

        // Control: below the byte trigger (a token-triggered rewrite) a blob
        // image keeps costing its reference, and the recent turns stay.
        let calm = meerkat_core::ProviderRequestPressure::new(1_000_000, Some(CAMERA_PROVIDER_CAP));
        let calm_rewrite = c.rebuild_history_under_pressure(&messages, "summary", Some(calm));
        assert_eq!(camera_blob_images(&calm_rewrite.messages), 8);
    }

    /// One turn whose two images alone cross the byte trigger still compacts,
    /// once: the rewrite sheds that turn's payloads, so the next boundary is
    /// under the trigger instead of crossing again.
    #[test]
    fn an_oversized_image_turn_compacts_once() {
        const IMAGE_BASE64_BYTES: u64 = 4_000_000;
        let c = camera_compactor();
        let mut messages = vec![
            Message::System(SystemMessage::new("You watch the house.")),
            Message::User(UserMessage::text("is anyone home?")),
        ];
        messages.extend(camera_turn(1, None));
        let before = camera_request_bytes(&messages, IMAGE_BASE64_BYTES);
        assert!(
            c.should_compact(&camera_context(before, 10)),
            "a single turn's images over 4/5 of the cap compact"
        );
        let pressure =
            meerkat_core::ProviderRequestPressure::new(before, Some(CAMERA_PROVIDER_CAP));
        let rewrite = c.rebuild_history_under_pressure(&messages, "summary", Some(pressure));
        let after = camera_request_bytes(&rewrite.messages, IMAGE_BASE64_BYTES);
        assert!(
            after < before,
            "the rewrite shrinks the request ({before} -> {after})"
        );
        assert!(
            !c.should_compact(&camera_context(after, 11)),
            "the next boundary compacts again: the rewrite kept {} blob images ({after} bytes)",
            camera_blob_images(&rewrite.messages)
        );
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
        assert!(
            matches!(&result[1], ContentBlock::Text { text } if text == "[image (image/png) omitted from this summary request]")
        );
        assert!(
            matches!(&result[2], ContentBlock::Text { text } if text == "[video (video/mp4) omitted from this summary request]")
        );
        assert!(matches!(&result[3], ContentBlock::Text { text } if text == "world"));
    }

    #[test]
    fn compaction_image_placeholder_excludes_source_path() {
        // source_path must NOT appear in the placeholder — it's internal metadata
        // that would leak filesystem paths through transcript history APIs.
        let blocks = vec![inline_image_block("image/png", "base64data")];
        let result = project_media_for_summarization(&blocks);
        assert_eq!(result.len(), 1);
        assert!(
            matches!(&result[0], ContentBlock::Text { text } if text == "[image (image/png) omitted from this summary request]")
        );
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

    /// The summarizing model's budget bounds the source, not this
    /// compactor's threshold rule: a large model budget passes a source the
    /// threshold rule trims, and a small one trims it.
    #[test]
    fn prepare_for_summarization_bounds_the_source_by_the_given_budget() {
        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 1_000,
            ..make_config()
        });
        let middle = "MARKER-IN-THE-MIDDLE";
        let half = "x".repeat(15_000);
        let messages = vec![Message::User(UserMessage::text(format!(
            "{half}{middle}{half}"
        )))];
        let source_bytes = serde_json::to_vec(&messages).unwrap().len();

        let SummarizationSource {
            messages: by_threshold,
            bound,
        } = c.prepare_for_summarization(&messages, None);
        assert_eq!(
            bound,
            SummarizationSourceBound::Trimmed {
                through: meerkat_core::compact::SummarizationTrimStage::UserRequests
            }
        );
        let threshold_bytes = c.summarization_source_budget_bytes(None);
        assert!(serde_json::to_vec(&by_threshold).unwrap().len() <= threshold_bytes);
        assert!(
            !serde_json::to_string(&by_threshold)
                .unwrap()
                .contains(middle)
        );

        let large = SummarizationSourceBudget::from_compaction_threshold(400_000);
        assert!(source_bytes <= usize::try_from(large.bytes()).unwrap());
        let SummarizationSource {
            messages: by_large_budget,
            bound,
        } = c.prepare_for_summarization(&messages, Some(large));
        assert_eq!(bound, SummarizationSourceBound::Whole);
        assert!(
            serde_json::to_string(&by_large_budget)
                .unwrap()
                .contains(middle),
            "a source within the budget is kept whole"
        );

        let small = SummarizationSourceBudget::from_compaction_threshold(40_000);
        let SummarizationSource {
            messages: by_small_budget,
            bound,
        } = c.prepare_for_summarization(&messages, Some(small));
        assert_eq!(
            bound,
            SummarizationSourceBound::Trimmed {
                through: meerkat_core::compact::SummarizationTrimStage::UserRequests
            }
        );
        assert!(
            serde_json::to_vec(&by_small_budget).unwrap().len()
                <= usize::try_from(small.bytes()).unwrap()
        );
        assert!(
            !serde_json::to_string(&by_small_budget)
                .unwrap()
                .contains(middle)
        );
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

        let prepared = c.prepare_for_summarization(&messages, None).messages;
        assert_eq!(prepared.len(), 2);

        // User message: text preserved, image replaced
        if let Message::User(u) = &prepared[0] {
            assert_eq!(u.content.len(), 3);
            assert!(matches!(&u.content[0], ContentBlock::Text { text } if text == "Look at this"));
            assert!(
                matches!(&u.content[1], ContentBlock::Text { text } if text == "[image (image/jpeg) omitted from this summary request]")
            );
            assert!(
                matches!(&u.content[2], ContentBlock::Text { text } if text == "[video (video/mp4) omitted from this summary request]")
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
                matches!(&results[0].content[1], ContentBlock::Text { text } if text == "[image (image/png) omitted from this summary request]")
            );
            assert!(
                matches!(&results[0].content[2], ContentBlock::Text { text } if text == "[video (video/webm) omitted from this summary request]")
            );
        } else {
            panic!("expected ToolResults message");
        }
    }

    #[test]
    fn prepare_for_summarization_bounds_single_jump_past_context_window() {
        use meerkat_core::types::ToolResult;

        let config = CompactionConfig {
            // Without the summarizing model's budget, a 100k threshold gives
            // this test a 25k source budget.
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

        let SummarizationSource {
            messages: prepared,
            bound,
        } = c.prepare_for_summarization(&messages, None);
        assert_eq!(
            bound,
            SummarizationSourceBound::Trimmed {
                through: meerkat_core::compact::SummarizationTrimStage::ToolResults
            }
        );
        assert_eq!(
            prepared.len(),
            messages.len(),
            "the oversized source keeps its typed rows; their content is trimmed"
        );
        assert!(matches!(
            &prepared[0],
            Message::User(user) if user.text_content() == "inspect the large result"
        ));
        let Message::ToolResults { results, .. } = &prepared[1] else {
            panic!("the tool result row stays a tool result");
        };
        let ContentBlock::Text { text } = &results[0].content[0] else {
            panic!("the trimmed tool result keeps its text block");
        };
        assert!(text.contains("trimmed for the compaction source"));
        let bounded = serde_json::to_vec(&prepared).unwrap().len();
        assert!(
            bounded <= c.summarization_source_budget_bytes(None),
            "bounded source was {bounded} bytes for a {} byte budget",
            c.summarization_source_budget_bytes(None)
        );

        // The last-resort source (user requests only) is bounded in the same
        // unit: its final serialized size, envelope and JSON escaping
        // included. Eight user rows of control characters, each already at
        // the trimming floor, escape to six bytes per character.
        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 1_000,
            ..make_config()
        });
        let budget = c.summarization_source_budget_bytes(None);
        let messages: Vec<Message> = (0..8)
            .map(|n| {
                Message::User(UserMessage::text(format!(
                    "req-{n} {}",
                    "\u{1}".repeat(MIN_TRIMMED_TEXT_BYTES - 6)
                )))
            })
            .collect();
        assert!(serde_json::to_vec(&messages).unwrap().len() > budget);
        let SummarizationSource {
            messages: prepared,
            bound,
        } = c.prepare_for_summarization(&messages, None);
        let source = serde_json::to_string(&prepared).unwrap();
        assert!(
            source.len() <= budget,
            "the requests-only source was {} bytes for a {budget} byte budget",
            source.len()
        );
        assert!(source.contains("req-7"), "the newest request is kept");
        // The source says how many of the requests it carries, in its text
        // and in its bound.
        let SummarizationSourceBound::RequestsOnly { kept, of } = bound else {
            panic!("an over-budget source falls back to the requests: {bound:?}");
        };
        assert_eq!(of, 8);
        assert!((1..8).contains(&kept), "kept {kept} of 8");
        assert!(source.contains(&format!("Only the newest {kept} of the 8 user requests")));
        assert_eq!(
            (0..8)
                .filter(|n| source.contains(&format!("req-{n}")))
                .count(),
            kept,
            "the note counts exactly the requests the source carries"
        );

        // The returned source is the exact message that was measured: at a
        // budget equal to one measured source, every returned source stays
        // within it (a rebuilt message would carry a new timestamp of
        // varying serialized width).
        let tight = serde_json::to_vec(&requests_only_source(&messages[7..], usize::MAX).messages)
            .unwrap()
            .len();
        for _ in 0..500 {
            let returned = requests_only_source(&messages, tight).messages;
            assert!(
                serde_json::to_vec(&returned).unwrap().len() <= tight,
                "the returned requests-only source exceeds the budget it was checked against"
            );
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

        let prepared = c.prepare_for_summarization(&messages, None).messages;

        match &prepared[0] {
            Message::User(user) => {
                assert_eq!(user.content.len(), 3);
                assert!(
                    matches!(&user.content[1], ContentBlock::Text { text } if text == "[image (image/png) omitted from this summary request]")
                );
                assert!(
                    matches!(&user.content[2], ContentBlock::Text { text } if text == "[video (video/webm) omitted from this summary request]")
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
                    matches!(&results[0].content[1], ContentBlock::Text { text } if text == "[image (image/jpeg) omitted from this summary request]")
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

    /// The summary request names each image it leaves out by its media type
    /// and says only that the content is omitted from that request, and the
    /// prompt tells the summarizer that such a placeholder says nothing about
    /// what the agent could see. A placeholder that named only the media type
    /// was read as the agent having been unable to see the image, and the
    /// resumed context inherited that claim. The rebuilt history keeps the
    /// typed images.
    #[test]
    fn summary_request_names_media_as_omitted_and_rebuilt_history_keeps_it() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, ToolResult};

        let c = DefaultCompactor::new(make_config());
        let messages = vec![
            Message::User(UserMessage::text("an earlier request")),
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "does the page match this mockup?".to_string(),
                },
                inline_image_block("image/jpeg", "mockup-data"),
            ])),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tc_screenshot".to_string(),
                    name: "screenshot".to_string(),
                    args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tc_screenshot".to_string(),
                vec![
                    ContentBlock::Text {
                        text: "screenshot captured".to_string(),
                    },
                    blob_image_block("image/png", "sha256:screenshot"),
                ],
                false,
            )]),
        ];
        let text_of = |block: &ContentBlock| match block {
            ContentBlock::Text { text } => text.clone(),
            other => panic!("the summary request carries text in place of media, got {other:?}"),
        };

        // The summary request is the prepared source, then the prompt.
        let source = c.prepare_for_summarization(&messages, None).messages;
        match &source[1] {
            Message::User(user) => {
                assert_eq!(
                    text_of(&user.content[1]),
                    "[image (image/jpeg) omitted from this summary request]"
                );
            }
            other => panic!("expected the projected user row, got {other:?}"),
        }
        match &source[3] {
            Message::ToolResults { results, .. } => {
                assert_eq!(
                    text_of(&results[0].content[1]),
                    "[image (image/png) omitted from this summary request]"
                );
            }
            other => panic!("expected the projected tool results, got {other:?}"),
        }
        let prompt = c.compaction_prompt();
        for instruction in [
            "have their content omitted from this summary request",
            "Their absence here says nothing about what the agent could see.",
            "Do not infer the agent's ability to see them from these placeholders",
        ] {
            assert!(
                prompt.contains(instruction),
                "the compaction prompt must say: {instruction}"
            );
        }

        // The rebuilt active history keeps both images typed.
        let result = c.rebuild_history(&messages, "summary");
        assert_eq!(result.messages.len(), 4, "summary + the kept turn");
        match &result.messages[1] {
            Message::User(user) => {
                assert_inline_image(&user.content[1], "image/jpeg", "mockup-data");
            }
            other => panic!("expected the kept user row, got {other:?}"),
        }
        match &result.messages[3] {
            Message::ToolResults { results, .. } => {
                assert_blob_image(&results[0].content[1], "image/png", "sha256:screenshot");
            }
            other => panic!("expected the kept tool results, got {other:?}"),
        }
    }

    #[test]
    fn rebuild_history_retains_assistant_message_ids_and_discards_old_ones() {
        let c = DefaultCompactor::new(CompactionConfig {
            recent_turn_budget: 1,
            ..make_config()
        });
        let id = |uuid: &str| -> meerkat_core::AssistantMessageId {
            serde_json::from_value(serde_json::json!(uuid)).expect("id")
        };
        let old_answer = id("0190f5c2-4a1e-7c3d-8e2f-00000000f001");
        let kept_answer = id("0190f5c2-4a1e-7c3d-8e2f-00000000f002");
        let answer = |text: &str, id| {
            let mut message = BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: text.to_string(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            );
            message.assistant_message_id = Some(id);
            Message::BlockAssistant(message)
        };
        let messages = vec![
            Message::User(UserMessage::text("old question")),
            answer("same", old_answer),
            Message::User(UserMessage::text("new question")),
            answer("same", kept_answer),
        ];

        let result = c.rebuild_history(&messages, "summary");

        let retained_ids = result
            .retained
            .iter()
            .filter_map(|retention| match &retention.message {
                Message::BlockAssistant(assistant) => assistant.assistant_message_id,
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            retained_ids,
            vec![kept_answer],
            "retained rows keep their ids"
        );
        assert!(result.messages.iter().any(|message| matches!(
            message,
            Message::BlockAssistant(assistant) if assistant.assistant_message_id == Some(kept_answer)
        )));
        assert!(
            result.discarded.iter().any(|discard| matches!(
                &discard.message,
                Message::BlockAssistant(assistant) if assistant.assistant_message_id == Some(old_answer)
            )),
            "the discarded occurrence is handed to the rewrite unchanged"
        );
        assert!(
            result.messages.iter().all(|message| !matches!(
                message,
                Message::BlockAssistant(assistant) if assistant.assistant_message_id == Some(old_answer)
            )),
            "a discarded id is never carried into the rebuilt transcript"
        );
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
            !json.contains("omitted from this summary request"),
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

        let prepared = c.prepare_for_summarization(&messages, None).messages;
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

        let prepared = c.prepare_for_summarization(&messages, None).messages;
        assert_eq!(
            prepared.len(),
            2,
            "reasoning-only assistant should be dropped"
        );
        assert!(matches!(&prepared[0], Message::User(u) if u.text_content() == "First"));
        assert!(matches!(&prepared[1], Message::User(u) if u.text_content() == "Second"));
    }

    fn texts_of(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect()
    }

    /// A downstream app's mid-turn rewrite: System rows larger than the retention
    /// budget must not crowd out the in-flight turn. Its request stays
    /// verbatim whether the whole turn fits (kept whole) or not (the request
    /// row kept, the rest of the turn summarized).
    ///
    /// This is the compactor alone. The agent hands it a whole transcript,
    /// open request included, only at a run's first boundary, before the
    /// run's first response; inside a run, after its first response, it
    /// hands over only the rows before the run, whose newest turn is
    /// answered, and folds the run's own completed rounds itself
    /// (`agent::compact::compaction_scope`).
    #[test]
    fn a_system_heavy_session_keeps_the_in_flight_request() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, ToolResult};

        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 100_000,
            recent_turn_budget: 2,
            ..make_config()
        });
        let request = "please refactor the parser and keep the public API";
        let in_flight = |result_bytes: usize| {
            let mut messages: Vec<Message> = (0..5)
                .map(|n| Message::System(SystemMessage::new(format!("{n}{}", "s".repeat(20_000)))))
                .collect();
            messages.push(Message::User(UserMessage::text("an earlier request")));
            messages.push(Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "done earlier".into(),
                    meta: None,
                }],
                StopReason::EndTurn,
            )));
            messages.push(Message::User(UserMessage::text(request)));
            messages.push(Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tc_read".into(),
                    name: "read".into(),
                    args: serde_json::value::RawValue::from_string("{}".into()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
            )));
            messages.push(Message::tool_results(vec![ToolResult::new(
                "tc_read".into(),
                "r".repeat(result_bytes),
                false,
            )]));
            messages
        };

        // The in-flight turn fits: it is kept whole.
        let messages = in_flight(1_000);
        let result = c.rebuild_history(&messages, "summary");
        let kept = texts_of(&result.messages);
        assert!(
            kept.iter().any(|text| text == request),
            "the in-flight request survives a System-heavy rewrite: {kept:?}"
        );
        assert_eq!(
            result.messages.len(),
            5 + 1 + 3,
            "System rows, the summary and the whole in-flight turn"
        );
        assert!(
            result
                .discarded
                .iter()
                .any(|discard| discard.source_offset == 5),
            "the earlier turn is summarized"
        );

        // The in-flight turn's tool result alone is larger than any retention
        // budget: the request row is still kept verbatim.
        let messages = in_flight(1_000_000);
        let result = c.rebuild_history(&messages, "summary");
        let kept = texts_of(&result.messages);
        assert!(
            kept.iter().any(|text| text == request),
            "the in-flight request survives even when its turn cannot: {kept:?}"
        );
        assert!(
            matches!(result.messages.last(), Some(Message::User(user)) if user.text_content() == request),
            "the request is the last row, its oversized turn tail summarized"
        );
        assert!(
            result
                .discarded
                .iter()
                .any(|discard| matches!(discard.message, Message::ToolResults { .. })),
            "the oversized tool result is summarized, not retained"
        );
    }

    /// The single active instruction of a turn whose tool exchanges are
    /// oversized (small System, no earlier turn) survives the rewrite of
    /// that same vector verbatim, with exact source/rebuilt mappings: the
    /// System row and the instruction keep their offsets, the summary takes
    /// the first discarded position, and every tool exchange is discarded.
    #[test]
    fn the_single_active_instruction_survives_oversized_tool_exchanges() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, ToolResult};

        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 100_000,
            recent_turn_budget: 2,
            ..make_config()
        });
        let instruction = "audit every invoice from last quarter";
        let mut messages = vec![
            Message::System(SystemMessage::new("You are helpful.")),
            Message::User(UserMessage::text(instruction)),
        ];
        for n in 0..2 {
            let id = format!("tc_{n}");
            messages.push(Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: id.clone(),
                    name: "fetch".into(),
                    args: serde_json::value::RawValue::from_string("{}".into()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
            )));
            messages.push(Message::tool_results(vec![ToolResult::new(
                id,
                "i".repeat(200_000),
                false,
            )]));
        }

        let result = c.rebuild_history(&messages, "summary");
        let retained: Vec<(u64, u64)> = result
            .retained
            .iter()
            .map(|retention| (retention.source_offset, retention.rebuilt_offset))
            .collect();
        assert_eq!(
            retained,
            vec![(0, 0), (1, 1)],
            "the System row and the instruction keep their exact offsets"
        );
        assert!(matches!(
            &result.messages[1],
            Message::User(user) if user.text_content() == instruction
        ));
        assert_eq!(result.summary.rebuilt_offset, 2);
        assert_eq!(result.messages.len(), 3);
        assert_eq!(
            result
                .discarded
                .iter()
                .map(|discard| discard.source_offset)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 5],
            "every tool exchange is discarded"
        );
    }

    /// A user request in the middle of an oversized transcript (a large
    /// System prompt before it, a large tool result after it) reaches the
    /// summarizer within the source budget.
    #[test]
    fn a_middle_user_request_reaches_the_summarizer_source() {
        use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, ToolResult};

        let c = DefaultCompactor::new(CompactionConfig {
            auto_compact_threshold: 100_000,
            ..make_config()
        });
        let request = "migrate the billing tables to the new schema";
        let messages = vec![
            Message::System(SystemMessage::new("p".repeat(600_000))),
            Message::User(UserMessage::text(request)),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tc_dump".into(),
                    name: "dump".into(),
                    args: serde_json::value::RawValue::from_string("{}".into()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::new(
                "tc_dump".into(),
                "d".repeat(2_000_000),
                false,
            )]),
        ];

        let prepared = c.prepare_for_summarization(&messages, None).messages;
        let source = serde_json::to_string(&prepared).unwrap();
        assert!(
            source.contains(request),
            "the middle request reaches the summarizer source"
        );
        assert!(
            source.len() <= c.summarization_source_budget_bytes(None),
            "the source ({} bytes) stays within its budget ({} bytes)",
            source.len(),
            c.summarization_source_budget_bytes(None)
        );
    }
}

//! Transcript model: the list of rendered blocks and the agent-event → block mapping.
//!
//! The mapping mirrors `meerkat-cli/src/stream_renderer.rs` but produces structured
//! blocks instead of ANSI text. Each block has a per-width layout cache that is
//! invalidated whenever the block mutates.

use std::time::Instant;

use indexmap::IndexMap;
use meerkat_core::event::BackgroundJobTerminalStatus;
use meerkat_core::types::text_content;
use meerkat_core::{
    AgentEvent, HookPoint, LlmRetrySchedule, ScopedAgentEvent, StreamScopeFrame, TurnUsage,
};
use ratatui::text::Line;
use serde_json::Value;

use crate::delivery::PendingId;

use super::markdown::truncate_chars;
use super::render::block_lines;

/// Maximum tool-result lines kept in the preview (matches the CLI stream renderer).
pub const MAX_TOOL_RESULT_LINES: usize = 20;
/// Maximum characters of serialized tool args shown in a header (matches the CLI).
pub const MAX_TOOL_ARGS_PREVIEW: usize = 200;

/// Index of a block in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

/// Severity of a notice block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    /// Informational chrome.
    Info,
    /// Something degraded but the run continues.
    Warn,
    /// A failure.
    Error,
}

/// Lifecycle state of a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    /// The model asked for the call; approval/dispatch pending.
    Requested,
    /// The tool is executing.
    Running {
        /// When execution started.
        since: Instant,
    },
    /// Execution finished.
    Done {
        /// Whether the tool reported an error.
        is_error: bool,
        /// Wall-clock execution time.
        duration_ms: u64,
    },
    /// A hook (approval engine) denied the call.
    Denied {
        /// Human-readable reason.
        reason: String,
    },
}

/// Delivery state shown on a [`Block::Queued`] placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuedLabel {
    /// Waiting for the integrator to accept it.
    Queued,
    /// In the agent's inbox, waiting for the next turn boundary.
    Delivering,
    /// Could not be injected; sent when the turn ends.
    Deferred,
    /// Being pulled back into the editor.
    Recalling,
}

impl QueuedLabel {
    /// Text shown after the message.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delivering => "delivering…",
            Self::Deferred => "will send after this turn",
            Self::Recalling => "recalling…",
        }
    }
}

/// A file diff attached to a tool block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    /// File the diff applies to.
    pub path: String,
    /// Unified diff text.
    pub unified: String,
    /// Added line count.
    pub added: usize,
    /// Removed line count.
    pub removed: usize,
}

/// One rendered unit of the transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A prompt typed by the user.
    User {
        /// Prompt text.
        text: String,
    },
    /// A message typed while a turn was running, not yet seen by the model.
    /// Replaced in place by [`Block::User`] once delivered.
    Queued {
        /// Pending-queue id.
        id: PendingId,
        /// Message text.
        text: String,
        /// Delivery state.
        label: QueuedLabel,
    },
    /// Assistant output (markdown source).
    Assistant {
        /// Markdown source accumulated from text deltas.
        md: String,
    },
    /// Model reasoning / thinking.
    Reasoning {
        /// Reasoning text.
        text: String,
        /// Whether only a one-line summary is shown.
        collapsed: bool,
    },
    /// A tool call and its lifecycle.
    Tool {
        /// Tool name (sub-agent calls carry a scope label prefix).
        name: String,
        /// Arguments as requested by the model.
        args: Value,
        /// Lifecycle state.
        state: ToolState,
        /// Truncated result preview.
        result_preview: Option<String>,
        /// Attached file diff, if the tool edited a file.
        diff: Option<DiffView>,
        /// Nesting depth (0 for the primary agent, >0 for sub-agents).
        depth: usize,
        /// Whether the result preview and diff are hidden behind the header.
        /// Tool output starts collapsed; a click (or Ctrl-T) toggles it.
        collapsed: bool,
    },
    /// An approval decision.
    Approval {
        /// Tool the decision applies to.
        tool: String,
        /// Tier / mechanism that decided.
        tier: String,
        /// Whether the call was allowed.
        allowed: bool,
        /// Optional explanation.
        reason: Option<String>,
    },
    /// Chrome line (retry, budget, compaction, errors, command output).
    Notice {
        /// Severity.
        level: NoticeLevel,
        /// Text.
        text: String,
    },
}

/// Cached wrapped lines for one block at one width.
#[derive(Debug, Clone)]
pub struct CachedLayout {
    /// Width the lines were computed for.
    pub width: u16,
    /// Wrapped lines (including the trailing spacer line).
    pub lines: Vec<Line<'static>>,
}

/// The transcript: blocks, tool-id index and per-block layout cache.
#[derive(Debug, Default)]
pub struct Transcript {
    blocks: Vec<Block>,
    by_tool_id: IndexMap<String, BlockId>,
    layout: Vec<Option<CachedLayout>>,
    open_assistant: Option<BlockId>,
    open_reasoning: Option<BlockId>,
    text_seen_this_turn: bool,
    reasoning_seen_this_turn: bool,
}

impl Transcript {
    /// Create an empty transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// All blocks in order.
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Number of blocks.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether the transcript has no blocks.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Block by id.
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.get(id.0)
    }

    /// Block id registered for a tool-use id.
    pub fn tool_block(&self, tool_use_id: &str) -> Option<BlockId> {
        self.by_tool_id.get(tool_use_id).copied()
    }

    /// Append a block and return its id. Closes any open streaming blocks.
    pub fn push(&mut self, block: Block) -> BlockId {
        self.close_streams();
        self.push_raw(block)
    }

    /// Append a notice block.
    pub fn push_notice(&mut self, level: NoticeLevel, text: impl Into<String>) -> BlockId {
        self.push(Block::Notice {
            level,
            text: text.into(),
        })
    }

    /// Remove every block.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Replace the whole transcript with `blocks` (e.g. after resuming a session).
    pub fn reset(&mut self, blocks: Vec<Block>) {
        self.clear();
        for block in blocks {
            self.push_raw(block);
        }
    }

    /// Mutable access to a block; invalidates its layout.
    pub fn block_mut(&mut self, id: BlockId) -> Option<&mut Block> {
        if let Some(slot) = self.layout.get_mut(id.0) {
            *slot = None;
        }
        self.blocks.get_mut(id.0)
    }

    /// Replace a block in place (keeps transcript order). Returns whether `id`
    /// existed.
    pub fn replace_block(&mut self, id: BlockId, block: Block) -> bool {
        match self.block_mut(id) {
            Some(slot) => {
                *slot = block;
                true
            }
            None => false,
        }
    }

    /// Remove a block. Later block ids shift down by one; the tool-id index and
    /// open streams are fixed up, callers holding other ids must adjust them.
    pub fn remove_block(&mut self, id: BlockId) -> Option<Block> {
        if id.0 >= self.blocks.len() {
            return None;
        }
        let block = self.blocks.remove(id.0);
        if id.0 < self.layout.len() {
            self.layout.remove(id.0);
        }
        let shift = |b: BlockId| if b.0 > id.0 { BlockId(b.0 - 1) } else { b };
        self.by_tool_id.retain(|_, b| *b != id);
        for b in self.by_tool_id.values_mut() {
            *b = shift(*b);
        }
        self.open_assistant = self.open_assistant.filter(|b| *b != id).map(shift);
        self.open_reasoning = self.open_reasoning.filter(|b| *b != id).map(shift);
        Some(block)
    }

    /// Drop every cached layout (e.g. on resize).
    pub fn invalidate_all(&mut self) {
        for slot in &mut self.layout {
            *slot = None;
        }
    }

    /// Toggle collapse on the most recent reasoning block. Returns whether one existed.
    pub fn toggle_last_reasoning(&mut self) -> bool {
        let Some(idx) = self
            .blocks
            .iter()
            .rposition(|b| matches!(b, Block::Reasoning { .. }))
        else {
            return false;
        };
        if let Some(Block::Reasoning { collapsed, .. }) = self.block_mut(BlockId(idx)) {
            *collapsed = !*collapsed;
        }
        true
    }

    /// Expand or collapse the output of the tool block `id`. Returns the new
    /// `collapsed` state, or `None` when `id` is not a tool block with output.
    pub fn toggle_tool(&mut self, id: BlockId) -> Option<bool> {
        if !self.block(id).is_some_and(tool_has_output) {
            return None;
        }
        match self.block_mut(id) {
            Some(Block::Tool { collapsed, .. }) => {
                *collapsed = !*collapsed;
                Some(*collapsed)
            }
            _ => None,
        }
    }

    /// The most recent tool block with output to show or hide.
    pub fn last_tool_with_output(&self) -> Option<BlockId> {
        self.blocks.iter().rposition(tool_has_output).map(BlockId)
    }

    /// Attach a diff to the tool block for `tool_use_id`. Returns whether the block was found.
    pub fn attach_diff(&mut self, tool_use_id: &str, view: DiffView) -> bool {
        let Some(id) = self.tool_block(tool_use_id) else {
            return false;
        };
        if let Some(Block::Tool { diff, .. }) = self.block_mut(id) {
            *diff = Some(view);
            return true;
        }
        false
    }

    /// Ensure every block has a layout at `width`; returns the total line count.
    pub fn ensure_layout(&mut self, width: u16) -> usize {
        let mut total = 0;
        for (block, slot) in self.blocks.iter().zip(self.layout.iter_mut()) {
            let stale = slot.as_ref().is_none_or(|c| c.width != width);
            if stale {
                *slot = Some(CachedLayout {
                    width,
                    lines: block_lines(block, width),
                });
            }
            total += slot.as_ref().map_or(0, |c| c.lines.len());
        }
        total
    }

    /// Lines `[start, start + len)` of the laid-out transcript. Call [`Self::ensure_layout`] first.
    pub fn visible_lines(&self, start: usize, len: usize) -> Vec<Line<'static>> {
        self.layout
            .iter()
            .flatten()
            .flat_map(|c| c.lines.iter())
            .skip(start)
            .take(len)
            .cloned()
            .collect()
    }

    /// Laid-out line `idx` of the whole transcript, as of the last
    /// [`Self::ensure_layout`] (`None` past the end or for stale blocks).
    pub fn line(&self, idx: usize) -> Option<&Line<'static>> {
        self.layout
            .iter()
            .flatten()
            .flat_map(|c| c.lines.iter())
            .nth(idx)
    }

    /// Number of laid-out lines as of the last [`Self::ensure_layout`].
    pub fn line_count(&self) -> usize {
        self.layout.iter().flatten().map(|c| c.lines.len()).sum()
    }

    /// The block drawn on each of the lines `[start, start + len)`, matching
    /// [`Self::visible_lines`]. Call [`Self::ensure_layout`] first.
    pub fn visible_blocks(&self, start: usize, len: usize) -> Vec<BlockId> {
        self.layout
            .iter()
            .enumerate()
            .flat_map(|(i, slot)| {
                let n = slot.as_ref().map_or(0, |c| c.lines.len());
                std::iter::repeat_n(BlockId(i), n)
            })
            .skip(start)
            .take(len)
            .collect()
    }

    /// Apply a primary-agent event. Returns the usage of a completed LLM turn, if any
    /// (a turn the provider sent no accounting for returns `None`).
    pub fn apply_agent_event(&mut self, event: &AgentEvent) -> Option<TurnUsage> {
        self.apply_agent_event_at(event, Instant::now())
    }

    /// [`Self::apply_agent_event`] with an explicit clock (for tests).
    #[allow(clippy::too_many_lines)]
    pub fn apply_agent_event_at(&mut self, event: &AgentEvent, now: Instant) -> Option<TurnUsage> {
        match event {
            AgentEvent::TurnStarted { .. } => {
                self.close_streams();
                self.text_seen_this_turn = false;
                self.reasoning_seen_this_turn = false;
            }
            AgentEvent::TurnCompleted { usage, .. } => {
                self.close_streams();
                return usage.clone();
            }
            AgentEvent::ReasoningDelta { delta, .. } => self.append_reasoning(delta),
            AgentEvent::ReasoningComplete { content, .. } => {
                if self.open_reasoning.is_none()
                    && !self.reasoning_seen_this_turn
                    && !content.is_empty()
                {
                    self.append_reasoning(content);
                }
                self.open_reasoning = None;
            }
            AgentEvent::TextDelta { delta, .. } => self.append_text(delta),
            AgentEvent::TextComplete { content, .. } => {
                if self.open_assistant.is_none() && !self.text_seen_this_turn && !content.is_empty()
                {
                    self.append_text(content);
                }
                self.open_assistant = None;
            }
            AgentEvent::ToolCallRequested { id, name, args } => {
                self.request_tool(id.clone(), name.clone(), args.as_value().clone(), 0);
            }
            AgentEvent::ToolExecutionStarted { id, .. } => self.start_tool(id, now),
            AgentEvent::ToolExecutionCompleted {
                id,
                is_error,
                duration_ms,
                ..
            } => self.complete_tool(id, *is_error, *duration_ms, None),
            // The result body arrives on its own event, after the execution fact.
            AgentEvent::ToolResultReceived { id, content, .. } => {
                self.tool_result(id, &text_content(content));
            }
            AgentEvent::ToolExecutionTimedOut { id, timeout_ms, .. } => {
                let text = format!("timed out after {timeout_ms}ms");
                self.complete_tool(id, true, *timeout_ms, Some(&text));
            }
            AgentEvent::HookDenied {
                hook_id,
                point,
                message,
                payload,
                ..
            } => self.hook_denied(&hook_id.0, *point, message, payload.as_ref()),
            AgentEvent::HookFailed {
                hook_id, reason, ..
            } => {
                self.push_notice(
                    NoticeLevel::Warn,
                    format!("hook failed: {hook_id}: {reason}"),
                );
            }
            AgentEvent::HookLaunchRefused {
                hook_id, reason, ..
            } => {
                self.push_notice(
                    NoticeLevel::Warn,
                    format!("hook refused to launch: {hook_id}: {reason}"),
                );
            }
            AgentEvent::Retrying { retry, .. } => {
                self.push_notice(NoticeLevel::Warn, retry_notice(retry));
            }
            AgentEvent::BudgetWarning {
                budget_type,
                used,
                limit,
                percent,
            } => {
                self.push_notice(
                    NoticeLevel::Warn,
                    format!(
                        "⚠ budget: {budget_type:?} at {:.0}% ({used}/{limit})",
                        percent * 100.0
                    ),
                );
            }
            AgentEvent::CompactionStarted {
                message_count,
                estimated_history_tokens,
                ..
            } => {
                self.push_notice(
                    NoticeLevel::Info,
                    format!(
                        "⟳ compacting context ({message_count} messages, ~{estimated_history_tokens} tokens)…"
                    ),
                );
            }
            AgentEvent::CompactionCompleted {
                summary_tokens,
                messages_before,
                messages_after,
                ..
            } => {
                self.push_notice(
                    NoticeLevel::Info,
                    format!(
                        "✓ compacted: {messages_before} → {messages_after} messages ({summary_tokens} summary tokens)"
                    ),
                );
            }
            AgentEvent::CompactionFailed { reason } => {
                self.push_notice(NoticeLevel::Warn, format!("✗ compaction failed: {reason}"));
            }
            AgentEvent::RunFailed { error_report, .. } => {
                self.push_notice(
                    NoticeLevel::Error,
                    format!("error: {}", error_report.message),
                );
            }
            AgentEvent::StreamTruncated { reason } => {
                self.push_notice(
                    NoticeLevel::Warn,
                    format!("some stream events were dropped: {reason}"),
                );
            }
            AgentEvent::SkillResolutionFailed { skill_key, reason } => {
                let text = match skill_key {
                    Some(key) => format!("⚠ skill failed: {key}: {reason}"),
                    None => format!("⚠ skill failed: {reason}"),
                };
                self.push_notice(NoticeLevel::Warn, text);
            }
            other => {
                if let Some((level, text)) = upstream_notice(other) {
                    self.push_notice(level, text);
                }
            }
        }
        None
    }

    /// Apply a scoped (sub-agent) event. Primary-scope events are ignored because they
    /// already arrive on the primary channel.
    pub fn apply_scoped_event(&mut self, scoped: &ScopedAgentEvent) {
        self.apply_scoped_event_at(scoped, Instant::now());
    }

    /// [`Self::apply_scoped_event`] with an explicit clock (for tests).
    pub fn apply_scoped_event_at(&mut self, scoped: &ScopedAgentEvent, now: Instant) {
        let mut depth = 0usize;
        let mut label = None;
        for frame in &scoped.scope_path {
            if let StreamScopeFrame::MobMember { agent_identity, .. } = frame {
                depth += 1;
                label = Some(agent_identity.clone());
            }
        }
        let Some(label) = label else {
            return;
        };
        let key = |id: &str| format!("{}/{id}", scoped.scope_id);
        match &scoped.event {
            AgentEvent::ToolCallRequested { id, name, args } => {
                self.request_tool(
                    key(id),
                    format!("[{label}] {name}"),
                    args.as_value().clone(),
                    depth,
                );
            }
            AgentEvent::ToolExecutionStarted { id, .. } => self.start_tool(&key(id), now),
            AgentEvent::ToolExecutionCompleted {
                id,
                is_error,
                duration_ms,
                ..
            } => self.complete_tool(&key(id), *is_error, *duration_ms, None),
            AgentEvent::ToolResultReceived { id, content, .. } => {
                self.tool_result(&key(id), &text_content(content));
            }
            AgentEvent::ToolExecutionTimedOut { id, timeout_ms, .. } => {
                let text = format!("timed out after {timeout_ms}ms");
                self.complete_tool(&key(id), true, *timeout_ms, Some(&text));
            }
            AgentEvent::RunFailed { error_report, .. } => {
                self.push_notice(
                    NoticeLevel::Error,
                    format!("[{label}] error: {}", error_report.message),
                );
            }
            _ => {}
        }
    }

    fn push_raw(&mut self, block: Block) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(block);
        self.layout.push(None);
        id
    }

    fn close_streams(&mut self) {
        self.open_assistant = None;
        self.open_reasoning = None;
    }

    fn append_text(&mut self, delta: &str) {
        self.open_reasoning = None;
        self.text_seen_this_turn = true;
        if let Some(id) = self.open_assistant
            && let Some(Block::Assistant { md }) = self.block_mut(id)
        {
            md.push_str(delta);
            return;
        }
        let id = self.push_raw(Block::Assistant {
            md: delta.to_string(),
        });
        self.open_assistant = Some(id);
    }

    fn append_reasoning(&mut self, delta: &str) {
        self.open_assistant = None;
        self.reasoning_seen_this_turn = true;
        if let Some(id) = self.open_reasoning
            && let Some(Block::Reasoning { text, .. }) = self.block_mut(id)
        {
            text.push_str(delta);
            return;
        }
        let id = self.push_raw(Block::Reasoning {
            text: delta.to_string(),
            collapsed: true,
        });
        self.open_reasoning = Some(id);
    }

    fn request_tool(&mut self, key: String, name: String, args: Value, depth: usize) {
        let id = self.push(Block::Tool {
            name,
            args,
            state: ToolState::Requested,
            result_preview: None,
            diff: None,
            depth,
            collapsed: true,
        });
        self.by_tool_id.insert(key, id);
    }

    fn start_tool(&mut self, key: &str, now: Instant) {
        let Some(id) = self.tool_block(key) else {
            return;
        };
        if let Some(Block::Tool { state, .. }) = self.block_mut(id) {
            *state = ToolState::Running { since: now };
        }
    }

    fn complete_tool(&mut self, key: &str, is_error: bool, duration_ms: u64, result: Option<&str>) {
        let Some(id) = self.tool_block(key) else {
            return;
        };
        if let Some(Block::Tool {
            state,
            result_preview,
            ..
        }) = self.block_mut(id)
        {
            *state = ToolState::Done {
                is_error,
                duration_ms,
            };
            if let Some(result) = result {
                *result_preview = Some(result).filter(|r| !r.is_empty()).map(preview_result);
            }
        }
    }

    /// Attach a tool's result body (it arrives after the execution fact).
    fn tool_result(&mut self, key: &str, result: &str) {
        let Some(id) = self.tool_block(key) else {
            return;
        };
        if let Some(Block::Tool { result_preview, .. }) = self.block_mut(id) {
            *result_preview = Some(result).filter(|r| !r.is_empty()).map(preview_result);
        }
    }

    fn hook_denied(
        &mut self,
        hook_id: &str,
        point: HookPoint,
        message: &str,
        payload: Option<&Value>,
    ) {
        if point != HookPoint::PreToolExecution {
            self.push_notice(
                NoticeLevel::Warn,
                format!("⊘ hook denied ({point:?}): {hook_id}: {message}"),
            );
            return;
        }
        let payload_str = |key: &str| {
            payload
                .and_then(|p| p.get(key))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let target = payload_str("tool_use_id")
            .and_then(|tid| self.tool_block(&tid))
            .or_else(|| {
                self.by_tool_id.values().copied().find(|id| {
                    matches!(
                        self.block(*id),
                        Some(Block::Tool {
                            state: ToolState::Requested,
                            ..
                        })
                    )
                })
            });
        let mut tool = payload_str("tool").unwrap_or_default();
        if let Some(id) = target
            && let Some(Block::Tool { name, state, .. }) = self.block_mut(id)
        {
            *state = ToolState::Denied {
                reason: message.to_string(),
            };
            if tool.is_empty() {
                tool.clone_from(name);
            }
        }
        let tier = payload_str("tier").unwrap_or_else(|| hook_id.to_string());
        self.push(Block::Approval {
            tool,
            tier,
            allowed: false,
            reason: Some(message.to_string()),
        });
    }
}

/// Serialized, truncated tool arguments for headers.
pub fn args_preview(args: &Value) -> String {
    let raw = serde_json::to_string(args).unwrap_or_default();
    truncate_chars(&raw, MAX_TOOL_ARGS_PREVIEW)
}

/// Whether `block` is a tool block with a result preview or a diff.
fn tool_has_output(block: &Block) -> bool {
    matches!(block, Block::Tool { result_preview, diff, .. }
        if result_preview.is_some() || diff.is_some())
}

/// Number of lines in the tool result a preview was made from (the preview
/// itself is capped at [`MAX_TOOL_RESULT_LINES`] plus a `... (N more lines)`
/// trailer, see [`preview_result`]).
pub fn result_line_count(preview: &str) -> usize {
    let shown = preview.lines().count();
    let more = preview
        .lines()
        .last()
        .and_then(|l| l.strip_prefix("... ("))
        .and_then(|l| l.strip_suffix(" more lines)"))
        .and_then(|n| n.parse::<usize>().ok());
    match more {
        Some(more) if shown == MAX_TOOL_RESULT_LINES + 1 => MAX_TOOL_RESULT_LINES + more,
        _ => shown,
    }
}

fn preview_result(result: &str) -> String {
    let total = result.lines().count();
    let mut out: Vec<&str> = result.lines().take(MAX_TOOL_RESULT_LINES).collect();
    let more;
    if total > MAX_TOOL_RESULT_LINES {
        more = format!("... ({} more lines)", total - MAX_TOOL_RESULT_LINES);
        out.push(&more);
    }
    out.join("\n")
}

/// The retry notice: attempt, diagnostic and back-off delay.
fn retry_notice(retry: &LlmRetrySchedule) -> String {
    format!(
        "⟳ retry {}/{}: {} ({}ms)",
        retry.plan.attempt,
        retry.plan.max_retries,
        retry.failure.message,
        retry.plan.selected_delay_ms
    )
}

/// Notices for the upstream events the transcript has no dedicated block for.
///
/// Rendered as a notice until P1 maps it: steer/queue boundary appends,
/// model fallback, background jobs, tool-config changes, peer content, run
/// resumption and request-bounded tool results. Events with no user-facing
/// meaning (run/hook/interaction lifecycle, accounting disputes, transcript
/// rewrites, cache breakpoints, reasoning lowering) yield `None`.
fn upstream_notice(event: &AgentEvent) -> Option<(NoticeLevel, String)> {
    let notice = match event {
        AgentEvent::BoundaryAppendApplied { append_count, .. } => (
            NoticeLevel::Info,
            format!("↳ {append_count} message(s) appended to the running turn"),
        ),
        AgentEvent::BoundaryAppendsDiscarded(discarded) => (
            NoticeLevel::Warn,
            format!(
                "⚠ {} appended message(s) discarded",
                discarded.input_ids.len()
            ),
        ),
        AgentEvent::ModelFallbackCommitted {
            previous, target, ..
        } => (
            NoticeLevel::Warn,
            format!("⇄ model fallback: {} → {}", previous.model, target.model),
        ),
        AgentEvent::ModelFallbackTargetFailed { target, error, .. } => (
            NoticeLevel::Warn,
            format!(
                "✗ fallback model {} failed: {}",
                target.model, error.message
            ),
        ),
        AgentEvent::BackgroundJobCompleted {
            display_name,
            terminal_status,
            detail,
            ..
        } => {
            let level = if *terminal_status == BackgroundJobTerminalStatus::Completed {
                NoticeLevel::Info
            } else {
                NoticeLevel::Warn
            };
            let mut text = format!("⚙ {display_name}: {}", terminal_status.as_str());
            if !detail.is_empty() {
                text.push_str(" — ");
                text.push_str(detail);
            }
            (level, text)
        }
        AgentEvent::ToolConfigChanged { payload } => (
            NoticeLevel::Info,
            format!("tools: {:?} {}", payload.operation, payload.target),
        ),
        AgentEvent::PeerContentIngested { kind, .. } => (
            NoticeLevel::Info,
            format!("✉ peer content received ({kind:?})"),
        ),
        AgentEvent::RunResumed { round, .. } => {
            (NoticeLevel::Info, format!("⟳ run resumed (round {round})"))
        }
        AgentEvent::ToolResultsBoundedForRequest { results, .. } => {
            let names = results
                .iter()
                .map(|r| format!("{} {}/{} bytes", r.tool_name, r.kept_bytes, r.total_bytes))
                .collect::<Vec<_>>()
                .join(", ");
            (
                NoticeLevel::Info,
                format!("tool results trimmed for the request: {names}"),
            )
        }
        AgentEvent::TurnUsageAccountingUnmeasured { .. } => (
            NoticeLevel::Info,
            "the provider reported no usage for a turn; the cost readout misses it".to_string(),
        ),
        AgentEvent::ServerToolContent { kind, .. } => {
            (NoticeLevel::Info, format!("◆ server tool: {kind}"))
        }
        AgentEvent::AssistantImageAppended { .. } => (
            NoticeLevel::Info,
            "◆ image added to the transcript".to_string(),
        ),
        _ => return None,
    };
    Some(notice)
}

/// Event builders shared by the UI unit tests.
#[cfg(test)]
pub(crate) mod test_support {
    use meerkat_core::types::ContentBlock;
    use meerkat_core::{AgentEvent, Provider, ToolCallArguments, TurnUsage, Usage};
    use serde_json::Value;

    /// Tool-call arguments from a JSON object literal.
    #[allow(clippy::expect_used)]
    pub(crate) fn test_args(args: Value) -> ToolCallArguments {
        ToolCallArguments::from_value(args).expect("tool-call arguments are a JSON object")
    }

    /// The body of a finished tool call, as upstream publishes it after
    /// `ToolExecutionCompleted`.
    pub(crate) fn test_tool_result(id: &str, text: &str) -> AgentEvent {
        AgentEvent::ToolResultReceived {
            id: id.into(),
            name: String::new(),
            content: vec![ContentBlock::Text { text: text.into() }],
            is_error: false,
        }
    }

    /// Provider-accounted usage for a `TurnCompleted` event.
    pub(crate) fn test_usage(usage: Usage) -> Option<TurnUsage> {
        Some(TurnUsage::host_declared(Provider::Anthropic, "test", usage))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::test_support::{test_args, test_tool_result, test_usage};
    use super::*;
    use meerkat_core::{StopReason, Usage};
    use serde_json::json;

    fn text(d: &str) -> AgentEvent {
        AgentEvent::TextDelta {
            delta: d.to_string(),
            assistant_message_id: None,
        }
    }

    #[test]
    fn text_deltas_accumulate_into_one_assistant_block() {
        let mut t = Transcript::new();
        t.apply_agent_event(&AgentEvent::TurnStarted {
            turn_number: 0,
            assistant_message_id: None,
        });
        t.apply_agent_event(&text("Hel"));
        t.apply_agent_event(&text("lo"));
        t.apply_agent_event(&AgentEvent::TextComplete {
            content: "Hello".into(),
            assistant_message_id: None,
        });
        assert_eq!(t.blocks(), &[Block::Assistant { md: "Hello".into() }]);
    }

    #[test]
    fn text_complete_without_deltas_creates_block() {
        let mut t = Transcript::new();
        t.apply_agent_event(&AgentEvent::TurnStarted {
            turn_number: 0,
            assistant_message_id: None,
        });
        t.apply_agent_event(&AgentEvent::TextComplete {
            content: "whole".into(),
            assistant_message_id: None,
        });
        assert_eq!(t.blocks(), &[Block::Assistant { md: "whole".into() }]);
    }

    #[test]
    fn tool_splits_assistant_blocks_and_tracks_lifecycle() {
        let mut t = Transcript::new();
        let now = Instant::now();
        t.apply_agent_event(&text("before"));
        t.apply_agent_event(&AgentEvent::ToolCallRequested {
            id: "t1".into(),
            name: "read_file".into(),
            args: test_args(json!({"path": "a.rs"})),
        });
        let id = t.tool_block("t1");
        assert_eq!(id, Some(BlockId(1)));
        t.apply_agent_event_at(
            &AgentEvent::ToolExecutionStarted {
                id: "t1".into(),
                name: "read_file".into(),
            },
            now,
        );
        assert!(matches!(
            t.block(BlockId(1)),
            Some(Block::Tool { state: ToolState::Running { since }, .. }) if *since == now
        ));
        let long = (0..30)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        t.apply_agent_event(&AgentEvent::ToolExecutionCompleted {
            id: "t1".into(),
            name: "read_file".into(),
            is_error: false,
            duration_ms: 7,
        });
        t.apply_agent_event(&test_tool_result("t1", &long));
        match t.block(BlockId(1)) {
            Some(Block::Tool {
                state,
                result_preview: Some(preview),
                ..
            }) => {
                assert_eq!(
                    *state,
                    ToolState::Done {
                        is_error: false,
                        duration_ms: 7
                    }
                );
                assert_eq!(preview.lines().count(), MAX_TOOL_RESULT_LINES + 1);
                assert!(preview.ends_with("... (10 more lines)"));
            }
            other => panic!("unexpected block {other:?}"),
        }
        t.apply_agent_event(&text("after"));
        assert_eq!(t.len(), 3);
        assert_eq!(t.blocks()[2], Block::Assistant { md: "after".into() });
    }

    #[test]
    fn hook_denied_marks_tool_and_adds_approval_block() {
        let mut t = Transcript::new();
        t.apply_agent_event(&AgentEvent::ToolCallRequested {
            id: "t9".into(),
            name: "shell".into(),
            args: test_args(json!({"command": "rm -rf /"})),
        });
        t.apply_agent_event(&AgentEvent::HookDenied {
            hook_id: "tui-auto-mode".into(),
            point: HookPoint::PreToolExecution,
            reason_code: meerkat_core::HookReasonCode::PolicyViolation,
            message: "destructive".into(),
            payload: Some(json!({"tool_use_id": "t9", "tier": "classifier"})),
        });
        assert!(matches!(
            t.block(BlockId(0)),
            Some(Block::Tool { state: ToolState::Denied { reason }, .. }) if reason == "destructive"
        ));
        assert_eq!(
            t.blocks()[1],
            Block::Approval {
                tool: "shell".into(),
                tier: "classifier".into(),
                allowed: false,
                reason: Some("destructive".into()),
            }
        );
    }

    #[test]
    fn turn_completed_returns_usage_and_reasoning_is_collapsed() {
        let mut t = Transcript::new();
        t.apply_agent_event(&AgentEvent::ReasoningDelta {
            delta: "hmm".into(),
            assistant_message_id: None,
        });
        assert!(matches!(
            t.blocks()[0],
            Block::Reasoning {
                collapsed: true,
                ..
            }
        ));
        assert!(t.toggle_last_reasoning());
        assert!(matches!(
            t.blocks()[0],
            Block::Reasoning {
                collapsed: false,
                ..
            }
        ));
        let usage = t.apply_agent_event(&AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: test_usage(Usage {
                input_tokens: 5,
                output_tokens: 3,
                ..Usage::default()
            }),
            assistant_message_id: None,
        });
        assert_eq!(usage.map(|u| u.output_tokens), Some(3));
    }

    #[test]
    fn notices_for_retry_and_run_failed() {
        let mut t = Transcript::new();
        t.apply_agent_event(&AgentEvent::Retrying {
            retry: LlmRetrySchedule {
                failure: meerkat_core::LlmRetryFailure {
                    provider: "anthropic".into(),
                    kind: meerkat_core::LlmRetryFailureKind::NetworkTimeout,
                    retry_after_ms: None,
                    duration_ms: None,
                    message: "boom".into(),
                },
                plan: meerkat_core::LlmRetryPlan {
                    attempt: 1,
                    max_retries: 3,
                    computed_delay_ms: 10,
                    selected_delay_ms: 10,
                    retry_after_hint_ms: None,
                    rate_limit_floor_applied: false,
                    budget_capped: false,
                },
            },
            assistant_message_id: None,
        });
        t.apply_agent_event(&AgentEvent::RunFailed {
            session_id: meerkat_core::SessionId::new(),
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            error_report: meerkat_core::AgentErrorReport {
                class: meerkat_core::AgentErrorClass::Llm,
                reason: None,
                message: "dead".into(),
            },
            terminal_cause_kind: None,
        });
        assert!(matches!(
            &t.blocks()[0],
            Block::Notice { level: NoticeLevel::Warn, text } if text.contains("retry 1/3")
        ));
        assert!(matches!(
            &t.blocks()[1],
            Block::Notice { level: NoticeLevel::Error, text } if text.contains("dead")
        ));
    }

    #[test]
    fn scoped_sub_agent_tools_are_nested() {
        let mut t = Transcript::new();
        let ev = ScopedAgentEvent::new(
            vec![
                StreamScopeFrame::Primary {
                    session_id: "s".into(),
                },
                StreamScopeFrame::MobMember {
                    flow_run_id: "f1".into(),
                    agent_identity: "helper".into(),
                    agent_runtime_id: None,
                    fence_token: None,
                    generation: None,
                },
            ],
            AgentEvent::ToolCallRequested {
                id: "x".into(),
                name: "glob".into(),
                args: test_args(json!({})),
            },
        );
        t.apply_scoped_event(&ev);
        assert!(matches!(
            &t.blocks()[0],
            Block::Tool { name, depth: 1, .. } if name == "[helper] glob"
        ));
        // Primary-only scope is ignored.
        t.apply_scoped_event(&ScopedAgentEvent::primary("s", text("dup")));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn layout_cache_invalidates_on_mutation() {
        let mut t = Transcript::new();
        t.apply_agent_event(&text("one"));
        let h1 = t.ensure_layout(40);
        t.apply_agent_event(&text("\ntwo\nthree"));
        let h2 = t.ensure_layout(40);
        assert_eq!(h2, h1 + 2);
    }

    #[test]
    fn replace_and_remove_blocks_keep_order_and_indices() {
        let mut t = Transcript::new();
        t.push(Block::User { text: "a".into() });
        let q = t.push(Block::Queued {
            id: PendingId(1),
            text: "b".into(),
            label: QueuedLabel::Queued,
        });
        t.apply_agent_event(&AgentEvent::ToolCallRequested {
            id: "t1".into(),
            name: "glob".into(),
            args: test_args(json!({})),
        });
        assert_eq!(t.tool_block("t1"), Some(BlockId(2)));
        assert!(t.replace_block(q, Block::User { text: "b".into() }));
        assert_eq!(t.block(q), Some(&Block::User { text: "b".into() }));
        assert!(!t.replace_block(BlockId(9), Block::User { text: "x".into() }));
        assert_eq!(
            t.remove_block(BlockId(1)),
            Some(Block::User { text: "b".into() })
        );
        assert_eq!(t.len(), 2);
        assert_eq!(t.tool_block("t1"), Some(BlockId(1)));
        assert!(t.remove_block(BlockId(5)).is_none());
        assert_eq!(t.ensure_layout(40), t.visible_lines(0, 100).len());
    }

    #[test]
    fn visible_blocks_match_visible_lines() {
        let mut t = Transcript::new();
        t.push(Block::User { text: "a".into() });
        t.push(Block::User {
            text: "b\nc".into(),
        });
        let total = t.ensure_layout(40);
        assert_eq!(total, 5);
        assert_eq!(
            t.visible_blocks(0, 10),
            [0, 0, 1, 1, 1].map(BlockId).to_vec()
        );
        assert_eq!(t.visible_blocks(1, 2), [BlockId(0), BlockId(1)]);
        assert_eq!(t.visible_blocks(0, 10).len(), t.visible_lines(0, 10).len());
    }

    #[test]
    fn tool_output_starts_collapsed_and_toggles() {
        let mut t = Transcript::new();
        t.apply_agent_event(&AgentEvent::ToolCallRequested {
            id: "t1".into(),
            name: "glob".into(),
            args: test_args(json!({})),
        });
        let id = t.tool_block("t1").unwrap();
        // Nothing to show yet: not toggleable.
        assert_eq!(t.toggle_tool(id), None);
        assert_eq!(t.last_tool_with_output(), None);
        t.apply_agent_event(&AgentEvent::ToolExecutionCompleted {
            id: "t1".into(),
            name: "glob".into(),
            is_error: false,
            duration_ms: 1,
        });
        t.apply_agent_event(&test_tool_result("t1", "a\nb"));
        assert!(matches!(
            t.block(id),
            Some(Block::Tool {
                collapsed: true,
                ..
            })
        ));
        assert_eq!(t.ensure_layout(40), 2);
        assert_eq!(t.last_tool_with_output(), Some(id));
        assert_eq!(t.toggle_tool(id), Some(false));
        assert_eq!(t.ensure_layout(40), 4);
        assert_eq!(t.toggle_tool(id), Some(true));
        t.push(Block::User { text: "x".into() });
        assert_eq!(t.toggle_tool(BlockId(1)), None);
    }

    #[test]
    fn result_line_count_reads_the_preview_trailer() {
        assert_eq!(result_line_count("one"), 1);
        assert_eq!(result_line_count("a\nb"), 2);
        let long = (0..57)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(result_line_count(&preview_result(&long)), 57);
        let exact = (0..20)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(result_line_count(&preview_result(&exact)), 20);
    }
}

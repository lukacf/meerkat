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
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::delivery::PendingId;

use super::markdown::truncate_chars;
use super::render::{SPINNER, SPINNER_FRAME_MS, block_lines};

/// Maximum tool-result lines kept in the preview (matches the CLI stream renderer).
pub const MAX_TOOL_RESULT_LINES: usize = 20;
/// Maximum characters of serialized tool args shown in a header (matches the CLI).
pub const MAX_TOOL_ARGS_PREVIEW: usize = 200;
/// Maximum characters of a helper header's last-activity preview.
pub const MAX_HELPER_PREVIEW: usize = 60;
/// Most notes kept for tool calls whose block has not arrived yet.
const MAX_PENDING_NOTES: usize = 64;

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
    /// Waiting for the runtime to admit it.
    Queued,
    /// Admitted; written into the turn at its next model boundary (or run
    /// as a follow-up turn).
    Delivering,
    /// Held by the UI; sent when the current work ends.
    Deferred,
}

impl QueuedLabel {
    /// Text shown after the message.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Delivering => "delivering…",
            Self::Deferred => "will send after this turn",
        }
    }
}

/// How helper headers start: folded to one line (the default) or showing the
/// helper's nested blocks (`tui.toml` `[ui] helpers`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HelperView {
    /// One header line per helper; open one to watch it.
    #[default]
    Folded,
    /// Every helper shows its nested blocks.
    Expanded,
}

/// Lifecycle state of a helper, as its header shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperState {
    /// The helper is working.
    Running,
    /// Its run completed (or it left the running helpers).
    Done,
    /// Its run failed.
    Failed,
    /// Its run was cancelled.
    Cancelled,
    /// Its events could not be followed: its usage is unmeasured.
    Unmeasured,
}

impl HelperState {
    /// The word the header shows.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unmeasured => "unmeasured",
        }
    }

    /// Whether the helper is still working.
    #[must_use]
    pub fn is_running(self) -> bool {
        self == Self::Running
    }
}

/// What a helper did last (the running header's preview).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HelperLast {
    /// Nothing yet.
    #[default]
    Nothing,
    /// A tool call: its name and a short argument preview.
    Tool {
        /// Tool name.
        name: String,
        /// Short argument preview (the first string argument, quoted).
        args: String,
    },
    /// The first line of its latest text.
    Text(String),
}

/// A helper's header: one line standing for everything the helper did.
///
/// The helper's own blocks (tool calls, errors, the headers of helpers it
/// spawned) stay in the transcript, owned by this header; the renderer skips
/// them while the header is folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperFold {
    /// Member identity (`scout`, `fork-1`).
    pub label: String,
    /// The model it runs on, when known.
    pub model: Option<String>,
    /// Lifecycle state.
    pub state: HelperState,
    /// Tool calls requested so far.
    pub calls: usize,
    /// Its latest activity.
    pub last: HelperLast,
    /// Whether its nested blocks are shown.
    pub expanded: bool,
    /// Tokens (input + output) of its model calls, when any were reported.
    pub usage: Option<u64>,
    /// Nesting depth (1 for a helper of the session's own agent).
    pub depth: usize,
    /// Spinner frame of a running helper.
    pub spin: usize,
}

impl HelperFold {
    /// `[label · model]`, or `[label]` while the model is unknown.
    #[must_use]
    pub fn tag(&self) -> String {
        match &self.model {
            Some(model) => format!("[{} · {model}]", self.label),
            None => format!("[{}]", self.label),
        }
    }

    /// The header's status text after the tag: `running · 4 tool calls ·
    /// grep "fn resolve"`, `done · 7 tool calls · 2.1k tokens`.
    #[must_use]
    pub fn status_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        if self.state.is_running() {
            let glyph = SPINNER.get(self.spin % SPINNER.len()).copied().unwrap_or('⠋');
            out.push(glyph);
            out.push(' ');
        }
        out.push_str(self.state.text());
        let plural = if self.calls == 1 { "" } else { "s" };
        let _ = write!(out, " · {} tool call{plural}", self.calls);
        if self.state.is_running() {
            match &self.last {
                HelperLast::Nothing => {}
                HelperLast::Tool { name, args } if args.is_empty() => {
                    let _ = write!(out, " · {name}");
                }
                HelperLast::Tool { name, args } => {
                    let _ = write!(out, " · {name} {args}");
                }
                HelperLast::Text(text) => {
                    let _ = write!(out, " · {text}");
                }
            }
        } else if let Some(tokens) = self.usage {
            let _ = write!(out, " · {} tokens", super::render::fmt_tokens(tokens));
        }
        out
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
        /// One dim line under the header, shown even when collapsed (e.g.
        /// `allowed by classifier: <reason>`).
        note: Option<String>,
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
    /// A helper's header (see [`HelperFold`]).
    Helper(HelperFold),
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
    /// The model of each helper (by label) the UI has heard of, for block
    /// labels.
    helper_models: IndexMap<String, String>,
    /// Per block, the header of the helper it belongs to (`None`: the
    /// session's own blocks and top-level helper headers).
    owner: Vec<Option<BlockId>>,
    /// The current header of each helper, by label.
    helper_folds: IndexMap<String, BlockId>,
    /// Helpers listed as running by the last live update.
    live_helpers: Vec<String>,
    /// How new helper headers start.
    helper_view: HelperView,
    /// Notes for tool calls whose block has not arrived yet, by tool-use id
    /// (at most [`MAX_PENDING_NOTES`], oldest dropped first).
    pending_notes: IndexMap<String, String>,
    /// The spinner clock of running helper headers.
    helper_clock: Option<Instant>,
    /// Number of helper headers.
    helper_count: usize,
}

impl Transcript {
    /// Create an empty transcript.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set how new helper headers start (folded or expanded).
    pub fn set_helper_view(&mut self, view: HelperView) {
        self.helper_view = view;
    }

    /// Remember that the helper `label` runs on `model` (its blocks name it).
    pub fn note_helper_model(&mut self, label: &str, model: &str) {
        self.helper_models
            .insert(label.to_string(), model.to_string());
        if let Some(id) = self.helper_folds.get(label).copied()
            && let Some(Block::Helper(fold)) = self.block(id)
            && fold.model.as_deref() != Some(model)
            && let Some(Block::Helper(fold)) = self.block_mut(id)
        {
            fold.model = Some(model.to_string());
        }
    }

    /// The helpers running now, `(label, model, depth)`: notes their models,
    /// gives a newly spawned top-level helper its header at once, marks a
    /// helper that is no longer listed done, and lets a helper whose label
    /// is spawned again after it finished get a fresh header.
    pub fn note_live_helpers<'a>(
        &mut self,
        live: impl IntoIterator<Item = (&'a str, &'a str, usize)>,
    ) {
        let mut now_live = Vec::new();
        for (label, model, depth) in live {
            self.note_helper_model(label, model);
            let fresh = !self.live_helpers.iter().any(|l| l == label);
            if fresh
                && let Some(id) = self.helper_folds.get(label).copied()
                && matches!(self.block(id), Some(Block::Helper(f)) if !f.state.is_running())
            {
                self.helper_folds.shift_remove(label);
            }
            if depth == 1 {
                self.ensure_fold(&[label]);
            }
            now_live.push(label.to_string());
        }
        let gone: Vec<BlockId> = self
            .live_helpers
            .iter()
            .filter(|label| !now_live.contains(label))
            .filter_map(|label| self.helper_folds.get(label.as_str()).copied())
            .collect();
        for id in gone {
            self.update_fold(id, |fold| {
                if fold.state.is_running() {
                    fold.state = HelperState::Done;
                }
            });
        }
        self.live_helpers = now_live;
    }

    /// Helper `label`'s usage could not be observed: its header says so
    /// (a header is made if it has none).
    pub fn helper_unmeasured(&mut self, label: &str) {
        let id = self.ensure_fold(&[label]);
        self.update_fold(id, |fold| {
            if matches!(fold.state, HelperState::Running | HelperState::Done) {
                fold.state = HelperState::Unmeasured;
            }
        });
    }

    /// Advance the spinner of running helper headers. Returns whether any
    /// header changed.
    pub fn tick_helpers(&mut self, now: Instant) -> bool {
        let Some(clock) = self.helper_clock else {
            return false;
        };
        let elapsed = now.saturating_duration_since(clock).as_millis();
        let frame = (elapsed / SPINNER_FRAME_MS) as usize % SPINNER.len();
        let mut changed = false;
        for i in 0..self.helper_folds.len() {
            let Some((_, id)) = self.helper_folds.get_index(i) else {
                continue;
            };
            let id = *id;
            if matches!(self.block(id), Some(Block::Helper(f)) if f.state.is_running() && f.spin != frame)
            {
                self.update_fold(id, |fold| fold.spin = frame);
                changed = true;
            }
        }
        changed
    }

    /// The helper headers, in transcript order.
    pub fn helper_headers(&self) -> impl Iterator<Item = (BlockId, &HelperFold)> {
        self.blocks.iter().enumerate().filter_map(|(i, b)| match b {
            Block::Helper(fold) => Some((BlockId(i), fold)),
            _ => None,
        })
    }

    /// The helper headers on screen (not inside a folded helper), in
    /// transcript order.
    pub fn visible_helper_headers(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.helper_headers()
            .map(|(id, _)| id)
            .filter(|id| !self.is_hidden(*id))
    }

    /// The newest header of the helper `label`.
    pub fn helper_header(&self, label: &str) -> Option<BlockId> {
        self.helper_headers()
            .filter(|(_, fold)| fold.label == label)
            .map(|(id, _)| id)
            .last()
    }

    /// Whether the transcript has any helper header.
    pub fn has_helpers(&self) -> bool {
        self.helper_count > 0
    }

    /// Expand or fold the helper header `id`. Returns the new `expanded`
    /// state, or `None` when `id` is not a helper header.
    pub fn toggle_helper(&mut self, id: BlockId) -> Option<bool> {
        let expanded = match self.block(id) {
            Some(Block::Helper(fold)) => !fold.expanded,
            _ => return None,
        };
        self.set_helper_expanded(id, expanded);
        Some(expanded)
    }

    /// Expand (`true`) or fold the helper header `id`. Returns whether `id`
    /// is a helper header.
    pub fn set_helper_expanded(&mut self, id: BlockId, expanded: bool) -> bool {
        if !matches!(self.block(id), Some(Block::Helper(_))) {
            return false;
        }
        self.update_fold(id, |fold| fold.expanded = expanded);
        // Every block inside the helper appears or disappears.
        for i in 0..self.blocks.len() {
            if self.owned_by(BlockId(i), id)
                && let Some(slot) = self.layout.get_mut(i)
            {
                *slot = None;
            }
        }
        true
    }

    /// Whether `block` sits (at any depth) inside the helper header `header`.
    fn owned_by(&self, block: BlockId, header: BlockId) -> bool {
        let mut at = block;
        while let Some(Some(up)) = self.owner.get(at.0) {
            if *up == header {
                return true;
            }
            at = *up;
        }
        false
    }

    /// Whether `block` is inside a folded helper (the renderer skips it).
    pub fn is_hidden(&self, block: BlockId) -> bool {
        let mut at = block;
        while let Some(Some(up)) = self.owner.get(at.0) {
            if matches!(self.block(*up), Some(Block::Helper(f)) if !f.expanded) {
                return true;
            }
            at = *up;
        }
        false
    }

    /// The header of the helper `path` (outermost first), made (with the
    /// headers of the helpers above it) when missing.
    fn ensure_fold(&mut self, path: &[&str]) -> BlockId {
        let Some((label, parents)) = path.split_last() else {
            return BlockId(usize::MAX);
        };
        if let Some(id) = self.helper_folds.get(*label) {
            return *id;
        }
        let parent = (!parents.is_empty()).then(|| self.ensure_fold(parents));
        let id = self.push(Block::Helper(HelperFold {
            label: (*label).to_string(),
            model: self.helper_model(label).map(str::to_string),
            state: HelperState::Running,
            calls: 0,
            last: HelperLast::Nothing,
            expanded: self.helper_view == HelperView::Expanded,
            usage: None,
            depth: path.len(),
            spin: 0,
        }));
        self.set_owner(id, parent);
        self.helper_folds.insert((*label).to_string(), id);
        if self.helper_clock.is_none() {
            self.helper_clock = Some(Instant::now());
        }
        id
    }

    fn set_owner(&mut self, id: BlockId, owner: Option<BlockId>) {
        if let Some(slot) = self.owner.get_mut(id.0) {
            *slot = owner;
        }
    }

    fn update_fold(&mut self, id: BlockId, f: impl FnOnce(&mut HelperFold)) {
        if let Some(Block::Helper(fold)) = self.block_mut(id) {
            f(fold);
        }
    }

    /// The model of the helper `label`, when known.
    pub fn helper_model(&self, label: &str) -> Option<&str> {
        self.helper_models.get(label).map(String::as_str)
    }

    /// `[label · model]`, or `[label]` while the model is unknown.
    fn helper_tag(&self, label: &str) -> String {
        match self.helper_model(label) {
            Some(model) => format!("[{label} · {model}]"),
            None => format!("[{label}]"),
        }
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

    /// Remove every block (the helper view setting stays).
    pub fn clear(&mut self) {
        let helper_view = self.helper_view;
        *self = Self::default();
        self.helper_view = helper_view;
    }

    /// Replace the whole transcript with `blocks` (e.g. after resuming a
    /// session). Helper headers from history start folded and finished.
    pub fn reset(&mut self, blocks: Vec<Block>) {
        self.clear();
        for mut block in blocks {
            if let Block::Helper(fold) = &mut block {
                fold.expanded = false;
                if fold.state.is_running() {
                    fold.state = HelperState::Done;
                }
            }
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
        if matches!(block, Block::Helper(_)) {
            self.helper_count = self.helper_count.saturating_sub(1);
        }
        if id.0 < self.layout.len() {
            self.layout.remove(id.0);
        }
        if id.0 < self.owner.len() {
            self.owner.remove(id.0);
        }
        let shift = |b: BlockId| if b.0 > id.0 { BlockId(b.0 - 1) } else { b };
        self.by_tool_id.retain(|_, b| *b != id);
        for b in self.by_tool_id.values_mut() {
            *b = shift(*b);
        }
        self.helper_folds.retain(|_, b| *b != id);
        for b in self.helper_folds.values_mut() {
            *b = shift(*b);
        }
        for slot in &mut self.owner {
            *slot = slot.filter(|b| *b != id).map(shift);
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

    /// The most recent tool block on screen with output to show or hide.
    pub fn last_tool_with_output(&self) -> Option<BlockId> {
        self.blocks
            .iter()
            .enumerate()
            .rev()
            .find(|(i, b)| tool_has_output(b) && !self.is_hidden(BlockId(*i)))
            .map(|(i, _)| BlockId(i))
    }

    /// The tool block of a call by its tool-use id as the model issued it: a
    /// session call is keyed by that id, a helper's by `{scope}/{id}`.
    fn tool_block_by_call_id(&self, tool_use_id: &str) -> Option<BlockId> {
        self.tool_block(tool_use_id).or_else(|| {
            self.by_tool_id
                .iter()
                .rev()
                .find(|(key, _)| {
                    key.rsplit_once('/')
                        .is_some_and(|(_, call)| call == tool_use_id)
                })
                .map(|(_, id)| *id)
        })
    }

    /// Attach a one-line note to the tool block of `tool_use_id` (a session
    /// call's or a helper's), shown dim under its header. A note for a call
    /// whose block has not arrived yet waits for it.
    pub fn attach_note(&mut self, tool_use_id: &str, text: String) {
        if let Some(id) = self.tool_block_by_call_id(tool_use_id)
            && let Some(Block::Tool { note, .. }) = self.block_mut(id)
        {
            *note = Some(text);
            return;
        }
        if self.pending_notes.len() >= MAX_PENDING_NOTES {
            self.pending_notes.shift_remove_index(0);
        }
        self.pending_notes.insert(tool_use_id.to_string(), text);
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

    /// Ensure every block has a layout at `width`; returns the total line
    /// count. Blocks inside a folded helper lay out to no lines.
    pub fn ensure_layout(&mut self, width: u16) -> usize {
        let mut total = 0;
        for i in 0..self.blocks.len() {
            let stale = self
                .layout
                .get(i)
                .is_some_and(|slot| slot.as_ref().is_none_or(|c| c.width != width));
            if stale {
                let lines = match self.blocks.get(i) {
                    Some(_) if self.is_hidden(BlockId(i)) => Vec::new(),
                    Some(block) => block_lines(block, width),
                    None => Vec::new(),
                };
                if let Some(slot) = self.layout.get_mut(i) {
                    *slot = Some(CachedLayout { width, lines });
                }
            }
            total += self
                .layout
                .get(i)
                .and_then(Option::as_ref)
                .map_or(0, |c| c.lines.len());
        }
        total
    }

    /// The first laid-out line of block `id`, as of the last
    /// [`Self::ensure_layout`].
    pub fn first_line_of(&self, id: BlockId) -> Option<usize> {
        if id.0 >= self.layout.len() {
            return None;
        }
        Some(
            self.layout
                .iter()
                .take(id.0)
                .flatten()
                .map(|c| c.lines.len())
                .sum(),
        )
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
    ///
    /// The helper's blocks belong to its header (made when missing, nested
    /// in its parent helper's), which also counts its calls, its usage and
    /// its latest activity.
    pub fn apply_scoped_event_at(&mut self, scoped: &ScopedAgentEvent, now: Instant) {
        let path: Vec<&str> = scoped
            .scope_path
            .iter()
            .filter_map(|frame| match frame {
                StreamScopeFrame::MobMember { agent_identity, .. } => {
                    Some(agent_identity.as_str())
                }
                _ => None,
            })
            .collect();
        let Some(label) = path.last().copied() else {
            return;
        };
        let depth = path.len();
        let fold = self.ensure_fold(&path);
        let tag = self.helper_tag(label);
        let key = |id: &str| format!("{}/{id}", scoped.scope_id);
        match &scoped.event {
            AgentEvent::RunStarted { .. } => self.update_fold(fold, |f| {
                if matches!(
                    f.state,
                    HelperState::Done | HelperState::Failed | HelperState::Cancelled
                ) {
                    f.state = HelperState::Running;
                }
            }),
            AgentEvent::RunCompleted { .. } => self.update_fold(fold, |f| {
                if f.state.is_running() {
                    f.state = HelperState::Done;
                }
            }),
            AgentEvent::TurnCompleted {
                usage: Some(usage), ..
            } => {
                let tokens = usage.input_tokens.saturating_add(usage.output_tokens);
                self.update_fold(fold, |f| {
                    f.usage = Some(f.usage.unwrap_or(0).saturating_add(tokens));
                });
            }
            AgentEvent::TextComplete { content, .. } => {
                if let Some(line) = content.lines().find(|l| !l.trim().is_empty()) {
                    let line = truncate_chars(line.trim(), MAX_HELPER_PREVIEW);
                    self.update_fold(fold, |f| f.last = HelperLast::Text(line));
                }
            }
            AgentEvent::ToolCallRequested { id, name, args } => {
                let preview = helper_args_preview(args.as_value());
                self.update_fold(fold, |f| {
                    f.calls += 1;
                    f.last = HelperLast::Tool {
                        name: name.clone(),
                        args: preview,
                    };
                });
                let block = self.request_tool(
                    key(id),
                    format!("{tag} {name}"),
                    args.as_value().clone(),
                    depth,
                );
                self.set_owner(block, Some(fold));
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
                let state = if error_report.class == meerkat_core::AgentErrorClass::Cancelled {
                    HelperState::Cancelled
                } else {
                    HelperState::Failed
                };
                self.update_fold(fold, |f| f.state = state);
                let notice = self.push_notice(
                    NoticeLevel::Error,
                    format!("{tag} error: {}", error_report.message),
                );
                self.set_owner(notice, Some(fold));
            }
            _ => {}
        }
    }

    fn push_raw(&mut self, block: Block) -> BlockId {
        let id = BlockId(self.blocks.len());
        if matches!(block, Block::Helper(_)) {
            self.helper_count += 1;
        }
        self.blocks.push(block);
        self.layout.push(None);
        self.owner.push(None);
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

    fn request_tool(&mut self, key: String, name: String, args: Value, depth: usize) -> BlockId {
        let note = self.pending_notes.shift_remove(&key).or_else(|| {
            key.rsplit_once('/')
                .and_then(|(_, call)| self.pending_notes.shift_remove(call))
        });
        let id = self.push(Block::Tool {
            name,
            args,
            state: ToolState::Requested,
            result_preview: None,
            diff: None,
            note,
            depth,
            collapsed: true,
        });
        self.by_tool_id.insert(key, id);
        id
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

/// A helper header's preview of a call's arguments: the first string
/// argument, quoted and shortened (`"fn resolve"`), else the serialized
/// arguments shortened; empty for no arguments.
fn helper_args_preview(args: &Value) -> String {
    /// Arguments that say most about a call, checked first (a JSON object's
    /// own key order is not kept).
    const TELLING: [&str; 6] = ["command", "pattern", "query", "path", "file_path", "url"];
    let first_str = args.as_object().and_then(|map| {
        TELLING
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str))
            .or_else(|| map.values().find_map(Value::as_str))
    });
    match first_str {
        Some(s) => {
            let line = s.lines().next().unwrap_or_default();
            format!("\"{}\"", truncate_chars(line, MAX_HELPER_PREVIEW))
        }
        None if args.as_object().is_some_and(serde_json::Map::is_empty) || args.is_null() => {
            String::new()
        }
        None => truncate_chars(&serde_json::to_string(args).unwrap_or_default(), MAX_HELPER_PREVIEW),
    }
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
/// Model fallback, background jobs, tool-config changes, peer content, run
/// resumption and request-bounded tool results become notices. A steered
/// message's `boundary_append_applied` yields `None`: the runner maps it to
/// [`super::Msg::SteerDelivered`] and its placeholder becomes the user block.
/// Events with no user-facing meaning (run/hook/interaction lifecycle,
/// accounting disputes, transcript rewrites, cache breakpoints, reasoning
/// lowering) yield `None`.
fn upstream_notice(event: &AgentEvent) -> Option<(NoticeLevel, String)> {
    let notice = match event {
        AgentEvent::BoundaryAppendsDiscarded(discarded) => (
            NoticeLevel::Info,
            format!(
                "↺ {} message(s) taken back out of the stopped step; the runtime delivers them again",
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
        // The helper's header, then its tool block.
        assert!(matches!(&t.blocks()[0], Block::Helper(fold) if fold.label == "helper"));
        assert!(matches!(
            &t.blocks()[1],
            Block::Tool { name, depth: 1, .. } if name == "[helper] glob"
        ));
        // Primary-only scope is ignored.
        t.apply_scoped_event(&ScopedAgentEvent::primary("s", text("dup")));
        assert_eq!(t.len(), 2);
        // Once its model is known, a helper's blocks name it; a helper of a
        // helper nests one level deeper.
        t.note_helper_model("inner", "gpt-5.5");
        let mut nested = ev.clone();
        nested.scope_path.push(StreamScopeFrame::MobMember {
            flow_run_id: String::new(),
            agent_identity: "inner".into(),
            agent_runtime_id: None,
            fence_token: None,
            generation: None,
        });
        nested.scope_id = ScopedAgentEvent::scope_id_from_path(&nested.scope_path);
        t.apply_scoped_event(&nested);
        assert!(matches!(
            &t.blocks()[3],
            Block::Tool { name, depth: 2, .. } if name == "[inner · gpt-5.5] glob"
        ));
    }

    /// `event` of the helper `path` (outermost first).
    fn helper_event(path: &[&str], event: AgentEvent) -> ScopedAgentEvent {
        let mut frames = vec![StreamScopeFrame::Primary {
            session_id: "s".into(),
        }];
        frames.extend(path.iter().map(|label| StreamScopeFrame::MobMember {
            flow_run_id: String::new(),
            agent_identity: (*label).into(),
            agent_runtime_id: None,
            fence_token: None,
            generation: None,
        }));
        ScopedAgentEvent::new(frames, event)
    }

    fn helper_call(path: &[&str], id: &str, name: &str, args: Value) -> ScopedAgentEvent {
        helper_event(
            path,
            AgentEvent::ToolCallRequested {
                id: id.into(),
                name: name.into(),
                args: test_args(args),
            },
        )
    }

    fn fold_of(t: &Transcript, label: &str) -> HelperFold {
        let id = t.helper_header(label).expect("helper header");
        match t.block(id) {
            Some(Block::Helper(fold)) => fold.clone(),
            other => panic!("not a helper header: {other:?}"),
        }
    }

    fn note_of(t: &Transcript, id: BlockId) -> Option<String> {
        match t.block(id) {
            Some(Block::Tool { note, .. }) => note.clone(),
            other => panic!("not a tool block: {other:?}"),
        }
    }

    #[test]
    fn notes_attach_to_tool_blocks_by_call_id() {
        let mut t = Transcript::new();
        t.apply_agent_event(&AgentEvent::ToolCallRequested {
            id: "t1".into(),
            name: "shell".into(),
            args: test_args(json!({"command": "rm -rf scratch.txt"})),
        });
        t.attach_note("t1", "allowed by classifier: in-project cleanup".into());
        assert_eq!(
            note_of(&t, BlockId(0)).as_deref(),
            Some("allowed by classifier: in-project cleanup")
        );
        // The note is one dim line under the header, shown while collapsed.
        let lines = block_lines(t.block(BlockId(0)).unwrap(), 80);
        let second: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(second, "  allowed by classifier: in-project cleanup");
        let dim = |style: ratatui::style::Style| {
            style
                .add_modifier
                .contains(ratatui::style::Modifier::DIM)
        };
        assert!(dim(lines[1].style) || lines[1].spans.iter().all(|s| dim(s.style)));
        // A note that arrives before its call waits for the block.
        t.attach_note("t2", "allowed by classifier: early".into());
        t.apply_agent_event(&AgentEvent::ToolCallRequested {
            id: "t2".into(),
            name: "shell".into(),
            args: test_args(json!({"command": "ls"})),
        });
        assert_eq!(
            note_of(&t, BlockId(1)).as_deref(),
            Some("allowed by classifier: early")
        );
        // A call without a note has none.
        t.apply_agent_event(&AgentEvent::ToolCallRequested {
            id: "t3".into(),
            name: "read_file".into(),
            args: test_args(json!({"path": "a.rs"})),
        });
        assert_eq!(note_of(&t, BlockId(2)), None);
    }

    #[test]
    fn a_helper_calls_note_hides_with_its_fold() {
        let mut t = Transcript::new();
        t.push(Block::User { text: "go".into() });
        t.apply_scoped_event(&helper_call(&["scout"], "h1", "shell", json!({"command": "make"})));
        let tool = BlockId(2);
        // The engine names the call by the id the helper's model issued.
        t.attach_note("h1", "allowed by classifier: builds the project".into());
        assert_eq!(
            note_of(&t, tool).as_deref(),
            Some("allowed by classifier: builds the project")
        );
        // Folded: the user block and the header only.
        assert_eq!(t.ensure_layout(80), 2 + 2);
        assert!(t.is_hidden(tool));
        let header = t.helper_header("scout").unwrap();
        assert!(t.set_helper_expanded(header, true));
        // Expanded: the tool block shows header, note and spacer.
        assert_eq!(t.ensure_layout(80), 2 + 2 + 3);
    }

    #[test]
    fn helper_events_fold_into_one_header() {
        let mut t = Transcript::new();
        t.note_helper_model("scout", "claude-opus-5-5");
        t.push(Block::User { text: "go".into() });
        t.apply_scoped_event(&helper_call(&["scout"], "a", "read_file", json!({"path": "a.rs"})));
        t.apply_scoped_event(&helper_call(
            &["scout"],
            "b",
            "grep",
            json!({"pattern": "fn resolve", "path": "src"}),
        ));
        t.apply_scoped_event(&helper_event(&["scout"], test_tool_result("b", "x\ny")));
        // The header first, then its two tool blocks.
        assert_eq!(t.len(), 4);
        let fold = fold_of(&t, "scout");
        assert_eq!(t.helper_header("scout"), Some(BlockId(1)));
        assert_eq!(fold.calls, 2);
        assert_eq!(fold.state, HelperState::Running);
        assert!(!fold.expanded);
        assert_eq!(fold.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(
            fold.last,
            HelperLast::Tool {
                name: "grep".into(),
                args: "\"fn resolve\"".into()
            }
        );
        assert_eq!(
            fold.status_text(),
            "⠋ running · 2 tool calls · grep \"fn resolve\""
        );
        // Folded: only the user block and the header lay out.
        let folded = t.ensure_layout(80);
        assert_eq!(folded, 2 + 2);
        assert!(t.is_hidden(BlockId(2)) && t.is_hidden(BlockId(3)));
        assert_eq!(t.last_tool_with_output(), None);
        // Text replaces the preview with its first line.
        t.apply_scoped_event(&helper_event(
            &["scout"],
            AgentEvent::TextComplete {
                content: "\nFound it in resolve.rs.\nMore.".into(),
                assistant_message_id: None,
            },
        ));
        assert_eq!(
            fold_of(&t, "scout").last,
            HelperLast::Text("Found it in resolve.rs.".into())
        );
    }

    #[test]
    fn expanding_a_helper_reveals_its_blocks() {
        let mut t = Transcript::new();
        t.apply_scoped_event(&helper_call(&["scout"], "a", "glob", json!({"pattern": "*"})));
        t.apply_scoped_event(&helper_event(&["scout"], test_tool_result("a", "one")));
        let header = t.helper_header("scout").unwrap();
        assert_eq!(t.ensure_layout(80), 2);
        assert_eq!(t.toggle_helper(header), Some(true));
        assert_eq!(t.ensure_layout(80), 4);
        assert!(!t.is_hidden(BlockId(1)));
        assert_eq!(t.last_tool_with_output(), Some(BlockId(1)));
        assert_eq!(t.toggle_helper(header), Some(false));
        assert_eq!(t.ensure_layout(80), 2);
        // Not a header.
        assert_eq!(t.toggle_helper(BlockId(1)), None);
        // The expanded setting starts new headers open.
        let mut t = Transcript::new();
        t.set_helper_view(HelperView::Expanded);
        t.apply_scoped_event(&helper_call(&["scout"], "a", "glob", json!({})));
        assert!(fold_of(&t, "scout").expanded);
        assert_eq!(t.ensure_layout(80), 4);
    }

    #[test]
    fn a_nested_helper_folds_inside_its_parent() {
        let mut t = Transcript::new();
        t.apply_scoped_event(&helper_call(&["outer"], "a", "glob", json!({})));
        t.apply_scoped_event(&helper_call(&["outer", "inner"], "b", "grep", json!({})));
        // outer header, its tool, inner header, inner's tool.
        assert_eq!(t.len(), 4);
        let outer = t.helper_header("outer").unwrap();
        let inner = t.helper_header("inner").unwrap();
        assert_eq!(fold_of(&t, "inner").depth, 2);
        assert!(t.is_hidden(inner));
        assert_eq!(t.ensure_layout(80), 2);
        // Opening the parent shows the nested helper folded.
        t.toggle_helper(outer);
        assert!(!t.is_hidden(inner));
        assert!(t.is_hidden(BlockId(3)));
        assert_eq!(t.ensure_layout(80), 6);
        assert_eq!(t.visible_helper_headers().collect::<Vec<_>>(), vec![outer, inner]);
        t.toggle_helper(inner);
        assert_eq!(t.ensure_layout(80), 8);
        // Folding the parent hides everything below it again.
        t.toggle_helper(outer);
        assert_eq!(t.ensure_layout(80), 2);
        assert_eq!(t.visible_helper_headers().collect::<Vec<_>>(), vec![outer]);
        // The parent's own call count is its own.
        assert_eq!(fold_of(&t, "outer").calls, 1);
        assert_eq!(fold_of(&t, "inner").calls, 1);
    }

    #[test]
    fn helper_finished_states_and_tokens() {
        let mut t = Transcript::new();
        t.note_live_helpers([("scout", "m", 1)]);
        // A newly live helper gets its header at once.
        assert_eq!(fold_of(&t, "scout").state, HelperState::Running);
        t.apply_scoped_event(&helper_call(&["scout"], "a", "glob", json!({})));
        t.apply_scoped_event(&helper_event(
            &["scout"],
            AgentEvent::TurnCompleted {
                stop_reason: StopReason::EndTurn,
                usage: test_usage(Usage {
                    input_tokens: 2000,
                    output_tokens: 100,
                    ..Usage::default()
                }),
                assistant_message_id: None,
            },
        ));
        t.apply_scoped_event(&helper_event(
            &["scout"],
            AgentEvent::RunCompleted {
                session_id: meerkat_core::SessionId::new(),
                identity: Default::default(),
                result: "ok".into(),
                structured_output: None,
                extraction_required: false,
                usage: Default::default(),
                terminal_cause_kind: None,
                assistant_message_id: None,
            },
        ));
        let fold = fold_of(&t, "scout");
        assert_eq!(fold.state, HelperState::Done);
        assert_eq!(fold.usage, Some(2100));
        assert_eq!(fold.status_text(), "done · 1 tool call · 2.1k tokens");
        // Leaving the live list keeps it done; a new spawn of the label gets
        // a fresh header.
        t.note_live_helpers(std::iter::empty());
        t.note_live_helpers([("scout", "m", 1)]);
        assert_eq!(t.helper_headers().count(), 2);
        assert_eq!(fold_of(&t, "scout").state, HelperState::Running);
        // A failure and a cancellation.
        let failed = |class| {
            helper_event(
                &["scout"],
                AgentEvent::RunFailed {
                    session_id: meerkat_core::SessionId::new(),
                    identity: Default::default(),
                    error_report: meerkat_core::AgentErrorReport {
                        class,
                        reason: None,
                        message: "dead".into(),
                    },
                    terminal_cause_kind: None,
                },
            )
        };
        t.apply_scoped_event(&failed(meerkat_core::AgentErrorClass::Llm));
        assert_eq!(fold_of(&t, "scout").state, HelperState::Failed);
        assert_eq!(fold_of(&t, "scout").status_text(), "failed · 0 tool calls");
        // The error notice belongs to the helper.
        let last = BlockId(t.len() - 1);
        assert!(matches!(t.block(last), Some(Block::Notice { .. })));
        assert!(t.is_hidden(last));
        t.apply_scoped_event(&failed(meerkat_core::AgentErrorClass::Cancelled));
        assert_eq!(fold_of(&t, "scout").state, HelperState::Cancelled);
        // A helper that left the live list while running is done; one whose
        // events could not be followed is unmeasured.
        t.note_live_helpers([("other", "m", 1)]);
        t.note_live_helpers(std::iter::empty());
        assert_eq!(fold_of(&t, "other").state, HelperState::Done);
        t.helper_unmeasured("lost");
        assert_eq!(fold_of(&t, "lost").state, HelperState::Unmeasured);
        assert_eq!(fold_of(&t, "lost").status_text(), "unmeasured · 0 tool calls");
    }

    #[test]
    fn running_helpers_spin_and_history_helpers_start_folded_and_finished() {
        let mut t = Transcript::new();
        let start = Instant::now();
        t.apply_scoped_event(&helper_call(&["scout"], "a", "glob", json!({})));
        let later = start + std::time::Duration::from_secs(1);
        assert!(t.tick_helpers(later));
        assert_ne!(fold_of(&t, "scout").spin, 0);
        // A resumed transcript's helper headers are folded and finished.
        let mut fold = fold_of(&t, "scout");
        fold.expanded = true;
        t.reset(vec![Block::Helper(fold)]);
        let fold = fold_of(&t, "scout");
        assert!(!fold.expanded);
        assert_eq!(fold.state, HelperState::Done);
        assert!(t.has_helpers());
        assert!(!t.tick_helpers(later));
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

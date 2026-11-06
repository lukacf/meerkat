//! ratatui application: Elm-style state + pure `update`, a renderer and a terminal runtime.
//!
//! [`App::update`] is pure — it never touches the terminal or performs I/O — and
//! returns [`Effect`]s that the integrator (session/approval wiring) executes.
//! Everything the integrator wants to show is pushed back in as a [`Msg`].

pub mod commands;
pub mod compaction;
pub mod cost;
pub mod delivery;
pub mod history;
pub mod input;
pub mod keymap;
pub mod markdown;
pub mod modal;
pub mod render;
pub mod runtime;
pub mod search;
pub mod selection;
pub mod transcript;

use crate::delivery::{PendingId, SteerFailure};
use crate::models::ModelGroup;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use meerkat_core::{AgentEvent, EventEnvelope, ScopedAgentEvent};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::text::Line;

pub use commands::{
    AutoCommand, Command, CommandError, HelperTarget, HelpersCommand, ModelCommand, RuleEffect,
    RulesCommand,
};
pub use compaction::{
    COMPACT_NOW, CompactFailure, CompactMode, CompactionGauge, CompactionPolicy, CompactionTrigger,
};
pub use cost::{CostConfidence, CostReadout, SessionCost, SessionCostSnapshot, UsageTotals};
pub use delivery::{Pending, PendingQueue, PendingState};
pub use history::History;
pub use input::InputEditor;
pub use keymap::{Action, EditAction};
pub use modal::{
    EscalationAnswer, EscalationId, EscalationPrompt, Modal, ModelPicker, ResumePicker,
};
pub use runtime::{TerminalGuard, UiChannels, UiError, install_panic_hook, run_ui};
pub use search::{HistorySearch, SearchOutcome};
pub use selection::{CellPos, SelectMode, Selection};
pub use transcript::{
    Block, BlockId, DiffView, HelperFold, HelperLast, HelperState, HelperView, NoticeLevel,
    QueuedLabel, ToolState, Transcript,
};

/// Window in which a second Ctrl-C on an idle, empty prompt quits.
pub const DOUBLE_CTRL_C_WINDOW: Duration = Duration::from_millis(1500);

/// Window in which a second Ctrl-D / `/quit` confirms quitting while a turn
/// is running or messages are queued.
pub const QUIT_CONFIRM_WINDOW: Duration = Duration::from_secs(3);

/// Window after Ctrl-X in which the second key of a chord is accepted.
pub const CHORD_WINDOW: Duration = Duration::from_millis(1500);

/// Window in which a second (third) left press on the same cell is a double
/// (triple) click.
pub const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// A left button press not yet released.
#[derive(Debug, Clone, Copy)]
struct Press {
    /// Cell pressed.
    pos: CellPos,
    /// Screen row pressed (for click-to-expand).
    row: u16,
    /// 1 = single, 2 = double, 3 = triple click.
    clicks: u8,
    /// Whether the pointer moved with the button held.
    dragged: bool,
}

/// The last left press, for multi-click detection.
#[derive(Debug, Clone, Copy)]
struct LastDown {
    at: Instant,
    row: u16,
    column: u16,
    clicks: u8,
}

/// How keys reach the input area.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum InputMode {
    /// Keys edit the prompt.
    #[default]
    Normal,
    /// Keys drive a reverse history search (Ctrl-R).
    Search(HistorySearch),
}

/// Result of a finished turn, reported by the runner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnSummary {
    /// Final assistant text.
    pub text: String,
    /// Input tokens consumed by the run.
    pub input_tokens: u64,
    /// Output tokens produced by the run.
    pub output_tokens: u64,
}

/// How a turn ended without a result. With `Ok(TurnSummary)` (completed)
/// these are the turn's typed ends.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TurnFailure {
    /// An input of the turn failed (the first failure wins).
    #[error("{0}")]
    Failed(String),
    /// The user interrupted the turn (the runtime reported the inputs
    /// cancelled).
    #[error("interrupted")]
    Interrupted,
    /// What the runtime did with an input is not known (a receipt without
    /// an outcome, a completion channel that closed, an input reported
    /// terminal at admission without an outcome). Never resubmit it.
    #[error("outcome unknown: {0}")]
    Unknown(String),
    /// The turn waits for an external tool result rkat-tui does not provide.
    #[error("pending: the turn waits for an external result for `{tool}`")]
    Pending {
        /// The tool whose result the turn waits for.
        tool: String,
    },
}

/// Result of a turn (the started prompt and every follow-up run the runtime
/// added for steered messages) plus the steered messages it never delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnOutcome {
    /// The turn's result: completed, or how it ended without one.
    pub result: Result<TurnSummary, TurnFailure>,
    /// Steered messages the runtime ended without writing them into the
    /// conversation (the turn was interrupted or failed), oldest first. A
    /// message already shown as delivered can be listed here when its run
    /// was discarded.
    pub undelivered: Vec<PendingId>,
}

impl TurnOutcome {
    /// An outcome with nothing undelivered.
    #[must_use]
    pub fn plain(result: Result<TurnSummary, TurnFailure>) -> Self {
        Self {
            result,
            undelivered: Vec::new(),
        }
    }
}

/// An approval decision to show in the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalNotice {
    /// Tool name.
    pub tool: String,
    /// Tier / mechanism that decided (e.g. `rule`, `classifier`, `user`).
    pub tier: String,
    /// Whether the call was allowed.
    pub allowed: bool,
    /// Optional explanation.
    pub reason: Option<String>,
}

/// Status-bar update pushed by the integrator.
#[derive(Debug, Clone, PartialEq)]
pub enum StatusUpdate {
    /// Provider and model changed.
    Model {
        /// Provider name.
        provider: String,
        /// Model id.
        model: String,
    },
    /// Active session id (shortened for display).
    Session(String),
    /// Absolute token counters (e.g. after resuming).
    Tokens {
        /// Input tokens.
        input: u64,
        /// Output tokens.
        output: u64,
    },
    /// Auto-mode state and denial counters.
    Auto {
        /// Display state, e.g. `active`, `PAUSED`, `off`.
        state: String,
        /// `(consecutive, consecutive_limit, total, total_limit)`.
        denials: (u32, u32, u32, u32),
    },
    /// Whether the classifier is currently running.
    Classifying(bool),
    /// Current context usage (tokens in the last model call); `approx` after
    /// compaction or resume when the number is an estimate.
    Context {
        /// Tokens used, `None` when unknown.
        used: Option<u64>,
        /// Whether `used` is an estimate.
        approx: bool,
    },
    /// The model's context window in tokens, `None` when unknown.
    ContextWindow(Option<u64>),
    /// The helpers (mob members) running now.
    Helpers(Vec<crate::mobs::HelperStatus>),
    /// An estimated context window (the model is not in the catalog; the
    /// window is a family or provider default). The gauge shows `~`.
    ApproxContextWindow(u64),
    /// Replace the session's cost totals (new or resumed session).
    CostTotals(SessionCostSnapshot),
    /// The price table changed (start-up, fetched lists); the cost readout
    /// is recomputed.
    Prices(Arc<crate::pricing::PriceTable>),
}

/// Text queued for the system clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedCopy {
    /// The text, capped to the clipboard limit.
    pub text: String,
    /// Whether the cap cut it.
    pub truncated: bool,
}

/// Input to [`App::update`].
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    /// A key press.
    Key(KeyEvent),
    /// Bracketed paste.
    Paste(String),
    /// Terminal resized to `(columns, rows)`.
    Resize(u16, u16),
    /// Mouse event (only when mouse capture is enabled).
    Mouse(MouseEvent),
    /// Primary agent stream event.
    Agent(EventEnvelope<AgentEvent>),
    /// Scoped (sub-agent) stream event.
    Scoped(ScopedAgentEvent),
    /// The turn started by [`Effect::StartTurn`] finished.
    TurnFinished(TurnOutcome),
    /// The runner's answer to [`Effect::Steer`].
    SteerAck {
        /// The queued message.
        id: PendingId,
        /// `Ok` when the runtime admitted the message.
        result: Result<(), SteerFailure>,
    },
    /// The runtime wrote a steered message into the conversation (at a model
    /// boundary of the running turn, or as a follow-up turn).
    SteerDelivered(PendingId),
    /// The runtime discarded a delivered message's application (its step was
    /// stopped) and holds it again for redelivery.
    SteerRequeued(PendingId),
    /// The runner's answer to [`Effect::RecallQueued`]: the steered
    /// messages the runtime abandoned before they joined the run (oldest
    /// first; back to the editor), and the cancels that failed. Admitted
    /// messages not listed already reached the agent.
    Recalled {
        /// Abandoned messages.
        recalled: Vec<PendingId>,
        /// Failed cancels.
        errors: Vec<String>,
    },
    /// A file tool produced a diff.
    Diff {
        /// Tool-use id of the producing call.
        tool_use_id: String,
        /// File path.
        path: String,
        /// Unified diff.
        unified: String,
        /// Added lines.
        added: usize,
        /// Removed lines.
        removed: usize,
    },
    /// The approval engine needs a human decision.
    Escalation(EscalationPrompt, EscalationId),
    /// An escalation was resolved elsewhere (timeout, dropped); close it.
    EscalationResolved(EscalationId),
    /// An approval decision to log in the transcript.
    Approval(ApprovalNotice),
    /// One safety-classifier model call's usage: counted in the session's
    /// cost under the classifier model.
    ClassifierUsage {
        /// Classifier model id.
        model: String,
        /// Its usage.
        usage: meerkat_core::Usage,
    },
    /// A notice to show in the transcript.
    Notice {
        /// Severity.
        level: NoticeLevel,
        /// Text.
        text: String,
    },
    /// Status-bar update.
    Status(StatusUpdate),
    /// Open the resume picker with `(ref, label)` items.
    ShowResumePicker(Vec<(String, String)>),
    /// Open the model picker (answer to [`Effect::RequestModelPicker`]): one
    /// group per endpoint, the current endpoint and model first; a group's
    /// warning says why its list is missing.
    ShowModelPicker(Vec<ModelGroup>),
    /// Replace the transcript (e.g. after resuming a session).
    ResetTranscript(Vec<Block>),
    /// `/compact now`: the integrator rebuilt the session with a one-turn
    /// compaction trigger, so its next model call compacts (or why it could
    /// not). The `CompactionStarted`/`Completed` events confirm the
    /// compaction itself.
    CompactRebuilt(Result<(), CompactFailure>),
    /// Some of helper `label`'s usage could not be observed (its events
    /// could not be followed): the cost readout becomes a lower bound.
    HelperUnmeasured(String),
    /// Animation tick.
    Tick,
}

/// Notice shown once `/compact now` rebuilt the session.
pub const COMPACT_REBUILT: &str = "the context will be compacted at the start of your next message (Meerkat has no on-demand compaction; the ✓ compacted notice confirms it)";

/// Side effect requested by [`App::update`], executed by the integrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Run a turn with this prompt; answer with [`Msg::TurnFinished`].
    StartTurn(String),
    /// Interrupt the running turn.
    Interrupt,
    /// Pull back every steered message the runtime still holds queued (not
    /// yet joined to the run) without touching the run; answered with
    /// [`Msg::Recalled`].
    RecallQueued,
    /// Interrupt the running turn and send this prompt next (Ctrl-X Ctrl-S).
    InterruptAndSend(String),
    /// Steer a message into the running turn (a durable `Steer` input);
    /// answered with [`Msg::SteerAck`], then [`Msg::SteerDelivered`] or the
    /// turn's [`TurnOutcome::undelivered`].
    Steer {
        /// Queue id allocated by the UI.
        id: PendingId,
        /// Raw user text, sent as an ordinary user message.
        text: String,
    },
    /// Leave the application. The integrator interrupts a running turn,
    /// persists the session and saves undelivered messages before exiting.
    Quit,
    /// Sent right before [`Effect::Quit`]: every message the UI still holds
    /// (queued, deferred, or the Ctrl-X Ctrl-S prompt as `None`). The
    /// integrator saves those the model never received.
    SaveUnsent(Vec<(Option<PendingId>, String)>),
    /// Start a fresh session.
    NewSession,
    /// Resume a session by reference (`last`, `~N`, id prefix).
    ResumeSession(String),
    /// Load session list; answer with [`Msg::ShowResumePicker`].
    RequestResumePicker,
    /// Answer an escalation.
    RespondEscalation(EscalationId, EscalationAnswer),
    /// Persist an approval rule.
    AddRule {
        /// Allow or deny.
        effect: RuleEffect,
        /// Tool name.
        tool: String,
        /// Optional argument pattern.
        pattern: Option<String>,
    },
    /// Remove rule number `n`.
    RemoveRule(usize),
    /// List rules; answer with [`Msg::Notice`].
    ListRules,
    /// `/rules trust`: show the realm `tui.toml` and record its digest in
    /// the host trust record; answered with [`Msg::Notice`].
    TrustRealmRules,
    /// `/mcp trust`: record the project `.rkat/mcp.toml`'s digest in the
    /// host trust record; its servers start from the next session.
    TrustProjectMcp,
    /// Enable/disable auto mode.
    SetAutoMode(bool),
    /// Resume auto mode after a denial-limit pause.
    ResumeAutoMode,
    /// Switch model.
    SetModel(String),
    /// Load the provider's model list (cached for the session unless
    /// `refresh`); answer with [`Msg::ShowModelPicker`].
    RequestModelPicker {
        /// Refetch instead of using the cached list.
        refresh: bool,
    },
    /// Append a line to the workspace AGENTS.md and push it live.
    Remember(String),
    /// `/compact now` (idle only): rebuild the session so its next turn
    /// compacts before the first model call; answered with
    /// [`Msg::CompactRebuilt`].
    CompactNow,
    /// The session's cost totals after a model call; the integrator
    /// persists them next to the session.
    CostSnapshot(SessionCostSnapshot),
}

/// Everything the status bar shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusInfo {
    /// Provider name.
    pub provider: String,
    /// Model id.
    pub model: String,
    /// Short session id.
    pub session_short: String,
    /// Cumulative input tokens.
    pub input_tokens: u64,
    /// Cumulative output tokens.
    pub output_tokens: u64,
    /// Auto-mode display state (`active`, `PAUSED`, `off`; empty hides the segment).
    pub auto_state: String,
    /// `(consecutive, consecutive_limit, total, total_limit)` denials.
    pub denials: (u32, u32, u32, u32),
    /// When the running turn started.
    pub running_since: Option<Instant>,
    /// Number of messages typed mid-turn that have not been delivered yet.
    pub queued: usize,
    /// Number of helpers running now.
    pub helpers: usize,
    /// Whether the transcript has helper headers (the status bar names the
    /// key that focuses them).
    pub helper_headers: bool,
    /// Whether the classifier is running.
    pub classifying: bool,
    /// Tokens in the last model call, `None` when unknown.
    pub context_used: Option<u64>,
    /// Model context window in tokens, `None` when unknown.
    pub context_window: Option<u64>,
    /// Whether `context_used` is an estimate.
    pub context_approx: bool,
    /// Whether `context_window` is an estimate (model not in the catalog).
    pub window_approx: bool,
    /// Token totals per model, for the cost readout.
    pub cost: SessionCost,
    /// Prices for the cost readout.
    pub prices: Arc<crate::pricing::PriceTable>,
}

/// The model a helper's usage is counted under when neither the provider
/// nor the helper host named one.
const UNKNOWN_HELPER_MODEL: &str = "unknown";

/// Number of session-id characters shown in the status bar.
const SESSION_SHORT_LEN: usize = 8;

impl StatusInfo {
    /// Set provider and model.
    pub fn set_model(&mut self, provider: impl Into<String>, model: impl Into<String>) {
        self.provider = provider.into();
        self.model = model.into();
    }

    /// Set the session id (shortened to its first 8 characters).
    pub fn set_session(&mut self, session_id: &str) {
        self.session_short = session_id.chars().take(SESSION_SHORT_LEN).collect();
    }

    /// Set absolute token counters.
    pub fn set_tokens(&mut self, input: u64, output: u64) {
        self.input_tokens = input;
        self.output_tokens = output;
    }

    /// Add to the token counters.
    pub fn add_tokens(&mut self, input: u64, output: u64) {
        self.input_tokens = self.input_tokens.saturating_add(input);
        self.output_tokens = self.output_tokens.saturating_add(output);
    }

    /// Set auto-mode state and denial counters.
    pub fn set_auto(&mut self, state: impl Into<String>, denials: (u32, u32, u32, u32)) {
        self.auto_state = state.into();
        self.denials = denials;
    }

    /// Set the classifier-running flag.
    pub fn set_classifying(&mut self, classifying: bool) {
        self.classifying = classifying;
    }

    /// Apply an integrator update.
    pub fn apply(&mut self, update: StatusUpdate) {
        match update {
            StatusUpdate::Model { provider, model } => self.set_model(provider, model),
            StatusUpdate::Session(id) => self.set_session(&id),
            StatusUpdate::Tokens { input, output } => self.set_tokens(input, output),
            StatusUpdate::Auto { state, denials } => self.set_auto(state, denials),
            StatusUpdate::Classifying(c) => self.set_classifying(c),
            StatusUpdate::Context { used, approx } => {
                self.context_used = used;
                self.context_approx = approx;
            }
            StatusUpdate::ContextWindow(window) => {
                self.context_window = window;
                self.window_approx = false;
            }
            StatusUpdate::ApproxContextWindow(window) => {
                self.context_window = Some(window);
                self.window_approx = true;
            }
            StatusUpdate::Helpers(helpers) => self.helpers = helpers.len(),
            StatusUpdate::CostTotals(cost) => self.cost = cost,
            StatusUpdate::Prices(prices) => self.prices = prices,
        }
    }
}

/// Longest quoted excerpt of a message in a notice, in characters.
const QUOTE_MAX_CHARS: usize = 40;

/// `“text”`, shortened to its first line and [`QUOTE_MAX_CHARS`] characters.
fn quote_short(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let mut out: String = line.chars().take(QUOTE_MAX_CHARS).collect();
    if out.len() < line.len() || text.trim().lines().count() > 1 {
        out.push('…');
    }
    format!("“{out}”")
}

/// Tokens in the context after a model call: everything sent plus the reply.
fn context_tokens(usage: &meerkat_core::Usage) -> u64 {
    usage
        .input_tokens
        .saturating_add(usage.output_tokens)
        .saturating_add(usage.cache_creation_tokens.unwrap_or(0))
        .saturating_add(usage.cache_read_tokens.unwrap_or(0))
}

/// Transcript scroll position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    /// Lines between the bottom of the viewport and the end of the transcript.
    pub offset_from_bottom: usize,
    /// Whether the view sticks to the bottom as output arrives.
    pub follow: bool,
    last_total: usize,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            offset_from_bottom: 0,
            follow: true,
            last_total: 0,
        }
    }
}

impl ScrollState {
    /// Scroll towards older output.
    pub fn up(&mut self, lines: usize) {
        self.offset_from_bottom = self.offset_from_bottom.saturating_add(lines);
        self.follow = false;
    }

    /// Scroll towards newer output; reaching the bottom re-enables follow.
    pub fn down(&mut self, lines: usize) {
        self.offset_from_bottom = self.offset_from_bottom.saturating_sub(lines);
        if self.offset_from_bottom == 0 {
            self.follow = true;
        }
    }

    /// Jump to the oldest output.
    pub fn top(&mut self) {
        self.offset_from_bottom = usize::MAX;
        self.follow = false;
    }

    /// Jump to the newest output and follow.
    pub fn bottom(&mut self) {
        self.offset_from_bottom = 0;
        self.follow = true;
    }

    /// Reconcile with the laid-out transcript and return the first visible line.
    ///
    /// When not following, growth at the bottom shifts the offset so the visible
    /// window stays put.
    pub fn window(&mut self, total: usize, height: usize) -> usize {
        if self.follow {
            self.offset_from_bottom = 0;
        } else if total > self.last_total && self.last_total > 0 {
            self.offset_from_bottom = self
                .offset_from_bottom
                .saturating_add(total - self.last_total);
        }
        self.last_total = total;
        let max_offset = total.saturating_sub(height);
        self.offset_from_bottom = self.offset_from_bottom.min(max_offset);
        if self.offset_from_bottom == 0 {
            self.follow = true;
        }
        total.saturating_sub(height) - self.offset_from_bottom
    }
}

/// Application state.
///
/// `App` is pure: it performs no I/O. Prompt-history persistence is the
/// integrator's job — seed recall with [`App::with_history`] (e.g. from
/// [`History::load`]) and, after each [`App::update`], drain newly submitted
/// lines with [`App::take_submitted`] and append them to the [`History`] file.
#[derive(Debug)]
pub struct App {
    /// Rendered conversation.
    pub transcript: Transcript,
    /// Prompt editor.
    pub input: InputEditor,
    /// Transcript scroll position.
    pub scroll: ScrollState,
    /// Status-bar data.
    pub status: StatusInfo,
    /// Open modal, if any.
    pub modal: Option<Modal>,
    /// Messages typed while a turn was running, not yet delivered.
    pub pending: PendingQueue,
    /// Prompt to send once the interrupted turn ends (Ctrl-X Ctrl-S).
    pub send_next: Option<String>,
    /// Sequence number of the last primary-stream event processed.
    pub last_seq: u64,
    /// Normal editing or history search.
    pub mode: InputMode,
    /// When Ctrl-X was pressed, while a chord is pending.
    pub chord: Option<Instant>,
    /// Whether `/compact now` is rebuilding the session ([`Effect::CompactNow`]
    /// sent, no [`Msg::CompactRebuilt`] yet). A prompt sent meanwhile is
    /// admitted as an ordinary `Queue` input once the rebuild is done (the
    /// runner serves effects in order); session switches wait.
    pub compacting: bool,
    /// When a Ctrl-D / `/quit` was refused because work is in flight; a
    /// second one within [`QUIT_CONFIRM_WINDOW`] quits anyway.
    pub quit_confirm: Option<Instant>,
    /// Context-gauge policy: warn about a filling context (warn mode).
    gauge: CompactionGauge,
    /// The history estimate of a compaction that started and has not
    /// completed: the input of its summary call, counted on completion.
    compaction_input: Option<u64>,
    /// Whether assistant text streamed during the current turn (the turn
    /// summary is shown only when nothing did).
    streamed_text: bool,
    /// Global (cross-workspace) prompt history, oldest first; searched after the
    /// workspace history.
    global_history: Vec<String>,
    /// Search corpus (newest first, deduplicated), built when a search starts.
    search_corpus: Vec<String>,
    pending_escalations: VecDeque<(EscalationId, EscalationPrompt)>,
    /// A picker ([`Modal::Model`] or [`Modal::Resume`]) an escalation
    /// displaced; restored once no escalation remains.
    parked: Option<Modal>,
    submitted: Vec<String>,
    running: bool,
    run_first_block: usize,
    last_ctrl_c: Option<Instant>,
    viewport_height: usize,
    /// Screen area of the transcript and, per drawn row, the block and the
    /// transcript logical line shown there, as of the last draw (for mouse
    /// hit-testing and selection).
    transcript_area: Rect,
    transcript_rows: Vec<(BlockId, usize)>,
    /// Mouse text selection (logical line coordinates).
    selection: Option<Selection>,
    press: Option<Press>,
    last_down: Option<LastDown>,
    /// Text to put on the system clipboard (drained by the runtime).
    clipboard: Option<QueuedCopy>,
    /// The helper header Ctrl+Up/Down focused (Enter toggles it).
    helper_focus: Option<BlockId>,
    /// An [`Effect::RecallQueued`] is waiting for its [`Msg::Recalled`].
    recalling: bool,
    /// Admitted messages a recall found already with the agent: Esc no
    /// longer tries to recall them (it interrupts).
    reached: Vec<PendingId>,
    dirty: bool,
}

impl App {
    /// Create an app with the given status-bar data.
    pub fn new(status: StatusInfo) -> Self {
        Self {
            transcript: Transcript::new(),
            input: InputEditor::new(),
            scroll: ScrollState::default(),
            status,
            modal: None,
            pending: PendingQueue::new(),
            send_next: None,
            last_seq: 0,
            mode: InputMode::Normal,
            chord: None,
            compacting: false,
            quit_confirm: None,
            gauge: CompactionGauge::default(),
            compaction_input: None,
            streamed_text: false,
            global_history: Vec::new(),
            search_corpus: Vec::new(),
            pending_escalations: VecDeque::new(),
            parked: None,
            submitted: Vec::new(),
            running: false,
            run_first_block: 0,
            last_ctrl_c: None,
            viewport_height: 20,
            transcript_area: Rect::default(),
            transcript_rows: Vec::new(),
            selection: None,
            press: None,
            last_down: None,
            clipboard: None,
            helper_focus: None,
            recalling: false,
            reached: Vec::new(),
            dirty: true,
        }
    }

    /// Set how helper headers start: folded (default) or expanded.
    #[must_use]
    pub fn with_helper_view(mut self, view: HelperView) -> Self {
        self.transcript.set_helper_view(view);
        self
    }

    /// The focused helper header, if it is still one on screen.
    pub fn helper_focus(&self) -> Option<BlockId> {
        self.helper_focus.filter(|id| {
            matches!(self.transcript.block(*id), Some(Block::Helper(_)))
                && !self.transcript.is_hidden(*id)
        })
    }

    /// Ctrl+Up (`forward == false`) / Ctrl+Down: move the helper focus over
    /// the headers on screen, starting at the newest and wrapping around.
    fn move_helper_focus(&mut self, forward: bool) {
        let headers: Vec<BlockId> = self.transcript.visible_helper_headers().collect();
        let (Some(oldest), Some(newest)) = (headers.first().copied(), headers.last().copied())
        else {
            self.helper_focus = None;
            return;
        };
        let next = match self
            .helper_focus()
            .and_then(|f| headers.iter().position(|h| *h == f))
        {
            None => newest,
            Some(i) if forward => headers.get(i + 1).copied().unwrap_or(oldest),
            Some(i) => i
                .checked_sub(1)
                .and_then(|j| headers.get(j).copied())
                .unwrap_or(newest),
        };
        self.helper_focus = Some(next);
        self.reveal_block(next);
    }

    /// Scroll so block `id`'s first line is on screen.
    fn reveal_block(&mut self, id: BlockId) {
        if self.transcript_area.width == 0 {
            return;
        }
        let total = self.transcript.ensure_layout(self.transcript_area.width);
        let Some(line) = self.transcript.first_line_of(id) else {
            return;
        };
        let height = self.viewport_height;
        let first = total
            .saturating_sub(height)
            .saturating_sub(self.scroll.offset_from_bottom);
        if line < first {
            self.scroll.offset_from_bottom = total.saturating_sub(height).saturating_sub(line);
            self.scroll.follow = false;
        } else if line >= first + height {
            self.scroll.offset_from_bottom = total.saturating_sub(line + 1);
            self.scroll.follow = self.scroll.offset_from_bottom == 0;
        }
    }

    /// Expand or fold the helper header `id`; expanding stops following new
    /// output so the header stays where it is.
    fn toggle_helper(&mut self, id: BlockId) -> bool {
        match self.transcript.toggle_helper(id) {
            Some(expanded) => {
                if expanded {
                    self.scroll.follow = false;
                }
                true
            }
            None => false,
        }
    }

    /// `/helpers`: list, expand or fold.
    fn helpers_command(&mut self, cmd: HelpersCommand) {
        let (target, expand) = match cmd {
            HelpersCommand::List => {
                let text = self.helpers_listing();
                self.transcript.push_notice(NoticeLevel::Info, text);
                return;
            }
            HelpersCommand::Expand(target) => (target, true),
            HelpersCommand::Fold(target) => (target, false),
        };
        let ids: Vec<BlockId> = match &target {
            HelperTarget::All => self.transcript.helper_headers().map(|(id, _)| id).collect(),
            HelperTarget::Label(label) => self.transcript.helper_header(label).into_iter().collect(),
        };
        if ids.is_empty() {
            let text = match target {
                HelperTarget::All => "no helpers yet".to_string(),
                HelperTarget::Label(label) => format!("no helper `{label}` (try /helpers)"),
            };
            self.transcript.push_notice(NoticeLevel::Warn, text);
            return;
        }
        for id in ids {
            self.transcript.set_helper_expanded(id, expand);
        }
        if expand {
            self.scroll.follow = false;
        }
    }

    /// The `/helpers` listing: one line per helper header.
    fn helpers_listing(&self) -> String {
        use std::fmt::Write as _;
        let mut text = String::from("helpers:");
        let mut any = false;
        for (_, fold) in self.transcript.helper_headers() {
            any = true;
            let indent = "  ".repeat(fold.depth);
            let marker = if fold.expanded { "▾" } else { "▸" };
            let status = fold.status_text();
            // No spinner glyph in a static listing.
            let status = if fold.state.is_running() {
                status.split_once(' ').map_or(status.as_str(), |(_, rest)| rest)
            } else {
                status.as_str()
            };
            let _ = write!(text, "\n{indent}{marker} {} {status}", fold.tag());
        }
        if any {
            text.push_str("\n(/helpers expand|fold <label>|all, or Ctrl+Up/Down then Enter)");
        } else {
            text.push_str(" none yet");
        }
        text
    }

    /// Seed the prompt history (oldest first).
    pub fn with_history(mut self, history: Vec<String>) -> Self {
        self.input = InputEditor::new().with_history(history);
        self
    }

    /// Set the context-compaction policy (default: warn at 75%, repeat at 85%).
    #[must_use]
    pub fn with_compaction_policy(mut self, policy: CompactionPolicy) -> Self {
        self.gauge = CompactionGauge::new(policy);
        self
    }

    /// The context-compaction policy in effect.
    pub fn compaction_policy(&self) -> CompactionPolicy {
        self.gauge.policy()
    }

    /// Seed the global prompt history (oldest first), searched by Ctrl-R after
    /// the workspace history. Up/Down recall stays workspace-only.
    pub fn with_global_history(mut self, history: Vec<String>) -> Self {
        self.global_history = history;
        self
    }

    /// The active history search and its corpus, if searching.
    pub fn search(&self) -> Option<(&HistorySearch, &[String])> {
        match &self.mode {
            InputMode::Search(s) => Some((s, &self.search_corpus)),
            InputMode::Normal => None,
        }
    }

    /// Whether a Ctrl-X chord is pending at `now` (used for the input hint).
    pub fn chord_pending(&self, now: Instant) -> bool {
        self.chord
            .is_some_and(|t| now.saturating_duration_since(t) < CHORD_WINDOW)
    }

    /// Lines submitted with Enter (prompts and slash commands, as typed) since the
    /// last call, oldest first. The integrator appends them to the persistent
    /// [`History`]; blank submissions are never recorded.
    pub fn take_submitted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.submitted)
    }

    /// Whether a turn is running.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Whether a queued message can be recalled (Esc / ↑ on an empty editor).
    pub fn has_recallable(&self) -> bool {
        self.pending.newest_recallable().is_some()
    }

    /// Escalations waiting behind the open modal.
    pub fn pending_escalations(&self) -> usize {
        self.pending_escalations.len()
    }

    /// Whether a second Ctrl-C at `now` would quit (used for the input hint).
    pub fn quit_armed(&self, now: Instant) -> bool {
        self.last_ctrl_c
            .is_some_and(|t| now.saturating_duration_since(t) < DOUBLE_CTRL_C_WINDOW)
    }

    /// Whether a second Ctrl-D / `/quit` at `now` would quit despite a running
    /// turn or queued messages (used for the input hint).
    pub fn quit_confirm_armed(&self, now: Instant) -> bool {
        self.quit_confirm
            .is_some_and(|t| now.saturating_duration_since(t) < QUIT_CONFIRM_WINDOW)
    }

    /// Record the transcript viewport height (set by the renderer).
    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.max(1);
    }

    /// Record where the transcript was drawn: its screen area, the logical
    /// line of its first row and the block shown on each row (set by the
    /// renderer; row `i` shows logical line `first_line + i`).
    pub fn set_transcript_rows(&mut self, area: Rect, first_line: usize, rows: Vec<BlockId>) {
        self.transcript_area = area;
        self.transcript_rows = rows
            .into_iter()
            .enumerate()
            .map(|(i, id)| (id, first_line + i))
            .collect();
    }

    /// The transcript block drawn at screen row `row` in the last frame.
    pub fn block_at_row(&self, row: u16) -> Option<BlockId> {
        let offset = row.checked_sub(self.transcript_area.y)?;
        self.transcript_rows
            .get(usize::from(offset))
            .map(|(id, _)| *id)
    }

    /// The transcript cell (logical line, display column) at a screen
    /// position in the last frame.
    pub fn cell_at(&self, row: u16, column: u16) -> Option<CellPos> {
        let offset = row.checked_sub(self.transcript_area.y)?;
        let col = column.checked_sub(self.transcript_area.x)?;
        if col >= self.transcript_area.width {
            return None;
        }
        let (_, line) = self.transcript_rows.get(usize::from(offset))?;
        Some(CellPos::new(*line, usize::from(col)))
    }

    /// The current mouse selection.
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// The copy queued since the last call. The runtime writes it with OSC 52
    /// after the next frame and reports the outcome as a notice
    /// ([`selection::copy_notice`]).
    pub fn take_clipboard(&mut self) -> Option<QueuedCopy> {
        self.clipboard.take()
    }

    /// The text of [`Self::take_clipboard`].
    pub fn take_clipboard_text(&mut self) -> Option<String> {
        self.take_clipboard().map(|copy| copy.text)
    }

    /// Bring the layout up to date at the last drawn width (so logical line
    /// indices match what is on screen) and return line `idx`.
    fn layout_line(&mut self, idx: usize) -> Option<Line<'static>> {
        if self.transcript_area.width > 0 {
            self.transcript.ensure_layout(self.transcript_area.width);
        }
        self.transcript.line(idx).cloned()
    }

    fn mouse(&mut self, m: MouseEvent, now: Instant) -> bool {
        match m.kind {
            MouseEventKind::ScrollUp => self.scroll.up(keymap::LINE_SCROLL),
            MouseEventKind::ScrollDown => self.scroll.down(keymap::LINE_SCROLL),
            MouseEventKind::Down(MouseButton::Left) => {
                return self.mouse_down(m.row, m.column, now);
            }
            MouseEventKind::Drag(MouseButton::Left) => return self.mouse_drag(m.row, m.column),
            MouseEventKind::Up(MouseButton::Left) => return self.mouse_up(),
            _ => return false,
        }
        true
    }

    /// Left press: a single click clears the selection (and may become a
    /// click or a drag); a double click selects a word, a triple click a line.
    fn mouse_down(&mut self, row: u16, column: u16, now: Instant) -> bool {
        if self.modal.is_some() {
            return false;
        }
        let had_selection = self.selection.take().is_some();
        let Some(pos) = self.cell_at(row, column) else {
            self.press = None;
            self.last_down = None;
            return had_selection;
        };
        let clicks = match self.last_down {
            Some(d)
                if d.row == row
                    && d.column == column
                    && now.saturating_duration_since(d.at) < MULTI_CLICK_WINDOW =>
            {
                d.clicks % 3 + 1
            }
            _ => 1,
        };
        self.last_down = Some(LastDown {
            at: now,
            row,
            column,
            clicks,
        });
        self.press = Some(Press {
            pos,
            row,
            clicks,
            dragged: false,
        });
        self.selection = match clicks {
            1 => None,
            2 => {
                let line = self.layout_line(pos.line);
                Some(Selection::new(pos, SelectMode::Word, line.as_ref()))
            }
            _ => Some(Selection::new(pos, SelectMode::Line, None)),
        };
        had_selection || clicks > 1
    }

    /// Drag with the left button: extend the selection, scrolling when the
    /// pointer is past the top or bottom edge of the transcript.
    fn mouse_drag(&mut self, row: u16, column: u16) -> bool {
        if self.modal.is_some() {
            return false;
        }
        let Some(press) = self.press.as_mut() else {
            return false;
        };
        press.dragged = true;
        let start = press.pos;
        let Some(pos) = self.drag_target(row, column) else {
            return false;
        };
        let line = self.layout_line(pos.line);
        let sel = self
            .selection
            .get_or_insert_with(|| Selection::new(start, SelectMode::Char, None));
        sel.extend(pos, line.as_ref());
        true
    }

    /// The cell a drag at a screen position selects to. Past the top edge
    /// (or below the transcript while scrolled back) it scrolls one line and
    /// targets the line that scrolls in; below the last line it targets the
    /// end of the last line.
    fn drag_target(&mut self, row: u16, column: u16) -> Option<CellPos> {
        let first = self.transcript_rows.first()?.1;
        let last = self.transcript_rows.last()?.1;
        let area = self.transcript_area;
        let col = usize::from(column.saturating_sub(area.x));
        if row < area.y {
            if first > 0 {
                self.scroll.up(1);
            }
            return Some(CellPos::new(first.saturating_sub(1), col));
        }
        if let Some((_, line)) = self.transcript_rows.get(usize::from(row - area.y)) {
            return Some(CellPos::new(*line, col));
        }
        if row >= area.bottom() && self.scroll.offset_from_bottom > 0 {
            self.scroll.down(1);
            let max = self.transcript.line_count().saturating_sub(1);
            return Some(CellPos::new((last + 1).min(max), col));
        }
        Some(CellPos::new(last, usize::MAX))
    }

    /// Left release: a press without drag is a click (expand/collapse a tool
    /// block); after a drag or a double/triple click the selection is copied.
    fn mouse_up(&mut self) -> bool {
        let Some(press) = self.press.take() else {
            return false;
        };
        if self.modal.is_some() {
            return false;
        }
        if !press.dragged && press.clicks == 1 {
            return self.click(press.row);
        }
        self.copy_selection();
        true
    }

    /// Queue the selected text for the clipboard.
    fn copy_selection(&mut self) {
        let Some(sel) = self.selection else {
            return;
        };
        let (start, end) = sel.range();
        if self.transcript_area.width > 0 {
            self.transcript.ensure_layout(self.transcript_area.width);
        }
        let lines = self
            .transcript
            .visible_lines(start.line, end.line - start.line + 1);
        let text = sel.extract_window(start.line, &lines);
        self.copy_text(&text);
    }

    /// Ctrl-Y: copy the selection again, or else the last assistant message.
    fn copy_again(&mut self) {
        if self.selection.is_some() {
            self.copy_selection();
            return;
        }
        let last = self.transcript.blocks().iter().rev().find_map(|b| match b {
            Block::Assistant { md } if !md.trim().is_empty() => Some(md.clone()),
            _ => None,
        });
        self.copy_text(last.as_deref().unwrap_or(""));
    }

    /// Queue `text` (capped) for the clipboard; the runtime reports how the
    /// copy went.
    fn copy_text(&mut self, text: &str) {
        if text.trim().is_empty() {
            self.transcript
                .push_notice(NoticeLevel::Info, "nothing to copy");
            return;
        }
        let (capped, truncated) = selection::cap_clipboard(text);
        self.clipboard = Some(QueuedCopy {
            text: capped.to_string(),
            truncated,
        });
    }

    /// Clear the selection and any press in progress.
    fn clear_selection(&mut self) {
        self.selection = None;
        self.press = None;
    }

    /// A left click: expand or collapse the tool block under it. Expanding
    /// stops following new output so the clicked block stays where it is.
    /// Returns whether anything changed.
    fn click(&mut self, row: u16) -> bool {
        if self.modal.is_some() {
            return false;
        }
        let Some(id) = self.block_at_row(row) else {
            return false;
        };
        if matches!(self.transcript.block(id), Some(Block::Helper(_))) {
            return self.toggle_helper(id);
        }
        match self.transcript.toggle_tool(id) {
            Some(collapsed) => {
                if !collapsed {
                    self.scroll.follow = false;
                }
                true
            }
            None => false,
        }
    }

    /// Return and reset the redraw flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Process one message. Pure: no I/O, only state changes and returned effects.
    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        self.update_at(msg, Instant::now())
    }

    /// [`Self::update`] with an explicit clock (for tests).
    pub fn update_at(&mut self, msg: Msg, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        let mut dirty = true;
        match msg {
            Msg::Key(key) => {
                let copy_key =
                    key.code == KeyCode::Char('y') && key.modifiers.contains(KeyModifiers::CONTROL);
                if self.selection.is_some() && !copy_key {
                    self.clear_selection();
                    let ctrl_c = key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL);
                    if (key.code == KeyCode::Esc || ctrl_c) && self.modal.is_none() {
                        // Esc and Ctrl-C only dismiss the selection.
                        self.dirty = true;
                        return fx;
                    }
                }
                if self.modal.is_some() {
                    self.modal_key(&key, &mut fx);
                } else {
                    self.main_key(&key, now, &mut fx);
                }
            }
            Msg::Paste(text) => {
                if self.modal.is_none() {
                    self.chord = None;
                    if let InputMode::Search(search) = &mut self.mode {
                        for c in text.chars().filter(|c| !c.is_control()) {
                            search.push(c, &self.search_corpus);
                        }
                    } else {
                        self.input.insert_str(&text);
                    }
                }
            }
            Msg::Resize(..) => {
                // Logical lines change with the width.
                self.clear_selection();
                self.transcript.invalidate_all();
            }
            Msg::Mouse(m) => dirty = self.mouse(m, now),
            Msg::Agent(env) => {
                self.last_seq = env.seq;
                match &env.payload {
                    AgentEvent::TextDelta { delta, .. } if !delta.is_empty() => {
                        self.streamed_text = true;
                    }
                    AgentEvent::TextComplete { content, .. } if !content.is_empty() => {
                        self.streamed_text = true;
                    }
                    // A turn without provider accounting advances no counter.
                    // Its usage counts for the model the provider accounted
                    // it to (the session's model unless it names none).
                    AgentEvent::TurnCompleted {
                        usage: Some(usage), ..
                    } => {
                        self.status.context_used = Some(context_tokens(usage));
                        self.status.context_approx = false;
                        let accounted = usage.accounting().model.trim();
                        let model = if accounted.is_empty() {
                            self.status.model.clone()
                        } else {
                            accounted.to_string()
                        };
                        self.status.cost.record(&model, usage);
                        fx.push(Effect::CostSnapshot(self.status.cost.clone()));
                    }
                    AgentEvent::CompactionStarted {
                        estimated_history_tokens,
                        ..
                    } => {
                        self.compaction_input = Some(*estimated_history_tokens);
                    }
                    AgentEvent::CompactionCompleted {
                        summary_tokens,
                        estimated_history_tokens_after,
                        summary_source,
                        ..
                    } => {
                        // The history estimate the rewrite left (the
                        // summary alone for a completion that reports none).
                        self.status.context_used =
                            Some(estimated_history_tokens_after.unwrap_or(*summary_tokens));
                        self.status.context_approx = true;
                        self.gauge.compacted();
                        let input = self.compaction_input.take().unwrap_or(0);
                        // Only a provider call costs anything; its input is
                        // the history estimate, its output the summary.
                        if summary_source.as_ref().is_none_or(|source| {
                            matches!(
                                source,
                                meerkat_core::CompactionSummarySource::ProviderCall { .. }
                            )
                        }) {
                            self.status.cost.record_compaction(
                                &self.status.model,
                                input,
                                *summary_tokens,
                            );
                            fx.push(Effect::CostSnapshot(self.status.cost.clone()));
                        }
                    }
                    _ => {}
                }
                if let Some(usage) = self.transcript.apply_agent_event_at(&env.payload, now) {
                    self.status
                        .add_tokens(usage.input_tokens, usage.output_tokens);
                }
            }
            Msg::Scoped(ev) => {
                self.record_helper_usage(&ev, &mut fx);
                self.transcript.apply_scoped_event_at(&ev, now);
            }
            Msg::TurnFinished(outcome) => self.turn_finished(outcome, now, &mut fx),
            Msg::SteerAck { id, result } => self.steer_ack(id, result, now, &mut fx),
            Msg::SteerDelivered(id) => self.settle(id),
            Msg::SteerRequeued(id) => self.requeue(id),
            Msg::Recalled { recalled, errors } => self.recalled(&recalled, &errors),
            Msg::Diff {
                tool_use_id,
                path,
                unified,
                added,
                removed,
            } => {
                let view = DiffView {
                    path: path.clone(),
                    unified,
                    added,
                    removed,
                };
                if !self.transcript.attach_diff(&tool_use_id, view) {
                    self.transcript.push_notice(
                        NoticeLevel::Info,
                        format!("edited {path} (+{added} -{removed})"),
                    );
                }
            }
            Msg::Escalation(prompt, id) => self.escalation(prompt, id),
            Msg::EscalationResolved(id) => self.escalation_resolved(id),
            Msg::Approval(n) => {
                self.transcript.push(Block::Approval {
                    tool: n.tool,
                    tier: n.tier,
                    allowed: n.allowed,
                    reason: n.reason,
                });
            }
            Msg::ClassifierUsage { model, usage } => {
                self.status.cost.record(&model, &usage);
                fx.push(Effect::CostSnapshot(self.status.cost.clone()));
            }
            Msg::Notice { level, text } => {
                self.transcript.push_notice(level, text);
            }
            Msg::Status(update) => {
                // Envelope sequence numbers restart with every session task.
                if matches!(update, StatusUpdate::Session(_)) {
                    self.last_seq = 0;
                }
                if let StatusUpdate::Helpers(helpers) = &update {
                    self.transcript.note_live_helpers(
                        helpers
                            .iter()
                            .map(|h| (h.label.as_str(), h.model.as_str(), h.depth)),
                    );
                }
                self.status.apply(update);
            }
            Msg::ShowResumePicker(items) => {
                let picker = Modal::Resume(ResumePicker { items, selected: 0 });
                match self.modal {
                    None => self.modal = Some(picker),
                    // Shown once the escalations are answered; replaces a
                    // picker parked earlier.
                    Some(Modal::Escalation { .. }) => self.parked = Some(picker),
                    Some(_) => {}
                }
            }
            Msg::ShowModelPicker(groups) => {
                if self.modal.is_none() && self.parked.is_none() {
                    self.modal = Some(Modal::Model(ModelPicker::new(groups)));
                } else {
                    for warning in ModelPicker::warnings(&groups) {
                        self.transcript.push_notice(NoticeLevel::Warn, warning);
                    }
                }
            }
            Msg::ResetTranscript(blocks) => {
                self.clear_selection();
                self.helper_focus = None;
                self.transcript.reset(blocks);
                self.last_seq = 0;
                self.pending.clear_settled();
                // Keep placeholders for messages still in flight.
                for item in self.pending.iter_mut() {
                    item.block = self.transcript.push(Block::Queued {
                        id: item.id,
                        text: item.text.clone(),
                        label: item.state.label(),
                    });
                }
                self.run_first_block = self.transcript.len();
                self.scroll.bottom();
            }
            Msg::CompactRebuilt(result) => self.compact_rebuilt(result),
            Msg::HelperUnmeasured(label) => {
                self.transcript.helper_unmeasured(&label);
                self.status.cost.mark_unmeasured(&label);
                fx.push(Effect::CostSnapshot(self.status.cost.clone()));
            }
            Msg::Tick => {
                let helpers_spun = self.transcript.tick_helpers(now);
                dirty = helpers_spun
                    || self.status.running_since.is_some()
                    || self.status.classifying
                    || self.last_ctrl_c.is_some()
                    || self.quit_confirm.is_some()
                    || self.chord.is_some();
                if self.last_ctrl_c.is_some() && !self.quit_armed(now) {
                    self.last_ctrl_c = None;
                }
                if self.quit_confirm.is_some() && !self.quit_confirm_armed(now) {
                    self.quit_confirm = None;
                }
                if self.chord.is_some() && !self.chord_pending(now) {
                    self.chord = None;
                }
            }
        }
        self.status.queued = self.pending.len();
        self.status.helper_headers = self.transcript.has_helpers();
        self.dirty |= dirty;
        fx
    }

    fn main_key(&mut self, key: &KeyEvent, now: Instant, fx: &mut Vec<Effect>) {
        if let InputMode::Search(search) = &mut self.mode {
            self.last_ctrl_c = None;
            match search.key(key, &self.search_corpus) {
                SearchOutcome::Continue => {}
                SearchOutcome::Accept(text) | SearchOutcome::Cancel(text) => {
                    self.end_search(&text);
                }
            }
            return;
        }
        if let Some(started) = self.chord.take()
            && now.saturating_duration_since(started) < CHORD_WINDOW
        {
            match key.code {
                KeyCode::Esc => {
                    self.last_ctrl_c = None;
                    return;
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.last_ctrl_c = None;
                    self.interrupt_and_send(now, fx);
                    return;
                }
                // Anything else cancels the chord and is processed normally.
                _ => {}
            }
        }
        let action = keymap::map_key(key, self.input.is_empty());
        if action != Action::CtrlC {
            self.last_ctrl_c = None;
        }
        let page = self.viewport_height.saturating_sub(2).max(1);
        // Editing drops the helper focus; Enter and Esc act on it first.
        if matches!(action, Action::Edit(_) | Action::Newline) {
            self.helper_focus = None;
        }
        match action {
            Action::Submit => match self.helper_focus() {
                Some(id) => {
                    self.toggle_helper(id);
                }
                None => {
                    self.helper_focus = None;
                    self.submit(now, fx);
                }
            },
            Action::Escape if self.helper_focus().is_some() => self.helper_focus = None,
            Action::HelperPrev => self.move_helper_focus(false),
            Action::HelperNext => self.move_helper_focus(true),
            Action::Newline => self.input.newline(),
            Action::CtrlC => self.ctrl_c(now, fx),
            Action::CtrlD => {
                if self.input.is_empty() {
                    self.request_quit(now, fx);
                } else {
                    self.input.delete();
                }
            }
            Action::Escape => self.escape(fx),
            Action::Edit(EditAction::Up) if self.input.is_empty() && self.has_recallable() => {
                self.recall_newest();
            }
            Action::ToggleReasoning => {
                self.transcript.toggle_last_reasoning();
            }
            Action::ToggleToolOutput => {
                if let Some(id) = self.transcript.last_tool_with_output() {
                    self.transcript.toggle_tool(id);
                }
            }
            Action::Copy => self.copy_again(),
            Action::HistorySearch => self.start_search(),
            Action::ChordPrefix => self.chord = Some(now),
            Action::ScrollUp(n) => self.scroll.up(n),
            Action::ScrollDown(n) => self.scroll.down(n),
            Action::PageUp => self.scroll.up(page),
            Action::PageDown => self.scroll.down(page),
            Action::ScrollTop => self.scroll.top(),
            Action::ScrollBottom => self.scroll.bottom(),
            Action::Edit(edit) => self.edit(edit),
            Action::None => {}
        }
    }

    fn start_search(&mut self) {
        self.search_corpus = search::build_corpus(self.input.history(), &self.global_history);
        self.mode = InputMode::Search(HistorySearch::start(self.input.text(), &self.search_corpus));
    }

    /// Leave search mode with `text` in the editor (cursor at the end).
    fn end_search(&mut self, text: &str) {
        self.mode = InputMode::Normal;
        self.search_corpus = Vec::new();
        self.input.clear();
        self.input.set_text(text);
    }

    /// Ctrl-X Ctrl-S: when idle, the same as Enter; when running, interrupt and
    /// send the editor contents next (plain interrupt when the editor is empty).
    fn interrupt_and_send(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        if !self.running {
            self.submit(now, fx);
        } else if self.input.text().trim().is_empty() {
            self.interrupt(fx);
        } else {
            let text = self.input.take();
            self.submitted.push(text.clone());
            self.send_next = Some(match self.send_next.take() {
                Some(prev) => format!("{prev}\n\n{text}"),
                None => text.clone(),
            });
            fx.push(Effect::InterruptAndSend(text));
        }
    }

    fn edit(&mut self, edit: EditAction) {
        let e = &mut self.input;
        match edit {
            EditAction::Insert(c) => e.insert_char(c),
            EditAction::Backspace => e.backspace(),
            EditAction::Delete => e.delete(),
            EditAction::Left => e.left(),
            EditAction::Right => e.right(),
            EditAction::Home => e.home(),
            EditAction::End => e.end(),
            EditAction::WordLeft => e.word_left(),
            EditAction::WordRight => e.word_right(),
            EditAction::Up => e.up(),
            EditAction::Down => e.down(),
            EditAction::DeleteWordBack => e.delete_word_back(),
            EditAction::KillToLineStart => e.kill_to_line_start(),
        }
    }

    fn modal_key(&mut self, key: &KeyEvent, fx: &mut Vec<Effect>) {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        // A held key must not answer, select or close: repeats only move,
        // edit the filter and type.
        let repeat = key.kind == KeyEventKind::Repeat;
        match &mut self.modal {
            Some(Modal::Escalation { id, .. }) => {
                let id = *id;
                if repeat {
                    return;
                }
                if ctrl_c {
                    self.stop_all_escalations(fx);
                    return;
                }
                let answer = match key.code {
                    KeyCode::Char('y' | 'Y') => EscalationAnswer::AllowOnce,
                    KeyCode::Char('a' | 'A') => EscalationAnswer::AllowAndAddRule,
                    KeyCode::Char('n' | 'N') | KeyCode::Esc => EscalationAnswer::Deny,
                    KeyCode::Char('s' | 'S') => EscalationAnswer::Stop,
                    _ => return,
                };
                fx.push(Effect::RespondEscalation(id, answer));
                self.advance_escalation();
            }
            Some(Modal::Resume(picker)) => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') if repeat => {}
                _ if ctrl_c && repeat => {}
                _ if ctrl_c => self.modal = None,
                KeyCode::Up | KeyCode::Char('k') => picker.up(),
                KeyCode::Down | KeyCode::Char('j') => picker.down(),
                KeyCode::Enter => {
                    let selected = picker.selected_ref().map(str::to_string);
                    self.modal = None;
                    if let Some(r) = selected {
                        fx.push(Effect::ResumeSession(r));
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.modal = None,
                _ => {}
            },
            Some(Modal::Model(picker)) => match key.code {
                KeyCode::Enter | KeyCode::Esc if repeat => {}
                _ if ctrl_c && repeat => {}
                _ if ctrl_c => self.modal = None,
                KeyCode::Esc => self.modal = None,
                KeyCode::Up => picker.up(),
                KeyCode::Down => picker.down(),
                KeyCode::Backspace => picker.pop_filter(),
                KeyCode::Enter => {
                    if let Some(model) = picker.selected_ref() {
                        self.modal = None;
                        let current = format!("{}:{}", self.status.provider, self.status.model);
                        if model != current {
                            self.switch_model(model, fx);
                        }
                    }
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    picker.push_filter(c);
                }
                _ => {}
            },
            None => {}
        }
    }

    /// Whether the model may change now; pushes the reason when not.
    fn model_switch_allowed(&mut self) -> bool {
        if self.running || !self.pending.is_empty() {
            self.transcript.push_notice(
                NoticeLevel::Warn,
                "/model is only available while idle; wait for the turn or interrupt it (Ctrl-C)",
            );
            false
        } else if self.compacting {
            self.transcript.push_notice(
                NoticeLevel::Warn,
                "the session is being rebuilt for /compact now; wait for it to finish",
            );
            false
        } else {
            true
        }
    }

    /// `/model <id>` or a picker selection: switch when idle.
    fn switch_model(&mut self, model: String, fx: &mut Vec<Effect>) {
        if self.model_switch_allowed() {
            fx.push(Effect::SetModel(model));
        }
    }

    fn submit(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        if self.input.text().trim().is_empty() {
            return;
        }
        let raw = self.input.take();
        self.submitted.push(raw.clone());
        match commands::parse(&raw) {
            Some(Ok(cmd)) => self.run_command(cmd, now, fx),
            Some(Err(err)) => {
                self.transcript
                    .push_notice(NoticeLevel::Warn, err.to_string());
            }
            None => {
                if let Some(text) = commands::parse_remember(&raw) {
                    self.run_command(Command::Remember(text), now, fx);
                    return;
                }
                let prompt = match raw.trim_start().strip_prefix("//") {
                    Some(rest) => format!("/{rest}"),
                    None => commands::unescape_hash(&raw).unwrap_or(raw),
                };
                if self.running {
                    self.enqueue(prompt, fx);
                } else {
                    self.start_turn(prompt, now, fx);
                }
            }
        }
    }

    /// Start a turn with user content, shown as a user block.
    fn start_turn(&mut self, prompt: String, now: Instant, fx: &mut Vec<Effect>) {
        self.begin_turn(prompt, true, now, fx);
    }

    fn begin_turn(&mut self, prompt: String, user_block: bool, now: Instant, fx: &mut Vec<Effect>) {
        self.run_first_block = self.transcript.len();
        self.streamed_text = false;
        if user_block {
            self.transcript.push(Block::User {
                text: prompt.clone(),
            });
        }
        self.running = true;
        self.status.running_since = Some(now);
        self.scroll.bottom();
        fx.push(Effect::StartTurn(prompt));
    }

    /// Enter while a turn runs: queue the message and steer it into the turn.
    fn enqueue(&mut self, text: String, fx: &mut Vec<Effect>) {
        let id = self.pending.alloc_id();
        let block = self.transcript.push(Block::Queued {
            id,
            text: text.clone(),
            label: QueuedLabel::Queued,
        });
        self.pending.push(Pending {
            id,
            text: text.clone(),
            state: PendingState::Queued,
            block,
        });
        self.scroll.bottom();
        fx.push(Effect::Steer { id, text });
    }

    /// `/compact now` answered: report the rebuild (or why it failed).
    fn compact_rebuilt(&mut self, result: Result<(), CompactFailure>) {
        self.compacting = false;
        match result {
            Ok(()) => {
                self.transcript
                    .push_notice(NoticeLevel::Info, COMPACT_REBUILT);
            }
            Err(err) => {
                self.transcript
                    .push_notice(NoticeLevel::Error, format!("/compact now failed: {err}"));
            }
        }
    }

    fn set_pending_state(&mut self, id: PendingId, state: PendingState) {
        let Some(item) = self.pending.get_mut(id) else {
            return;
        };
        item.state = state;
        let block = item.block;
        if let Some(Block::Queued { label, .. }) = self.transcript.block_mut(block) {
            *label = state.label();
        }
    }

    /// Show a pending message as delivered: its placeholder becomes a user
    /// block in place. The text is kept until the turn ends, in case the
    /// runtime discards the application again.
    fn settle(&mut self, id: PendingId) {
        if let Some(item) = self.pending.settle(id) {
            let (block, text) = (item.block, item.text.clone());
            self.transcript.replace_block(block, Block::User { text });
        }
    }

    /// The runtime discarded a delivered message's application: show it as
    /// waiting again.
    fn requeue(&mut self, id: PendingId) {
        let Some(item) = self.pending.take_settled(id) else {
            return;
        };
        let state = PendingState::Admitted;
        self.transcript.replace_block(
            item.block,
            Block::Queued {
                id,
                text: item.text.clone(),
                label: state.label(),
            },
        );
        self.pending.push(Pending { state, ..item });
    }

    /// Remove transcript blocks (highest index first, so the remaining ids
    /// stay valid) and fix up every id the app holds.
    fn drop_blocks(&mut self, blocks: impl IntoIterator<Item = BlockId>) {
        let mut blocks: Vec<BlockId> = blocks.into_iter().collect();
        blocks.sort_by_key(|b| std::cmp::Reverse(b.0));
        for block in blocks {
            if self.transcript.remove_block(block).is_some() {
                self.pending.block_removed(block);
                self.helper_focus = match self.helper_focus {
                    Some(f) if f == block => None,
                    Some(f) if f.0 > block.0 => Some(BlockId(f.0 - 1)),
                    other => other,
                };
                if self.run_first_block > block.0 {
                    self.run_first_block -= 1;
                }
            }
        }
    }

    /// Take pending items out of the queue and the transcript, returning their
    /// texts oldest first.
    fn withdraw(&mut self, ids: &[PendingId]) -> Vec<String> {
        let taken = self.pending.take_where(|p| ids.contains(&p.id));
        self.drop_blocks(taken.iter().map(|p| p.block));
        taken.into_iter().map(|p| p.text).collect()
    }

    /// Put recalled texts (oldest first) before the editor contents.
    fn prepend_to_editor(&mut self, texts: Vec<String>) {
        if texts.is_empty() {
            return;
        }
        let mut text = texts.join("\n");
        let current = self.input.text();
        if !text.is_empty() && !current.is_empty() {
            text.push('\n');
        }
        text.push_str(&current);
        self.input.clear();
        self.input.set_text(&text);
    }

    fn steer_ack(
        &mut self,
        id: PendingId,
        result: Result<(), SteerFailure>,
        now: Instant,
        fx: &mut Vec<Effect>,
    ) {
        let Some(text) = self.pending.get(id).map(|p| p.text.clone()) else {
            return;
        };
        match result {
            Ok(()) => self.set_pending_state(id, PendingState::Admitted),
            // The turn ended before the message reached the runtime: hold it
            // for the turn's end (the runner's `TurnFinished` follows).
            Err(SteerFailure::NotRunning) if self.running => {
                self.set_pending_state(id, PendingState::Deferred);
            }
            Err(SteerFailure::NotRunning) => {
                if let Some(item) = self.pending.remove(id) {
                    self.transcript
                        .replace_block(item.block, Block::User { text: item.text });
                }
                self.begin_turn(text, false, now, fx);
            }
            Err(cause @ SteerFailure::Rejected(_)) => {
                let texts = self.withdraw(&[id]);
                self.prepend_to_editor(texts);
                self.transcript.push_notice(
                    NoticeLevel::Warn,
                    format!(
                        "couldn't send {}: {cause}; it is back in the editor",
                        quote_short(&text)
                    ),
                );
            }
        }
    }

    /// Esc: pull back what can still be pulled back, else interrupt the
    /// running turn. With messages the runtime admitted and no recall has
    /// found with the agent, ask the runner to recall them (the answer also
    /// brings back the held ones, so the editor keeps their order); with
    /// only held messages, recall those; while a recall is on its way, wait
    /// for it.
    fn escape(&mut self, fx: &mut Vec<Effect>) {
        if self.recalling {
            return;
        }
        if self.running && self.has_admitted_recallable() {
            self.recalling = true;
            fx.push(Effect::RecallQueued);
        } else if self.has_recallable() {
            self.recall_all();
        } else if self.running {
            self.interrupt(fx);
        }
    }

    /// Messages handed to the runner (admitted, or not acknowledged yet)
    /// that no recall has found with the agent.
    fn has_admitted_recallable(&self) -> bool {
        self.pending.items().iter().any(|p| {
            matches!(p.state, PendingState::Queued | PendingState::Admitted)
                && !self.reached.contains(&p.id)
        })
    }

    /// [`Msg::Recalled`]: the recalled messages and every held one go back
    /// to the editor (oldest first, before the draft); the admitted ones
    /// left already reached the agent.
    fn recalled(&mut self, recalled: &[PendingId], errors: &[String]) {
        self.recalling = false;
        let back: Vec<PendingId> = self
            .pending
            .items()
            .iter()
            .filter(|p| recalled.contains(&p.id) || p.state.recallable())
            .map(|p| p.id)
            .collect();
        let texts = self.withdraw(&back);
        let returned = texts.len();
        self.prepend_to_editor(texts);
        let left: Vec<PendingId> = self
            .pending
            .items()
            .iter()
            .filter(|p| matches!(p.state, PendingState::Queued | PendingState::Admitted))
            .map(|p| p.id)
            .collect();
        let reached = left.len();
        self.reached.extend(left);
        if !errors.is_empty() {
            self.transcript.push_notice(
                NoticeLevel::Warn,
                format!("could not recall: {}", errors.join("; ")),
            );
        }
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let text = match (returned, reached) {
            (0, 0) => return,
            (0, _) if self.running => {
                "your messages already reached the agent; Esc again interrupts".to_string()
            }
            (0, _) => return,
            (n, 0) => format!("{n} message{} returned to the editor", plural(n)),
            (n, m) if self.running => format!(
                "{n} message{} returned to the editor; {m} already reached the agent (Esc again interrupts)",
                plural(n)
            ),
            (n, _) => format!("{n} message{} returned to the editor", plural(n)),
        };
        self.transcript.push_notice(NoticeLevel::Info, text);
        self.scroll.bottom();
    }

    /// Esc with held messages: pull every one the UI still holds back into
    /// the editor. Messages the runtime admitted stay (see [`delivery`]).
    fn recall_all(&mut self) {
        let held: Vec<PendingId> = self
            .pending
            .items()
            .iter()
            .filter(|p| p.state.recallable())
            .map(|p| p.id)
            .collect();
        let texts = self.withdraw(&held);
        self.prepend_to_editor(texts);
    }

    /// ↑ on an empty editor with held messages: pull back the newest one.
    fn recall_newest(&mut self) {
        let Some(id) = self.pending.newest_recallable().map(|p| p.id) else {
            return;
        };
        let texts = self.withdraw(&[id]);
        self.prepend_to_editor(texts);
    }

    /// A helper's model call: count its usage for the helper (on the model
    /// the provider accounted it to, else the helper's known model).
    fn record_helper_usage(&mut self, ev: &ScopedAgentEvent, fx: &mut Vec<Effect>) {
        let AgentEvent::TurnCompleted {
            usage: Some(usage), ..
        } = &ev.event
        else {
            return;
        };
        let Some(label) = ev.scope_path.iter().rev().find_map(|frame| match frame {
            meerkat_core::StreamScopeFrame::MobMember { agent_identity, .. } => {
                Some(agent_identity.as_str())
            }
            _ => None,
        }) else {
            return;
        };
        let accounted = usage.accounting().model.trim();
        let model = if accounted.is_empty() {
            self.transcript
                .helper_model(label)
                .unwrap_or(UNKNOWN_HELPER_MODEL)
                .to_string()
        } else {
            accounted.to_string()
        };
        self.status.cost.record_helper(label, &model, usage);
        fx.push(Effect::CostSnapshot(self.status.cost.clone()));
    }

    fn turn_finished(&mut self, outcome: TurnOutcome, now: Instant, fx: &mut Vec<Effect>) {
        let TurnOutcome {
            result,
            undelivered,
        } = outcome;
        self.running = false;
        self.recalling = false;
        self.reached.clear();
        self.status.running_since = None;
        self.status.classifying = false;
        let interrupted = matches!(&result, Err(TurnFailure::Interrupted));
        let succeeded = result.is_ok();
        match result {
            Ok(summary) => {
                if !self.streamed_text && !summary.text.is_empty() {
                    self.transcript.push(Block::Assistant { md: summary.text });
                }
            }
            Err(err @ TurnFailure::Failed(_)) => {
                self.transcript
                    .push_notice(NoticeLevel::Error, format!("turn failed: {err}"));
            }
            Err(TurnFailure::Interrupted) => {
                self.transcript
                    .push_notice(NoticeLevel::Warn, "turn interrupted");
            }
            Err(err @ (TurnFailure::Unknown(_) | TurnFailure::Pending { .. })) => {
                self.transcript
                    .push_notice(NoticeLevel::Warn, format!("turn ended: {err}"));
            }
        }
        self.streamed_text = false;
        let unsent = self.collect_unsent(&undelivered);
        self.next_after_turn(unsent, interrupted, succeeded, now, fx);
        self.check_context();
    }

    /// After a turn: let the context gauge warn (warn mode; in auto mode
    /// the engine compacts on its own).
    fn check_context(&mut self) {
        let Some(used) = self.status.context_used else {
            return;
        };
        if let Some(text) = self.gauge.after_turn(used, self.status.context_window) {
            self.transcript.push_notice(NoticeLevel::Warn, text);
        }
    }

    /// After a turn: every message the turn did not deliver, oldest first.
    /// Undelivered steers (queued or already shown as delivered), messages
    /// the UI held, and (defensively) admitted messages the runtime never
    /// reported on. Messages still waiting for their admission ack stay: the
    /// ack decides.
    fn collect_unsent(&mut self, undelivered: &[PendingId]) -> Vec<Pending> {
        let mut unsent: Vec<Pending> = undelivered
            .iter()
            .filter_map(|id| self.pending.take_settled(*id))
            .collect();
        unsent.extend(self.pending.take_where(|p| {
            undelivered.contains(&p.id)
                || matches!(p.state, PendingState::Deferred | PendingState::Admitted)
        }));
        self.pending.clear_settled();
        unsent.sort_by_key(|p| p.id);
        unsent
    }

    /// Decide what to send after a turn, in priority order: the Ctrl-X Ctrl-S
    /// prompt (with anything unsent), unsent messages back to the editor after
    /// an interrupt or a failure, unsent messages as a new turn.
    fn next_after_turn(
        &mut self,
        unsent: Vec<Pending>,
        interrupted: bool,
        succeeded: bool,
        now: Instant,
        fx: &mut Vec<Effect>,
    ) {
        if let Some(next) = self.send_next.take() {
            let mut texts = self.unsent_as_user(unsent);
            texts.push(next.clone());
            self.transcript.push(Block::User { text: next });
            self.begin_turn(texts.join("\n\n"), false, now, fx);
        } else if interrupted || !succeeded {
            if !unsent.is_empty() && !interrupted {
                self.transcript.push_notice(
                    NoticeLevel::Warn,
                    format!(
                        "{} undelivered message(s) returned to the editor",
                        unsent.len()
                    ),
                );
            }
            self.drop_blocks(unsent.iter().map(|p| p.block));
            self.prepend_to_editor(unsent.into_iter().map(|p| p.text).collect());
        } else if !unsent.is_empty() {
            self.send_unsent(unsent, now, fx);
        }
    }

    /// Start one turn with unsent messages (joined by blank lines), showing
    /// them as user blocks in place.
    fn send_unsent(&mut self, unsent: Vec<Pending>, now: Instant, fx: &mut Vec<Effect>) {
        let texts = self.unsent_as_user(unsent);
        self.begin_turn(texts.join("\n\n"), false, now, fx);
    }

    /// Show unsent messages as user blocks (they are about to be sent) and
    /// return their texts.
    fn unsent_as_user(&mut self, unsent: Vec<Pending>) -> Vec<String> {
        unsent
            .into_iter()
            .map(|p| {
                self.transcript.replace_block(
                    p.block,
                    Block::User {
                        text: p.text.clone(),
                    },
                );
                p.text
            })
            .collect()
    }

    fn ctrl_c(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        if self.running {
            self.last_ctrl_c = None;
            self.interrupt(fx);
        } else if !self.input.is_empty() {
            self.last_ctrl_c = None;
            self.input.clear();
        } else if self.quit_armed(now) {
            fx.push(Effect::Quit);
        } else {
            self.last_ctrl_c = Some(now);
        }
    }

    /// Interrupt the running turn. The runner also cancels every steered
    /// message not yet delivered; [`TurnOutcome::undelivered`] brings them
    /// back to the editor.
    fn interrupt(&mut self, fx: &mut Vec<Effect>) {
        fx.push(Effect::Interrupt);
    }

    /// Ctrl-D on an empty editor or `/quit`: quit right away when idle;
    /// otherwise warn and quit only on a second request within
    /// [`QUIT_CONFIRM_WINDOW`]. Quitting hands every held message to the
    /// integrator ([`Effect::SaveUnsent`]) so undelivered ones are saved.
    fn request_quit(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        let queued = self.pending.len() + usize::from(self.send_next.is_some());
        let busy = self.running || queued > 0;
        if busy && !self.quit_confirm_armed(now) {
            self.quit_confirm = Some(now);
            let mut what = Vec::new();
            if self.running {
                what.push("a turn is running".to_string());
            }
            if queued > 0 {
                what.push(format!(
                    "{queued} message{} queued",
                    if queued == 1 { "" } else { "s" }
                ));
            }
            self.transcript.push_notice(
                NoticeLevel::Warn,
                format!(
                    "{}; press again to quit anyway (the turn is interrupted, undelivered messages are saved)",
                    what.join(" / ")
                ),
            );
            self.scroll.bottom();
            return;
        }
        self.quit_confirm = None;
        // Delivered (settled) messages are in the conversation already.
        let mut held: Vec<(Option<PendingId>, String)> = self
            .pending
            .items()
            .iter()
            .map(|p| (Some(p.id), p.text.clone()))
            .collect();
        if let Some(next) = &self.send_next {
            held.push((None, next.clone()));
        }
        if !held.is_empty() {
            fx.push(Effect::SaveUnsent(held));
        }
        fx.push(Effect::Quit);
    }

    fn run_command(&mut self, cmd: Command, now: Instant, fx: &mut Vec<Effect>) {
        let busy = |app: &mut Self| {
            app.transcript.push_notice(
                NoticeLevel::Warn,
                "a turn is running; interrupt it first (Ctrl-C)",
            );
        };
        let compacting = |app: &mut Self| {
            app.transcript.push_notice(
                NoticeLevel::Warn,
                "the session is being rebuilt for /compact now; wait for it to finish",
            );
        };
        match cmd {
            Command::Quit => self.request_quit(now, fx),
            Command::Help => {
                self.transcript
                    .push_notice(NoticeLevel::Info, commands::HELP_TEXT);
            }
            Command::Cost => {
                let text = self.status.cost.breakdown(&self.status.prices);
                self.transcript.push_notice(NoticeLevel::Info, text);
            }
            Command::Clear => {
                if self.running {
                    busy(self);
                } else if self.compacting {
                    compacting(self);
                } else if !self.pending.is_empty() {
                    self.transcript.push_notice(
                        NoticeLevel::Warn,
                        "messages are still queued; wait for them or recall them (Esc)",
                    );
                } else {
                    // The integrator resets the transcript and counters once
                    // the new session is live (and keeps them on failure).
                    fx.push(Effect::NewSession);
                }
            }
            Command::Resume(reference) => {
                if self.running {
                    busy(self);
                } else if self.compacting {
                    compacting(self);
                } else if !self.pending.is_empty() {
                    self.transcript.push_notice(
                        NoticeLevel::Warn,
                        "messages are still queued; wait for them or recall them (Esc)",
                    );
                } else {
                    fx.push(reference.map_or(Effect::RequestResumePicker, Effect::ResumeSession));
                }
            }
            Command::Remember(text) => {
                self.transcript
                    .push_notice(NoticeLevel::Info, format!("remembering: {text}"));
                fx.push(Effect::Remember(text));
            }
            Command::Compact { now: false } => {
                let text = self.gauge.policy().describe(self.status.context_window);
                self.transcript.push_notice(NoticeLevel::Info, text);
            }
            Command::Compact { now: true } => {
                if self.compacting {
                    compacting(self);
                } else if self.running || !self.pending.is_empty() {
                    self.transcript.push_notice(
                        NoticeLevel::Warn,
                        "finish or interrupt the current turn first",
                    );
                } else if self.status.context_used.is_some() {
                    self.compacting = true;
                    self.transcript
                        .push_notice(NoticeLevel::Info, "rebuilding the session so the next model call compacts…");
                    self.scroll.bottom();
                    fx.push(Effect::CompactNow);
                } else {
                    self.transcript
                        .push_notice(NoticeLevel::Info, CompactFailure::NothingYet.to_string());
                }
            }
            Command::Model(ModelCommand::Set(model)) => self.switch_model(model, fx),
            Command::Model(cmd @ (ModelCommand::Pick | ModelCommand::Refresh)) => {
                if self.model_switch_allowed() {
                    fx.push(Effect::RequestModelPicker {
                        refresh: cmd == ModelCommand::Refresh,
                    });
                }
            }
            Command::Auto(AutoCommand::Status) => {
                let (c, cm, t, tm) = self.status.denials;
                let text = format!(
                    "auto mode: {} (consecutive denials {c}/{cm}, total {t}/{tm})",
                    self.status.auto_state
                );
                self.transcript.push_notice(NoticeLevel::Info, text);
            }
            Command::Auto(AutoCommand::On) => fx.push(Effect::SetAutoMode(true)),
            Command::Auto(AutoCommand::Off) => fx.push(Effect::SetAutoMode(false)),
            Command::Auto(AutoCommand::Resume) => fx.push(Effect::ResumeAutoMode),
            Command::Rules(RulesCommand::List) => fx.push(Effect::ListRules),
            Command::Rules(RulesCommand::Add {
                effect,
                tool,
                pattern,
            }) => fx.push(Effect::AddRule {
                effect,
                tool,
                pattern,
            }),
            Command::Rules(RulesCommand::Remove(n)) => fx.push(Effect::RemoveRule(n)),
            Command::Rules(RulesCommand::Trust) => fx.push(Effect::TrustRealmRules),
            Command::McpTrust => fx.push(Effect::TrustProjectMcp),
            Command::Helpers(cmd) => self.helpers_command(cmd),
        }
    }

    fn escalation(&mut self, prompt: EscalationPrompt, id: EscalationId) {
        match &mut self.modal {
            Some(Modal::Escalation { queued_behind, .. }) => {
                self.pending_escalations.push_back((id, prompt));
                *queued_behind = self.pending_escalations.len();
            }
            _ => {
                // Escalations are time-limited and shown at once; an open
                // picker waits until they are answered.
                if let Some(picker) = self.modal.take() {
                    self.parked = Some(picker);
                }
                self.modal = Some(Modal::Escalation {
                    id,
                    prompt,
                    queued_behind: self.pending_escalations.len(),
                });
            }
        }
    }

    fn advance_escalation(&mut self) {
        self.modal = match self.pending_escalations.pop_front() {
            Some((id, prompt)) => Some(Modal::Escalation {
                id,
                prompt,
                queued_behind: self.pending_escalations.len(),
            }),
            None => self.parked.take(),
        };
    }

    fn escalation_resolved(&mut self, resolved: EscalationId) {
        match &mut self.modal {
            Some(Modal::Escalation { id, .. }) if *id == resolved => self.advance_escalation(),
            Some(Modal::Escalation { queued_behind, .. }) => {
                self.pending_escalations.retain(|(id, _)| *id != resolved);
                *queued_behind = self.pending_escalations.len();
            }
            _ => self.pending_escalations.retain(|(id, _)| *id != resolved),
        }
    }

    fn stop_all_escalations(&mut self, fx: &mut Vec<Effect>) {
        if let Some(Modal::Escalation { id, .. }) = self.modal.take() {
            fx.push(Effect::RespondEscalation(id, EscalationAnswer::Stop));
        }
        for (id, _) in self.pending_escalations.drain(..) {
            fx.push(Effect::RespondEscalation(id, EscalationAnswer::Stop));
        }
        if self.modal.is_none() {
            self.modal = self.parked.take();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::transcript::test_support::{test_args, test_tool_result, test_usage};
    use super::*;
    use meerkat_core::{StopReason, Usage};
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    use serde_json::json;

    fn key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Msg {
        Msg::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn agent(ev: AgentEvent) -> Msg {
        Msg::Agent(EventEnvelope::new("s", 0, None, ev))
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.update(key(KeyCode::Char(c)));
        }
    }

    fn summary() -> TurnSummary {
        TurnSummary {
            text: String::new(),
            input_tokens: 0,
            output_tokens: 0,
        }
    }

    #[test]
    fn submit_starts_turn_and_streams_into_one_block() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "hi");
        let fx = app.update(key(KeyCode::Enter));
        assert_eq!(fx, vec![Effect::StartTurn("hi".into())]);
        assert!(app.is_running());
        assert!(app.status.running_since.is_some());
        app.update(agent(AgentEvent::TurnStarted {
            turn_number: 0,
            assistant_message_id: None,
        }));
        for d in ["Hel", "lo ", "there"] {
            app.update(agent(AgentEvent::TextDelta {
                delta: d.into(),
                assistant_message_id: None,
            }));
        }
        app.update(agent(AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: test_usage(Usage {
                input_tokens: 10,
                output_tokens: 4,
                ..Usage::default()
            }),
            assistant_message_id: None,
        }));
        assert_eq!(
            app.transcript.blocks(),
            &[
                Block::User { text: "hi".into() },
                Block::Assistant {
                    md: "Hello there".into()
                }
            ]
        );
        assert_eq!((app.status.input_tokens, app.status.output_tokens), (10, 4));
        app.update(Msg::TurnFinished(TurnOutcome::plain(Ok(summary()))));
        assert!(!app.is_running());
        assert!(app.status.running_since.is_none());
    }

    #[test]
    fn turn_summary_text_used_when_nothing_streamed() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "q");
        app.update(key(KeyCode::Enter));
        app.update(Msg::TurnFinished(TurnOutcome::plain(Ok(TurnSummary {
            text: "answer".into(),
            ..summary()
        }))));
        assert_eq!(
            app.transcript.blocks().last(),
            Some(&Block::Assistant {
                md: "answer".into()
            })
        );
    }

    #[test]
    fn tool_lifecycle_updates_block_states() {
        let mut app = App::new(StatusInfo::default());
        app.update(agent(AgentEvent::ToolCallRequested {
            id: "t1".into(),
            name: "edit_file".into(),
            args: test_args(json!({"path": "src/x.rs"})),
        }));
        let id = app.transcript.tool_block("t1").unwrap();
        app.update(agent(AgentEvent::ToolExecutionStarted {
            id: "t1".into(),
            name: "edit_file".into(),
        }));
        assert!(matches!(
            app.transcript.block(id),
            Some(Block::Tool {
                state: ToolState::Running { .. },
                ..
            })
        ));
        app.update(Msg::Diff {
            tool_use_id: "t1".into(),
            path: "src/x.rs".into(),
            unified: "-a\n+b".into(),
            added: 1,
            removed: 1,
        });
        app.update(agent(AgentEvent::ToolExecutionCompleted {
            id: "t1".into(),
            name: "edit_file".into(),
            is_error: true,
            duration_ms: 12,
        }));
        app.update(agent(test_tool_result("t1", "ok")));
        match app.transcript.block(id) {
            Some(Block::Tool {
                state,
                result_preview,
                diff: Some(d),
                ..
            }) => {
                assert_eq!(
                    *state,
                    ToolState::Done {
                        is_error: true,
                        duration_ms: 12
                    }
                );
                assert_eq!(result_preview.as_deref(), Some("ok"));
                assert_eq!(d.added, 1);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    /// What the renderer does: lay out at `width` and record the rows of a
    /// transcript drawn from screen row `top` with `height` rows.
    fn draw_rows(app: &mut App, top: u16, width: u16, height: usize) {
        let total = app.transcript.ensure_layout(width);
        let start = app.scroll.window(total, height);
        let rows = app.transcript.visible_blocks(start, height);
        let area = Rect::new(0, top, width, u16::try_from(height).unwrap());
        app.set_transcript_rows(area, start, rows);
    }

    fn mouse(kind: MouseEventKind, row: u16, column: u16) -> Msg {
        Msg::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn down(row: u16, column: u16) -> Msg {
        mouse(MouseEventKind::Down(MouseButton::Left), row, column)
    }

    fn drag(row: u16, column: u16) -> Msg {
        mouse(MouseEventKind::Drag(MouseButton::Left), row, column)
    }

    fn up(row: u16, column: u16) -> Msg {
        mouse(MouseEventKind::Up(MouseButton::Left), row, column)
    }

    /// A left click (press and release without moving) at column 3.
    fn click_at(app: &mut App, row: u16) {
        app.update(down(row, 3));
        app.update(up(row, 3));
    }

    fn tool_collapsed(app: &App, id: BlockId) -> bool {
        match app.transcript.block(id) {
            Some(Block::Tool { collapsed, .. }) => *collapsed,
            other => panic!("not a tool block: {other:?}"),
        }
    }

    #[test]
    fn click_toggles_tool_output_and_ctrl_t_toggles_the_last() {
        let mut app = App::new(StatusInfo::default());
        app.transcript.push(user("look"));
        for id in ["t1", "t2"] {
            app.update(agent(AgentEvent::ToolCallRequested {
                id: id.into(),
                name: "read_file".into(),
                args: test_args(json!({"path": "a.rs"})),
            }));
            app.update(agent(AgentEvent::ToolExecutionCompleted {
                id: id.into(),
                name: "read_file".into(),
                is_error: false,
                duration_ms: 1,
            }));
            app.update(agent(test_tool_result(id, "a\nb\nc")));
        }
        let (t1, t2) = (
            app.transcript.tool_block("t1").unwrap(),
            app.transcript.tool_block("t2").unwrap(),
        );
        assert!(tool_collapsed(&app, t1) && tool_collapsed(&app, t2));
        // Transcript drawn from screen row 2: user (2 rows), t1 (2), t2 (2).
        draw_rows(&mut app, 2, 60, 20);
        assert_eq!(app.block_at_row(1), None);
        assert_eq!(app.block_at_row(4), Some(t1));
        assert_eq!(app.block_at_row(30), None);

        app.take_dirty();
        click_at(&mut app, 4);
        assert!(!tool_collapsed(&app, t1));
        assert!(tool_collapsed(&app, t2));
        assert!(app.take_dirty());
        // Expanded t1 is 5 rows (header, 3 lines, spacer); a click on its
        // output collapses it again.
        draw_rows(&mut app, 2, 60, 20);
        assert_eq!(app.block_at_row(6), Some(t1));
        click_at(&mut app, 6);
        assert!(tool_collapsed(&app, t1));

        // A click on a non-tool block changes nothing and needs no redraw.
        draw_rows(&mut app, 2, 60, 20);
        app.take_dirty();
        click_at(&mut app, 2);
        assert!(!app.take_dirty());
        assert_eq!(app.take_clipboard_text(), None);

        app.update(ctrl('t'));
        assert!(!tool_collapsed(&app, t2));
        assert!(tool_collapsed(&app, t1));
        app.update(ctrl('t'));
        assert!(tool_collapsed(&app, t2));
    }

    #[test]
    fn clicks_are_ignored_under_a_modal() {
        let mut app = App::new(StatusInfo::default());
        app.update(agent(AgentEvent::ToolCallRequested {
            id: "t1".into(),
            name: "glob".into(),
            args: test_args(json!({})),
        }));
        app.update(agent(AgentEvent::ToolExecutionCompleted {
            id: "t1".into(),
            name: "glob".into(),
            is_error: false,
            duration_ms: 1,
        }));
        app.update(agent(test_tool_result("t1", "x")));
        let t1 = app.transcript.tool_block("t1").unwrap();
        draw_rows(&mut app, 0, 60, 10);
        app.update(Msg::ShowResumePicker(vec![("a".into(), "a".into())]));
        click_at(&mut app, 0);
        assert!(tool_collapsed(&app, t1));
        app.update(down(0, 0));
        app.update(drag(0, 5));
        app.update(up(0, 5));
        assert!(app.selection().is_none());
        assert_eq!(app.take_clipboard_text(), None);
    }

    /// `› hello world` / spacer / `› second line here` / spacer, drawn from
    /// screen row 1 at width 60.
    fn selection_app() -> App {
        let mut app = App::new(StatusInfo::default());
        app.transcript.push(user("hello world"));
        app.transcript.push(user("second line here"));
        draw_rows(&mut app, 1, 60, 20);
        app
    }

    #[test]
    fn drag_selects_and_queues_clipboard_text() {
        let mut app = selection_app();
        app.update(down(1, 2));
        assert!(app.selection().is_none());
        app.update(drag(1, 4));
        app.update(drag(3, 7));
        let sel = *app.selection().unwrap();
        assert_eq!(sel.mode, SelectMode::Char);
        assert_eq!(sel.range(), (CellPos::new(0, 2), CellPos::new(2, 7)));
        assert_eq!(app.take_clipboard_text(), None);
        app.update(up(3, 7));
        assert_eq!(
            app.take_clipboard(),
            Some(QueuedCopy {
                text: "hello world\n\n› second".into(),
                truncated: false,
            })
        );
        // The runtime reports how the copy went; the app only queues it.
        assert_eq!(last_notice(&app), None);
        // The selection stays visible after copying.
        assert!(app.selection().is_some());
    }

    #[test]
    fn double_and_triple_click_select_word_and_line() {
        let mut app = selection_app();
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        app.update_at(down(1, 9), at(0));
        app.update_at(up(1, 9), at(10));
        assert_eq!(app.take_clipboard_text(), None);
        app.update_at(down(1, 9), at(100));
        assert_eq!(app.selection().unwrap().mode, SelectMode::Word);
        app.update_at(up(1, 9), at(110));
        assert_eq!(app.take_clipboard_text().as_deref(), Some("world"));
        app.update_at(down(1, 9), at(200));
        assert_eq!(app.selection().unwrap().mode, SelectMode::Line);
        app.update_at(up(1, 9), at(210));
        assert_eq!(app.take_clipboard_text().as_deref(), Some("› hello world"));
        // Too slow, or elsewhere: a plain click again, which clears the selection.
        app.update_at(down(1, 9), at(1000));
        assert!(app.selection().is_none());
        app.update_at(up(1, 9), at(1010));
        app.update_at(down(1, 3), at(1100));
        app.update_at(up(1, 3), at(1110));
        assert_eq!(app.take_clipboard_text(), None);
        // Dragging a double-click selection extends it by words.
        app.update_at(down(3, 4), at(2000));
        app.update_at(up(3, 4), at(2010));
        app.update_at(down(3, 4), at(2100));
        app.update_at(drag(3, 10), at(2150));
        app.update_at(up(3, 10), at(2200));
        assert_eq!(app.take_clipboard_text().as_deref(), Some("second line"));
    }

    #[test]
    fn esc_and_other_keys_clear_the_selection() {
        let mut app = selection_app();
        app.update(down(1, 2));
        app.update(drag(1, 6));
        app.update(up(1, 6));
        assert!(app.selection().is_some());
        app.running = true;
        // Esc only dismisses the selection; it does not interrupt.
        assert!(app.update(key(KeyCode::Esc)).is_empty());
        assert!(app.selection().is_none());
        assert_eq!(app.update(key(KeyCode::Esc)), vec![Effect::Interrupt]);

        // So does the first Ctrl-C; the second interrupts.
        app.update(down(1, 2));
        app.update(drag(1, 6));
        app.update(up(1, 6));
        assert!(app.selection().is_some());
        assert!(app.update(ctrl('c')).is_empty());
        assert!(app.selection().is_none());
        assert_eq!(app.update(ctrl('c')), vec![Effect::Interrupt]);

        app.update(down(1, 2));
        app.update(drag(1, 6));
        app.update(up(1, 6));
        app.update(key(KeyCode::Char('x')));
        assert!(app.selection().is_none());
        assert_eq!(app.input.text(), "x");
    }

    #[test]
    fn ctrl_y_recopies_the_selection_or_the_last_assistant_message() {
        let mut app = selection_app();
        app.update(ctrl('y'));
        assert_eq!(app.take_clipboard_text(), None);
        assert_eq!(
            last_notice(&app),
            Some((NoticeLevel::Info, "nothing to copy".into()))
        );
        app.transcript.push(Block::Assistant {
            md: "first **answer**".into(),
        });
        app.transcript.push(Block::Assistant {
            md: "the *last* answer".into(),
        });
        app.update(ctrl('y'));
        assert_eq!(
            app.take_clipboard_text().as_deref(),
            Some("the *last* answer")
        );

        draw_rows(&mut app, 1, 60, 20);
        app.update(down(1, 2));
        app.update(drag(1, 6));
        app.update(up(1, 6));
        assert_eq!(app.take_clipboard_text().as_deref(), Some("hello"));
        app.update(ctrl('y'));
        assert_eq!(app.take_clipboard_text().as_deref(), Some("hello"));
        assert!(app.selection().is_some());
    }

    #[test]
    fn dragging_past_the_edges_scrolls() {
        let mut app = App::new(StatusInfo::default());
        for i in 0..20 {
            app.transcript.push(user(&format!("line {i}")));
        }
        // 40 lines, 10 visible from screen row 2; following the bottom.
        draw_rows(&mut app, 2, 40, 10);
        assert_eq!(app.scroll.offset_from_bottom, 0);
        app.update(down(5, 2));
        app.update(drag(1, 2));
        assert_eq!(app.scroll.offset_from_bottom, 1);
        assert!(!app.scroll.follow);
        // The head is on the line above the first visible one (line 30).
        assert_eq!(app.selection().unwrap().range().0, CellPos::new(29, 2));
        draw_rows(&mut app, 2, 40, 10);
        app.update(drag(0, 2));
        assert_eq!(app.scroll.offset_from_bottom, 2);
        draw_rows(&mut app, 2, 40, 10);
        // Below the transcript (input box rows) scrolls back down.
        app.update(drag(13, 2));
        assert_eq!(app.scroll.offset_from_bottom, 1);
        app.update(up(13, 2));
        assert!(app.take_clipboard_text().is_some());
    }

    fn agent_at(ev: AgentEvent, seq: u64) -> Msg {
        Msg::Agent(EventEnvelope::new("s", seq, None, ev))
    }

    fn submit(app: &mut App, s: &str) -> Vec<Effect> {
        type_str(app, s);
        app.update(key(KeyCode::Enter))
    }

    fn finished(result: Result<TurnSummary, TurnFailure>, undelivered: &[u64]) -> Msg {
        Msg::TurnFinished(TurnOutcome {
            result,
            undelivered: undelivered.iter().copied().map(PendingId).collect(),
        })
    }

    fn ack(id: u64, result: Result<(), SteerFailure>) -> Msg {
        Msg::SteerAck {
            id: PendingId(id),
            result,
        }
    }

    fn queued_block(id: u64, text: &str, label: QueuedLabel) -> Block {
        Block::Queued {
            id: PendingId(id),
            text: text.into(),
            label,
        }
    }

    fn user(text: &str) -> Block {
        Block::User { text: text.into() }
    }

    /// A running app with "one" as the turn prompt.
    fn running_app() -> App {
        let mut app = App::new(StatusInfo::default());
        assert_eq!(
            submit(&mut app, "one"),
            vec![Effect::StartTurn("one".into())]
        );
        app
    }

    fn steer(id: u64, text: &str) -> Effect {
        Effect::Steer {
            id: PendingId(id),
            text: text.into(),
        }
    }

    #[test]
    fn enter_while_running_queues_and_steers() {
        let mut app = running_app();
        assert_eq!(submit(&mut app, "two"), vec![steer(1, "two")]);
        assert_eq!(submit(&mut app, "three"), vec![steer(2, "three")]);
        assert_eq!(app.status.queued, 2);
        assert_eq!(
            app.transcript.blocks(),
            &[
                user("one"),
                queued_block(1, "two", QueuedLabel::Queued),
                queued_block(2, "three", QueuedLabel::Queued),
            ]
        );
        // The runtime holds them: nothing to recall locally.
        assert!(!app.has_recallable());
    }

    #[test]
    fn admitted_then_delivered_in_place() {
        let mut app = running_app();
        app.update(agent_at(
            AgentEvent::TextDelta {
                delta: "a".into(),
                assistant_message_id: None,
            },
            4,
        ));
        submit(&mut app, "two");
        app.update(ack(1, Ok(())));
        assert_eq!(
            app.pending.get(PendingId(1)).map(|p| p.state),
            Some(PendingState::Admitted)
        );
        assert_eq!(
            app.transcript.block(BlockId(2)),
            Some(&queued_block(1, "two", QueuedLabel::Delivering))
        );
        // A turn boundary alone does not count: only the runtime's ack.
        app.update(agent_at(
            AgentEvent::TurnStarted {
                turn_number: 1,
                assistant_message_id: None,
            },
            5,
        ));
        assert_eq!(app.status.queued, 1);
        assert!(app.update(Msg::SteerDelivered(PendingId(1))).is_empty());
        app.update(agent_at(
            AgentEvent::TextDelta {
                delta: "b".into(),
                assistant_message_id: None,
            },
            6,
        ));
        assert_eq!(app.status.queued, 0);
        // Replaced in place: transcript order is preserved.
        assert_eq!(
            app.transcript.blocks(),
            &[
                user("one"),
                Block::Assistant { md: "a".into() },
                user("two"),
                Block::Assistant { md: "b".into() },
            ]
        );
        let fx = app.update(finished(Ok(summary()), &[]));
        assert!(fx.is_empty());
        assert!(!app.is_running());
    }

    #[test]
    fn requeued_delivery_returns_to_waiting() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(ack(1, Ok(())));
        app.update(Msg::SteerDelivered(PendingId(1)));
        assert_eq!(app.transcript.block(BlockId(1)), Some(&user("two")));
        app.update(Msg::SteerRequeued(PendingId(1)));
        assert_eq!(
            app.transcript.block(BlockId(1)),
            Some(&queued_block(1, "two", QueuedLabel::Delivering))
        );
        assert_eq!(app.status.queued, 1);
        app.update(Msg::SteerDelivered(PendingId(1)));
        assert_eq!(app.status.queued, 0);
        assert!(app.update(finished(Ok(summary()), &[])).is_empty());
        assert_eq!(app.transcript.blocks(), &[user("one"), user("two")]);
    }

    #[test]
    fn ack_not_running_defers_or_starts_turn() {
        let mut app = running_app();
        submit(&mut app, "two");
        // The turn is ending: hold it for the turn's end.
        assert!(app.update(ack(1, Err(SteerFailure::NotRunning))).is_empty());
        assert_eq!(
            app.transcript.block(BlockId(1)),
            Some(&queued_block(1, "two", QueuedLabel::Deferred))
        );
        let fx = app.update(finished(Ok(summary()), &[]));
        assert_eq!(fx, vec![Effect::StartTurn("two".into())]);
        assert_eq!(app.transcript.blocks(), &[user("one"), user("two")]);
        app.update(finished(Ok(summary()), &[]));

        // Already idle when the refusal arrives: send it as a prompt,
        // converting the placeholder in place.
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(finished(Ok(summary()), &[]));
        assert!(!app.is_running());
        assert_eq!(app.status.queued, 1);
        assert_eq!(
            app.update(ack(1, Err(SteerFailure::NotRunning))),
            vec![Effect::StartTurn("two".into())]
        );
        assert!(app.is_running());
        assert_eq!(app.status.queued, 0);
        assert_eq!(app.transcript.blocks(), &[user("one"), user("two")]);
    }

    #[test]
    fn rejected_steer_returns_to_the_editor_with_a_notice() {
        let mut app = running_app();
        submit(&mut app, "two");
        type_str(&mut app, "draft");
        assert!(
            app.update(ack(1, Err(SteerFailure::Rejected("closed".into()))))
                .is_empty()
        );
        assert_eq!(app.input.text(), "two\ndraft");
        assert!(app.pending.is_empty());
        assert!(matches!(
            last_notice(&app),
            Some((NoticeLevel::Warn, text)) if text.contains("closed") && text.contains("“two”")
        ));
    }

    #[test]
    fn esc_recalls_only_held_messages_and_otherwise_interrupts() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(ack(1, Err(SteerFailure::NotRunning)));
        type_str(&mut app, "draft");
        assert!(app.has_recallable());
        // Only a held message: it comes back without the runner.
        assert!(app.update(key(KeyCode::Esc)).is_empty());
        assert_eq!(app.input.text(), "two\ndraft");
        assert_eq!(app.transcript.blocks(), &[user("one")]);
        // Esc again: interrupt.
        assert_eq!(app.update(key(KeyCode::Esc)), vec![Effect::Interrupt]);
    }

    #[test]
    fn esc_with_an_admitted_message_recalls_it_without_interrupting() {
        let mut app = running_app();
        submit(&mut app, "two");
        submit(&mut app, "three");
        app.update(ack(1, Ok(())));
        app.update(ack(2, Err(SteerFailure::NotRunning)));
        type_str(&mut app, "draft");
        assert_eq!(app.update(key(KeyCode::Esc)), vec![Effect::RecallQueued]);
        // While the recall is on its way, Esc waits for it.
        assert!(app.update(key(KeyCode::Esc)).is_empty());
        let fx = app.update(Msg::Recalled {
            recalled: vec![PendingId(1)],
            errors: Vec::new(),
        });
        assert!(fx.is_empty());
        // The recalled and the held message, oldest first, before the draft.
        assert_eq!(app.input.text(), "two\nthree\ndraft");
        assert_eq!(app.transcript.blocks()[0], user("one"));
        assert!(
            !app.transcript
                .blocks()
                .iter()
                .any(|b| matches!(b, Block::Queued { .. }))
        );
        assert_eq!(
            last_notice(&app),
            Some((NoticeLevel::Info, "2 messages returned to the editor".into()))
        );
        assert_eq!(app.status.queued, 0);
        // Nothing left to recall: Esc interrupts.
        assert_eq!(app.update(key(KeyCode::Esc)), vec![Effect::Interrupt]);
    }

    #[test]
    fn esc_when_the_messages_already_reached_the_agent_notices_then_interrupts() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(ack(1, Ok(())));
        assert_eq!(app.update(key(KeyCode::Esc)), vec![Effect::RecallQueued]);
        let fx = app.update(Msg::Recalled {
            recalled: Vec::new(),
            errors: Vec::new(),
        });
        assert!(fx.is_empty());
        assert_eq!(
            last_notice(&app),
            Some((
                NoticeLevel::Info,
                "your messages already reached the agent; Esc again interrupts".into()
            ))
        );
        // The message stays with the runtime.
        assert_eq!(
            app.transcript.blocks()[1],
            queued_block(1, "two", QueuedLabel::Delivering)
        );
        assert_eq!(app.update(key(KeyCode::Esc)), vec![Effect::Interrupt]);
    }

    #[test]
    fn esc_with_nothing_pending_interrupts() {
        let mut app = running_app();
        assert_eq!(app.update(key(KeyCode::Esc)), vec![Effect::Interrupt]);
        let mut idle = App::new(StatusInfo::default());
        assert!(idle.update(key(KeyCode::Esc)).is_empty());
    }

    #[test]
    fn up_recalls_the_newest_held_message() {
        let mut app = running_app().with_history(vec!["old".into()]);
        submit(&mut app, "two");
        submit(&mut app, "three");
        app.update(ack(1, Err(SteerFailure::NotRunning)));
        app.update(ack(2, Ok(())));
        assert!(app.update(key(KeyCode::Up)).is_empty());
        assert_eq!(app.input.text(), "two");
        assert_eq!(app.status.queued, 1);
        // With nothing held, ↑ is history recall again.
        app.update(ctrl('u'));
        app.update(key(KeyCode::Up));
        assert_eq!(app.input.text(), "three");
    }

    #[test]
    fn successful_turn_sends_held_messages_as_the_next_turn() {
        let mut app = running_app();
        submit(&mut app, "two");
        submit(&mut app, "three");
        app.update(ack(1, Err(SteerFailure::NotRunning)));
        app.update(ack(2, Err(SteerFailure::NotRunning)));
        let fx = app.update(finished(Ok(summary()), &[]));
        assert_eq!(fx, vec![Effect::StartTurn("two\n\nthree".into())]);
        assert_eq!(
            app.transcript.blocks(),
            &[user("one"), user("two"), user("three")]
        );
    }

    #[test]
    fn interrupted_turn_returns_undelivered_to_the_editor() {
        let mut app = running_app();
        submit(&mut app, "two");
        submit(&mut app, "three");
        app.update(ack(1, Ok(())));
        app.update(ack(2, Ok(())));
        app.update(Msg::SteerDelivered(PendingId(1)));
        assert_eq!(app.update(ctrl('c')), vec![Effect::Interrupt]);
        assert_eq!(app.status.queued, 1);
        type_str(&mut app, "draft");
        let fx = app.update(finished(Err(TurnFailure::Interrupted), &[2]));
        assert!(fx.is_empty());
        assert_eq!(app.input.text(), "three\ndraft");
        assert!(app.transcript.blocks().contains(&user("two")));
        assert!(
            !app.transcript
                .blocks()
                .iter()
                .any(|b| matches!(b, Block::Queued { .. })
                    || matches!(b, Block::User { text } if text == "three"))
        );
        assert_eq!(app.status.queued, 0);
    }

    #[test]
    fn discarded_delivery_of_a_stopped_turn_returns_too() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(ack(1, Ok(())));
        app.update(Msg::SteerDelivered(PendingId(1)));
        let fx = app.update(finished(Err(TurnFailure::Interrupted), &[1]));
        assert!(fx.is_empty());
        assert_eq!(app.input.text(), "two");
        assert!(
            !app.transcript
                .blocks()
                .iter()
                .any(|b| matches!(b, Block::User { text } if text == "two"))
        );
    }

    #[test]
    fn failed_turn_returns_undelivered_with_a_notice() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(ack(1, Ok(())));
        assert!(app.update(finished(Err(TurnFailure::Failed("boom".into())), &[1])).is_empty());
        assert_eq!(app.input.text(), "two");
        assert!(matches!(
            last_notice(&app),
            Some((NoticeLevel::Warn, text)) if text.contains("returned to the editor")
        ));
    }

    #[test]
    fn send_next_takes_priority_over_undelivered() {
        let mut app = running_app();
        let t0 = Instant::now();
        submit(&mut app, "two");
        app.update(ack(1, Ok(())));
        type_str(&mut app, "urgent");
        app.update_at(ctrl('x'), t0);
        assert_eq!(
            app.update_at(ctrl('s'), t0),
            vec![Effect::InterruptAndSend("urgent".into())]
        );
        assert_eq!(app.send_next.as_deref(), Some("urgent"));
        // The queued message stays in place until the turn ends.
        assert_eq!(app.status.queued, 1);
        let fx = app.update(finished(Err(TurnFailure::Interrupted), &[1]));
        assert_eq!(fx, vec![Effect::StartTurn("two\n\nurgent".into())]);
        assert!(app.send_next.is_none());
        assert!(app.is_running());
        assert!(app.input.is_empty());
        let users: Vec<&Block> = app
            .transcript
            .blocks()
            .iter()
            .filter(|b| matches!(b, Block::User { .. }))
            .collect();
        assert_eq!(users, vec![&user("one"), &user("two"), &user("urgent")]);
    }

    #[test]
    fn clear_is_refused_while_messages_are_pending() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(finished(Ok(summary()), &[]));
        assert_eq!(app.status.queued, 1);
        assert!(submit(&mut app, "/clear").is_empty());
        assert!(matches!(
            app.transcript.blocks().last(),
            Some(Block::Notice { level: NoticeLevel::Warn, text }) if text.contains("queued")
        ));
        assert!(!app.transcript.is_empty());
    }

    #[test]
    fn reset_transcript_keeps_queued_placeholders() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(Msg::ResetTranscript(vec![user("restored")]));
        assert_eq!(
            app.transcript.blocks(),
            &[
                user("restored"),
                queued_block(1, "two", QueuedLabel::Queued)
            ]
        );
        assert_eq!(
            app.pending.get(PendingId(1)).map(|p| p.block),
            Some(BlockId(1))
        );
    }

    #[test]
    fn ctrl_c_interrupts_running_turn_and_keeps_pending() {
        let mut app = running_app();
        submit(&mut app, "two");
        assert_eq!(app.update(ctrl('c')), vec![Effect::Interrupt]);
        assert_eq!(app.status.queued, 1);
        assert!(app.input.is_empty());
    }

    #[test]
    fn ctrl_c_clears_input_then_double_press_quits() {
        let mut app = App::new(StatusInfo::default());
        let t0 = Instant::now();
        type_str(&mut app, "draft");
        assert!(app.update_at(ctrl('c'), t0).is_empty());
        assert!(app.input.is_empty());
        assert!(app.update_at(ctrl('c'), t0).is_empty());
        assert!(app.quit_armed(t0));
        // Too late: re-arms instead of quitting.
        let late = t0 + Duration::from_secs(2);
        assert!(app.update_at(ctrl('c'), late).is_empty());
        let soon = late + Duration::from_millis(500);
        assert_eq!(app.update_at(ctrl('c'), soon), vec![Effect::Quit]);
    }

    #[test]
    fn ctrl_c_with_modal_stops_every_pending_escalation() {
        let mut app = App::new(StatusInfo::default());
        let prompt = |t: &str| EscalationPrompt {
            agent: None,
            tool: t.into(),
            args_preview: "{}".into(),
            hint: None,
        };
        app.update(Msg::Escalation(prompt("a"), EscalationId(1)));
        app.update(Msg::Escalation(prompt("b"), EscalationId(2)));
        assert_eq!(app.pending_escalations(), 1);
        assert!(matches!(
            app.modal,
            Some(Modal::Escalation {
                queued_behind: 1,
                ..
            })
        ));
        let fx = app.update(ctrl('c'));
        assert_eq!(
            fx,
            vec![
                Effect::RespondEscalation(EscalationId(1), EscalationAnswer::Stop),
                Effect::RespondEscalation(EscalationId(2), EscalationAnswer::Stop),
            ]
        );
        assert!(app.modal.is_none());
        assert_eq!(app.pending_escalations(), 0);
    }

    #[test]
    fn escalation_keys_answer_and_advance() {
        let mut app = App::new(StatusInfo::default());
        let p = EscalationPrompt {
            agent: None,
            tool: "shell".into(),
            args_preview: "rm x".into(),
            hint: Some("deletes".into()),
        };
        app.update(Msg::Escalation(p.clone(), EscalationId(7)));
        app.update(Msg::Escalation(p.clone(), EscalationId(8)));
        app.update(Msg::Escalation(p, EscalationId(9)));
        // Typing goes to the modal, not the input.
        assert!(app.update(key(KeyCode::Char('x'))).is_empty());
        assert!(app.input.is_empty());
        assert_eq!(
            app.update(key(KeyCode::Char('y'))),
            vec![Effect::RespondEscalation(
                EscalationId(7),
                EscalationAnswer::AllowOnce
            )]
        );
        app.update(Msg::EscalationResolved(EscalationId(8)));
        assert!(matches!(
            app.modal,
            Some(Modal::Escalation {
                id: EscalationId(9),
                ..
            })
        ));
        assert_eq!(
            app.update(key(KeyCode::Char('a'))),
            vec![Effect::RespondEscalation(
                EscalationId(9),
                EscalationAnswer::AllowAndAddRule
            )]
        );
        assert!(app.modal.is_none());
    }

    #[test]
    fn ctrl_d_quits_only_on_empty_input() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "ab");
        app.update(key(KeyCode::Left));
        assert!(app.update(ctrl('d')).is_empty());
        assert_eq!(app.input.text(), "a");
        app.update(key(KeyCode::Backspace));
        assert_eq!(app.update(ctrl('d')), vec![Effect::Quit]);
    }

    #[test]
    fn ctrl_o_toggles_reasoning() {
        let mut app = App::new(StatusInfo::default());
        app.update(agent(AgentEvent::ReasoningDelta {
            delta: "think".into(),
            assistant_message_id: None,
        }));
        app.update(ctrl('o'));
        assert!(matches!(
            app.transcript.blocks()[0],
            Block::Reasoning {
                collapsed: false,
                ..
            }
        ));
    }

    #[test]
    fn newline_variants_and_paste() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "a");
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));
        type_str(&mut app, "b");
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)));
        app.update(ctrl('j'));
        app.update(Msg::Paste("x\ny".into()));
        assert_eq!(app.input.text(), "a\nb\n\nx\ny");
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::StartTurn("a\nb\n\nx\ny".into())]
        );
    }

    #[test]
    fn scroll_follow_semantics() {
        let mut s = ScrollState::default();
        assert_eq!(s.window(100, 10), 90);
        s.up(5);
        assert!(!s.follow);
        assert_eq!(s.window(100, 10), 85);
        // New output does not move the view while scrolled back.
        assert_eq!(s.window(110, 10), 85);
        s.down(100);
        assert!(s.follow);
        assert_eq!(s.window(120, 10), 110);
        s.top();
        assert_eq!(s.window(120, 10), 0);
        s.bottom();
        assert!(s.follow);
        assert_eq!(s.window(120, 10), 110);
        // Short transcripts never scroll.
        let mut short = ScrollState::default();
        short.up(3);
        assert_eq!(short.window(4, 10), 0);
        assert!(short.follow);
    }

    #[test]
    fn page_and_end_keys_drive_scroll_state() {
        let mut app = App::new(StatusInfo::default());
        app.set_viewport_height(12);
        app.update(key(KeyCode::PageUp));
        assert_eq!(app.scroll.offset_from_bottom, 10);
        assert!(!app.scroll.follow);
        app.update(Msg::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT)));
        assert_eq!(app.scroll.offset_from_bottom, 7);
        app.update(key(KeyCode::End));
        assert!(app.scroll.follow);
        assert_eq!(app.scroll.offset_from_bottom, 0);
    }

    #[test]
    fn slash_commands_map_to_effects() {
        let mut app = App::new(StatusInfo::default());
        let run = |app: &mut App, s: &str| {
            type_str(app, s);
            app.update(key(KeyCode::Enter))
        };
        assert_eq!(
            run(&mut app, "/model gpt-5.4"),
            vec![Effect::SetModel("gpt-5.4".into())]
        );
        assert_eq!(run(&mut app, "/auto off"), vec![Effect::SetAutoMode(false)]);
        assert_eq!(run(&mut app, "/resume"), vec![Effect::RequestResumePicker]);
        assert_eq!(
            run(&mut app, "/resume last"),
            vec![Effect::ResumeSession("last".into())]
        );
        assert_eq!(run(&mut app, "/rules rm 2"), vec![Effect::RemoveRule(2)]);
        // The transcript is reset only once the integrator confirms.
        app.transcript.push_notice(NoticeLevel::Info, "old");
        assert_eq!(run(&mut app, "/clear"), vec![Effect::NewSession]);
        assert!(!app.transcript.is_empty());
        app.update(Msg::ResetTranscript(Vec::new()));
        assert!(app.transcript.is_empty());
        assert!(run(&mut app, "/bogus").is_empty());
        assert!(matches!(
            app.transcript.blocks().last(),
            Some(Block::Notice { level: NoticeLevel::Warn, text }) if text.contains("/bogus")
        ));
        assert_eq!(
            run(&mut app, "//etc"),
            vec![Effect::StartTurn("/etc".into())]
        );
        // While running, /clear and /model are refused, /quit asks again.
        assert!(run(&mut app, "/clear").is_empty());
        assert!(run(&mut app, "/model gpt-5.2").is_empty());
        assert!(matches!(
            app.transcript.blocks().last(),
            Some(Block::Notice { level: NoticeLevel::Warn, text }) if text.contains("idle")
        ));
        assert!(run(&mut app, "/quit").is_empty());
        assert_eq!(run(&mut app, "/quit"), vec![Effect::Quit]);
    }

    #[test]
    fn model_is_refused_while_messages_are_pending() {
        let mut app = running_app();
        submit(&mut app, "two");
        app.update(finished(Ok(summary()), &[]));
        assert!(!app.is_running());
        assert_eq!(app.status.queued, 1);
        assert!(submit(&mut app, "/model gpt-5.2").is_empty());
        app.pending = PendingQueue::new();
        assert_eq!(
            submit(&mut app, "/model gpt-5.2"),
            vec![Effect::SetModel("gpt-5.2".into())]
        );
    }

    #[test]
    fn quit_while_busy_needs_confirmation_and_hands_over_held_messages() {
        let mut app = running_app();
        let t0 = Instant::now();
        submit(&mut app, "two");
        app.update(ack(1, Err(SteerFailure::NotRunning)));
        // First Ctrl-D warns and arms.
        assert!(app.update_at(ctrl('d'), t0).is_empty());
        assert!(app.quit_confirm_armed(t0));
        assert!(matches!(
            last_notice(&app),
            Some((NoticeLevel::Warn, text))
                if text.contains("a turn is running") && text.contains("1 message queued")
        ));
        // The window expires on a tick; the next press only re-arms.
        app.update_at(Msg::Tick, t0 + QUIT_CONFIRM_WINDOW);
        assert!(app.quit_confirm.is_none());
        assert!(
            app.update_at(ctrl('d'), t0 + QUIT_CONFIRM_WINDOW)
                .is_empty()
        );
        assert!(app.quit_confirm_armed(t0 + QUIT_CONFIRM_WINDOW));
        // Ctrl-D then /quit (typing does not disarm) within the window quits,
        // handing over the text.
        let t1 = t0 + Duration::from_secs(10);
        assert!(app.update_at(ctrl('d'), t1).is_empty());
        for c in "/quit".chars() {
            app.update_at(key(KeyCode::Char(c)), t1);
        }
        assert_eq!(
            app.update_at(key(KeyCode::Enter), t1 + Duration::from_secs(1)),
            vec![
                Effect::SaveUnsent(vec![(Some(PendingId(1)), "two".into())]),
                Effect::Quit
            ]
        );
    }

    #[test]
    fn idle_quit_is_immediate() {
        let mut app = App::new(StatusInfo::default());
        assert_eq!(app.update(ctrl('d')), vec![Effect::Quit]);
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "/quit");
        assert_eq!(app.update(key(KeyCode::Enter)), vec![Effect::Quit]);
    }

    #[test]
    fn last_seq_resets_with_the_session() {
        let mut app = running_app();
        app.update(agent_at(
            AgentEvent::TurnStarted {
                turn_number: 0,
                assistant_message_id: None,
            },
            41,
        ));
        assert_eq!(app.last_seq, 41);
        app.update(Msg::Status(StatusUpdate::Session("next".into())));
        assert_eq!(app.last_seq, 0);
        app.update(agent_at(
            AgentEvent::TurnStarted {
                turn_number: 0,
                assistant_message_id: None,
            },
            7,
        ));
        app.update(Msg::ResetTranscript(Vec::new()));
        assert_eq!(app.last_seq, 0);
    }

    #[test]
    fn summary_text_is_not_repeated_after_streamed_text() {
        let mut app = running_app();
        app.update(agent(AgentEvent::TextDelta {
            delta: "streamed".into(),
            assistant_message_id: None,
        }));
        // A notice after the text must not make the summary look new.
        app.update(Msg::Notice {
            level: NoticeLevel::Info,
            text: "x".into(),
        });
        app.update(Msg::TurnFinished(TurnOutcome::plain(Ok(TurnSummary {
            text: "streamed".into(),
            ..summary()
        }))));
        let assistant = app
            .transcript
            .blocks()
            .iter()
            .filter(|b| matches!(b, Block::Assistant { .. }))
            .count();
        assert_eq!(assistant, 1);
    }

    #[test]
    fn resume_picker_selects() {
        let mut app = App::new(StatusInfo::default());
        app.update(Msg::ShowResumePicker(vec![
            ("aaa".into(), "first".into()),
            ("bbb".into(), "second".into()),
        ]));
        app.update(key(KeyCode::Down));
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::ResumeSession("bbb".into())]
        );
        assert!(app.modal.is_none());
    }

    fn picker_models() -> Vec<ModelGroup> {
        picker_models_warning(None)
    }

    fn picker_models_warning(warning: Option<&str>) -> Vec<ModelGroup> {
        use crate::models::{ModelEntry, ModelSource};
        let entry = |id: &str, source| ModelEntry {
            id: id.into(),
            display_name: None,
            source,
            context_window: None,
            max_output_tokens: None,
        };
        vec![ModelGroup {
            endpoint: "anthropic".into(),
            dialect: crate::endpoints::WireDialect::Anthropic,
            base_url: None,
            models: vec![
                entry("claude-sonnet-4-6", ModelSource::Current),
                entry("claude-opus-4-6", ModelSource::Provider),
                entry("claude-haiku-4-5", ModelSource::Provider),
            ],
            warning: warning.map(Into::into),
        }]
    }

    fn app_on_sonnet() -> App {
        let mut status = StatusInfo::default();
        status.set_model("anthropic", "claude-sonnet-4-6");
        App::new(status)
    }

    #[test]
    fn model_command_requests_picker_or_refresh() {
        let mut app = app_on_sonnet();
        type_str(&mut app, "/model");
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::RequestModelPicker { refresh: false }]
        );
        type_str(&mut app, "/model refresh");
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::RequestModelPicker { refresh: true }]
        );
        let mut running = running_app();
        type_str(&mut running, "/model");
        assert!(running.update(key(KeyCode::Enter)).is_empty());
    }

    #[test]
    fn model_picker_filters_and_selects() {
        let mut app = app_on_sonnet();
        app.update(Msg::ShowModelPicker(picker_models()));
        assert!(matches!(app.modal, Some(Modal::Model(_))));
        // `j`/`k`/`q` are filter characters here, not navigation.
        type_str(&mut app, "haiku");
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::SetModel("anthropic:claude-haiku-4-5".into())]
        );
        assert!(app.modal.is_none());
    }

    #[test]
    fn model_picker_navigation_backspace_and_esc() {
        let mut app = app_on_sonnet();
        app.update(Msg::ShowModelPicker(picker_models()));
        type_str(&mut app, "zz");
        // Nothing matches: Enter keeps the picker open.
        assert!(app.update(key(KeyCode::Enter)).is_empty());
        assert!(app.modal.is_some());
        app.update(key(KeyCode::Backspace));
        app.update(key(KeyCode::Backspace));
        app.update(key(KeyCode::Down));
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::SetModel("anthropic:claude-opus-4-6".into())]
        );
        // Selecting the current model changes nothing.
        app.update(Msg::ShowModelPicker(picker_models()));
        assert!(app.update(key(KeyCode::Enter)).is_empty());
        assert!(app.modal.is_none());
        app.update(Msg::ShowModelPicker(picker_models()));
        assert!(app.update(key(KeyCode::Esc)).is_empty());
        assert!(app.modal.is_none());
    }

    fn shell_prompt() -> EscalationPrompt {
        EscalationPrompt {
            agent: None,
            tool: "shell".into(),
            args_preview: "{}".into(),
            hint: None,
        }
    }

    /// A model picker with a filter typed and the selection moved.
    fn app_with_moved_model_picker() -> (App, Modal) {
        let mut app = app_on_sonnet();
        app.update(Msg::ShowModelPicker(picker_models()));
        type_str(&mut app, "claude");
        app.update(key(KeyCode::Down));
        let picker = app.modal.clone().expect("picker open");
        (app, picker)
    }

    #[test]
    fn escalation_parks_the_model_picker_and_restores_it() {
        let (mut app, picker) = app_with_moved_model_picker();
        app.update(Msg::Escalation(shell_prompt(), EscalationId(1)));
        assert!(matches!(app.modal, Some(Modal::Escalation { .. })));
        // A parked picker counts as open: a second list only warns.
        app.update(Msg::ShowModelPicker(picker_models()));
        assert_eq!(
            app.update(key(KeyCode::Char('y'))),
            vec![Effect::RespondEscalation(
                EscalationId(1),
                EscalationAnswer::AllowOnce
            )]
        );
        assert_eq!(app.modal.as_ref(), Some(&picker));
        if let Some(Modal::Model(p)) = &app.modal {
            assert_eq!(p.filter, "claude");
        }
        // Esc in an escalation denies it and still brings the picker back.
        app.update(Msg::Escalation(shell_prompt(), EscalationId(2)));
        assert_eq!(
            app.update(key(KeyCode::Esc)),
            vec![Effect::RespondEscalation(
                EscalationId(2),
                EscalationAnswer::Deny
            )]
        );
        assert_eq!(app.modal.as_ref(), Some(&picker));
    }

    #[test]
    fn parked_picker_returns_after_the_last_queued_escalation() {
        let (mut app, picker) = app_with_moved_model_picker();
        app.update(Msg::Escalation(shell_prompt(), EscalationId(1)));
        app.update(Msg::Escalation(shell_prompt(), EscalationId(2)));
        app.update(key(KeyCode::Char('y')));
        assert!(matches!(
            app.modal,
            Some(Modal::Escalation {
                id: EscalationId(2),
                ..
            })
        ));
        app.update(Msg::EscalationResolved(EscalationId(2)));
        assert_eq!(app.modal.as_ref(), Some(&picker));
    }

    #[test]
    fn stopping_all_escalations_restores_the_parked_picker() {
        let (mut app, picker) = app_with_moved_model_picker();
        app.update(Msg::Escalation(shell_prompt(), EscalationId(1)));
        app.update(Msg::Escalation(shell_prompt(), EscalationId(2)));
        assert_eq!(
            app.update(ctrl('c')),
            vec![
                Effect::RespondEscalation(EscalationId(1), EscalationAnswer::Stop),
                Effect::RespondEscalation(EscalationId(2), EscalationAnswer::Stop),
            ]
        );
        assert_eq!(app.modal.as_ref(), Some(&picker));
        assert_eq!(app.pending_escalations(), 0);
    }

    #[test]
    fn escalation_parks_the_resume_picker_and_restores_it() {
        let mut app = App::new(StatusInfo::default());
        app.update(Msg::ShowResumePicker(vec![
            ("aaa".into(), "first".into()),
            ("bbb".into(), "second".into()),
        ]));
        app.update(key(KeyCode::Down));
        app.update(Msg::Escalation(shell_prompt(), EscalationId(1)));
        app.update(key(KeyCode::Char('n')));
        assert!(matches!(
            app.modal,
            Some(Modal::Resume(ResumePicker { selected: 1, .. }))
        ));
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::ResumeSession("bbb".into())]
        );
        // A resume list arriving during an escalation waits for it.
        app.update(Msg::Escalation(shell_prompt(), EscalationId(2)));
        app.update(Msg::ShowResumePicker(vec![("ccc".into(), "third".into())]));
        assert!(matches!(app.modal, Some(Modal::Escalation { .. })));
        app.update(Msg::EscalationResolved(EscalationId(2)));
        assert!(matches!(app.modal, Some(Modal::Resume(_))));
    }

    #[test]
    fn repeated_enter_and_esc_do_not_act_in_modals() {
        let repeat = |code: KeyCode| {
            let mut k = KeyEvent::new(code, KeyModifiers::NONE);
            k.kind = KeyEventKind::Repeat;
            Msg::Key(k)
        };
        let mut app = app_on_sonnet();
        app.update(Msg::ShowModelPicker(picker_models()));
        assert!(app.update(repeat(KeyCode::Enter)).is_empty());
        assert!(app.update(repeat(KeyCode::Esc)).is_empty());
        assert!(matches!(app.modal, Some(Modal::Model(_))));
        // Held arrows still move.
        app.update(repeat(KeyCode::Down));
        assert_eq!(
            app.update(key(KeyCode::Enter)),
            vec![Effect::SetModel("anthropic:claude-opus-4-6".into())]
        );
        assert!(app.modal.is_none());
        // A held answer key does not answer an escalation.
        app.update(Msg::Escalation(shell_prompt(), EscalationId(1)));
        assert!(app.update(repeat(KeyCode::Char('y'))).is_empty());
        assert!(app.update(repeat(KeyCode::Esc)).is_empty());
        assert!(matches!(app.modal, Some(Modal::Escalation { .. })));
    }

    #[test]
    fn model_picker_warning_becomes_notice_when_modal_busy() {
        let mut app = app_on_sonnet();
        app.update(Msg::ShowResumePicker(vec![("a".into(), "x".into())]));
        app.update(Msg::ShowModelPicker(picker_models_warning(Some(
            "could not list models",
        ))));
        assert!(matches!(app.modal, Some(Modal::Resume(_))));
        assert!(app.transcript.blocks().iter().any(|b| matches!(
            b,
            Block::Notice { text, .. } if text.contains("could not list models")
        )));
    }

    #[test]
    fn status_updates_and_dirty_tracking() {
        let mut app = App::new(StatusInfo::default());
        assert!(app.take_dirty());
        app.update(Msg::Tick);
        assert!(!app.take_dirty());
        app.update(Msg::Status(StatusUpdate::Auto {
            state: "PAUSED".into(),
            denials: (3, 3, 5, 20),
        }));
        assert!(app.take_dirty());
        assert_eq!(app.status.auto_state, "PAUSED");
        app.update(Msg::Status(StatusUpdate::Session(
            "0123456789abcdef".into(),
        )));
        assert_eq!(app.status.session_short, "01234567");
    }

    #[test]
    fn history_recall_with_up() {
        let mut app = App::new(StatusInfo::default()).with_history(vec!["old".into()]);
        app.update(key(KeyCode::Up));
        assert_eq!(app.input.text(), "old");
    }

    #[test]
    fn take_submitted_drains_prompts_and_commands() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "hello");
        app.update(key(KeyCode::Enter));
        app.update(key(KeyCode::Enter)); // blank: ignored
        type_str(&mut app, "/help");
        app.update(key(KeyCode::Enter));
        assert_eq!(
            app.take_submitted(),
            vec!["hello".to_string(), "/help".to_string()]
        );
        assert!(app.take_submitted().is_empty());
    }

    #[test]
    fn ctrl_r_enters_search_and_enter_accepts() {
        let mut app = App::new(StatusInfo::default())
            .with_history(vec!["cargo build".into(), "git status".into()])
            .with_global_history(vec!["cargo test --global".into(), "git status".into()]);
        type_str(&mut app, "draft");
        app.update(ctrl('r'));
        assert!(matches!(app.mode, InputMode::Search(_)));
        let (_, corpus) = app.search().unwrap();
        assert_eq!(corpus, ["git status", "cargo build", "cargo test --global"]);
        type_str(&mut app, "cargo");
        let (s, corpus) = app.search().unwrap();
        assert_eq!(s.current(corpus), Some("cargo build"));
        app.update(ctrl('r'));
        let (s, corpus) = app.search().unwrap();
        assert_eq!(s.current(corpus), Some("cargo test --global"));
        // Enter accepts into the editor without submitting.
        assert!(app.update(key(KeyCode::Enter)).is_empty());
        assert_eq!(app.mode, InputMode::Normal);
        assert_eq!(app.input.text(), "cargo test --global");
        assert_eq!(app.input.cursor(), (0, 19));
        assert!(app.take_submitted().is_empty());
        // Searching while a turn runs is fine.
        app.update(key(KeyCode::Enter));
        assert!(app.is_running());
        app.update(ctrl('r'));
        type_str(&mut app, "git");
        app.update(key(KeyCode::End));
        assert_eq!(app.input.text(), "git status");
    }

    #[test]
    fn esc_cancels_search_restores_input() {
        let mut app = App::new(StatusInfo::default()).with_history(vec!["hello".into()]);
        type_str(&mut app, "draft");
        app.update(ctrl('r'));
        type_str(&mut app, "hel");
        assert_eq!(app.input.text(), "draft");
        assert!(app.update(key(KeyCode::Esc)).is_empty());
        assert_eq!(app.mode, InputMode::Normal);
        assert_eq!(app.input.text(), "draft");
        // Ctrl-C cancels too and does not arm quit / clear the input.
        app.update(ctrl('r'));
        assert!(app.update(ctrl('c')).is_empty());
        assert_eq!(app.input.text(), "draft");
        assert!(!app.quit_armed(Instant::now()));
    }

    #[test]
    fn chord_ctrl_x_ctrl_s_interrupt_and_send_when_running() {
        let mut app = App::new(StatusInfo::default());
        let t0 = Instant::now();
        // Idle: behaves like Enter.
        type_str(&mut app, "first");
        assert!(app.update_at(ctrl('x'), t0).is_empty());
        assert!(app.chord_pending(t0));
        assert_eq!(
            app.update_at(ctrl('s'), t0),
            vec![Effect::StartTurn("first".into())]
        );
        assert!(app.chord.is_none());
        // Running with input: interrupt and send.
        type_str(&mut app, "next");
        app.update_at(ctrl('x'), t0);
        assert_eq!(
            app.update_at(ctrl('s'), t0),
            vec![Effect::InterruptAndSend("next".into())]
        );
        assert!(app.input.is_empty());
        assert_eq!(
            app.take_submitted(),
            vec!["first".to_string(), "next".into()]
        );
        assert_eq!(app.input.history().last().map(String::as_str), Some("next"));
        // Running with an empty editor: plain interrupt.
        app.update_at(ctrl('x'), t0);
        assert_eq!(app.update_at(ctrl('s'), t0), vec![Effect::Interrupt]);
        // Esc cancels the chord without interrupting.
        app.update_at(ctrl('x'), t0);
        assert!(app.update_at(key(KeyCode::Esc), t0).is_empty());
        assert!(app.chord.is_none());
        // Any other key cancels the chord and is processed normally.
        app.update_at(ctrl('x'), t0);
        app.update_at(key(KeyCode::Char('a')), t0);
        assert!(app.chord.is_none());
        assert_eq!(app.input.text(), "a");
        assert!(app.update_at(ctrl('s'), t0).is_empty());
    }

    #[test]
    fn chord_expires_after_window() {
        let mut app = App::new(StatusInfo::default());
        let t0 = Instant::now();
        type_str(&mut app, "x");
        app.update_at(ctrl('x'), t0);
        let late = t0 + CHORD_WINDOW + Duration::from_millis(1);
        assert!(!app.chord_pending(late));
        assert!(app.update_at(ctrl('s'), late).is_empty());
        assert_eq!(app.input.text(), "x");
        assert!(!app.is_running());
        // A tick after the window clears the pending chord.
        app.update_at(ctrl('x'), t0);
        app.update_at(Msg::Tick, late);
        assert!(app.chord.is_none());
    }

    #[test]
    fn ctrl_s_alone_is_noop() {
        let mut app = App::new(StatusInfo::default());
        type_str(&mut app, "text");
        assert!(app.update(ctrl('s')).is_empty());
        assert_eq!(app.input.text(), "text");
        assert!(!app.is_running());
    }

    fn last_notice(app: &App) -> Option<(NoticeLevel, String)> {
        app.transcript.blocks().iter().rev().find_map(|b| match b {
            Block::Notice { level, text } => Some((*level, text.clone())),
            _ => None,
        })
    }

    #[test]
    fn hash_input_remembers_and_escapes_do_not() {
        let mut app = App::new(StatusInfo::default());
        assert_eq!(
            submit(&mut app, "#  prefer tabs "),
            vec![Effect::Remember("prefer tabs".into())]
        );
        assert!(!app.is_running());
        assert_eq!(
            last_notice(&app),
            Some((NoticeLevel::Info, "remembering: prefer tabs".into()))
        );
        assert_eq!(
            submit(&mut app, "/remember use nextest"),
            vec![Effect::Remember("use nextest".into())]
        );
        assert_eq!(
            submit(&mut app, "## heading"),
            vec![Effect::StartTurn("## heading".into())]
        );
        app.update(finished(Ok(summary()), &[]));
        assert_eq!(
            submit(&mut app, "\\#tag"),
            vec![Effect::StartTurn("#tag".into())]
        );
        app.update(finished(Ok(summary()), &[]));
        type_str(&mut app, "#a");
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)));
        assert_eq!(
            submit(&mut app, "b"),
            vec![Effect::StartTurn("#a\nb".into())]
        );
    }

    #[test]
    fn remember_is_allowed_while_running() {
        let mut app = running_app();
        assert_eq!(
            submit(&mut app, "#note"),
            vec![Effect::Remember("note".into())]
        );
        assert!(app.pending.is_empty());
        assert!(app.is_running());
    }

    #[test]
    fn context_gauge_tracks_usage_compaction_and_clear() {
        let mut app = App::new(StatusInfo::default());
        app.update(Msg::Status(StatusUpdate::ContextWindow(Some(200_000))));
        app.update(agent(AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: test_usage(Usage {
                input_tokens: 1_000,
                output_tokens: 200,
                cache_creation_tokens: Some(30),
                cache_read_tokens: Some(80_000),
                ..Usage::default()
            }),
            assistant_message_id: None,
        }));
        assert_eq!(app.status.context_used, Some(81_230));
        assert!(!app.status.context_approx);
        app.update(agent(AgentEvent::CompactionCompleted {
            summary_tokens: 2_500,
            messages_before: 40,
            messages_after: 3,
            summary_source: None,
            estimated_history_tokens_after: None,
        }));
        assert_eq!(app.status.context_used, Some(2_500));
        assert!(app.status.context_approx);
        app.update(agent(AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: test_usage(Usage {
                input_tokens: 3_000,
                output_tokens: 100,
                ..Usage::default()
            }),
            assistant_message_id: None,
        }));
        assert_eq!(app.status.context_used, Some(3_100));
        assert!(!app.status.context_approx);
        assert_eq!(submit(&mut app, "/clear"), vec![Effect::NewSession]);
        // What the integrator sends once the new session is live.
        app.update(Msg::Status(StatusUpdate::Context {
            used: None,
            approx: false,
        }));
        app.update(Msg::Status(StatusUpdate::ContextWindow(Some(200_000))));
        assert_eq!(app.status.context_used, None);
        assert_eq!(app.status.context_window, Some(200_000));
        app.update(Msg::Status(StatusUpdate::ApproxContextWindow(128_000)));
        assert!(app.status.window_approx);
        assert_eq!(app.status.context_window, Some(128_000));
        app.update(Msg::Status(StatusUpdate::ContextWindow(Some(200_000))));
        assert!(!app.status.window_approx);
    }

    fn gauge_app(mode: CompactMode) -> App {
        let mut app = App::new(StatusInfo::default()).with_compaction_policy(CompactionPolicy {
            mode,
            ..CompactionPolicy::default()
        });
        app.update(Msg::Status(StatusUpdate::ContextWindow(Some(100_000))));
        app
    }

    /// Run one turn that ends with `used` context tokens.
    fn turn_at(app: &mut App, used: u64) -> Vec<Effect> {
        assert_eq!(submit(app, "go"), vec![Effect::StartTurn("go".into())]);
        app.update(agent(AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: test_usage(Usage {
                input_tokens: used,
                ..Usage::default()
            }),
            assistant_message_id: None,
        }));
        app.update(finished(Ok(summary()), &[]))
    }

    fn warn_count(app: &App) -> usize {
        app.transcript
            .blocks()
            .iter()
            .filter(|b| {
                matches!(
                    b,
                    Block::Notice {
                        level: NoticeLevel::Warn,
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn warn_mode_warns_once_on_crossing_then_every_turn_at_auto() {
        let mut app = gauge_app(CompactMode::Warn);
        assert!(turn_at(&mut app, 50_000).is_empty());
        assert_eq!(warn_count(&app), 0);
        assert!(turn_at(&mut app, 76_000).is_empty());
        assert_eq!(
            last_notice(&app),
            Some((
                NoticeLevel::Warn,
                "context 76% full (76k/100k tokens); run /compact now to free space".into()
            ))
        );
        assert!(turn_at(&mut app, 80_000).is_empty());
        assert_eq!(warn_count(&app), 1, "no repeat below the auto threshold");
        assert!(
            turn_at(&mut app, 86_000).is_empty(),
            "warn mode never compacts"
        );
        assert!(turn_at(&mut app, 87_000).is_empty());
        assert_eq!(
            warn_count(&app),
            3,
            "repeats every turn at the auto threshold"
        );
        assert!(last_notice(&app).is_some_and(|(_, t)| t.contains("--auto-compact")));
        assert!(!app.compacting);
    }

    #[test]
    fn compact_now_refused_while_running_or_pending() {
        let mut app = running_app();
        app.status.context_used = Some(10_000);
        assert!(submit(&mut app, "/compact now").is_empty());
        assert_eq!(
            last_notice(&app),
            Some((
                NoticeLevel::Warn,
                "finish or interrupt the current turn first".into()
            ))
        );
        submit(&mut app, "two");
        app.update(finished(Ok(summary()), &[]));
        assert!(!app.is_running());
        assert_eq!(app.status.queued, 1);
        assert!(submit(&mut app, "/compact now").is_empty());
        assert!(!app.compacting);
        assert_eq!(last_notice(&app).map(|(l, _)| l), Some(NoticeLevel::Warn));
    }

    #[test]
    fn compact_reports_the_policy_and_now_needs_context() {
        let mut app = gauge_app(CompactMode::Auto);
        assert!(submit(&mut app, "/compact").is_empty());
        assert!(
            last_notice(&app).is_some_and(|(level, text)| level == NoticeLevel::Info
                && text.contains("auto-compaction is on")
                && text.contains("85k tokens"))
        );
        let mut app = gauge_app(CompactMode::Warn);
        assert!(submit(&mut app, "/compact").is_empty());
        assert!(last_notice(&app).is_some_and(|(_, text)| text.contains("auto-compaction is off")));
        // Nothing reported yet: nothing to compact.
        assert!(submit(&mut app, "/compact now").is_empty());
        assert_eq!(
            last_notice(&app),
            Some((NoticeLevel::Info, "nothing to compact yet".into()))
        );
        assert!(!app.compacting);
    }

    #[test]
    fn prompts_during_the_rebuild_are_queued_inputs_not_held_text() {
        let mut app = App::new(StatusInfo::default());
        app.status.context_used = Some(40_000);
        assert_eq!(submit(&mut app, "/compact now"), vec![Effect::CompactNow]);
        assert!(app.compacting);
        assert_eq!(
            last_notice(&app),
            Some((
                NoticeLevel::Info,
                "rebuilding the session so the next model call compacts…".into()
            ))
        );
        // The prompt goes to the runner at once: it admits it as a `Queue`
        // input after the rebuild (effects are served in order).
        assert_eq!(
            submit(&mut app, "one"),
            vec![Effect::StartTurn("one".into())]
        );
        assert!(app.is_running());
        assert_eq!(app.status.queued, 0);
        // Lifecycle commands wait for the rebuild; /remember does not.
        for cmd in ["/clear", "/resume last", "/compact now"] {
            assert!(submit(&mut app, cmd).is_empty(), "{cmd}");
            assert_eq!(
                last_notice(&app).map(|(l, _)| l),
                Some(NoticeLevel::Warn),
                "{cmd}"
            );
        }
        assert_eq!(submit(&mut app, "#x"), vec![Effect::Remember("x".into())]);
        assert!(app.update(Msg::CompactRebuilt(Ok(()))).is_empty());
        assert!(!app.compacting);
        assert!(app.transcript.blocks().iter().any(
            |b| matches!(b, Block::Notice { level: NoticeLevel::Info, text } if text == COMPACT_REBUILT)
        ));
        // The wording never claims the compaction itself happened.
        assert!(!COMPACT_REBUILT.contains("armed"));
    }

    #[test]
    fn a_failed_rebuild_is_reported() {
        let mut app = App::new(StatusInfo::default());
        app.status.context_used = Some(1);
        submit(&mut app, "/compact now");
        let fx = app.update(Msg::CompactRebuilt(Err(CompactFailure::Rebuild(
            "boom".into(),
        ))));
        assert!(fx.is_empty());
        assert!(!app.compacting);
        assert!(!app.is_running());
        assert_eq!(
            last_notice(&app),
            Some((
                NoticeLevel::Error,
                "/compact now failed: could not rebuild the session: boom".into()
            ))
        );
    }

    #[test]
    fn an_unmeasured_helper_marks_the_cost_a_lower_bound() {
        let mut app = App::new(StatusInfo::default());
        let fx = app.update(Msg::HelperUnmeasured("scout".into()));
        assert!(matches!(fx.as_slice(), [Effect::CostSnapshot(_)]), "{fx:?}");
        assert_eq!(
            app.status.cost.confidence,
            cost::CostConfidence::Unmeasured
        );
    }

    #[test]
    fn auto_mode_neither_warns_nor_compacts_itself() {
        let mut app = gauge_app(CompactMode::Auto);
        for used in [80_000, 90_000, 99_000] {
            assert!(turn_at(&mut app, used).is_empty());
        }
        assert_eq!(warn_count(&app), 0);
        assert!(!app.compacting);
        // Unknown window: no warning in auto mode either.
        let mut app = App::new(StatusInfo::default()).with_compaction_policy(CompactionPolicy {
            mode: CompactMode::Auto,
            ..CompactionPolicy::default()
        });
        assert!(turn_at(&mut app, 150_000).is_empty());
        assert_eq!(warn_count(&app), 0);
    }

    #[test]
    fn a_native_compaction_rearms_the_warning() {
        let mut app = gauge_app(CompactMode::Warn);
        assert!(turn_at(&mut app, 76_000).is_empty());
        assert_eq!(warn_count(&app), 1);
        app.update(agent(AgentEvent::CompactionCompleted {
            summary_tokens: 500,
            messages_before: 40,
            messages_after: 3,
            summary_source: None,
            estimated_history_tokens_after: Some(4_000),
        }));
        assert_eq!(app.status.context_used, Some(4_000));
        assert!(app.status.context_approx);
        assert!(turn_at(&mut app, 77_000).is_empty());
        assert_eq!(warn_count(&app), 2);
    }

    #[test]
    fn auto_compact_flag_overrides_settings() {
        let settings = crate::approval::rules::AutoSettings::default();
        let args =
            <crate::cli::Args as clap::Parser>::try_parse_from(["rkat-tui", "--auto-compact"])
                .unwrap();
        let overlay = crate::overlay::ConfigOverlay::from_args(&args)
            .unwrap()
            .rules_file()
            .unwrap()
            .unwrap();
        let mode = overlay.auto.and_then(|auto| auto.compact);
        let app = App::new(StatusInfo::default())
            .with_compaction_policy(settings.compaction_policy(mode));
        assert_eq!(app.compaction_policy().mode, CompactMode::Auto);
        assert_eq!(
            App::new(StatusInfo::default())
                .with_compaction_policy(settings.compaction_policy(None))
                .compaction_policy()
                .mode,
            CompactMode::Native
        );
    }

    fn priced(entries: &[(&str, f64, f64)]) -> Arc<crate::pricing::PriceTable> {
        use crate::approval::rules::PricingSection;
        Arc::new(crate::pricing::PriceTable::new(
            entries
                .iter()
                .map(|(id, input, output)| {
                    (
                        (*id).to_string(),
                        PricingSection {
                            input: Some(*input),
                            output: Some(*output),
                            ..PricingSection::default()
                        },
                    )
                })
                .collect(),
        ))
    }

    /// A turn whose usage the provider accounted to `model`.
    fn completed_by(model: &str, input: u64, output: u64) -> Msg {
        agent(AgentEvent::TurnCompleted {
            stop_reason: StopReason::EndTurn,
            usage: Some(meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                model,
                Usage {
                    input_tokens: input,
                    output_tokens: output,
                    ..Usage::default()
                },
            )),
            assistant_message_id: None,
        })
    }

    fn footer(app: &App) -> String {
        render::status_line(&app.status, Instant::now())
    }

    #[test]
    fn turn_completed_attributes_cost_to_the_accounted_model_and_snapshots() {
        let mut app = App::new(StatusInfo::default());
        app.update(Msg::Status(StatusUpdate::Model {
            provider: "anthropic".into(),
            model: "a".into(),
        }));
        let fx = app.update(completed_by("a", 1_000_000, 0));
        let mut expected = SessionCost::default();
        expected.add(
            "a",
            &UsageTotals {
                input: 1_000_000,
                ..UsageTotals::default()
            },
        );
        assert_eq!(fx, vec![Effect::CostSnapshot(expected.clone())]);
        // A model switch: later usage goes to the new model.
        app.update(Msg::Status(StatusUpdate::Model {
            provider: "openai".into(),
            model: "b".into(),
        }));
        let fx = app.update(completed_by("b", 0, 1_000_000));
        expected.add(
            "b",
            &UsageTotals {
                output: 1_000_000,
                ..UsageTotals::default()
            },
        );
        assert_eq!(fx, vec![Effect::CostSnapshot(expected)]);
        // Compaction summaries count as output of the current model.
        let fx = app.update(agent(AgentEvent::CompactionCompleted {
            summary_tokens: 500,
            messages_before: 10,
            messages_after: 3,
            summary_source: None,
            estimated_history_tokens_after: None,
        }));
        assert_eq!(app.status.cost.per_model["b"].output, 1_000_500);
        assert!(matches!(fx.as_slice(), [Effect::CostSnapshot(_)]));
        // A compaction's input is an estimate: the readout is approximate.
        assert_eq!(app.status.cost.confidence, CostConfidence::Approximate);
        // Nothing priced yet.
        assert!(footer(&app).contains(" │ $? │ "), "{}", footer(&app));
        // Prices arrive: the footer recomputes.
        app.update(Msg::Status(StatusUpdate::Prices(priced(&[
            ("a", 1.0, 1.0),
            ("b", 2.0, 2.0),
        ]))));
        assert!(footer(&app).contains(" │ ~$3.00 │ "), "{}", footer(&app));
        app.update(Msg::Status(StatusUpdate::Prices(priced(&[(
            "a", 0.42, 1.0,
        )]))));
        assert!(footer(&app).contains(" │ ~$0.42 │ "), "{}", footer(&app));
        // Usage the provider accounted to another model (a dated snapshot
        // id, a fallback) counts for that model, whatever the status says.
        app.update(completed_by("c-2026-01-01", 2_000_000, 1_000_000));
        assert_eq!(app.status.cost.per_model["c-2026-01-01"].input, 2_000_000);
        assert_eq!(app.status.cost.per_model["b"].input, 0);
    }

    #[test]
    fn session_cost_arithmetic_from_turns_and_the_classifier() {
        let mut app = App::new(StatusInfo::default());
        app.status.set_model("anthropic", "main");
        app.update(Msg::Status(StatusUpdate::Prices(priced(&[
            ("main", 3.0, 15.0),
            ("judge", 1.0, 5.0),
        ]))));
        // 2 turns on `main`: 1.5M in / 0.2M out → 1.5×3 + 0.2×15 = $7.50.
        app.update(completed_by("main", 1_000_000, 100_000));
        app.update(completed_by("main", 500_000, 100_000));
        // The classifier bucket: 1M in / 0.1M out → 1×1 + 0.1×5 = $1.50.
        app.update(Msg::ClassifierUsage {
            model: "judge".into(),
            usage: Usage {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                ..Usage::default()
            },
        });
        let (usd, all) = app.status.cost.cost_usd(&app.status.prices);
        assert!(all);
        assert!((usd - 9.0).abs() < 1e-9, "{usd}");
        assert!(footer(&app).contains(" │ $9.00 │ "), "{}", footer(&app));
        let breakdown = app.status.cost.breakdown(&app.status.prices);
        assert!(
            breakdown.contains("main: in 1500000 · out 200000"),
            "{breakdown}"
        );
        assert!(
            breakdown.contains("judge: in 1000000 · out 100000"),
            "{breakdown}"
        );
        assert!(breakdown.contains("total: $9.00"), "{breakdown}");
    }

    #[test]
    fn cost_command_prints_the_breakdown() {
        let mut app = App::new(StatusInfo::default());
        assert!(submit(&mut app, "/cost").is_empty());
        let (_, text) = last_notice(&app).unwrap();
        assert!(text.contains("no tokens used"), "{text}");
        app.status.set_model("anthropic", "a");
        app.update(Msg::Status(StatusUpdate::Prices(priced(&[(
            "a", 3.0, 15.0,
        )]))));
        app.update(completed_by("a", 2_000_000, 100_000));
        assert!(submit(&mut app, "/cost").is_empty());
        let (level, text) = last_notice(&app).unwrap();
        assert_eq!(level, NoticeLevel::Info);
        assert!(text.contains("a: in 2000000 · out 100000"), "{text}");
        assert!(text.contains("$7.50 (price: tui.toml)"), "{text}");
        assert!(text.contains("total: $7.50"), "{text}");
    }

    #[test]
    fn resume_replaces_the_cost_totals() {
        let mut app = App::new(StatusInfo::default());
        app.update(Msg::Status(StatusUpdate::Prices(priced(&[(
            "a", 1.0, 1.0,
        )]))));
        app.status.set_model("anthropic", "a");
        app.update(completed_by("a", 5_000_000, 0));
        assert!(footer(&app).contains("$5.00"), "{}", footer(&app));
        // Resuming a session with recorded totals.
        let mut recorded = SessionCost::default();
        recorded.add(
            "a",
            &UsageTotals {
                output: 420_000,
                ..UsageTotals::default()
            },
        );
        app.update(Msg::Status(StatusUpdate::CostTotals(recorded)));
        assert!(footer(&app).contains(" │ $0.42"), "{}", footer(&app));
        // The next call adds to the resumed total.
        app.update(completed_by("a", 580_000, 0));
        assert!(footer(&app).contains(" │ $1.00"), "{}", footer(&app));
        // Totals derived from the session's usage are marked approximate.
        let derived = SessionCost::derived(
            "a",
            &Usage {
                input_tokens: 1_000_000,
                ..Usage::default()
            },
        );
        app.update(Msg::Status(StatusUpdate::CostTotals(derived)));
        assert!(footer(&app).contains(" │ ~$1.00"), "{}", footer(&app));
        // A new session starts from zero: no segment.
        app.update(Msg::Status(
            StatusUpdate::CostTotals(SessionCost::default()),
        ));
        assert!(!footer(&app).contains('$'), "{}", footer(&app));
    }

    fn helper_msg(label: &str, event: AgentEvent) -> Msg {
        Msg::Scoped(
            crate::mobs::HelperTag {
                owner: meerkat_core::SessionId::new(),
                path: vec![label.to_string()],
                model: "m".into(),
            }
            .scope(event),
        )
    }

    /// An app with the helpers `scout` then `fork-1`, one call each.
    fn helper_app() -> (App, BlockId, BlockId) {
        let mut app = App::new(StatusInfo::default());
        app.transcript.push(user("go"));
        let mut live = Vec::new();
        for label in ["scout", "fork-1"] {
            live.push(crate::mobs::HelperStatus {
                label: label.into(),
                model: "m".into(),
                depth: 1,
            });
            app.update(Msg::Status(StatusUpdate::Helpers(live.clone())));
            app.update(helper_msg(
                label,
                AgentEvent::ToolCallRequested {
                    id: "a".into(),
                    name: "glob".into(),
                    args: test_args(json!({"pattern": "*"})),
                },
            ));
        }
        let scout = app.transcript.helper_header("scout").unwrap();
        let fork = app.transcript.helper_header("fork-1").unwrap();
        (app, scout, fork)
    }

    fn ctrl_key(code: KeyCode) -> Msg {
        Msg::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
    }

    fn expanded(app: &App, id: BlockId) -> bool {
        match app.transcript.block(id) {
            Some(Block::Helper(fold)) => fold.expanded,
            other => panic!("not a helper header: {other:?}"),
        }
    }

    #[test]
    fn ctrl_down_focuses_the_newest_helper_and_ctrl_up_down_wrap() {
        let (mut app, scout, fork) = helper_app();
        assert!(app.status.helper_headers);
        assert_eq!(app.helper_focus(), None);
        app.update(ctrl_key(KeyCode::Down));
        assert_eq!(app.helper_focus(), Some(fork));
        // Down past the newest wraps to the oldest, up past the oldest to
        // the newest.
        app.update(ctrl_key(KeyCode::Down));
        assert_eq!(app.helper_focus(), Some(scout));
        app.update(ctrl_key(KeyCode::Up));
        assert_eq!(app.helper_focus(), Some(fork));
        app.update(ctrl_key(KeyCode::Up));
        assert_eq!(app.helper_focus(), Some(scout));
        // The first Ctrl+Up also starts at the newest.
        let (mut app, _, fork) = helper_app();
        app.update(ctrl_key(KeyCode::Up));
        assert_eq!(app.helper_focus(), Some(fork));
        // No helpers: nothing to focus.
        let mut app = App::new(StatusInfo::default());
        app.update(ctrl_key(KeyCode::Down));
        assert_eq!(app.helper_focus(), None);
        assert!(!app.status.helper_headers);
    }

    #[test]
    fn enter_toggles_the_focused_helper_without_submitting() {
        let (mut app, _, fork) = helper_app();
        type_str(&mut app, "draft");
        app.update(ctrl_key(KeyCode::Down));
        let fx = app.update(key(KeyCode::Enter));
        assert!(fx.is_empty(), "{fx:?}");
        assert!(!app.is_running());
        assert_eq!(app.input.text(), "draft");
        assert!(expanded(&app, fork));
        // Still focused: Enter folds it again.
        app.update(key(KeyCode::Enter));
        assert!(!expanded(&app, fork));
        assert_eq!(app.helper_focus(), Some(fork));
    }

    #[test]
    fn esc_and_editing_drop_the_helper_focus() {
        let (mut app, _, fork) = helper_app();
        app.update(ctrl_key(KeyCode::Down));
        assert_eq!(app.helper_focus(), Some(fork));
        // Esc drops the focus and does nothing else.
        let fx = app.update(key(KeyCode::Esc));
        assert!(fx.is_empty());
        assert_eq!(app.helper_focus(), None);
        // Typing drops it too (and types); Enter then submits.
        app.update(ctrl_key(KeyCode::Down));
        type_str(&mut app, "x");
        assert_eq!(app.helper_focus(), None);
        assert_eq!(app.input.text(), "x");
        let fx = app.update(key(KeyCode::Enter));
        assert_eq!(fx, vec![Effect::StartTurn("x".into())]);
        assert!(!expanded(&app, fork));
    }

    #[test]
    fn a_click_on_a_helper_header_toggles_it() {
        let (mut app, scout, _) = helper_app();
        draw_rows(&mut app, 0, 80, 20);
        // Rows: user (2 lines), then scout's header.
        assert_eq!(app.block_at_row(2), Some(scout));
        click_at(&mut app, 2);
        assert!(expanded(&app, scout));
        // Later than a double click.
        draw_rows(&mut app, 0, 80, 20);
        let later = Instant::now() + Duration::from_secs(2);
        app.update_at(down(2, 3), later);
        app.update_at(up(2, 3), later);
        assert!(!expanded(&app, scout));
    }

    #[test]
    fn helpers_command_lists_expands_and_folds() {
        let (mut app, scout, fork) = helper_app();
        type_str(&mut app, "/helpers");
        app.update(key(KeyCode::Enter));
        let listing = match app.transcript.blocks().last() {
            Some(Block::Notice { text, .. }) => text.clone(),
            other => panic!("no listing: {other:?}"),
        };
        assert!(listing.contains("▸ [scout · m] running · 1 tool call"), "{listing}");
        assert!(listing.contains("[fork-1 · m]"), "{listing}");
        type_str(&mut app, "/helpers expand scout");
        app.update(key(KeyCode::Enter));
        assert!(expanded(&app, scout));
        assert!(!expanded(&app, fork));
        type_str(&mut app, "/helpers expand all");
        app.update(key(KeyCode::Enter));
        assert!(expanded(&app, scout) && expanded(&app, fork));
        type_str(&mut app, "/helpers fold all");
        app.update(key(KeyCode::Enter));
        assert!(!expanded(&app, scout) && !expanded(&app, fork));
        type_str(&mut app, "/helpers fold nobody");
        app.update(key(KeyCode::Enter));
        assert!(matches!(
            app.transcript.blocks().last(),
            Some(Block::Notice { level: NoticeLevel::Warn, text }) if text.contains("nobody")
        ));
    }

    #[test]
    fn a_reset_drops_the_helper_focus() {
        let (mut app, _, _) = helper_app();
        app.update(ctrl_key(KeyCode::Down));
        app.update(Msg::ResetTranscript(Vec::new()));
        assert_eq!(app.helper_focus(), None);
        assert!(!app.status.helper_headers);
    }
}

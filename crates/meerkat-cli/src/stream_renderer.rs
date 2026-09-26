//! Rich streaming output renderer for the CLI.
//!
//! Renders scoped events to stderr (chrome: thinking, tool calls, status)
//! and stdout (text content) with ANSI styling.

use meerkat_core::{AgentEvent, CumulativeUsage, ScopedAgentEvent, TurnUsage, Usage};
use std::collections::{BTreeSet, HashMap};
use std::io::{self, IsTerminal, Write};

/// Maximum lines of tool result output to display in the default renderer.
const MAX_TOOL_RESULT_LINES: usize = 8;

/// Maximum width for each default tool result line preview (bytes).
const MAX_TOOL_RESULT_LINE_BYTES: usize = 240;

/// Maximum width for tool args preview (bytes).
const MAX_TOOL_ARGS_PREVIEW: usize = 200;

// ── ANSI escape codes ───────────────────────────────────────────────────

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const MAGENTA: &str = "\x1b[35m";

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRenderPolicy {
    PrimaryOnly,
    MuxAll,
    Focus(String),
}

/// Where a run's token total comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTotalSource {
    /// The host prints the primary run's total from its run result, which
    /// charges every call the run made (a one-shot `rkat run`). The renderer
    /// prints totals only for other scopes.
    HostRunResult,
    /// Every run's total is folded from the per-call rows on the stream. For
    /// hosts whose later runs return no result (keep-alive).
    StreamRows,
}

impl RunTotalSource {
    fn renderer_prints_total(self, scope_id: &str) -> bool {
        match self {
            Self::StreamRows => true,
            Self::HostRunResult => scope_id != "primary",
        }
    }
}

#[derive(Debug, Default)]
struct ScopeRenderState {
    in_thinking: bool,
    in_text: bool,
    reasoning_bytes: usize,
    tokens: RunTokenLedger,
}

/// Per-call token rows of the run in progress on one scope.
///
/// Every committed agent-loop call publishes `turn_completed` and every
/// extraction request a `request_usage` row on the extraction outcome, so the
/// rows are one line per provider request, and folding them the way the
/// session does gives the run's own total.
#[derive(Debug, Default)]
struct RunTokenLedger {
    run_usage: CumulativeUsage,
}

/// A token-accounting line, in print order.
#[derive(Debug, PartialEq, Eq)]
enum TokenLine {
    /// One provider request.
    Request(String),
    /// A call the provider sent no accounting for.
    Unmeasured,
    /// The run's total, closing the run.
    Total(String),
}

impl RunTokenLedger {
    /// Fold one event and return the token lines it contributes.
    fn observe(&mut self, event: &AgentEvent, prints_total: bool) -> Vec<TokenLine> {
        match event {
            AgentEvent::RunStarted { .. } => {
                self.run_usage = CumulativeUsage::default();
                Vec::new()
            }
            AgentEvent::TurnCompleted { usage, .. } => match usage {
                Some(usage) => vec![self.request(usage, None)],
                // Absent accounting reads as absent, never as `0 tokens`.
                None => vec![TokenLine::Unmeasured],
            },
            AgentEvent::RunCompleted {
                extraction_required,
                ..
            } => {
                // A run with structured output closes at its extraction
                // outcome, whose requests belong to the same run.
                if *extraction_required || !prints_total {
                    Vec::new()
                } else {
                    vec![self.total()]
                }
            }
            AgentEvent::ExtractionSucceeded { request_usage, .. }
            | AgentEvent::ExtractionFailed { request_usage, .. } => {
                let mut lines = request_usage
                    .iter()
                    .map(|usage| self.request(usage, Some("extraction")))
                    .collect::<Vec<_>>();
                if prints_total {
                    lines.push(self.total());
                }
                lines
            }
            _ => Vec::new(),
        }
    }

    fn request(&mut self, usage: &TurnUsage, label: Option<&str>) -> TokenLine {
        self.run_usage.add_turn(usage);
        let summary = meerkat_core::turn_usage_summary(usage);
        TokenLine::Request(match label {
            Some(label) => format!("{label}: {summary}"),
            None => summary,
        })
    }

    fn total(&self) -> TokenLine {
        TokenLine::Total(run_total_line(self.run_usage.as_usage()))
    }
}

fn run_total_line(usage: &Usage) -> String {
    format!("total: {}", meerkat_core::usage_summary(usage))
}

/// Print a run's closing total, as the renderer does at the end of a run.
/// Hosts call this with the authoritative total from the run result.
pub fn print_run_total(usage: &Usage) {
    let ansi = stderr_is_tty();
    chrome_line(
        false,
        "primary",
        &format!("\n{}────────{}", style(ansi, DIM), reset(ansi)),
    );
    chrome_line(
        false,
        "primary",
        &format!(
            "{}{}{}",
            style(ansi, DIM),
            run_total_line(usage),
            reset(ansi)
        ),
    );
}

#[derive(Debug, Clone, Copy)]
struct ToolResultPreviewLimits {
    max_lines: usize,
    max_line_bytes: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ToolResultPreview {
    lines: Vec<String>,
    omitted_lines: usize,
    shortened_lines: usize,
    hidden_bytes: usize,
}

impl ToolResultPreview {
    fn is_truncated(&self) -> bool {
        self.omitted_lines > 0 || self.shortened_lines > 0
    }
}

#[derive(Debug)]
pub struct StreamRenderSummary {
    pub focus_requested: Option<String>,
    pub focus_seen: bool,
    pub discovered_scopes: Vec<String>,
}

/// Rich streaming renderer.
///
/// Writes chrome (thinking traces, tool calls, turn info) to stderr and
/// text content to stdout. Maintains independent render state per scope.
pub struct StreamRenderer {
    ansi: bool,
    policy: StreamRenderPolicy,
    verbose: bool,
    run_totals: RunTotalSource,
    states: HashMap<String, ScopeRenderState>,
    discovered_scopes: BTreeSet<String>,
    focus_seen: bool,
}

impl StreamRenderer {
    /// Create a new renderer.
    pub fn new(
        ansi: bool,
        policy: StreamRenderPolicy,
        verbose: bool,
        run_totals: RunTotalSource,
    ) -> Self {
        Self {
            ansi,
            policy,
            verbose,
            run_totals,
            states: HashMap::new(),
            discovered_scopes: BTreeSet::new(),
            focus_seen: false,
        }
    }

    /// Process an attributed stream event.
    pub fn render(&mut self, scoped: &ScopedAgentEvent) {
        let scope_id = if scoped.scope_id.is_empty() {
            "primary".to_string()
        } else {
            scoped.scope_id.clone()
        };

        self.discovered_scopes.insert(scope_id.clone());

        let should_render = match &self.policy {
            StreamRenderPolicy::MuxAll => true,
            StreamRenderPolicy::Focus(focus) => {
                if &scope_id == focus {
                    self.focus_seen = true;
                    true
                } else {
                    false
                }
            }
            StreamRenderPolicy::PrimaryOnly => scope_id == "primary",
        };

        if !should_render {
            return;
        }

        let state = self.states.entry(scope_id.clone()).or_default();
        let mux = matches!(self.policy, StreamRenderPolicy::MuxAll);
        render_event(
            self.ansi,
            mux,
            &scope_id,
            state,
            self.verbose,
            &scoped.event,
        );
        let prints_total = self.run_totals.renderer_prints_total(&scope_id);
        let token_lines = state.tokens.observe(&scoped.event, prints_total);
        if !token_lines.is_empty() {
            end_text_block(state);
            end_thinking_block(mux, &scope_id, state);
        }
        render_token_lines(self.ansi, mux, &scope_id, token_lines);
    }

    /// Finalize rendering and return summary info for focus validation.
    pub fn finish(&mut self) -> StreamRenderSummary {
        let scope_ids: Vec<String> = self.states.keys().cloned().collect();
        for scope_id in scope_ids {
            if let Some(state) = self.states.get_mut(&scope_id) {
                end_text_block(state);
                end_thinking_block(
                    matches!(self.policy, StreamRenderPolicy::MuxAll),
                    &scope_id,
                    state,
                );
            }
        }

        let focus_requested = match &self.policy {
            StreamRenderPolicy::Focus(scope) => Some(scope.clone()),
            _ => None,
        };

        StreamRenderSummary {
            focus_requested,
            focus_seen: self.focus_seen,
            discovered_scopes: self.discovered_scopes.iter().cloned().collect(),
        }
    }
}

fn render_event(
    ansi: bool,
    mux: bool,
    scope_id: &str,
    state: &mut ScopeRenderState,
    verbose: bool,
    event: &AgentEvent,
) {
    match event {
        // ── Turn lifecycle ──────────────────────────────────────────
        AgentEvent::TurnStarted { turn_number } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
            let n = turn_number + 1;
            if n > 1 {
                chrome_line(
                    mux,
                    scope_id,
                    &format!("\n{}━━━ Turn {} ━━━{}", style(ansi, DIM), n, reset(ansi)),
                );
            }
        }

        // The turn's token line comes from the run token ledger.
        AgentEvent::TurnCompleted { .. } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
        }

        // The degradation markers are operator-facing: they name the exact
        // dimension that went unmeasured or contested while the turn itself
        // completed normally.
        AgentEvent::TurnUsageAccountingUnmeasured { unmeasured, .. } => {
            end_text_block(state);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}  ⚠ {} (turn committed){}",
                    style(ansi, DIM),
                    unmeasured,
                    reset(ansi)
                ),
            );
        }

        // A durable steer (for example a background job's persisted notice)
        // joined the running turn at a model boundary instead of waiting for
        // a follow-up turn.
        AgentEvent::BoundaryAppendApplied { append_count, .. } => {
            end_text_block(state);
            let rows = if *append_count == 1 { "row" } else { "rows" };
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}  ↳ steer joined this turn ({} transcript {}){}",
                    style(ansi, DIM),
                    append_count,
                    rows,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::TurnUsageAccountingIdentityDisputed { dispute, .. } => {
            end_text_block(state);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}  ⚠ {} (counters kept as reported){}",
                    style(ansi, DIM),
                    dispute,
                    reset(ansi)
                ),
            );
        }

        // ── Reasoning / thinking ───────────────────────────────────
        AgentEvent::ReasoningDelta { delta } => {
            end_text_block(state);
            if !state.in_thinking {
                state.in_thinking = true;
                state.reasoning_bytes = 0;
                chrome_line(
                    mux,
                    scope_id,
                    &format!(
                        "\n{}{}thinking{}",
                        style(ansi, ITALIC),
                        style(ansi, MAGENTA),
                        reset(ansi)
                    ),
                );
            }
            state.reasoning_bytes += delta.len();
            stderr_inline(
                mux,
                scope_id,
                &format!("{}{}{}", style(ansi, DIM), delta, reset(ansi)),
            );
        }

        AgentEvent::ReasoningComplete { .. } => {
            end_thinking_block(mux, scope_id, state);
        }

        // ── Text output ────────────────────────────────────────────
        AgentEvent::TextDelta { delta } => {
            end_thinking_block(mux, scope_id, state);
            if !state.in_text {
                state.in_text = true;
                stdout_inline(mux, scope_id, "\n");
            }
            stdout_inline(mux, scope_id, delta);
        }

        AgentEvent::TextComplete { .. } => {
            end_text_block(state);
        }

        // ── Tool calls ─────────────────────────────────────────────
        AgentEvent::ToolCallRequested { name, args, .. } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
            let args_str = serde_json::to_string(args).unwrap_or_default();
            let args_preview = truncate_str(&args_str, MAX_TOOL_ARGS_PREVIEW);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "\n{}{}tool{} {}{}({}){}",
                    style(ansi, ITALIC),
                    style(ansi, MAGENTA),
                    reset(ansi),
                    style(ansi, BOLD),
                    name,
                    args_preview,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::ToolExecutionStarted { name, .. } => {
            chrome_line(
                mux,
                scope_id,
                &format!("{}  ▸ running {}...{}", style(ansi, DIM), name, reset(ansi)),
            );
        }

        AgentEvent::ToolExecutionCompleted {
            name,
            content,
            is_error,
            duration_ms,
            ..
        } => {
            let result = meerkat_core::types::text_content(content);
            let (marker, color) = if *is_error {
                ("✗", RED)
            } else {
                ("✓", GREEN)
            };
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}  {} {}{} {}({}ms){}",
                    style(ansi, color),
                    marker,
                    name,
                    reset(ansi),
                    style(ansi, DIM),
                    "",
                    duration_ms,
                    reset(ansi)
                ),
            );
            if !result.is_empty() {
                let preview = preview_tool_result(
                    &result,
                    (!verbose).then_some(ToolResultPreviewLimits {
                        max_lines: MAX_TOOL_RESULT_LINES,
                        max_line_bytes: MAX_TOOL_RESULT_LINE_BYTES,
                    }),
                );
                for line in &preview.lines {
                    chrome_line(
                        mux,
                        scope_id,
                        &format!("{}    {}{}", style(ansi, DIM), line, reset(ansi)),
                    );
                }
                if let Some(summary) = tool_result_truncation_summary(&preview) {
                    chrome_line(
                        mux,
                        scope_id,
                        &format!("{}    ... ({summary}){}", style(ansi, DIM), reset(ansi)),
                    );
                }
            }
        }

        AgentEvent::ToolExecutionTimedOut {
            name, timeout_ms, ..
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}  ⏱ {} timed out ({}ms){}",
                    style(ansi, YELLOW),
                    style(ansi, BOLD),
                    name,
                    timeout_ms,
                    reset(ansi)
                ),
            );
        }

        // ── Compaction ─────────────────────────────────────────────
        AgentEvent::CompactionStarted {
            message_count,
            estimated_history_tokens,
            ..
        } => {
            end_text_block(state);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "\n{}⟳ Compacting context ({} messages, ~{} tokens)...{}",
                    style(ansi, DIM),
                    message_count,
                    estimated_history_tokens,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::CompactionCompleted {
            messages_before,
            messages_after,
            summary_tokens,
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}✓ Compacted: {} → {} messages ({} summary tokens){}",
                    style(ansi, DIM),
                    messages_before,
                    messages_after,
                    summary_tokens,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::CompactionFailed { reason } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}✗ Compaction failed: {}{}",
                    style(ansi, YELLOW),
                    style(ansi, BOLD),
                    reason,
                    reset(ansi)
                ),
            );
        }

        // ── Budget / retry ─────────────────────────────────────────
        AgentEvent::BudgetWarning {
            budget_type,
            used,
            limit,
            percent,
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}⚠ Budget: {:?} at {:.0}% ({}/{}){}",
                    style(ansi, YELLOW),
                    style(ansi, BOLD),
                    budget_type,
                    percent * 100.0,
                    used,
                    limit,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::Retrying { retry } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}⟳ Retry {}/{}: {} ({}ms){}",
                    style(ansi, YELLOW),
                    retry.plan.attempt,
                    retry.plan.max_retries,
                    retry.failure.message,
                    retry.plan.selected_delay_ms,
                    reset(ansi)
                ),
            );
        }

        // ── Session lifecycle ──────────────────────────────────────
        AgentEvent::RunStarted { .. } => {}

        // `run_completed.usage` is session-cumulative and precedes any
        // extraction, so it is not this run's total. The run token ledger
        // prints the total when the run (including extraction) closes.
        AgentEvent::RunCompleted { .. } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
        }

        AgentEvent::ExtractionFailed { reason, .. } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}⚠ extraction failed: {}{}",
                    style(ansi, YELLOW),
                    reason,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::RunFailed { error_report, .. } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "\n{}{}error: {}{}",
                    style(ansi, RED),
                    style(ansi, BOLD),
                    error_report.message,
                    reset(ansi)
                ),
            );
        }

        // ── Skills ─────────────────────────────────────────────────
        AgentEvent::SkillsResolved {
            skills,
            injection_bytes,
        } => {
            if !skills.is_empty() {
                let names: Vec<String> = skills
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect();
                chrome_line(
                    mux,
                    scope_id,
                    &format!(
                        "{}skills: {} ({} bytes){}",
                        style(ansi, DIM),
                        names.join(", "),
                        injection_bytes,
                        reset(ansi)
                    ),
                );
            }
        }

        AgentEvent::SkillResolutionFailed { skill_key, reason } => {
            let reference_display = skill_key
                .as_ref()
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| "<unknown>".to_string());
            let error_display = reason.to_string();
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}⚠ skill failed: {}: {}{}",
                    style(ansi, YELLOW),
                    style(ansi, BOLD),
                    reference_display,
                    error_display,
                    reset(ansi)
                ),
            );
        }

        // ── Hooks ──────────────────────────────────────────────────
        AgentEvent::HookStarted { hook_id, point } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}hook: {} ({:?}){}",
                    style(ansi, DIM),
                    hook_id,
                    point,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::HookCompleted {
            hook_id,
            duration_ms,
            ..
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}✓{} hook: {} ({}ms){}",
                    style(ansi, GREEN),
                    style(ansi, DIM),
                    reset(ansi),
                    hook_id,
                    duration_ms,
                    style(ansi, DIM),
                ),
            );
        }

        AgentEvent::HookFailed {
            hook_id, reason, ..
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}✗ hook failed: {}: {}{}",
                    style(ansi, RED),
                    style(ansi, BOLD),
                    hook_id,
                    reason,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::HookDenied {
            hook_id, message, ..
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}⊘ hook denied: {}: {}{}",
                    style(ansi, RED),
                    style(ansi, BOLD),
                    hook_id,
                    message,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::InteractionCallbackPending {
            tool_name, args, ..
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}⧖ callback pending:{} {} {}{}",
                    style(ansi, YELLOW),
                    style(ansi, BOLD),
                    reset(ansi),
                    tool_name,
                    truncate_str(&args.to_string(), MAX_TOOL_ARGS_PREVIEW),
                    reset(ansi)
                ),
            );
        }

        // Remaining hook/interaction/stream events - silent in stream mode
        _ => {}
    }
}

fn render_token_lines(ansi: bool, mux: bool, scope_id: &str, lines: Vec<TokenLine>) {
    for line in lines {
        let text = match line {
            TokenLine::Request(summary) => format!("  {summary}"),
            // The provider accounted for nothing on this call. Printing
            // `0 tokens` would be a wrong number that reads as a real one.
            TokenLine::Unmeasured => "  tokens unmeasured".to_string(),
            TokenLine::Total(total) => {
                chrome_line(
                    mux,
                    scope_id,
                    &format!("\n{}────────{}", style(ansi, DIM), reset(ansi)),
                );
                total
            }
        };
        chrome_line(
            mux,
            scope_id,
            &format!("{}{}{}", style(ansi, DIM), text, reset(ansi)),
        );
    }
}

fn chrome_line(mux: bool, scope_id: &str, msg: &str) {
    let mut stderr = io::stderr().lock();
    if mux {
        let _ = write!(stderr, "[{scope_id}] ");
    }
    let _ = writeln!(stderr, "{msg}");
    let _ = stderr.flush();
}

fn stderr_inline(mux: bool, scope_id: &str, msg: &str) {
    let mut stderr = io::stderr().lock();
    if mux {
        let _ = write!(stderr, "[{scope_id}] ");
    }
    let _ = write!(stderr, "{msg}");
    let _ = stderr.flush();
}

fn stdout_inline(mux: bool, scope_id: &str, msg: &str) {
    let mut stdout = io::stdout().lock();
    if mux {
        let _ = write!(stdout, "[{scope_id}] ");
    }
    let _ = write!(stdout, "{msg}");
    let _ = stdout.flush();
}

fn style(ansi: bool, code: &'static str) -> &'static str {
    if ansi { code } else { "" }
}

fn reset(ansi: bool) -> &'static str {
    if ansi { RESET } else { "" }
}

fn end_thinking_block(mux: bool, scope_id: &str, state: &mut ScopeRenderState) {
    if state.in_thinking {
        state.in_thinking = false;
        if state.reasoning_bytes > 0 {
            let mut stderr = io::stderr().lock();
            if mux {
                let _ = writeln!(stderr, "[{scope_id}] ");
            } else {
                let _ = writeln!(stderr);
            }
            let _ = stderr.flush();
        }
        state.reasoning_bytes = 0;
    }
}

fn end_text_block(state: &mut ScopeRenderState) {
    if state.in_text {
        state.in_text = false;
    }
}

fn preview_tool_result(result: &str, limits: Option<ToolResultPreviewLimits>) -> ToolResultPreview {
    let lines: Vec<&str> = result.lines().collect();
    let Some(limits) = limits else {
        return ToolResultPreview {
            lines: lines.iter().map(|line| (*line).to_string()).collect(),
            omitted_lines: 0,
            shortened_lines: 0,
            hidden_bytes: 0,
        };
    };

    let shown = lines.len().min(limits.max_lines);
    let mut preview = ToolResultPreview {
        lines: Vec::with_capacity(shown),
        omitted_lines: lines.len().saturating_sub(shown),
        shortened_lines: 0,
        hidden_bytes: 0,
    };

    for line in &lines[..shown] {
        if line.len() <= limits.max_line_bytes {
            preview.lines.push((*line).to_string());
            continue;
        }

        let prefix = truncate_str(line, limits.max_line_bytes);
        preview.hidden_bytes += line.len().saturating_sub(prefix.len());
        preview.shortened_lines += 1;
        preview.lines.push(format!("{prefix}..."));
    }

    preview
}

fn tool_result_truncation_summary(preview: &ToolResultPreview) -> Option<String> {
    if !preview.is_truncated() {
        return None;
    }

    let mut parts = Vec::new();
    if preview.omitted_lines > 0 {
        let noun = if preview.omitted_lines == 1 {
            "line"
        } else {
            "lines"
        };
        parts.push(format!("{} more {noun}", preview.omitted_lines));
    }
    if preview.shortened_lines > 0 {
        let noun = if preview.shortened_lines == 1 {
            "line"
        } else {
            "lines"
        };
        parts.push(format!(
            "{} bytes hidden from {} long {noun}",
            preview.hidden_bytes, preview.shortened_lines
        ));
    }
    parts.push("use --verbose for full output".to_string());
    Some(parts.join("; "))
}

/// Truncate a string to `max_bytes` respecting UTF-8 boundaries.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let truncate_at = s
        .char_indices()
        .take_while(|(i, c)| *i + c.len_utf8() <= max_bytes)
        .last()
        .map_or(0, |(i, c)| i + c.len_utf8());
    &s[..truncate_at]
}

/// Detect if stderr is a terminal (supports ANSI).
pub fn stderr_is_tty() -> bool {
    io::stderr().is_terminal()
}

/// Validate canonical scope selector format used by `--stream-focus`.
#[cfg(test)]
pub fn is_valid_scope_id(input: &str) -> bool {
    if input == "primary" {
        return true;
    }
    if let Some(sub) = input.strip_prefix("primary/sub:") {
        return is_scope_atom(sub);
    }
    if let Some(rest) = input.strip_prefix("mob:") {
        if let Some((member, sub)) = rest.split_once("/sub:") {
            return is_scope_atom(member) && is_scope_atom(sub);
        }
        return is_scope_atom(rest);
    }
    false
}

#[cfg(test)]
fn is_scope_atom(input: &str) -> bool {
    !input.is_empty() && !input.contains('/') && !input.chars().any(char::is_whitespace)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_within_limit() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_str_at_boundary() {
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn test_truncate_str_utf8() {
        let s = "café";
        assert_eq!(s.len(), 5);
        let truncated = truncate_str(s, 4);
        assert_eq!(truncated, "caf");
    }

    #[test]
    fn test_preview_tool_result_shortens_long_single_line() {
        let result = "a".repeat(20);
        let preview = preview_tool_result(
            &result,
            Some(ToolResultPreviewLimits {
                max_lines: 8,
                max_line_bytes: 5,
            }),
        );

        assert_eq!(preview.lines, vec!["aaaaa..."]);
        assert_eq!(preview.omitted_lines, 0);
        assert_eq!(preview.shortened_lines, 1);
        assert_eq!(preview.hidden_bytes, 15);
        assert_eq!(
            tool_result_truncation_summary(&preview).as_deref(),
            Some("15 bytes hidden from 1 long line; use --verbose for full output")
        );
    }

    #[test]
    fn test_preview_tool_result_limits_line_count() {
        let result = (0..12)
            .map(|index| format!("line{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let preview = preview_tool_result(
            &result,
            Some(ToolResultPreviewLimits {
                max_lines: 3,
                max_line_bytes: 80,
            }),
        );

        assert_eq!(preview.lines, vec!["line0", "line1", "line2"]);
        assert_eq!(preview.omitted_lines, 9);
        assert_eq!(
            tool_result_truncation_summary(&preview).as_deref(),
            Some("9 more lines; use --verbose for full output")
        );
    }

    #[test]
    fn test_preview_tool_result_unlimited_for_verbose_renderer() {
        let long = "x".repeat(20);
        let result = format!("{long}\nsecond");
        let preview = preview_tool_result(&result, None);

        assert_eq!(preview.lines, vec![long, "second".to_string()]);
        assert!(!preview.is_truncated());
        assert_eq!(tool_result_truncation_summary(&preview), None);
    }

    #[test]
    fn test_scope_id_validation() {
        assert!(is_valid_scope_id("primary"));
        assert!(is_valid_scope_id("primary/sub:op_1"));
        assert!(is_valid_scope_id("mob:planner"));
        assert!(is_valid_scope_id("mob:planner/sub:op_1"));
        assert!(!is_valid_scope_id(""));
        assert!(!is_valid_scope_id("sub:op_1"));
        assert!(!is_valid_scope_id("mob:/sub:op_1"));
        assert!(!is_valid_scope_id("mob:planner/sub:"));
    }

    #[test]
    fn test_renderer_policy_focus() {
        let mut renderer = StreamRenderer::new(
            false,
            StreamRenderPolicy::Focus("mob:a".into()),
            false,
            RunTotalSource::StreamRows,
        );
        renderer.render(&ScopedAgentEvent {
            scope_id: "mob:b".into(),
            scope_path: vec![],
            event: AgentEvent::TextDelta { delta: "x".into() },
        });
        renderer.render(&ScopedAgentEvent {
            scope_id: "mob:a".into(),
            scope_path: vec![],
            event: AgentEvent::TextDelta { delta: "y".into() },
        });
        let summary = renderer.finish();
        assert_eq!(summary.focus_requested, Some("mob:a".into()));
        assert!(summary.focus_seen);
        assert_eq!(
            summary.discovered_scopes,
            vec!["mob:a".to_string(), "mob:b".to_string()]
        );
    }

    #[test]
    fn test_renderer_policy_primary_only_matches_literal_primary_scope() {
        let mut renderer = StreamRenderer::new(
            false,
            StreamRenderPolicy::PrimaryOnly,
            false,
            RunTotalSource::StreamRows,
        );
        renderer.render(&ScopedAgentEvent {
            scope_id: "primary/sub:child-1".into(),
            scope_path: vec![],
            event: AgentEvent::TextDelta {
                delta: "child".into(),
            },
        });
        renderer.render(&ScopedAgentEvent {
            scope_id: "primary".into(),
            scope_path: vec![],
            event: AgentEvent::TextDelta {
                delta: "parent".into(),
            },
        });
        let summary = renderer.finish();
        assert_eq!(summary.discovered_scopes.len(), 2);
        assert!(renderer.states.contains_key("primary"));
        assert!(!renderer.states.contains_key("primary/sub:child-1"));
    }

    fn openai_row(prompt: u64, output: u64, cached: u64) -> TurnUsage {
        TurnUsage::new(
            Usage {
                input_tokens: prompt,
                output_tokens: output,
                cache_creation_tokens: None,
                cache_read_tokens: Some(cached),
                reasoning_tokens: None,
                provider_accounting: None,
            },
            meerkat_core::ProviderTokenAccounting::openai("gpt-test", prompt),
        )
    }

    fn anthropic_row(uncached: u64, written: u64, read: u64, output: u64) -> TurnUsage {
        TurnUsage::new(
            Usage {
                input_tokens: uncached,
                output_tokens: output,
                cache_creation_tokens: Some(written),
                cache_read_tokens: Some(read),
                reasoning_tokens: None,
                provider_accounting: None,
            },
            meerkat_core::ProviderTokenAccounting::anthropic(
                "claude-test",
                uncached,
                written,
                read,
            ),
        )
    }

    fn run_completed(extraction_required: bool) -> AgentEvent {
        AgentEvent::RunCompleted {
            session_id: meerkat_core::SessionId::new(),
            result: "done".into(),
            structured_output: None,
            extraction_required,
            // Session-cumulative and pre-extraction: never the run's total.
            usage: Usage {
                input_tokens: 999_999,
                output_tokens: 999_999,
                ..Usage::default()
            }
            .into(),
            terminal_cause_kind: None,
        }
    }

    fn run_started() -> AgentEvent {
        AgentEvent::RunStarted {
            session_id: meerkat_core::SessionId::new(),
            input: meerkat_core::RunInput::Content {
                content: meerkat_core::ContentInput::Text("go".into()),
            },
        }
    }

    fn fold(events: &[AgentEvent], prints_total: bool) -> Vec<TokenLine> {
        let mut ledger = RunTokenLedger::default();
        events
            .iter()
            .flat_map(|event| ledger.observe(event, prints_total))
            .collect()
    }

    /// Every provider request is a line, the first (tool-call) request and
    /// each extraction request included, and the total closes the run after
    /// extraction with exactly the fold of those lines.
    #[test]
    fn token_lines_cover_every_request_and_total_the_run() {
        let lines = fold(
            &[
                run_started(),
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::ToolUse,
                    usage: Some(openai_row(1000, 10, 0)),
                },
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_row(1200, 20, 1000)),
                },
                run_completed(true),
                AgentEvent::ExtractionSucceeded {
                    session_id: meerkat_core::SessionId::new(),
                    structured_output: serde_json::json!({"answer": "ok"}),
                    schema_warnings: None,
                    request_usage: vec![openai_row(1250, 25, 1200), openai_row(1300, 30, 1250)],
                    origin: meerkat_core::StructuredOutputOrigin::ExtractionRequest,
                },
            ],
            true,
        );
        assert_eq!(
            lines,
            vec![
                TokenLine::Request("1010 tokens (1000 in / 10 out, 0 cached)".into()),
                TokenLine::Request("1220 tokens (1200 in / 20 out, 1000 cached)".into()),
                TokenLine::Request(
                    "extraction: 1275 tokens (1250 in / 25 out, 1200 cached)".into()
                ),
                TokenLine::Request(
                    "extraction: 1330 tokens (1300 in / 30 out, 1250 cached)".into()
                ),
                TokenLine::Total("total: 4835 tokens (4750 in / 85 out, 3450 cached)".into()),
            ]
        );
    }

    /// A run without extraction closes at `run_completed`; its total is the
    /// run's own rows, not the session-cumulative `run_completed.usage`, and
    /// the next run starts from zero.
    #[test]
    fn a_run_total_is_the_runs_own_rows_and_resets_per_run() {
        let lines = fold(
            &[
                run_started(),
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_row(100, 5, 0)),
                },
                run_completed(false),
                run_started(),
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_row(200, 7, 100)),
                },
                run_completed(false),
            ],
            true,
        );
        assert_eq!(
            lines.last(),
            Some(&TokenLine::Total(
                "total: 207 tokens (200 in / 7 out, 100 cached)".into()
            ))
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| matches!(line, TokenLine::Total(_)))
                .count(),
            2
        );
    }

    /// Per-request lines use the presented-input denominator, so an
    /// Anthropic call's line counts its cached input and the lines add up to
    /// the total.
    #[test]
    fn anthropic_request_lines_count_presented_input() {
        let lines = fold(
            &[
                run_started(),
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(anthropic_row(120, 0, 4300, 90)),
                },
                run_completed(false),
            ],
            true,
        );
        assert_eq!(
            lines,
            vec![
                TokenLine::Request("4510 tokens (4420 in / 90 out, 4300 cached)".into()),
                TokenLine::Total("total: 4510 tokens (4420 in / 90 out, 4300 cached)".into()),
            ]
        );
    }

    /// With the host printing the primary run's total from its run result,
    /// the renderer prints the request lines and no total; an unmeasured
    /// call reads as unmeasured.
    #[test]
    fn host_run_result_totals_leave_only_request_lines() {
        let lines = fold(
            &[
                run_started(),
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::ToolUse,
                    usage: None,
                },
                AgentEvent::TurnCompleted {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                    usage: Some(openai_row(100, 5, 0)),
                },
                run_completed(false),
            ],
            RunTotalSource::HostRunResult.renderer_prints_total("primary"),
        );
        assert_eq!(
            lines,
            vec![
                TokenLine::Unmeasured,
                TokenLine::Request("105 tokens (100 in / 5 out, 0 cached)".into()),
            ]
        );
        assert!(RunTotalSource::HostRunResult.renderer_prints_total("mob:worker"));
        assert!(RunTotalSource::StreamRows.renderer_prints_total("primary"));
    }
}

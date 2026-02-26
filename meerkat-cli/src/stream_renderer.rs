//! Rich streaming output renderer for the CLI.
//!
//! Renders scoped events to stderr (chrome: thinking, tool calls, status)
//! and stdout (text content) with ANSI styling.

use meerkat_core::{AgentEvent, ScopedAgentEvent};
use std::collections::{BTreeSet, HashMap};
use std::io::{self, IsTerminal, Write};

/// Maximum lines of tool result output to display.
const MAX_TOOL_RESULT_LINES: usize = 20;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRenderPolicy {
    PrimaryOnly,
    MuxAll,
    Focus(String),
}

#[derive(Debug, Default)]
struct ScopeRenderState {
    in_thinking: bool,
    in_text: bool,
    reasoning_bytes: usize,
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
    states: HashMap<String, ScopeRenderState>,
    discovered_scopes: BTreeSet<String>,
    focus_seen: bool,
}

impl StreamRenderer {
    /// Create a new renderer.
    pub fn new(ansi: bool, policy: StreamRenderPolicy) -> Self {
        Self {
            ansi,
            policy,
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
        render_event(
            self.ansi,
            matches!(self.policy, StreamRenderPolicy::MuxAll),
            &scope_id,
            state,
            &scoped.event,
        );
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

        AgentEvent::TurnCompleted { stop_reason, usage } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}  {} tokens ({} in / {} out){}",
                    style(ansi, DIM),
                    usage.input_tokens + usage.output_tokens,
                    usage.input_tokens,
                    usage.output_tokens,
                    reset(ansi)
                ),
            );
            let _ = stop_reason;
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
            result,
            is_error,
            duration_ms,
            ..
        } => {
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
                let lines: Vec<&str> = result.lines().collect();
                let show = lines.len().min(MAX_TOOL_RESULT_LINES);
                for line in &lines[..show] {
                    chrome_line(
                        mux,
                        scope_id,
                        &format!("{}    {}{}", style(ansi, DIM), line, reset(ansi)),
                    );
                }
                if lines.len() > MAX_TOOL_RESULT_LINES {
                    chrome_line(
                        mux,
                        scope_id,
                        &format!(
                            "{}    ... ({} more lines){}",
                            style(ansi, DIM),
                            lines.len() - MAX_TOOL_RESULT_LINES,
                            reset(ansi)
                        ),
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

        AgentEvent::CompactionFailed { error } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}✗ Compaction failed: {}{}",
                    style(ansi, YELLOW),
                    style(ansi, BOLD),
                    error,
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

        AgentEvent::Retrying {
            attempt,
            max_attempts,
            error,
            delay_ms,
        } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}⟳ Retry {}/{}: {} ({}ms){}",
                    style(ansi, YELLOW),
                    attempt,
                    max_attempts,
                    error,
                    delay_ms,
                    reset(ansi)
                ),
            );
        }

        // ── Session lifecycle ──────────────────────────────────────
        AgentEvent::RunStarted { .. } => {}

        AgentEvent::RunCompleted { usage, .. } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
            chrome_line(
                mux,
                scope_id,
                &format!("\n{}────────{}", style(ansi, DIM), reset(ansi)),
            );
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}total: {} tokens ({} in / {} out){}",
                    style(ansi, DIM),
                    usage.input_tokens + usage.output_tokens,
                    usage.input_tokens,
                    usage.output_tokens,
                    reset(ansi)
                ),
            );
        }

        AgentEvent::RunFailed { error, .. } => {
            end_text_block(state);
            end_thinking_block(mux, scope_id, state);
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "\n{}{}error: {}{}",
                    style(ansi, RED),
                    style(ansi, BOLD),
                    error,
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

        AgentEvent::SkillResolutionFailed { reference, error } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}⚠ skill failed: {}: {}{}",
                    style(ansi, YELLOW),
                    style(ansi, BOLD),
                    reference,
                    error,
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

        AgentEvent::HookFailed { hook_id, error, .. } => {
            chrome_line(
                mux,
                scope_id,
                &format!(
                    "{}{}✗ hook failed: {}: {}{}",
                    style(ansi, RED),
                    style(ansi, BOLD),
                    hook_id,
                    error,
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

        // Remaining hook/interaction/stream events - silent in stream mode
        _ => {}
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
        let mut renderer = StreamRenderer::new(false, StreamRenderPolicy::Focus("mob:a".into()));
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
        let mut renderer = StreamRenderer::new(false, StreamRenderPolicy::PrimaryOnly);
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
}

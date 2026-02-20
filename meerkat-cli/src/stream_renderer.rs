//! Rich streaming output renderer for the CLI.
//!
//! Renders `AgentEvent`s to stderr (chrome: thinking, tool calls, status)
//! and stdout (text content) with ANSI styling. Inspired by Codex CLI patterns:
//! - Label-then-content for section headers
//! - Conditional ANSI via `with_ansi` flag
//! - Dimmed output for tool results, italic+magenta for thinking
//! - Function-call notation for tool invocations

use meerkat_core::AgentEvent;
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
// const CYAN: &str = "\x1b[36m"; // reserved for future use

/// Rich streaming renderer.
///
/// Writes chrome (thinking traces, tool calls, turn info) to stderr and
/// text content to stdout. All stderr output is conditional on ANSI support.
pub struct StreamRenderer {
    ansi: bool,
    /// Track whether we're currently inside a thinking block.
    in_thinking: bool,
    /// Track whether we're currently inside text output.
    in_text: bool,
    /// Accumulated reasoning text length (for deciding whether to show closing separator).
    reasoning_bytes: usize,
}

impl StreamRenderer {
    /// Create a new renderer.
    ///
    /// `ansi` enables ANSI color/style codes. Set to `false` for piped output.
    pub fn new(ansi: bool) -> Self {
        Self {
            ansi,
            in_thinking: false,
            in_text: false,
            reasoning_bytes: 0,
        }
    }

    /// Process an agent event and render it to the appropriate stream.
    pub fn render(&mut self, event: &AgentEvent) {
        match event {
            // ── Turn lifecycle ──────────────────────────────────────────
            AgentEvent::TurnStarted { turn_number } => {
                self.end_text_block();
                self.end_thinking_block();
                let n = turn_number + 1;
                if n > 1 {
                    self.chrome(&format!(
                        "\n{}━━━ Turn {} ━━━{}",
                        self.style(DIM),
                        n,
                        self.reset()
                    ));
                }
            }

            AgentEvent::TurnCompleted { stop_reason, usage } => {
                self.end_text_block();
                self.end_thinking_block();
                self.chrome(&format!(
                    "{}  {} tokens ({} in / {} out){}",
                    self.style(DIM),
                    usage.input_tokens + usage.output_tokens,
                    usage.input_tokens,
                    usage.output_tokens,
                    self.reset()
                ));
                let _ = stop_reason; // shown only in verbose
            }

            // ── Reasoning / thinking ───────────────────────────────────
            AgentEvent::ReasoningDelta { delta } => {
                self.end_text_block();
                if !self.in_thinking {
                    self.in_thinking = true;
                    self.reasoning_bytes = 0;
                    self.chrome(&format!(
                        "\n{}{}thinking{}",
                        self.style(ITALIC),
                        self.style(MAGENTA),
                        self.reset()
                    ));
                }
                self.reasoning_bytes += delta.len();
                // Write thinking content to stderr, dimmed
                let mut stderr = io::stderr().lock();
                let _ = write!(stderr, "{}{}{}", self.style(DIM), delta, self.reset());
                let _ = stderr.flush();
            }

            AgentEvent::ReasoningComplete { .. } => {
                self.end_thinking_block();
            }

            // ── Text output ────────────────────────────────────────────
            AgentEvent::TextDelta { delta } => {
                self.end_thinking_block();
                if !self.in_text {
                    self.in_text = true;
                    // Print a blank line before text if we had thinking
                    let mut stdout = io::stdout().lock();
                    let _ = write!(stdout, "\n");
                    let _ = stdout.flush();
                }
                let mut stdout = io::stdout().lock();
                let _ = write!(stdout, "{}", delta);
                let _ = stdout.flush();
            }

            AgentEvent::TextComplete { .. } => {
                // TextDelta already printed; just close any open block
                self.end_text_block();
            }

            // ── Tool calls ─────────────────────────────────────────────
            AgentEvent::ToolCallRequested { name, args, .. } => {
                self.end_text_block();
                self.end_thinking_block();
                let args_str = serde_json::to_string(args).unwrap_or_default();
                let args_preview = truncate_str(&args_str, MAX_TOOL_ARGS_PREVIEW);
                self.chrome(&format!(
                    "\n{}{}tool{} {}{}({}){}",
                    self.style(ITALIC),
                    self.style(MAGENTA),
                    self.reset(),
                    self.style(BOLD),
                    name,
                    args_preview,
                    self.reset()
                ));
            }

            AgentEvent::ToolExecutionStarted { name, .. } => {
                self.chrome(&format!(
                    "{}  ▸ running {}...{}",
                    self.style(DIM),
                    name,
                    self.reset()
                ));
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
                self.chrome(&format!(
                    "{}{}  {} {}{} {}({}ms){}",
                    self.style(color),
                    marker,
                    name,
                    self.reset(),
                    self.style(DIM),
                    "",
                    duration_ms,
                    self.reset()
                ));
                // Show truncated result output
                if !result.is_empty() {
                    let lines: Vec<&str> = result.lines().collect();
                    let show = lines.len().min(MAX_TOOL_RESULT_LINES);
                    let mut stderr = io::stderr().lock();
                    for line in &lines[..show] {
                        let _ = writeln!(stderr, "{}    {}{}", self.style(DIM), line, self.reset());
                    }
                    if lines.len() > MAX_TOOL_RESULT_LINES {
                        let _ = writeln!(
                            stderr,
                            "{}    ... ({} more lines){}",
                            self.style(DIM),
                            lines.len() - MAX_TOOL_RESULT_LINES,
                            self.reset()
                        );
                    }
                    let _ = stderr.flush();
                }
            }

            AgentEvent::ToolExecutionTimedOut {
                name, timeout_ms, ..
            } => {
                self.chrome(&format!(
                    "{}{}  ⏱ {} timed out ({}ms){}",
                    self.style(YELLOW),
                    self.style(BOLD),
                    name,
                    timeout_ms,
                    self.reset()
                ));
            }

            // ── Compaction ─────────────────────────────────────────────
            AgentEvent::CompactionStarted {
                message_count,
                estimated_history_tokens,
                ..
            } => {
                self.end_text_block();
                self.chrome(&format!(
                    "\n{}⟳ Compacting context ({} messages, ~{} tokens)...{}",
                    self.style(DIM),
                    message_count,
                    estimated_history_tokens,
                    self.reset()
                ));
            }

            AgentEvent::CompactionCompleted {
                messages_before,
                messages_after,
                summary_tokens,
            } => {
                self.chrome(&format!(
                    "{}✓ Compacted: {} → {} messages ({} summary tokens){}",
                    self.style(DIM),
                    messages_before,
                    messages_after,
                    summary_tokens,
                    self.reset()
                ));
            }

            AgentEvent::CompactionFailed { error } => {
                self.chrome(&format!(
                    "{}{}✗ Compaction failed: {}{}",
                    self.style(YELLOW),
                    self.style(BOLD),
                    error,
                    self.reset()
                ));
            }

            // ── Budget / retry ─────────────────────────────────────────
            AgentEvent::BudgetWarning {
                budget_type,
                used,
                limit,
                percent,
            } => {
                self.chrome(&format!(
                    "{}{}⚠ Budget: {:?} at {:.0}% ({}/{}){}",
                    self.style(YELLOW),
                    self.style(BOLD),
                    budget_type,
                    percent * 100.0,
                    used,
                    limit,
                    self.reset()
                ));
            }

            AgentEvent::Retrying {
                attempt,
                max_attempts,
                error,
                delay_ms,
            } => {
                self.chrome(&format!(
                    "{}⟳ Retry {}/{}: {} ({}ms){}",
                    self.style(YELLOW),
                    attempt,
                    max_attempts,
                    error,
                    delay_ms,
                    self.reset()
                ));
            }

            // ── Session lifecycle ──────────────────────────────────────
            AgentEvent::RunStarted { .. } => {}

            AgentEvent::RunCompleted { usage, .. } => {
                self.end_text_block();
                self.end_thinking_block();
                self.chrome(&format!(
                    "\n{}────────{}",
                    self.style(DIM),
                    self.reset()
                ));
                self.chrome(&format!(
                    "{}total: {} tokens ({} in / {} out){}",
                    self.style(DIM),
                    usage.input_tokens + usage.output_tokens,
                    usage.input_tokens,
                    usage.output_tokens,
                    self.reset()
                ));
            }

            AgentEvent::RunFailed { error, .. } => {
                self.end_text_block();
                self.end_thinking_block();
                self.chrome(&format!(
                    "\n{}{}error: {}{}",
                    self.style(RED),
                    self.style(BOLD),
                    error,
                    self.reset()
                ));
            }

            // ── Skills ─────────────────────────────────────────────────
            AgentEvent::SkillsResolved {
                skills,
                injection_bytes,
            } => {
                if !skills.is_empty() {
                    let names: Vec<String> = skills.iter().map(|s| s.to_string()).collect();
                    self.chrome(&format!(
                        "{}skills: {} ({} bytes){}",
                        self.style(DIM),
                        names.join(", "),
                        injection_bytes,
                        self.reset()
                    ));
                }
            }

            AgentEvent::SkillResolutionFailed { reference, error } => {
                self.chrome(&format!(
                    "{}{}⚠ skill failed: {}: {}{}",
                    self.style(YELLOW),
                    self.style(BOLD),
                    reference,
                    error,
                    self.reset()
                ));
            }

            // ── Hooks ──────────────────────────────────────────────────
            AgentEvent::HookStarted { hook_id, point } => {
                self.chrome(&format!(
                    "{}hook: {} ({:?}){}",
                    self.style(DIM),
                    hook_id,
                    point,
                    self.reset()
                ));
            }

            AgentEvent::HookCompleted {
                hook_id,
                duration_ms,
                ..
            } => {
                self.chrome(&format!(
                    "{}{}✓{} hook: {} ({}ms){}",
                    self.style(GREEN),
                    self.style(DIM),
                    self.reset(),
                    hook_id,
                    duration_ms,
                    self.style(DIM),
                ));
            }

            AgentEvent::HookFailed {
                hook_id, error, ..
            } => {
                self.chrome(&format!(
                    "{}{}✗ hook failed: {}: {}{}",
                    self.style(RED),
                    self.style(BOLD),
                    hook_id,
                    error,
                    self.reset()
                ));
            }

            AgentEvent::HookDenied {
                hook_id, message, ..
            } => {
                self.chrome(&format!(
                    "{}{}⊘ hook denied: {}: {}{}",
                    self.style(RED),
                    self.style(BOLD),
                    hook_id,
                    message,
                    self.reset()
                ));
            }

            // Remaining hook/interaction/stream events - silent in stream mode
            _ => {}
        }
    }

    /// Finalize rendering — ensure trailing newlines.
    pub fn finish(&mut self) {
        self.end_text_block();
        self.end_thinking_block();
    }

    // ── Internal helpers ────────────────────────────────────────────────

    /// Write a chrome line to stderr.
    fn chrome(&self, msg: &str) {
        let mut stderr = io::stderr().lock();
        let _ = writeln!(stderr, "{}", msg);
        let _ = stderr.flush();
    }

    /// Return ANSI code if enabled, empty string otherwise.
    fn style<'a>(&self, code: &'a str) -> &'a str {
        if self.ansi {
            code
        } else {
            ""
        }
    }

    /// Return reset code if enabled, empty string otherwise.
    fn reset(&self) -> &'static str {
        if self.ansi {
            RESET
        } else {
            ""
        }
    }

    /// Close an open thinking block.
    fn end_thinking_block(&mut self) {
        if self.in_thinking {
            self.in_thinking = false;
            if self.reasoning_bytes > 0 {
                // Ensure newline after thinking content
                let mut stderr = io::stderr().lock();
                let _ = writeln!(stderr);
                let _ = stderr.flush();
            }
            self.reasoning_bytes = 0;
        }
    }

    /// Close an open text block.
    fn end_text_block(&mut self) {
        if self.in_text {
            self.in_text = false;
        }
    }
}

/// Truncate a string to `max_bytes` respecting UTF-8 boundaries.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let truncate_at = s
        .char_indices()
        .take_while(|(i, _)| *i < max_bytes)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &s[..truncate_at]
}

/// Detect if stderr is a terminal (supports ANSI).
pub fn stderr_is_tty() -> bool {
    io::stderr().is_terminal()
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
        // "café" is 5 bytes (é = 2 bytes)
        let s = "café";
        assert_eq!(s.len(), 5);
        let truncated = truncate_str(s, 4);
        // Should truncate before the 'é'
        assert_eq!(truncated, "caf");
    }

    #[test]
    fn test_renderer_no_ansi() {
        let renderer = StreamRenderer::new(false);
        assert_eq!(renderer.style(BOLD), "");
        assert_eq!(renderer.reset(), "");
    }

    #[test]
    fn test_renderer_with_ansi() {
        let renderer = StreamRenderer::new(true);
        assert_eq!(renderer.style(BOLD), BOLD);
        assert_eq!(renderer.reset(), RESET);
    }
}

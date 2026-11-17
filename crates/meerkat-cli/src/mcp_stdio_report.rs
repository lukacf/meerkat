//! The operator's report of a stdio MCP server whose setup failed: what the
//! transport established (the phase, the server's own exit status) and the
//! last lines the server wrote to stderr.
//!
//! The stderr tail is SENSITIVE: arbitrary server output, redacted only as
//! defence in depth. It is written to this process's own stderr, the
//! operator's terminal, and to nothing else: not to an error, an event, a
//! log or anything a model or a remote client reads.

use meerkat_core::mcp_attachment::{McpStdioSetupFailure, McpStdioStderrTail};

/// The most lines of a server's stderr tail one report shows: the last ones.
pub(crate) const TAIL_PRINT_MAX_LINES: usize = 40;

/// The most bytes of a server's stderr tail one report shows: the last ones.
pub(crate) const TAIL_PRINT_MAX_BYTES: usize = 4 * 1024;

/// The most characters of a server name one report shows.
const NAME_PRINT_MAX_CHARS: usize = 64;

/// One failed server's report: its name, the failure in fixed words, and the
/// end of its stderr tail, marked when earlier output is not shown and when
/// the capture had not reached the end of the server's stderr as the tail
/// was read: the stream was still open, or its read stopped early (more may
/// have followed).
pub(crate) fn render(failure: &McpStdioSetupFailure, tail: Option<&McpStdioStderrTail>) -> String {
    // The name comes from configuration: it is shown escaped and bounded,
    // never as terminal input.
    let name: String = failure.server.chars().take(NAME_PRINT_MAX_CHARS).collect();
    let mut report = format!("MCP server '{}': {failure}", name.escape_debug());
    // No tail: the process never started, or its capture is gone.
    let Some(tail) = tail else {
        return report;
    };
    let (lines, left_out) = last_lines(&tail.text);
    if lines.is_empty() {
        report.push_str(match (tail.total_bytes == 0, tail.complete) {
            (true, true) => "\n  it wrote nothing to stderr",
            // The capture had not reached the end of the stream when the
            // tail was read: nothing so far is not nothing.
            (true, false) => {
                "\n  it had written nothing to stderr when this was read \
                 (the end of its stderr was not reached)"
            }
            (false, _) => "\n  its stderr output holds no whole line to show",
        });
        return report;
    }
    report.push_str(match (tail.truncated || left_out, tail.complete) {
        (false, true) => "\n  its last stderr output:",
        (true, true) => "\n  its last stderr output (earlier output cut):",
        (false, false) => "\n  its last stderr output (the end of its stderr was not reached):",
        (true, false) => {
            "\n  its last stderr output (earlier output cut; the end of its stderr was not reached):"
        }
    });
    for line in lines {
        report.push_str(if line.is_empty() { "\n  |" } else { "\n  | " });
        report.push_str(line);
    }
    report
}

/// The end of `text`: whole lines, at most [`TAIL_PRINT_MAX_LINES`] of them
/// and [`TAIL_PRINT_MAX_BYTES`] bytes (each line counted with its line
/// break). A last line longer than the byte bound keeps its end, cut on a
/// character boundary. The flag says whether earlier output is left out.
fn last_lines(text: &str) -> (Vec<&str>, bool) {
    let total = text.lines().count();
    let mut shown = Vec::new();
    let mut bytes = 0_usize;
    let mut line_cut = false;
    for line in text.lines().rev() {
        if shown.len() == TAIL_PRINT_MAX_LINES {
            break;
        }
        let cost = line.len() + 1;
        if bytes + cost > TAIL_PRINT_MAX_BYTES {
            if shown.is_empty() {
                let mut start = line.len().saturating_sub(TAIL_PRINT_MAX_BYTES);
                while !line.is_char_boundary(start) {
                    start += 1;
                }
                line_cut = start > 0;
                shown.push(&line[start..]);
            }
            break;
        }
        bytes += cost;
        shown.push(line);
    }
    let left_out = line_cut || shown.len() < total;
    shown.reverse();
    (shown, left_out)
}

/// The reports of every failed stdio server of `adapter` not reported yet.
pub(crate) async fn collect(adapter: &meerkat_mcp::McpRouterAdapter) -> Vec<String> {
    adapter
        .claim_stdio_failure_reports()
        .await
        .iter()
        .map(|(failure, tail)| render(failure, tail.as_ref()))
        .collect()
}

/// Tell the operator, on stderr, about every failed stdio server of
/// `adapter` not reported yet.
pub(crate) async fn print(adapter: &meerkat_mcp::McpRouterAdapter) {
    for report in collect(adapter).await {
        eprintln!("{report}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::mcp_attachment::{
        McpStdioExit, McpStdioSetupFailureClass, McpStdioSetupPhase,
    };

    fn exited(server: &str, code: i32) -> McpStdioSetupFailure {
        McpStdioSetupFailure::new(
            server,
            1,
            McpStdioSetupPhase::Initialize,
            McpStdioSetupFailureClass::Closed {
                exit: Some(McpStdioExit::Code(code)),
            },
            true,
        )
    }

    fn tail(text: &str, truncated: bool) -> McpStdioStderrTail {
        McpStdioStderrTail::new(text.to_owned(), text.len() as u64, truncated, true)
    }

    #[test]
    fn a_report_names_the_server_its_exit_status_and_its_stderr_lines() {
        let report = render(
            &exited("crashy", 3),
            Some(&tail("config file not found\ngiving up\n", false)),
        );
        assert_eq!(
            report,
            "MCP server 'crashy': stdio setup failed during initialize: \
             the server process ended, exit status 3\n  \
             its last stderr output:\n  \
             | config file not found\n  \
             | giving up"
        );
    }

    #[test]
    fn a_signal_is_named_in_the_report() {
        let failure = McpStdioSetupFailure::new(
            "killed",
            1,
            McpStdioSetupPhase::Initialize,
            McpStdioSetupFailureClass::Closed {
                exit: Some(McpStdioExit::Signal(9)),
            },
            true,
        );
        let report = render(&failure, Some(&tail("", false)));
        assert_eq!(
            report,
            "MCP server 'killed': stdio setup failed during initialize: \
             the server process ended, killed by signal 9 (SIGKILL)\n  \
             it wrote nothing to stderr"
        );
    }

    #[test]
    fn only_the_last_lines_are_shown_and_the_cut_is_marked() {
        let lines: Vec<String> = (1..=100).map(|line| format!("line {line}\n")).collect();
        let text = lines.concat();
        let report = render(&exited("chatty", 1), Some(&tail(&text, false)));
        let shown: Vec<&str> = report
            .lines()
            .filter_map(|line| line.strip_prefix("  | "))
            .collect();
        assert_eq!(shown.len(), TAIL_PRINT_MAX_LINES);
        assert_eq!(shown.first(), Some(&"line 61"));
        assert_eq!(shown.last(), Some(&"line 100"));
        assert!(
            report.contains("\n  its last stderr output (earlier output cut):\n"),
            "{report}"
        );
    }

    #[test]
    fn only_the_last_bytes_are_shown_and_the_cut_is_marked() {
        // Ten lines of 1000 bytes: four whole lines fit the bound.
        let lines: Vec<String> = (0..10)
            .map(|line| format!("{line}{}\n", "x".repeat(998)))
            .collect();
        let text = lines.concat();
        let report = render(&exited("wide", 1), Some(&tail(&text, false)));
        let shown: Vec<&str> = report
            .lines()
            .filter_map(|line| line.strip_prefix("  | "))
            .collect();
        assert_eq!(shown.len(), 4, "whole lines only");
        assert!(shown[0].starts_with('6') && shown[3].starts_with('9'));
        assert!(report.contains("(earlier output cut)"), "{report}");

        // One line longer than the bound keeps its end, cut on a character.
        let long = format!("{}é-the-end\n", "é".repeat(4000));
        let report = render(&exited("long", 1), Some(&tail(&long, false)));
        let shown: Vec<&str> = report
            .lines()
            .filter_map(|line| line.strip_prefix("  | "))
            .collect();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].len() <= TAIL_PRINT_MAX_BYTES, "{}", shown[0].len());
        assert!(shown[0].ends_with("é-the-end"));
        assert!(report.contains("(earlier output cut)"), "{report}");
    }

    #[test]
    fn a_tail_its_capture_already_cut_is_marked_as_cut() {
        let report = render(&exited("flooded", 5), Some(&tail("last line\n", true)));
        assert!(
            report.ends_with("  its last stderr output (earlier output cut):\n  | last line"),
            "{report}"
        );
    }

    #[test]
    fn a_capture_still_open_is_said_and_never_reads_as_nothing_written() {
        let open = |text: &str, truncated: bool| {
            McpStdioStderrTail::new(text.to_owned(), text.len() as u64, truncated, false)
        };
        let report = render(&exited("open", 1), Some(&open("", false)));
        assert!(
            report.ends_with(
                "\n  it had written nothing to stderr when this was read \
                 (the end of its stderr was not reached)"
            ),
            "{report}"
        );
        let report = render(&exited("open", 1), Some(&open("starting\n", false)));
        assert!(
            report.ends_with(
                "\n  its last stderr output (the end of its stderr was not reached):\n  | starting"
            ),
            "{report}"
        );
        let report = render(&exited("open", 1), Some(&open("late line\n", true)));
        assert!(
            report.ends_with(
                "\n  its last stderr output (earlier output cut; the end of its stderr was not reached):\
                 \n  | late line"
            ),
            "{report}"
        );
    }

    #[test]
    fn a_server_that_never_started_has_no_stderr_block() {
        let failure = McpStdioSetupFailure::new(
            "missing",
            1,
            McpStdioSetupPhase::Launch,
            McpStdioSetupFailureClass::SpawnFailed,
            false,
        );
        assert_eq!(
            render(&failure, None),
            "MCP server 'missing': stdio setup failed during launch: \
             the server process could not be started"
        );
    }

    #[test]
    fn a_server_name_is_shown_escaped_and_bounded() {
        let name = format!("bad\u{1b}[31m\n{}", "n".repeat(200));
        let report = render(&exited(&name, 2), None);
        let first = report.lines().next().unwrap();
        assert!(
            first.starts_with("MCP server 'bad\\u{1b}[31m\\n"),
            "{first}"
        );
        assert!(!report.contains('\u{1b}'), "{report:?}");
        assert_eq!(report.lines().count(), 1, "{report}");
        assert!(first.len() < 200, "{first}");
    }
}

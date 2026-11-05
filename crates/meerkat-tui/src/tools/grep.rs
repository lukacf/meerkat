//! `grep`: gitignore-aware regex search over file contents.

use super::glob::{search_root, walker};
use super::{GREP, Shared, ToolFailure, ToolOutcome, looks_binary, truncate_line};
use globset::Glob;
use meerkat_core::ToolDef;
use regex::RegexBuilder;
use serde::Deserialize;
use std::fmt::Write as _;

/// Default maximum number of output lines (entries) when `head_limit` is omitted.
pub(crate) const DEFAULT_HEAD_LIMIT: usize = 250;
/// Hard cap on output size regardless of `head_limit`.
pub(crate) const MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Matched lines longer than this many characters are truncated in `content` mode.
pub(crate) const MAX_LINE_CHARS: usize = 500;

/// What `grep` reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum OutputMode {
    /// One path per matching file.
    #[default]
    FilesWithMatches,
    /// Matching lines as `path:line:text` (context lines as `path-line-text`).
    Content,
    /// Match counts per file as `path:count`.
    Count,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GrepArgs {
    #[schemars(description = "Regular expression (Rust `regex` syntax) to search for.")]
    pub pattern: String,
    #[schemars(description = "File or directory to search (default: the project root).")]
    #[serde(default)]
    pub path: Option<String>,
    #[schemars(
        description = "Only search files whose path matches this glob, e.g. `*.rs` or `src/**/*.ts`."
    )]
    #[serde(default)]
    pub glob: Option<String>,
    #[schemars(description = "Match case-insensitively (default false).")]
    #[serde(default)]
    pub case_insensitive: bool,
    #[schemars(
        description = "`files_with_matches` (default): matching file paths; `content`: matching lines as path:line:text; `count`: path:count."
    )]
    #[serde(default)]
    pub output_mode: OutputMode,
    #[schemars(description = "Lines of context before and after each match (content mode only).")]
    #[serde(default)]
    pub context: Option<u32>,
    #[schemars(description = "Maximum number of output lines (default 250).")]
    #[serde(default)]
    pub head_limit: Option<usize>,
}

pub(crate) fn definition() -> ToolDef {
    ToolDef::new(
        GREP,
        format!(
            "Search file contents with a regular expression. Respects .gitignore and skips \
             binary files. output_mode selects file paths (default), matching lines, or \
             per-file counts. Output is capped at {DEFAULT_HEAD_LIMIT} lines by default \
             (head_limit) and {} KiB.",
            MAX_OUTPUT_BYTES / 1024
        ),
        meerkat_tools::schema_for::<GrepArgs>(),
    )
}

/// Line- and byte-capped output buffer.
struct Output {
    text: String,
    lines: usize,
    line_limit: usize,
    truncated: bool,
}

impl Output {
    fn new(line_limit: usize) -> Self {
        Self {
            text: String::new(),
            lines: 0,
            line_limit,
            truncated: false,
        }
    }

    /// Append a line; returns `false` once the output is full.
    fn push(&mut self, line: &str) -> bool {
        if self.lines >= self.line_limit || self.text.len() + line.len() + 1 > MAX_OUTPUT_BYTES {
            self.truncated = true;
            return false;
        }
        self.text.push_str(line);
        self.text.push('\n');
        self.lines += 1;
        true
    }
}

pub(crate) fn run(shared: &Shared, _id: String, args: GrepArgs) -> ToolOutcome {
    let regex = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive)
        .build()
        .map_err(|err| ToolFailure::recoverable(format!("invalid regex: {err}")))?;
    let filter = args
        .glob
        .as_deref()
        .map(|glob| {
            Glob::new(glob)
                .map(|g| g.compile_matcher())
                .map_err(|err| ToolFailure::recoverable(format!("invalid glob filter: {err}")))
        })
        .transpose()?;
    let root = search_root(shared, GREP, args.path.as_deref())?;
    let context = args.context.unwrap_or(0) as usize;
    let mut out = Output::new(args.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT).max(1));
    let mut any = false;

    let mut builder = walker(&root, shared.hide_secrets);
    builder.sort_by_file_name(std::cmp::Ord::cmp);
    'files: for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some(filter) = &filter {
            let relative = path.strip_prefix(&root).unwrap_or(path);
            let name_matches = path.file_name().is_some_and(|name| filter.is_match(name));
            if !filter.is_match(relative) && !name_matches {
                continue;
            }
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if looks_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let display = shared.display_abs(path);
        let keep_going = match args.output_mode {
            OutputMode::FilesWithMatches => {
                if regex.is_match(&text) {
                    any = true;
                    out.push(&display)
                } else {
                    true
                }
            }
            OutputMode::Count => {
                let count = text.lines().filter(|line| regex.is_match(line)).count();
                if count > 0 {
                    any = true;
                    out.push(&format!("{display}:{count}"))
                } else {
                    true
                }
            }
            OutputMode::Content => {
                let found = content_matches(&text, &regex, context, &display, &mut out);
                any |= found.any;
                found.keep_going
            }
        };
        if !keep_going {
            break 'files;
        }
    }

    if !any {
        return Ok(format!("No matches for `{}`", args.pattern));
    }
    let mut text = out.text;
    if out.truncated {
        let _ = writeln!(
            text,
            "… [output truncated after {} lines; narrow the search or raise head_limit]",
            out.lines
        );
    }
    Ok(text)
}

struct ContentResult {
    any: bool,
    keep_going: bool,
}

/// Emit matching lines (with `context` surrounding lines) for one file.
fn content_matches(
    text: &str,
    regex: &regex::Regex,
    context: usize,
    display: &str,
    out: &mut Output,
) -> ContentResult {
    let lines: Vec<&str> = text.lines().collect();
    let matches: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line))
        .map(|(idx, _)| idx)
        .collect();
    if matches.is_empty() {
        return ContentResult {
            any: false,
            keep_going: true,
        };
    }

    // Merge overlapping context windows into contiguous ranges.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &idx in &matches {
        let start = idx.saturating_sub(context);
        let end = (idx + context).min(lines.len() - 1);
        match ranges.last_mut() {
            Some((_, last_end)) if start <= *last_end + 1 => *last_end = (*last_end).max(end),
            _ => ranges.push((start, end)),
        }
    }

    let mut match_iter = matches.iter().peekable();
    for (n, &(start, end)) in ranges.iter().enumerate() {
        if context > 0 && n > 0 && !out.push("--") {
            return ContentResult {
                any: true,
                keep_going: false,
            };
        }
        for (idx, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            let is_match = match_iter.peek().is_some_and(|&&m| m == idx);
            if is_match {
                match_iter.next();
            }
            let sep = if is_match { ':' } else { '-' };
            let rendered = format!(
                "{display}{sep}{}{sep}{}",
                idx + 1,
                truncate_line(line, MAX_LINE_CHARS)
            );
            if !out.push(&rendered) {
                return ContentResult {
                    any: true,
                    keep_going: false,
                };
            }
        }
    }
    ContentResult {
        any: true,
        keep_going: true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn render(text: &str, pattern: &str, context: usize) -> String {
        let regex = regex::Regex::new(pattern).unwrap();
        let mut out = Output::new(100);
        content_matches(text, &regex, context, "f", &mut out);
        out.text
    }

    #[test]
    fn content_without_context() {
        assert_eq!(render("a\nb\na\n", "a", 0), "f:1:a\nf:3:a\n");
    }

    #[test]
    fn content_with_context_merges_and_separates() {
        let text = "x\nhit\ny\nz\nw\nv\nhit\n";
        let out = render(text, "hit", 1);
        assert_eq!(out, "f-1-x\nf:2:hit\nf-3-y\n--\nf-6-v\nf:7:hit\n");
    }

    #[test]
    fn output_caps_lines() {
        let mut out = Output::new(2);
        assert!(out.push("a"));
        assert!(out.push("b"));
        assert!(!out.push("c"));
        assert!(out.truncated);
    }

    #[test]
    fn output_mode_deserializes_snake_case() {
        let mode: OutputMode = serde_json::from_str("\"files_with_matches\"").unwrap();
        assert_eq!(mode, OutputMode::FilesWithMatches);
        let mode: Result<OutputMode, _> = serde_json::from_str("\"content\"");
        assert!(matches!(mode, Ok(OutputMode::Content)));
    }
}

//! `read_file`: numbered, windowed file reads that record the file's mtime.

use super::{READ_FILE, Shared, ToolFailure, ToolOutcome, looks_binary, truncate_line};
use meerkat_core::ToolDef;
use serde::Deserialize;
use std::fmt::Write as _;

/// Default number of lines returned when `limit` is omitted.
pub(crate) const DEFAULT_LIMIT: usize = 2000;
/// Lines longer than this many characters are truncated.
pub(crate) const MAX_LINE_CHARS: usize = 2000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadFileArgs {
    #[schemars(
        description = "Path to the file, absolute or relative to the project root. `~/` expands to the home directory."
    )]
    pub path: String,
    #[schemars(description = "1-based line number to start reading from (default 1).")]
    #[serde(default)]
    pub offset: Option<u64>,
    #[schemars(description = "Maximum number of lines to return (default 2000).")]
    #[serde(default)]
    pub limit: Option<u64>,
}

pub(crate) fn definition() -> ToolDef {
    ToolDef::new(
        READ_FILE,
        format!(
            "Read a text file. Output is numbered like `cat -n` (line number, tab, content). \
             Returns at most {DEFAULT_LIMIT} lines by default; use `offset`/`limit` to page \
             through larger files. Lines longer than {MAX_LINE_CHARS} characters are truncated. \
             A file must be read before it can be modified with write_file or edit_file."
        ),
        meerkat_tools::schema_for::<ReadFileArgs>(),
    )
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

pub(crate) fn run(shared: &Shared, _id: String, args: ReadFileArgs) -> ToolOutcome {
    let resolved = shared.resolve(READ_FILE, &args.path)?;
    let display = shared.display(&resolved);

    // Take the mtime before reading: if the file changes in between, the
    // recorded mtime is older than the content and later writes are refused.
    let meta =
        std::fs::metadata(&resolved.abs).map_err(|err| ToolFailure::io("read", &display, &err))?;
    if meta.is_dir() {
        return Err(ToolFailure::recoverable(format!(
            "{display} is a directory; use glob (e.g. pattern `*` with path `{display}`) to list its contents"
        )));
    }
    let mtime = meta
        .modified()
        .map_err(|err| ToolFailure::io("read the modification time of", &display, &err))?;
    let bytes =
        std::fs::read(&resolved.abs).map_err(|err| ToolFailure::io("read", &display, &err))?;
    if looks_binary(&bytes) {
        return Err(ToolFailure::recoverable(format!(
            "{display} is a binary file; read_file only reads text"
        )));
    }
    shared.record_read(resolved.abs, mtime);

    let text = String::from_utf8_lossy(&bytes);
    Ok(render(
        &text,
        to_usize(args.offset.unwrap_or(1)).max(1),
        to_usize(args.limit.unwrap_or(DEFAULT_LIMIT as u64)).max(1),
    ))
}

/// Render `text` as numbered lines starting at 1-based `offset`.
fn render(text: &str, offset: usize, limit: usize) -> String {
    if text.is_empty() {
        return "(empty file)".to_string();
    }
    let total = text.lines().count();
    if offset > total {
        return format!("(file has {total} lines; offset {offset} is past the end)");
    }
    let mut out = String::new();
    let mut last = offset;
    for (idx, line) in text.lines().enumerate().skip(offset - 1).take(limit) {
        let number = idx + 1;
        last = number;
        let _ = writeln!(out, "{number:>6}\t{}", truncate_line(line, MAX_LINE_CHARS));
    }
    if last < total {
        let _ = writeln!(
            out,
            "… [showing lines {offset}-{last} of {total}; use offset={} to continue]",
            last + 1
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_lines_from_one() {
        assert_eq!(render("a\nb\n", 1, 10), "     1\ta\n     2\tb\n");
    }

    #[test]
    fn windows_and_notes_remaining() {
        let out = render("a\nb\nc\nd\n", 2, 2);
        assert!(out.starts_with("     2\tb\n     3\tc\n"));
        assert!(out.contains("showing lines 2-3 of 4; use offset=4"));
    }

    #[test]
    fn offset_past_end() {
        assert!(render("a\n", 5, 10).contains("past the end"));
    }

    #[test]
    fn empty_file() {
        assert_eq!(render("", 1, 10), "(empty file)");
    }
}

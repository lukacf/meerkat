//! Mouse text selection over the laid-out transcript, and clipboard encoding.
//!
//! Pure: positions are in transcript *logical line* coordinates (the index of a
//! laid-out line in the whole transcript at the current width) and display
//! columns, so a selection survives scrolling. Highlighting post-processes the
//! visible lines only; the layout cache is never touched.
//!
//! Copying uses OSC 52 ([`osc52`]), which most modern terminals honour (kitty,
//! WezTerm, foot, Alacritty, iTerm2, Windows Terminal, recent xterm with
//! `allowWindowOps`); inside tmux the sequence is wrapped in a DCS passthrough
//! and tmux needs `set -g set-clipboard on`. VTE-based terminals (GNOME
//! Terminal and others) ignore it. Whether the terminal applied the sequence
//! cannot be observed, so the copy notice ([`copy_notice`]) says so.

use ratatui::style::{Modifier, Style};

use super::transcript::NoticeLevel;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Style patched onto selected cells.
pub const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);

/// Maximum OSC 52 payload (base64 bytes); longer text is truncated.
pub const CLIPBOARD_CAP: usize = 100 * 1024;

/// A cell of the laid-out transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CellPos {
    /// Transcript logical line index.
    pub line: usize,
    /// Display column within the line.
    pub col: usize,
}

impl CellPos {
    /// A cell position.
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// Selection granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectMode {
    /// Cell by cell (drag).
    #[default]
    Char,
    /// Whole words (double click).
    Word,
    /// Whole lines (triple click).
    Line,
}

/// A selection between an anchor (where it started) and a head (where the
/// pointer is). In word and line mode both ends cover whole units: `anchor`/
/// `head` are a unit's first cell and the private `*_end` its last cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// First cell of the unit where the selection started.
    pub anchor: CellPos,
    /// First cell of the unit under the pointer.
    pub head: CellPos,
    /// Granularity.
    pub mode: SelectMode,
    anchor_end: CellPos,
    head_end: CellPos,
}

/// One grapheme-ish cell group: a char plus the zero-width chars after it.
struct Cell<'a> {
    col: usize,
    width: usize,
    text: &'a str,
}

/// Split `s` (starting at display column `col`) into cell groups.
fn cells(s: &str, mut col: usize) -> Vec<Cell<'_>> {
    let mut out: Vec<Cell<'_>> = Vec::new();
    let mut start = 0;
    let mut width = 0;
    let mut open = false;
    for (i, c) in s.char_indices() {
        let w = c.width().unwrap_or(0);
        if w == 0 && open {
            continue;
        }
        if open {
            out.push(Cell {
                col,
                width,
                text: s.get(start..i).unwrap_or(""),
            });
            col += width;
        }
        start = i;
        width = w;
        open = true;
    }
    if open {
        out.push(Cell {
            col,
            width,
            text: s.get(start..).unwrap_or(""),
        });
    }
    out
}

/// Whether `text` belongs to a word for double-click selection.
fn is_word(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '/' | '-' | '~' | ':'))
}

/// The cells of a whole line, columns counted across spans.
fn line_cells<'a>(line: &'a Line<'_>) -> Vec<Cell<'a>> {
    let mut out = Vec::new();
    let mut col = 0;
    for span in &line.spans {
        let part = cells(&span.content, col);
        col += part.iter().map(|c| c.width).sum::<usize>();
        out.extend(part);
    }
    out
}

/// First and last column of the word (or single cell) at `col` in `line`.
fn word_bounds(line: &Line<'_>, col: usize) -> (usize, usize) {
    let cells = line_cells(line);
    let Some(i) = cells.iter().position(|c| col < c.col + c.width.max(1)) else {
        return (col, col);
    };
    let Some(hit) = cells.get(i) else {
        return (col, col);
    };
    if !is_word(hit.text) {
        return (hit.col, hit.col + hit.width.max(1) - 1);
    }
    let mut lo = i;
    while lo > 0 && cells.get(lo - 1).is_some_and(|c| is_word(c.text)) {
        lo -= 1;
    }
    let mut hi = i;
    while cells.get(hi + 1).is_some_and(|c| is_word(c.text)) {
        hi += 1;
    }
    let start = cells.get(lo).map_or(col, |c| c.col);
    let end = cells.get(hi).map_or(col, |c| c.col + c.width.max(1) - 1);
    (start, end)
}

/// The unit (first cell, last cell) containing `pos` in `mode`.
fn unit(pos: CellPos, mode: SelectMode, line: Option<&Line<'_>>) -> (CellPos, CellPos) {
    match mode {
        SelectMode::Char => (pos, pos),
        SelectMode::Line => (
            CellPos::new(pos.line, 0),
            CellPos::new(pos.line, usize::MAX),
        ),
        SelectMode::Word => {
            let (lo, hi) = line.map_or((pos.col, pos.col), |l| word_bounds(l, pos.col));
            (CellPos::new(pos.line, lo), CellPos::new(pos.line, hi))
        }
    }
}

impl Selection {
    /// Start a selection at `pos`. `line` is the laid-out line at `pos.line`
    /// (needed for word mode).
    pub fn new(pos: CellPos, mode: SelectMode, line: Option<&Line<'_>>) -> Self {
        let (lo, hi) = unit(pos, mode, line);
        Self {
            anchor: lo,
            head: lo,
            mode,
            anchor_end: hi,
            head_end: hi,
        }
    }

    /// Move the head to `pos` (expanded to its unit; `line` is the laid-out
    /// line at `pos.line`).
    pub fn extend(&mut self, pos: CellPos, line: Option<&Line<'_>>) {
        let (lo, hi) = unit(pos, self.mode, line);
        self.head = lo;
        self.head_end = hi;
    }

    /// First and last selected cell (inclusive), in order.
    pub fn range(&self) -> (CellPos, CellPos) {
        (
            self.anchor.min(self.head),
            self.anchor_end.max(self.head_end),
        )
    }

    /// Whether the cell at (`line`, `col`) is selected.
    pub fn contains(&self, line: usize, col: usize) -> bool {
        let (start, end) = self.range();
        let pos = CellPos::new(line, col);
        start <= pos && pos <= end
    }

    /// Whether a cell group spanning `[col, col + width)` on `line` overlaps
    /// the selection (wide chars are selected when any of their cells is).
    fn covers(&self, line: usize, col: usize, width: usize) -> bool {
        let (start, end) = self.range();
        let last = CellPos::new(line, col + width.max(1) - 1);
        last >= start && CellPos::new(line, col) <= end
    }

    /// The selected text; `lines[i]` is logical line `i`.
    pub fn extract(&self, lines: &[Line<'_>]) -> String {
        self.extract_window(0, lines)
    }

    /// The selected text; `lines[i]` is logical line `first_line + i`
    /// (lines outside the window are skipped). Each line's selected cells are
    /// joined with `\n`, trailing whitespace trimmed.
    pub fn extract_window(&self, first_line: usize, lines: &[Line<'_>]) -> String {
        let (start, end) = self.range();
        let mut out: Vec<String> = Vec::new();
        for idx in start.line..=end.line {
            let Some(line) = idx.checked_sub(first_line).and_then(|i| lines.get(i)) else {
                continue;
            };
            let text: String = line_cells(line)
                .into_iter()
                .filter(|c| self.covers(idx, c.col, c.width))
                .map(|c| c.text)
                .collect();
            out.push(text.trim_end().to_string());
        }
        out.join("\n")
    }

    /// `line` (logical line `idx`) with the selected cells patched with
    /// [`SELECTED`].
    pub fn highlight(&self, idx: usize, line: Line<'static>) -> Line<'static> {
        let (start, end) = self.range();
        if idx < start.line || idx > end.line {
            return line;
        }
        let Line {
            style,
            alignment,
            spans,
        } = line;
        let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len() + 2);
        let mut col = 0;
        for span in spans {
            let mut run = String::new();
            let mut run_selected = false;
            for cell in cells(&span.content, col) {
                let selected = self.covers(idx, cell.col, cell.width);
                if selected != run_selected && !run.is_empty() {
                    out.push(styled(std::mem::take(&mut run), span.style, run_selected));
                }
                run_selected = selected;
                run.push_str(cell.text);
                col += cell.width;
            }
            if !run.is_empty() {
                out.push(styled(run, span.style, run_selected));
            }
        }
        Line {
            style,
            alignment,
            spans: out,
        }
    }
}

fn styled(text: String, style: Style, selected: bool) -> Span<'static> {
    if selected {
        Span::styled(text, style.patch(SELECTED))
    } else {
        Span::styled(text, style)
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_char(sextet: u32) -> char {
    B64.get((sextet & 0x3f) as usize)
        .map_or('A', |b| char::from(*b))
}

/// Standard base64 (RFC 4648, padded).
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk.first().copied().unwrap_or(0));
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(b64_char(n >> 18));
        out.push(b64_char(n >> 12));
        out.push(if chunk.len() > 1 {
            b64_char(n >> 6)
        } else {
            '='
        });
        out.push(if chunk.len() > 2 { b64_char(n) } else { '=' });
    }
    out
}

/// `text` cut (at a char boundary) so its base64 fits in [`CLIPBOARD_CAP`];
/// the flag says whether anything was cut.
pub fn cap_clipboard(text: &str) -> (&str, bool) {
    let max = CLIPBOARD_CAP / 4 * 3;
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text.get(..end).unwrap_or(""), true)
}

/// The OSC 52 "set clipboard" sequence for `text` (callers cap it first with
/// [`cap_clipboard`]), for the clipboard and the primary selection (`cp`:
/// terminals ignore selection letters they do not support). With `tmux`,
/// wrapped in a DCS passthrough (every ESC doubled) so tmux forwards it to
/// the outer terminal.
pub fn osc52(text: &str, tmux: bool) -> String {
    let seq = format!("\x1b]52;cp;{}\x07", base64_encode(text.as_bytes()));
    if tmux {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    }
}

/// The notice for a copy of `chars` characters (`truncated`: cut to the
/// clipboard cap) whose OSC 52 write had `written` as its result.
pub fn copy_notice(
    chars: usize,
    truncated: bool,
    written: Result<&(), &std::io::Error>,
) -> (NoticeLevel, String) {
    match written {
        Ok(()) if truncated => (
            NoticeLevel::Info,
            format!("copied {chars} chars (truncated to 100 KiB; terminal must support OSC 52)"),
        ),
        Ok(()) => (
            NoticeLevel::Info,
            format!("copied {chars} chars (terminal must support OSC 52)"),
        ),
        Err(err) => (NoticeLevel::Warn, format!("copy failed: {err}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> CellPos {
        CellPos::new(line, col)
    }

    fn drag(a: CellPos, b: CellPos) -> Selection {
        let mut s = Selection::new(a, SelectMode::Char, None);
        s.extend(b, None);
        s
    }

    #[test]
    fn range_is_ordered_and_contains_is_inclusive() {
        let s = drag(p(3, 4), p(1, 7));
        assert_eq!(s.range(), (p(1, 7), p(3, 4)));
        assert!(s.contains(1, 7));
        assert!(!s.contains(1, 6));
        assert!(s.contains(2, 0));
        assert!(s.contains(2, 999));
        assert!(s.contains(3, 4));
        assert!(!s.contains(3, 5));
        assert!(!s.contains(0, 10));
        let fwd = drag(p(1, 7), p(3, 4));
        assert_eq!(fwd.range(), s.range());
    }

    #[test]
    fn extract_across_lines_trims_trailing_whitespace() {
        let lines = vec![
            Line::from("hello world   "),
            Line::from(vec![Span::raw("foo "), Span::raw("bar  ")]),
            Line::from("tail"),
        ];
        let s = drag(p(0, 6), p(2, 1));
        assert_eq!(s.extract(&lines), "world\nfoo bar\nta");
        // A window offset maps logical lines to slice indices.
        assert_eq!(s.extract_window(1, &lines[1..]), "foo bar\nta");
    }

    #[test]
    fn extract_honours_wide_chars() {
        // 日 and 本 are two columns each; the emoji too.
        let lines = vec![Line::from("a日本🦀b")];
        // Column 2 is the second half of 日: the whole char is selected.
        assert_eq!(drag(p(0, 2), p(0, 3)).extract(&lines), "日本");
        assert_eq!(drag(p(0, 5), p(0, 7)).extract(&lines), "🦀b");
        assert_eq!(drag(p(0, 0), p(0, 0)).extract(&lines), "a");
        // Combining marks stay with their base char.
        let lines = vec![Line::from("e\u{301}x")];
        assert_eq!(drag(p(0, 0), p(0, 0)).extract(&lines), "e\u{301}");
        assert_eq!(drag(p(0, 1), p(0, 1)).extract(&lines), "x");
    }

    #[test]
    fn word_mode_expands_over_path_like_runs() {
        let lines = vec![
            Line::from(vec![Span::raw("open "), Span::raw("src/ui/mod.rs now")]),
            Line::from("next line here"),
        ];
        let s = Selection::new(p(0, 9), SelectMode::Word, lines.first());
        assert_eq!(s.extract(&lines), "src/ui/mod.rs");
        // A non-word cell selects just itself.
        let s = Selection::new(p(0, 4), SelectMode::Word, lines.first());
        assert_eq!(s.range(), (p(0, 4), p(0, 4)));
        // Extending backwards keeps the anchor word whole.
        let mut s = Selection::new(p(1, 6), SelectMode::Word, lines.get(1));
        s.extend(p(0, 1), lines.first());
        assert_eq!(s.extract(&lines), "open src/ui/mod.rs now\nnext line");
    }

    #[test]
    fn line_mode_takes_whole_lines() {
        let lines = vec![Line::from("one  "), Line::from("two"), Line::from("3")];
        let mut s = Selection::new(p(0, 2), SelectMode::Line, None);
        assert_eq!(s.extract(&lines), "one");
        s.extend(p(1, 0), None);
        assert_eq!(s.extract(&lines), "one\ntwo");
    }

    #[test]
    fn highlight_reverses_only_selected_cells() {
        let line = Line::from(vec![Span::raw("ab"), Span::raw("日cd")]);
        let s = drag(p(0, 1), p(0, 2));
        let out = s.highlight(0, line.clone());
        let parts: Vec<(String, bool)> = out
            .spans
            .iter()
            .map(|sp| {
                (
                    sp.content.to_string(),
                    sp.style.add_modifier.contains(Modifier::REVERSED),
                )
            })
            .collect();
        assert_eq!(
            parts,
            vec![
                ("a".into(), false),
                ("b".into(), true),
                ("日".into(), true),
                ("cd".into(), false),
            ]
        );
        // Lines outside the range are untouched.
        assert_eq!(s.highlight(1, line.clone()), line);
    }

    #[test]
    fn base64_known_vectors() {
        for (input, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(input.as_bytes()), want);
        }
        assert_eq!(base64_encode(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn copy_notice_wording() {
        assert_eq!(
            copy_notice(21, false, Ok(&())),
            (
                NoticeLevel::Info,
                "copied 21 chars (terminal must support OSC 52)".into()
            )
        );
        assert_eq!(
            copy_notice(21, true, Ok(&())),
            (
                NoticeLevel::Info,
                "copied 21 chars (truncated to 100 KiB; terminal must support OSC 52)".into()
            )
        );
        let err = std::io::Error::other("broken pipe");
        assert_eq!(
            copy_notice(21, false, Err(&err)),
            (NoticeLevel::Warn, "copy failed: broken pipe".into())
        );
    }

    #[test]
    fn osc52_plain_and_tmux() {
        assert_eq!(osc52("hi", false), "\x1b]52;cp;aGk=\x07");
        assert_eq!(osc52("hi", true), "\x1bPtmux;\x1b\x1b]52;cp;aGk=\x07\x1b\\");
    }

    #[test]
    fn payload_is_capped_at_a_char_boundary() {
        let short = "abc";
        assert_eq!(cap_clipboard(short), ("abc", false));
        let long = "é".repeat(CLIPBOARD_CAP);
        let (cut, truncated) = cap_clipboard(&long);
        assert!(truncated);
        assert!(base64_encode(cut.as_bytes()).len() <= CLIPBOARD_CAP);
        assert!(cut.chars().all(|c| c == 'é'));
    }
}

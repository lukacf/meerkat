//! Multi-line prompt editor with history.
//!
//! The cursor column is a char index into the current line; display columns are
//! computed with `unicode-width` when rendering. Logical lines soft-wrap at the
//! field width ([`wrap_layout`]); wrapping is layout only, the stored lines are
//! never split.

use super::markdown::{display_width, sanitize};

/// A multi-line text editor with prompt history.
#[derive(Debug, Clone)]
pub struct InputEditor {
    lines: Vec<String>,
    row: usize,
    col: usize,
    history: Vec<String>,
    history_pos: Option<usize>,
    draft: Option<String>,
    /// Columns a visual row holds (set by the renderer; 0 = do not wrap).
    wrap_width: usize,
}

impl Default for InputEditor {
    fn default() -> Self {
        Self::new()
    }
}

fn byte_index(line: &str, col: usize) -> usize {
    line.char_indices().nth(col).map_or(line.len(), |(b, _)| b)
}

fn char_len(line: &str) -> usize {
    line.chars().count()
}

/// Display columns one char of a stored line takes once sanitized for the
/// screen (a tab expands, a control char becomes a replacement glyph).
fn cell_width(ch: char) -> usize {
    let mut buf = [0u8; 4];
    display_width(&sanitize(ch.encode_utf8(&mut buf)))
}

/// One visual row of the wrapped input: chars `start..end` of logical line
/// `line`, `width` display columns wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualRow {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    pub width: usize,
}

/// The input's logical lines soft-wrapped into visual rows (see [`wrap_layout`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapLayout {
    rows: Vec<VisualRow>,
}

/// Wrap `lines` at `width` display columns, at char boundaries (no word wrap).
///
/// A wide char never splits: it starts a new row when it does not fit. A
/// zero-width char stays on the row of the char before it. A line of width 0
/// still takes one row, and a line whose last row is exactly full gets an
/// empty row after it, where the cursor at its end sits. `width == 0` means
/// no wrapping (one row per line).
pub fn wrap_layout(lines: &[String], width: usize) -> WrapLayout {
    let limit = if width == 0 { usize::MAX } else { width };
    let mut rows = Vec::with_capacity(lines.len());
    for (line, text) in lines.iter().enumerate() {
        let (mut start, mut used, mut idx) = (0, 0usize, 0);
        for ch in text.chars() {
            let w = cell_width(ch);
            if w > 0 && used > 0 && used.saturating_add(w) > limit {
                rows.push(VisualRow {
                    line,
                    start,
                    end: idx,
                    width: used,
                });
                start = idx;
                used = 0;
            }
            used += w;
            idx += 1;
        }
        rows.push(VisualRow {
            line,
            start,
            end: idx,
            width: used,
        });
        if idx > start && used >= limit {
            rows.push(VisualRow {
                line,
                start: idx,
                end: idx,
                width: 0,
            });
        }
    }
    if rows.is_empty() {
        rows.push(VisualRow {
            line: 0,
            start: 0,
            end: 0,
            width: 0,
        });
    }
    WrapLayout { rows }
}

impl WrapLayout {
    /// The visual rows, top to bottom.
    pub fn rows(&self) -> &[VisualRow] {
        &self.rows
    }

    /// Number of visual rows (at least 1).
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Always false: a layout has at least one row.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The visual `(row, display column)` of logical `(row, char column)`.
    /// A position at a row boundary belongs to the later row.
    pub fn to_visual(&self, lines: &[String], row: usize, col: usize) -> (usize, usize) {
        let mut found = None;
        for (i, r) in self.rows.iter().enumerate() {
            if r.line > row {
                break;
            }
            if r.line == row && r.start <= col {
                found = Some(i);
            }
        }
        let Some(i) = found else {
            return (0, 0);
        };
        let start = self.rows.get(i).map_or(0, |r| r.start);
        let text = lines.get(row).map_or("", String::as_str);
        let vcol = text
            .chars()
            .skip(start)
            .take(col.saturating_sub(start))
            .map(cell_width)
            .sum();
        (i, vcol)
    }

    /// The logical `(row, char column)` nearest visual `(vrow, vcol)`: the
    /// last char boundary of the row at or left of `vcol`. Only the last row
    /// of a line can hold the position after its last char (elsewhere that
    /// position is the next row's start). `None` past the last row.
    pub fn from_visual(&self, lines: &[String], vrow: usize, vcol: usize) -> Option<(usize, usize)> {
        let r = *self.rows.get(vrow)?;
        let text = lines.get(r.line).map_or("", String::as_str);
        let (mut col, mut used) = (r.start, 0usize);
        for ch in text.chars().skip(r.start).take(r.end - r.start) {
            used += cell_width(ch);
            if used > vcol {
                break;
            }
            col += 1;
        }
        let last_of_line = self.rows.get(vrow + 1).is_none_or(|n| n.line != r.line);
        if !last_of_line && col >= r.end {
            col = r.end.saturating_sub(1).max(r.start);
        }
        Some((r.line, col))
    }
}

impl InputEditor {
    /// An empty editor with no history.
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            history: Vec::new(),
            history_pos: None,
            draft: None,
            wrap_width: 0,
        }
    }

    /// Set the columns a visual row holds (0 = do not wrap). The renderer
    /// sets it from the field width each frame; Up/Down move by visual rows.
    pub fn set_wrap_width(&mut self, width: usize) {
        self.wrap_width = width;
    }

    /// The current lines wrapped at the wrap width.
    pub fn layout(&self) -> WrapLayout {
        wrap_layout(&self.lines, self.wrap_width)
    }

    /// The cursor's visual `(row, display column)` in `layout`.
    pub fn cursor_visual(&self, layout: &WrapLayout) -> (usize, usize) {
        layout.to_visual(&self.lines, self.row, self.col)
    }

    /// Seed the history (oldest first).
    pub fn with_history(mut self, history: Vec<String>) -> Self {
        self.history = history;
        self
    }

    /// Submitted prompts, oldest first.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Current lines.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Cursor as `(row, char column)`.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    /// Full text joined with `\n`.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether the editor holds no text at all.
    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines.first().is_none_or(String::is_empty)
    }

    /// Remove all text (history is kept).
    pub fn clear(&mut self) {
        self.lines = vec![String::new()];
        self.row = 0;
        self.col = 0;
        self.history_pos = None;
        self.draft = None;
    }

    /// Replace the text and put the cursor at the end.
    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.row = self.lines.len() - 1;
        self.col = char_len(self.current());
    }

    /// Take the text for submission: clears the editor and records non-blank text in history.
    pub fn take(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        self.clear();
        text
    }

    fn current(&self) -> &str {
        self.lines.get(self.row).map_or("", String::as_str)
    }

    fn current_mut(&mut self) -> &mut String {
        if self.row >= self.lines.len() {
            self.lines.resize(self.row + 1, String::new());
        }
        &mut self.lines[self.row]
    }

    /// Insert one character at the cursor (`\n` splits the line).
    pub fn insert_char(&mut self, ch: char) {
        if ch == '\n' {
            self.newline();
            return;
        }
        let col = self.col;
        let line = self.current_mut();
        let at = byte_index(line, col);
        line.insert(at, ch);
        self.col += 1;
    }

    /// Insert text verbatim (bracketed paste). `\r\n` and `\r` become newlines.
    pub fn insert_str(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for ch in normalized.chars() {
            self.insert_char(ch);
        }
    }

    /// Split the line at the cursor.
    pub fn newline(&mut self) {
        let col = self.col;
        let line = self.current_mut();
        let at = byte_index(line, col);
        let rest = line.split_off(at);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    /// Delete the character before the cursor (joining lines at column 0).
    pub fn backspace(&mut self) {
        if self.col > 0 {
            let col = self.col;
            let line = self.current_mut();
            let at = byte_index(line, col - 1);
            line.remove(at);
            self.col -= 1;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = char_len(self.current());
            self.current_mut().push_str(&line);
        }
    }

    /// Delete the character under the cursor (joining lines at end of line).
    pub fn delete(&mut self) {
        let col = self.col;
        if col < char_len(self.current()) {
            let line = self.current_mut();
            let at = byte_index(line, col);
            line.remove(at);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.current_mut().push_str(&next);
        }
    }

    /// Move left one char (wrapping to the previous line).
    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = char_len(self.current());
        }
    }

    /// Move right one char (wrapping to the next line).
    pub fn right(&mut self) {
        if self.col < char_len(self.current()) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// Start of the current line.
    pub fn home(&mut self) {
        self.col = 0;
    }

    /// End of the current line.
    pub fn end(&mut self) {
        self.col = char_len(self.current());
    }

    /// Jump to the start of the previous word.
    pub fn word_left(&mut self) {
        if self.col == 0 {
            self.left();
            return;
        }
        let chars: Vec<char> = self.current().chars().collect();
        let mut c = self.col;
        while c > 0 && chars.get(c - 1).is_some_and(|ch| ch.is_whitespace()) {
            c -= 1;
        }
        while c > 0 && chars.get(c - 1).is_some_and(|ch| !ch.is_whitespace()) {
            c -= 1;
        }
        self.col = c;
    }

    /// Jump past the end of the next word.
    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.current().chars().collect();
        if self.col >= chars.len() {
            self.right();
            return;
        }
        let mut c = self.col;
        while c < chars.len() && chars[c].is_whitespace() {
            c += 1;
        }
        while c < chars.len() && !chars[c].is_whitespace() {
            c += 1;
        }
        self.col = c;
    }

    /// Delete the word before the cursor (Ctrl-W).
    pub fn delete_word_back(&mut self) {
        let end = self.col;
        self.word_left();
        if self.col < end && self.row < self.lines.len() {
            let start = self.col;
            let line = self.current_mut();
            let (a, b) = (byte_index(line, start), byte_index(line, end));
            line.replace_range(a..b, "");
        }
    }

    /// Delete from line start to the cursor (Ctrl-U).
    pub fn kill_to_line_start(&mut self) {
        let col = self.col;
        let line = self.current_mut();
        let at = byte_index(line, col);
        line.replace_range(..at, "");
        self.col = 0;
    }

    /// Move the cursor to visual row `vrow` at display column `vcol` (clamped).
    fn move_to_visual(&mut self, layout: &WrapLayout, vrow: usize, vcol: usize) {
        if let Some((row, col)) = layout.from_visual(&self.lines, vrow, vcol) {
            self.row = row;
            self.col = col;
        }
    }

    /// Up arrow: move up a visual row, or recall older history from the
    /// first visual row.
    pub fn up(&mut self) {
        let layout = self.layout();
        let (vrow, vcol) = self.cursor_visual(&layout);
        if vrow > 0 {
            self.move_to_visual(&layout, vrow - 1, vcol);
            return;
        }
        let next = match self.history_pos {
            None if self.history.is_empty() => return,
            None => {
                self.draft = Some(self.text());
                self.history.len() - 1
            }
            Some(0) => return,
            Some(p) => p - 1,
        };
        self.history_pos = Some(next);
        let entry = self.history.get(next).cloned().unwrap_or_default();
        self.set_text(&entry);
    }

    /// Down arrow: move down a visual row, or recall newer history from the
    /// last visual row.
    pub fn down(&mut self) {
        let layout = self.layout();
        let (vrow, vcol) = self.cursor_visual(&layout);
        if vrow + 1 < layout.len() {
            self.move_to_visual(&layout, vrow + 1, vcol);
            return;
        }
        let Some(p) = self.history_pos else {
            return;
        };
        if p + 1 < self.history.len() {
            self.history_pos = Some(p + 1);
            let entry = self.history.get(p + 1).cloned().unwrap_or_default();
            self.set_text(&entry);
        } else {
            self.history_pos = None;
            let draft = self.draft.take().unwrap_or_default();
            self.set_text(&draft);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn typed(s: &str) -> InputEditor {
        let mut e = InputEditor::new();
        for ch in s.chars() {
            e.insert_char(ch);
        }
        e
    }

    #[test]
    fn insert_and_backspace_unicode() {
        let mut e = typed("héllo");
        e.left();
        e.left();
        e.backspace();
        assert_eq!(e.text(), "hélo");
        assert_eq!(e.cursor(), (0, 2));
        e.delete();
        assert_eq!(e.text(), "héo");
    }

    #[test]
    fn newline_splits_and_backspace_joins() {
        let mut e = typed("abcd");
        e.left();
        e.left();
        e.newline();
        assert_eq!(e.lines(), &["ab".to_string(), "cd".to_string()]);
        assert_eq!(e.cursor(), (1, 0));
        e.backspace();
        assert_eq!(e.text(), "abcd");
        assert_eq!(e.cursor(), (0, 2));
    }

    #[test]
    fn paste_is_verbatim_with_crlf_normalized() {
        let mut e = InputEditor::new();
        e.insert_str("a\r\nb\tc");
        assert_eq!(e.text(), "a\nb\tc");
        assert_eq!(e.cursor(), (1, 3));
    }

    #[test]
    fn word_jumps_and_delete_word() {
        let mut e = typed("foo bar  baz");
        e.word_left();
        assert_eq!(e.cursor(), (0, 9));
        e.word_left();
        assert_eq!(e.cursor(), (0, 4));
        e.word_right();
        assert_eq!(e.cursor(), (0, 7));
        e.end();
        e.delete_word_back();
        assert_eq!(e.text(), "foo bar  ");
        e.home();
        assert_eq!(e.cursor(), (0, 0));
    }

    #[test]
    fn history_navigation_restores_draft() {
        let mut e = InputEditor::new().with_history(vec!["one".into(), "two".into()]);
        e.insert_str("dra");
        e.up();
        assert_eq!(e.text(), "two");
        e.up();
        assert_eq!(e.text(), "one");
        e.up();
        assert_eq!(e.text(), "one");
        e.down();
        assert_eq!(e.text(), "two");
        e.down();
        assert_eq!(e.text(), "dra");
    }

    #[test]
    fn up_moves_within_multiline_before_history() {
        let mut e = InputEditor::new().with_history(vec!["old".into()]);
        e.insert_str("a\nb");
        e.up();
        assert_eq!(e.cursor(), (0, 1));
        assert_eq!(e.text(), "a\nb");
        e.up();
        assert_eq!(e.text(), "old");
    }

    #[test]
    fn take_records_history() {
        let mut e = typed("hi");
        assert_eq!(e.take(), "hi");
        assert!(e.is_empty());
        assert_eq!(e.history(), &["hi".to_string()]);
        e.insert_str("   ");
        e.take();
        assert_eq!(e.history().len(), 1);
    }

    fn lines(texts: &[&str]) -> Vec<String> {
        texts.iter().map(|t| (*t).to_string()).collect()
    }

    fn spans(layout: &WrapLayout) -> Vec<(usize, usize, usize)> {
        layout
            .rows()
            .iter()
            .map(|r| (r.line, r.start, r.end))
            .collect()
    }

    #[test]
    fn wrap_layout_splits_a_long_line_at_the_width() {
        let ls = lines(&["abcdefghijklmnopqrstuvwxy"]);
        let layout = wrap_layout(&ls, 10);
        assert_eq!(spans(&layout), vec![(0, 0, 10), (0, 10, 20), (0, 20, 25)]);
        assert_eq!(
            layout.rows().iter().map(|r| r.width).collect::<Vec<_>>(),
            vec![10, 10, 5]
        );
        assert_eq!(layout.to_visual(&ls, 0, 25), (2, 5));
        assert_eq!(layout.to_visual(&ls, 0, 3), (0, 3));
    }

    #[test]
    fn wrap_layout_never_splits_a_wide_char() {
        // 4 narrow + 3 wide (2 columns each) = 10 columns at width 5.
        let ls = lines(&["abcd日本語"]);
        let layout = wrap_layout(&ls, 5);
        assert_eq!(spans(&layout), vec![(0, 0, 4), (0, 4, 6), (0, 6, 7)]);
        assert!(layout.rows().iter().all(|r| r.width <= 5));
        assert_eq!(layout.to_visual(&ls, 0, 5), (1, 2));
        // A zero-width joiner stays on the row of the char before it.
        let ls = lines(&["abcde\u{200D}f"]);
        assert_eq!(spans(&wrap_layout(&ls, 5)), vec![(0, 0, 6), (0, 6, 7)]);
    }

    #[test]
    fn wrap_layout_gives_an_empty_line_one_row() {
        let ls = lines(&["", "ab", ""]);
        let layout = wrap_layout(&ls, 10);
        assert_eq!(spans(&layout), vec![(0, 0, 0), (1, 0, 2), (2, 0, 0)]);
        assert_eq!(layout.to_visual(&ls, 2, 0), (2, 0));
        assert_eq!(wrap_layout(&[], 10).len(), 1);
    }

    #[test]
    fn wrap_layout_maps_row_ends_to_the_next_row() {
        let ls = lines(&["abcdefghijklmnopqrstuvwxy"]);
        let layout = wrap_layout(&ls, 10);
        // The boundary position belongs to the later row.
        assert_eq!(layout.to_visual(&ls, 0, 10), (1, 0));
        assert_eq!(layout.to_visual(&ls, 0, 20), (2, 0));
        // A column past a row's end clamps to its last char, except on the
        // line's last row.
        assert_eq!(layout.from_visual(&ls, 0, 99), Some((0, 9)));
        assert_eq!(layout.from_visual(&ls, 2, 99), Some((0, 25)));
        assert_eq!(layout.from_visual(&ls, 1, 4), Some((0, 14)));
        assert_eq!(layout.from_visual(&ls, 3, 0), None);
    }

    #[test]
    fn wrap_layout_puts_the_cursor_after_an_exactly_full_line_on_the_next_row() {
        let ls = lines(&["abcdefghij", "k"]);
        let layout = wrap_layout(&ls, 10);
        assert_eq!(spans(&layout), vec![(0, 0, 10), (0, 10, 10), (1, 0, 1)]);
        assert_eq!(layout.to_visual(&ls, 0, 10), (1, 0));
        assert_eq!(layout.from_visual(&ls, 1, 0), Some((0, 10)));
        assert_eq!(layout.from_visual(&ls, 0, 99), Some((0, 9)));
    }

    #[test]
    fn wrap_width_zero_does_not_wrap() {
        let ls = lines(&["abcdefghijklmnopqrstuvwxy"]);
        assert_eq!(spans(&wrap_layout(&ls, 0)), vec![(0, 0, 25)]);
    }

    #[test]
    fn up_and_down_move_by_visual_rows() {
        let mut e = InputEditor::new().with_history(vec!["old".into()]);
        e.set_wrap_width(10);
        e.insert_str("abcdefghijklmnopqrstuvwxy");
        e.left();
        e.left();
        assert_eq!(e.cursor(), (0, 23));
        e.up();
        assert_eq!(e.cursor(), (0, 13));
        e.up();
        assert_eq!(e.cursor(), (0, 3));
        e.down();
        assert_eq!(e.cursor(), (0, 13));
        e.up();
        e.up();
        assert_eq!(e.text(), "old");
    }
}

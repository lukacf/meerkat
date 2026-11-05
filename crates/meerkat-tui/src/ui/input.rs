//! Multi-line prompt editor with history.
//!
//! The cursor column is a char index into the current line; display columns are
//! computed with `unicode-width` when rendering.

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
        }
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

    /// Display column of the cursor within its line.
    pub fn cursor_display_col(&self) -> usize {
        let line = self.current();
        display_width(&sanitize(&line[..byte_index(line, self.col)]))
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

    /// Up arrow: move up a line, or recall older history from the first line.
    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(char_len(self.current()));
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

    /// Down arrow: move down a line, or recall newer history from the last line.
    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(char_len(self.current()));
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
}

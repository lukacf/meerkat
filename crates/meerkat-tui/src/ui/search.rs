//! Readline-style reverse incremental history search (Ctrl-R).
//!
//! Pure state: the corpus (newest first, deduplicated — see [`build_corpus`]) is
//! owned by the caller and passed into every call, so the search only stores
//! indices into it.

use std::collections::HashSet;
use std::ops::Range;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// State of an active reverse history search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySearch {
    /// Typed search string.
    pub query: String,
    /// Corpus indices of the entries matching `query`, newest first.
    matches: Vec<usize>,
    /// Position of the shown match within the matches.
    pub index: usize,
    /// Editor contents when the search started (restored on cancel).
    saved_input: String,
    /// Ctrl-R was pressed past the oldest match.
    exhausted: bool,
}

/// What the caller should do after a key press in search mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchOutcome {
    /// Keep searching.
    Continue,
    /// Leave search mode and put this text into the editor (not submitted).
    Accept(String),
    /// Leave search mode and restore this (the saved) editor text.
    Cancel(String),
}

/// Build the search corpus: workspace history, then global history, each given
/// oldest first; the result is newest first with later duplicates dropped.
pub fn build_corpus(workspace: &[String], global: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    workspace
        .iter()
        .rev()
        .chain(global.iter().rev())
        .filter(|e| seen.insert(e.as_str()))
        .cloned()
        .collect()
}

/// Find `needle` in `hay` with smart case (case-insensitive unless `needle`
/// contains an uppercase letter). Returns the byte range of the first match.
pub fn find_smart_case(hay: &str, needle: &str) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let fold = !needle.chars().any(char::is_uppercase);
    let eq = |h: char, n: char| {
        if fold {
            h.to_lowercase().eq(n.to_lowercase())
        } else {
            h == n
        }
    };
    hay.char_indices().find_map(|(start, _)| {
        let mut rest = hay[start..].char_indices();
        let mut end = start;
        for n in needle.chars() {
            match rest.next() {
                Some((off, h)) if eq(h, n) => end = start + off + h.len_utf8(),
                _ => return None,
            }
        }
        Some(start..end)
    })
}

impl HistorySearch {
    /// Start a search with an empty query; `saved_input` is restored on cancel.
    pub fn start(saved_input: String, corpus: &[String]) -> Self {
        let mut s = Self {
            query: String::new(),
            matches: Vec::new(),
            index: 0,
            saved_input,
            exhausted: false,
        };
        s.refilter(corpus);
        s
    }

    fn refilter(&mut self, corpus: &[String]) {
        self.matches = corpus
            .iter()
            .enumerate()
            .filter(|(_, e)| find_smart_case(e, &self.query).is_some())
            .map(|(i, _)| i)
            .collect();
        self.index = 0;
        self.exhausted = false;
    }

    /// Extend the query; the newest match is shown.
    pub fn push(&mut self, c: char, corpus: &[String]) {
        self.query.push(c);
        self.refilter(corpus);
    }

    /// Shorten the query; the newest match is shown.
    pub fn pop(&mut self, corpus: &[String]) {
        self.query.pop();
        self.refilter(corpus);
    }

    /// Step to the next older match; past the oldest the search is marked failing
    /// and stays on the oldest match.
    pub fn older(&mut self) {
        if self.index + 1 < self.matches.len() {
            self.index += 1;
        } else if !self.query.is_empty() {
            self.exhausted = true;
        }
    }

    /// The shown match, if any.
    pub fn current<'a>(&self, corpus: &'a [String]) -> Option<&'a str> {
        self.matches
            .get(self.index)
            .and_then(|&i| corpus.get(i))
            .map(String::as_str)
    }

    /// Whether the search failed (no match for a non-empty query, or Ctrl-R past
    /// the oldest match).
    pub fn failing(&self) -> bool {
        self.exhausted || (!self.query.is_empty() && self.matches.is_empty())
    }

    fn accept(&mut self, corpus: &[String]) -> SearchOutcome {
        let text = match self.current(corpus) {
            Some(m) => m.to_string(),
            None if self.query.is_empty() => std::mem::take(&mut self.saved_input),
            None => self.query.clone(),
        };
        SearchOutcome::Accept(text)
    }

    /// Handle one key press.
    pub fn key(&mut self, key: &KeyEvent, corpus: &[String]) -> SearchOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => SearchOutcome::Cancel(std::mem::take(&mut self.saved_input)),
            KeyCode::Char('g' | 'c') if ctrl => {
                SearchOutcome::Cancel(std::mem::take(&mut self.saved_input))
            }
            KeyCode::Char('r') if ctrl => {
                self.older();
                SearchOutcome::Continue
            }
            KeyCode::Enter
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End => self.accept(corpus),
            KeyCode::Backspace => {
                self.pop(corpus);
                SearchOutcome::Continue
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.push(c, corpus);
                SearchOutcome::Continue
            }
            _ => SearchOutcome::Continue,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn corpus() -> Vec<String> {
        // newest first
        ["cargo test -p tui", "git push", "Cargo build", "cargo fmt"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    fn typed(s: &str, c: &[String]) -> HistorySearch {
        let mut h = HistorySearch::start("draft".into(), c);
        for ch in s.chars() {
            h.push(ch, c);
        }
        h
    }

    fn k(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn filters_newest_first() {
        let c = corpus();
        let h = typed("cargo", &c);
        assert_eq!(h.current(&c), Some("cargo test -p tui"));
        assert!(!h.failing());
        let h = typed("push", &c);
        assert_eq!(h.current(&c), Some("git push"));
    }

    #[test]
    fn empty_query_shows_nothing_and_is_not_failing() {
        let c = corpus();
        let h = HistorySearch::start(String::new(), &c);
        assert_eq!(h.current(&c), None);
        assert!(!h.failing());
    }

    #[test]
    fn older_steps_and_stops_at_oldest() {
        let c = corpus();
        let mut h = typed("cargo", &c);
        h.older();
        assert_eq!(h.current(&c), Some("Cargo build"));
        h.older();
        assert_eq!(h.current(&c), Some("cargo fmt"));
        assert!(!h.failing());
        h.older();
        assert_eq!(h.current(&c), Some("cargo fmt"));
        assert!(h.failing());
        // Editing the query resets to the newest match and clears the failure.
        h.pop(&c);
        assert_eq!(h.current(&c), Some("cargo test -p tui"));
        assert!(!h.failing());
    }

    #[test]
    fn smart_case() {
        let c = corpus();
        let h = typed("Cargo", &c);
        assert_eq!(h.current(&c), Some("Cargo build"));
        let mut h = h;
        h.older();
        assert!(h.failing());
        assert_eq!(find_smart_case("Cargo build", "cargo"), Some(0..5));
        assert_eq!(find_smart_case("x ÄBC", "äb"), Some(2..5));
        assert_eq!(find_smart_case("cargo", "Cargo"), None);
    }

    #[test]
    fn failing_when_no_match() {
        let c = corpus();
        let mut h = typed("zzz", &c);
        assert!(h.failing());
        assert_eq!(h.current(&c), None);
        assert_eq!(
            h.key(&k(KeyCode::Enter, KeyModifiers::NONE), &c),
            SearchOutcome::Accept("zzz".into())
        );
    }

    #[test]
    fn keys_accept_and_cancel() {
        let c = corpus();
        let mut h = HistorySearch::start("draft".into(), &c);
        for ch in "gi".chars() {
            assert_eq!(
                h.key(&k(KeyCode::Char(ch), KeyModifiers::NONE), &c),
                SearchOutcome::Continue
            );
        }
        assert_eq!(
            h.key(&k(KeyCode::Right, KeyModifiers::NONE), &c),
            SearchOutcome::Accept("git push".into())
        );
        for cancel in [
            k(KeyCode::Esc, KeyModifiers::NONE),
            k(KeyCode::Char('g'), KeyModifiers::CONTROL),
            k(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let mut h = typed("git", &c);
            assert_eq!(h.key(&cancel, &c), SearchOutcome::Cancel("draft".into()));
        }
        // Plain 'g' / 'c' extend the query.
        let mut h = HistorySearch::start(String::new(), &c);
        h.key(&k(KeyCode::Char('g'), KeyModifiers::NONE), &c);
        h.key(&k(KeyCode::Char('i'), KeyModifiers::NONE), &c);
        h.key(&k(KeyCode::Backspace, KeyModifiers::NONE), &c);
        assert_eq!(h.query, "g");
        h.key(&k(KeyCode::Char('r'), KeyModifiers::CONTROL), &c);
        assert_eq!(h.index, 1);
        // Empty query accepts the saved input unchanged.
        let mut h = HistorySearch::start("draft".into(), &c);
        assert_eq!(
            h.key(&k(KeyCode::Enter, KeyModifiers::NONE), &c),
            SearchOutcome::Accept("draft".into())
        );
    }

    #[test]
    fn corpus_is_newest_first_and_deduped() {
        let ws: Vec<String> = ["a", "b", "c"].into_iter().map(String::from).collect();
        let global: Vec<String> = ["x", "c", "a", "y"].into_iter().map(String::from).collect();
        assert_eq!(build_corpus(&ws, &global), vec!["c", "b", "a", "y", "x"]);
        let ws: Vec<String> = ["a", "b", "a"].into_iter().map(String::from).collect();
        assert_eq!(build_corpus(&ws, &[]), vec!["a", "b"]);
    }
}

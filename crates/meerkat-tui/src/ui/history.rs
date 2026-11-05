//! Persistent prompt history: one entry per line, newest last.
//!
//! Entries are escaped so multi-line prompts fit on one line (`\n`, `\r` and `\\`
//! become two-character escapes). Consecutive duplicates are dropped and the file
//! is capped at [`MAX_ENTRIES`], rewritten when it grows past the cap.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Maximum number of entries kept on disk and in memory.
pub const MAX_ENTRIES: usize = 1000;

/// Prompt history backed by a line-oriented file.
#[derive(Debug, Clone)]
pub struct History {
    path: PathBuf,
    entries: Vec<String>,
    /// Number of lines currently in the file (may exceed `entries.len()` before a rewrite).
    file_lines: usize,
    /// Whether the file's last line lacks a terminating newline.
    missing_newline: bool,
}

impl History {
    /// Load history from `path`. A missing or unreadable file yields an empty
    /// history (unreadable files are logged, never surfaced as errors).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut missing_newline = false;
        let (entries, file_lines) = match fs::read_to_string(&path) {
            Ok(content) => {
                missing_newline = !content.is_empty() && !content.ends_with('\n');
                let mut entries: Vec<String> = Vec::new();
                let mut lines = 0usize;
                for line in content.lines() {
                    lines += 1;
                    if line.is_empty() {
                        continue;
                    }
                    let entry = unescape(line);
                    if entries.last() != Some(&entry) {
                        entries.push(entry);
                    }
                }
                let excess = entries.len().saturating_sub(MAX_ENTRIES);
                entries.drain(..excess);
                (entries, lines)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Vec::new(), 0),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read prompt history");
                (Vec::new(), 0)
            }
        };
        Self {
            path,
            entries,
            file_lines,
            missing_newline,
        }
    }

    /// File backing this history.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Entries, oldest first.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Consume the history, returning its entries (oldest first), e.g. for
    /// [`super::App::with_history`].
    pub fn into_entries(self) -> Vec<String> {
        self.entries
    }

    /// Append `entry` and persist it. Blank entries and repeats of the newest
    /// entry are ignored. When the file would exceed [`MAX_ENTRIES`] lines it is
    /// rewritten with only the newest entries.
    pub fn append(&mut self, entry: &str) -> io::Result<()> {
        if entry.trim().is_empty() || self.entries.last().is_some_and(|last| last == entry) {
            return Ok(());
        }
        self.entries.push(entry.to_string());
        let excess = self.entries.len().saturating_sub(MAX_ENTRIES);
        self.entries.drain(..excess);
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        if self.file_lines + 1 > MAX_ENTRIES {
            self.rewrite()
        } else {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            let sep = if self.missing_newline { "\n" } else { "" };
            writeln!(file, "{sep}{}", escape(entry))?;
            self.missing_newline = false;
            self.file_lines += 1;
            Ok(())
        }
    }

    /// Replace the file with the in-memory entries (via a temp file + rename).
    fn rewrite(&mut self) -> io::Result<()> {
        let mut body = String::new();
        for entry in &self.entries {
            body.push_str(&escape(entry));
            body.push('\n');
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &self.path)?;
        self.file_lines = self.entries.len();
        self.missing_newline = false;
        Ok(())
    }
}

fn escape(entry: &str) -> String {
    let mut out = String::with_capacity(entry.len());
    for ch in entry.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("realm").join("tui_history")
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let h = History::load(path(&dir));
        assert!(h.entries().is_empty());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = History::load(path(&dir));
        h.append("first").unwrap();
        h.append("second").unwrap();
        let loaded = History::load(path(&dir));
        assert_eq!(
            loaded.entries(),
            &["first".to_string(), "second".to_string()]
        );
        assert_eq!(loaded.into_entries().len(), 2);
    }

    #[test]
    fn dedupes_consecutive_repeats_and_skips_blank() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = History::load(path(&dir));
        for e in ["a", "a", "b", "  ", "", "a"] {
            h.append(e).unwrap();
        }
        assert_eq!(h.entries(), &["a", "b", "a"]);
        let content = fs::read_to_string(path(&dir)).unwrap();
        assert_eq!(content, "a\nb\na\n");
        assert_eq!(History::load(path(&dir)).entries(), &["a", "b", "a"]);
    }

    #[test]
    fn escapes_newlines_and_backslashes() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = History::load(path(&dir));
        let entry = "line one\nline two with \\n literal\r\nend\\";
        h.append(entry).unwrap();
        let content = fs::read_to_string(path(&dir)).unwrap();
        assert_eq!(content.lines().count(), 1);
        assert_eq!(History::load(path(&dir)).entries(), &[entry.to_string()]);
    }

    #[test]
    fn append_after_unterminated_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(&dir);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "old").unwrap();
        let mut h = History::load(&p);
        h.append("new").unwrap();
        assert_eq!(History::load(&p).entries(), &["old", "new"]);
    }

    #[test]
    fn caps_file_at_max_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(&dir);
        let mut h = History::load(&p);
        for i in 0..MAX_ENTRIES + 5 {
            h.append(&format!("p{i}")).unwrap();
        }
        assert_eq!(h.entries().len(), MAX_ENTRIES);
        let content = fs::read_to_string(&p).unwrap();
        assert!(content.lines().count() <= MAX_ENTRIES);
        let loaded = History::load(&p);
        assert_eq!(loaded.entries().len(), MAX_ENTRIES);
        assert_eq!(loaded.entries().first().map(String::as_str), Some("p5"));
        assert_eq!(
            loaded.entries().last().map(String::as_str),
            Some(format!("p{}", MAX_ENTRIES + 4).as_str())
        );
    }

    #[test]
    fn oversized_file_is_trimmed_on_load_and_rewritten_on_append() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(&dir);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let body = (0..MAX_ENTRIES + 50)
            .map(|i| format!("e{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&p, body).unwrap();
        let mut h = History::load(&p);
        assert_eq!(h.entries().len(), MAX_ENTRIES);
        h.append("new").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap().lines().count(), MAX_ENTRIES);
    }
}

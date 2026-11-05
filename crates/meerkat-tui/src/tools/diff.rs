//! Unified diffs for file modifications, rendered by the UI.

use similar::{ChangeTag, TextDiff};
use std::fmt;
use std::path::PathBuf;

/// A change made by `write_file` or `edit_file`, sent to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// The tool call that produced the change.
    pub tool_use_id: String,
    /// Display path (relative to the project root when inside it).
    pub path: PathBuf,
    /// Unified diff text with `a/<path>` / `b/<path>` headers.
    pub unified: String,
    /// Number of added lines.
    pub added: usize,
    /// Number of removed lines.
    pub removed: usize,
}

impl FileDiff {
    /// Compute the diff between `old` and `new` for `path`.
    pub fn new(tool_use_id: String, path: PathBuf, old: &str, new: &str) -> Self {
        let diff = TextDiff::from_lines(old, new);
        let (added, removed) = count_changes(&diff);
        let display = path.display().to_string();
        let unified = diff
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{display}"), &format!("b/{display}"))
            .to_string();
        Self {
            tool_use_id,
            path,
            unified,
            added,
            removed,
        }
    }

    /// A compact `(+added −removed)` summary.
    pub fn stats(&self) -> DiffStats {
        DiffStats {
            added: self.added,
            removed: self.removed,
        }
    }
}

/// Count inserted and deleted lines in a computed diff.
pub fn count_changes<'a>(diff: &TextDiff<'a, 'a, '_, str>) -> (usize, usize) {
    diff.iter_all_changes()
        .fold((0, 0), |(added, removed), change| match change.tag() {
            ChangeTag::Insert => (added + 1, removed),
            ChangeTag::Delete => (added, removed + 1),
            ChangeTag::Equal => (added, removed),
        })
}

/// Line-count summary that renders as `(+3 −1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffStats {
    /// Added lines.
    pub added: usize,
    /// Removed lines.
    pub removed: usize,
}

impl fmt::Display for DiffStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(+{} −{})", self.added, self.removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_headers() {
        let diff = FileDiff::new(
            "t1".into(),
            PathBuf::from("src/x.rs"),
            "a\nb\nc\n",
            "a\nB\nc\nd\n",
        );
        assert_eq!(diff.added, 2);
        assert_eq!(diff.removed, 1);
        assert!(diff.unified.contains("--- a/src/x.rs"));
        assert!(diff.unified.contains("+++ b/src/x.rs"));
        assert!(diff.unified.contains("-b\n"));
        assert!(diff.unified.contains("+B\n"));
        assert_eq!(diff.stats().to_string(), "(+2 −1)");
    }

    #[test]
    fn new_file_is_all_additions() {
        let diff = FileDiff::new("t".into(), PathBuf::from("n.txt"), "", "x\ny\n");
        assert_eq!((diff.added, diff.removed), (2, 0));
    }

    #[test]
    fn identical_content_has_no_changes() {
        let diff = FileDiff::new("t".into(), PathBuf::from("n.txt"), "x\n", "x\n");
        assert_eq!((diff.added, diff.removed), (0, 0));
        assert!(diff.unified.is_empty());
    }
}

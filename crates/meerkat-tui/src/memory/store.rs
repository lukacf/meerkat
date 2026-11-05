//! On-disk note store: one `<slug>.md` per note plus a regenerated `MEMORY.md`.
//!
//! The store is a plain directory so users can read and hand-edit notes; every
//! listing is a fresh directory scan. Writes are atomic (temp file + rename) and
//! refuse to go through a symlinked notes directory or note file, so a planted
//! link cannot redirect a note write (or, for reads, expose an arbitrary file
//! through the always-allowed `memory_read`).

use std::fmt;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

/// Maximum size of one note's (trimmed) content in bytes.
pub const NOTE_MAX_BYTES: usize = 4 * 1024;
/// Maximum number of notes per scope.
pub const MAX_NOTES: usize = 200;
/// Name of the regenerated index file in each notes directory.
pub const INDEX_FILE: &str = "MEMORY.md";
/// Maximum size of the system-prompt index produced by [`prompt_index`].
pub const PROMPT_INDEX_MAX_BYTES: usize = 8 * 1024;

/// Maximum length of a note slug in bytes.
const SLUG_MAX_LEN: usize = 64;
/// Maximum length of a note summary line in characters.
const FIRST_LINE_MAX_CHARS: usize = 120;
/// Bytes kept free in the prompt index for the "N more" marker.
const PROMPT_MARKER_RESERVE: usize = 64;
/// Slug reserved for the index file.
const RESERVED_SLUG: &str = "memory";

/// Which notes directory a note lives in.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Deserialize,
    serde::Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// User-level notes: durable personal preferences, shared by every project.
    User,
    /// Workspace notes: facts about the current project.
    Workspace,
}

impl Scope {
    /// Lowercase name, as used in tool arguments and rule subjects.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors from the note store. Messages are model-facing.
#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    /// The note name does not produce a usable slug.
    #[error("invalid note name `{0}`: use letters and digits (at most 64), not `memory`")]
    InvalidName(String),
    /// The note content is empty after trimming.
    #[error("note content is empty")]
    Empty,
    /// The note content exceeds [`NOTE_MAX_BYTES`].
    #[error("note is too large (limit {NOTE_MAX_BYTES} bytes); keep notes short")]
    TooLarge,
    /// The scope already holds [`MAX_NOTES`] notes.
    #[error("too many notes (limit {MAX_NOTES}); forget or merge some first")]
    TooMany,
    /// No note with this name exists.
    #[error("no note named `{0}`")]
    NotFound(String),
    /// The notes directory or note file is a symlink.
    #[error("refusing to use symlink {}", .0.display())]
    Symlink(PathBuf),
    /// Underlying I/O failure.
    #[error("memory I/O error: {0}")]
    Io(#[from] io::Error),
}

/// One line of a note listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSummary {
    /// Note slug (file stem).
    pub name: String,
    /// First non-blank line of the note, at most 120 characters.
    pub first_line: String,
}

/// A notes directory for one scope. Opening is lazy: no I/O happens until a
/// note is saved, read or listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteStore {
    dir: PathBuf,
    scope: Scope,
}

impl NoteStore {
    /// Use `dir` as the notes directory for `scope`. Performs no I/O.
    #[must_use]
    pub fn open(dir: PathBuf, scope: Scope) -> Self {
        Self { dir, scope }
    }

    /// The notes directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The scope of this store.
    #[must_use]
    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// Save (create or replace) a note. Returns the slug and whether an existing
    /// note was replaced. Regenerates `MEMORY.md`.
    ///
    /// # Errors
    ///
    /// [`NoteError::InvalidName`], [`NoteError::Empty`], [`NoteError::TooLarge`],
    /// [`NoteError::TooMany`], [`NoteError::Symlink`] or [`NoteError::Io`].
    pub fn save(&self, name: &str, content: &str) -> Result<(String, bool), NoteError> {
        let slug = slug(name)?;
        let content = content.trim();
        if content.is_empty() {
            return Err(NoteError::Empty);
        }
        if content.len() > NOTE_MAX_BYTES {
            return Err(NoteError::TooLarge);
        }
        self.ensure_dir()?;
        let path = self.note_path(&slug);
        let replaced = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(NoteError::Symlink(path)),
            Ok(_) => true,
            Err(err) if err.kind() == io::ErrorKind::NotFound => false,
            Err(err) => return Err(err.into()),
        };
        if !replaced && self.list()?.len() >= MAX_NOTES {
            return Err(NoteError::TooMany);
        }
        write_atomic(&path, &format!("{content}\n"))?;
        self.write_index()?;
        Ok((slug, replaced))
    }

    /// Read a note's content.
    ///
    /// # Errors
    ///
    /// [`NoteError::InvalidName`], [`NoteError::NotFound`], [`NoteError::Symlink`]
    /// or [`NoteError::Io`].
    pub fn read(&self, name: &str) -> Result<String, NoteError> {
        let slug = slug(name)?;
        self.check_dir()?;
        let path = self.existing_note(&slug)?;
        Ok(std::fs::read_to_string(path)?)
    }

    /// Delete a note. Regenerates `MEMORY.md`.
    ///
    /// # Errors
    ///
    /// [`NoteError::InvalidName`], [`NoteError::NotFound`], [`NoteError::Symlink`]
    /// or [`NoteError::Io`].
    pub fn forget(&self, name: &str) -> Result<(), NoteError> {
        let slug = slug(name)?;
        self.check_dir()?;
        let path = self.existing_note(&slug)?;
        std::fs::remove_file(path)?;
        self.write_index()
    }

    /// List the notes in this scope, sorted by name. Files whose stem is not a
    /// valid slug, symlinks and `MEMORY.md` are skipped. A missing directory is
    /// an empty listing.
    ///
    /// # Errors
    ///
    /// [`NoteError::Symlink`] or [`NoteError::Io`].
    pub fn list(&self) -> Result<Vec<NoteSummary>, NoteError> {
        if !self.check_dir()? {
            return Ok(Vec::new());
        }
        let mut notes = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if slug(stem).ok().as_deref() != Some(stem) {
                tracing::debug!(path = %path.display(), "skipping note with a non-slug name");
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(err) => {
                    tracing::debug!(path = %path.display(), %err, "skipping unreadable note");
                    continue;
                }
            };
            notes.push(NoteSummary {
                name: stem.to_string(),
                first_line: first_line(&content),
            });
        }
        notes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(notes)
    }

    /// Regenerate `MEMORY.md`: `# Memory (<scope>)` followed by one
    /// `- name: first line` bullet per note.
    ///
    /// # Errors
    ///
    /// [`NoteError::Symlink`] or [`NoteError::Io`].
    pub fn write_index(&self) -> Result<(), NoteError> {
        let notes = self.list()?;
        let mut out = format!("# Memory ({})\n\n", self.scope);
        for note in &notes {
            let _ = writeln!(out, "- {}: {}", note.name, note.first_line);
        }
        if notes.is_empty() {
            out.push_str("(no notes)\n");
        }
        self.ensure_dir()?;
        write_atomic(&self.dir.join(INDEX_FILE), &out)?;
        Ok(())
    }

    fn note_path(&self, slug: &str) -> PathBuf {
        self.dir.join(format!("{slug}.md"))
    }

    /// Path of an existing, non-symlinked note.
    fn existing_note(&self, slug: &str) -> Result<PathBuf, NoteError> {
        let path = self.note_path(slug);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => Err(NoteError::Symlink(path)),
            Ok(meta) if meta.is_file() => Ok(path),
            Ok(_) => Err(NoteError::NotFound(slug.to_string())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                Err(NoteError::NotFound(slug.to_string()))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Whether the notes directory exists; errors when it is a symlink or not a
    /// directory.
    fn check_dir(&self) -> Result<bool, NoteError> {
        match std::fs::symlink_metadata(&self.dir) {
            Ok(meta) if meta.file_type().is_symlink() => Err(NoteError::Symlink(self.dir.clone())),
            Ok(meta) if meta.is_dir() => Ok(true),
            Ok(_) => Err(NoteError::Io(io::Error::other(format!(
                "{} is not a directory",
                self.dir.display()
            )))),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    fn ensure_dir(&self) -> Result<(), NoteError> {
        if !self.check_dir()? {
            std::fs::create_dir_all(&self.dir)?;
        }
        Ok(())
    }
}

/// Turn a note name into its slug: lowercase ASCII letters and digits, with
/// every run of other characters collapsed to `-`, trimmed of `-`, at most 64
/// bytes, non-empty and not `memory`.
///
/// # Errors
///
/// [`NoteError::InvalidName`] when the result is empty or reserved.
pub fn slug(name: &str) -> Result<String, NoteError> {
    let mut out = String::with_capacity(name.len().min(SLUG_MAX_LEN));
    let mut pending_dash = false;
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
        if out.len() >= SLUG_MAX_LEN {
            break;
        }
    }
    out.truncate(SLUG_MAX_LEN);
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() || trimmed == RESERVED_SLUG {
        return Err(NoteError::InvalidName(name.to_string()));
    }
    Ok(trimmed.to_string())
}

/// The first non-blank line of `content`, trimmed, at most 120 characters.
fn first_line(content: &str) -> String {
    let line = content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(FIRST_LINE_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_string(),
    }
}

/// Write `content` to `path` via a sibling temp file and a rename.
fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::other("note path has no file name"))?;
    let tmp = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The memory section appended to the system prompt, or `None` when both
/// scopes are empty. Capped at [`PROMPT_INDEX_MAX_BYTES`]; overflowing notes
/// are summarized as `(N more — use memory_list)`.
#[must_use]
pub fn prompt_index(user: Option<&NoteStore>, workspace: &NoteStore) -> Option<String> {
    let sections: Vec<(&str, &NoteStore, Vec<NoteSummary>)> = user
        .map(|s| ("User notes", s))
        .into_iter()
        .chain(std::iter::once(("Workspace notes", workspace)))
        .map(|(title, store)| {
            let notes = store.list().unwrap_or_else(|err| {
                tracing::warn!(dir = %store.dir().display(), %err, "cannot list memory notes");
                Vec::new()
            });
            (title, store, notes)
        })
        .filter(|(_, _, notes)| !notes.is_empty())
        .collect();
    if sections.is_empty() {
        return None;
    }

    let budget = PROMPT_INDEX_MAX_BYTES - PROMPT_MARKER_RESERVE;
    let mut out = String::from(
        "# Memory\n\nNotes you saved earlier. Read one with memory_read; save with memory_save \
         (scope user = durable personal preferences, workspace = facts about this project).\n",
    );
    let mut omitted = 0usize;
    for (title, store, notes) in &sections {
        let header = format!("\n## {title} ({})\n", store.dir().display());
        if omitted > 0 || out.len() + header.len() > budget {
            omitted += notes.len();
            continue;
        }
        out.push_str(&header);
        for note in notes {
            let line = format!("- {}: {}\n", note.name, note.first_line);
            if omitted > 0 || out.len() + line.len() > budget {
                omitted += 1;
            } else {
                out.push_str(&line);
            }
        }
    }
    if omitted > 0 {
        let _ = write!(out, "\n({omitted} more — use memory_list)\n");
    }
    Some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn store(scope: Scope) -> (tempfile::TempDir, NoteStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = NoteStore::open(dir.path().join("notes"), scope);
        (dir, store)
    }

    #[test]
    fn slug_table() {
        let long = "a".repeat(100);
        let cases: &[(&str, Option<&str>)] = &[
            ("Prefers Rust!", Some("prefers-rust")),
            ("  build -- cmd  ", Some("build-cmd")),
            ("CamelCase_name.v2", Some("camelcase-name-v2")),
            ("--x--", Some("x")),
            ("ünïcode wörds", Some("n-code-w-rds")),
            (&long, Some(&long[..64])),
            ("memory", None),
            ("MEMORY", None),
            ("", None),
            ("!!!", None),
        ];
        for (input, expected) in cases {
            assert_eq!(slug(input).ok().as_deref(), *expected, "{input:?}");
        }
        // Truncation never leaves a trailing dash.
        let s = slug(&format!("{} b", "a".repeat(63))).unwrap();
        assert_eq!(s, "a".repeat(63));
    }

    #[test]
    fn save_read_replace_forget_list() {
        let (_tmp, store) = store(Scope::Workspace);
        assert!(store.list().unwrap().is_empty());
        let (slug, replaced) = store
            .save("Build Command", "  use `make ci`\nmore\n")
            .unwrap();
        assert_eq!((slug.as_str(), replaced), ("build-command", false));
        assert_eq!(
            store.read("build command").unwrap(),
            "use `make ci`\nmore\n"
        );
        let (_, replaced) = store.save("build-command", "use cargo rct").unwrap();
        assert!(replaced);
        assert_eq!(store.read("build-command").unwrap(), "use cargo rct\n");
        store.save("alpha", "\n\nfirst\nsecond").unwrap();
        assert_eq!(
            store.list().unwrap(),
            [
                NoteSummary {
                    name: "alpha".into(),
                    first_line: "first".into()
                },
                NoteSummary {
                    name: "build-command".into(),
                    first_line: "use cargo rct".into()
                },
            ]
        );
        let index = std::fs::read_to_string(store.dir().join(INDEX_FILE)).unwrap();
        assert_eq!(
            index,
            "# Memory (workspace)\n\n- alpha: first\n- build-command: use cargo rct\n"
        );
        store.forget("alpha").unwrap();
        assert!(matches!(store.read("alpha"), Err(NoteError::NotFound(_))));
        assert!(matches!(store.forget("alpha"), Err(NoteError::NotFound(_))));
        let index = std::fs::read_to_string(store.dir().join(INDEX_FILE)).unwrap();
        assert!(!index.contains("alpha"));
        assert!(matches!(store.save("x", "  \n"), Err(NoteError::Empty)));
    }

    #[test]
    fn hand_edited_files_are_listed() {
        let (_tmp, store) = store(Scope::User);
        std::fs::create_dir_all(store.dir()).unwrap();
        std::fs::write(store.dir().join("manual.md"), "# Manual\n").unwrap();
        std::fs::write(store.dir().join("Not A Slug.md"), "x").unwrap();
        std::fs::write(store.dir().join("other.txt"), "x").unwrap();
        std::fs::write(store.dir().join(INDEX_FILE), "stale").unwrap();
        let names: Vec<_> = store.list().unwrap().into_iter().map(|n| n.name).collect();
        assert_eq!(names, ["manual"]);
        let long = "y".repeat(200);
        store.save("long", &long).unwrap();
        let first = &store.list().unwrap()[0].first_line;
        assert_eq!(first.chars().count(), FIRST_LINE_MAX_CHARS + 1);
    }

    #[test]
    fn caps() {
        let (_tmp, store) = store(Scope::Workspace);
        assert!(matches!(
            store.save("big", &"x".repeat(NOTE_MAX_BYTES + 1)),
            Err(NoteError::TooLarge)
        ));
        store.save("big", &"x".repeat(NOTE_MAX_BYTES)).unwrap();
        std::fs::remove_file(store.dir().join("big.md")).unwrap();
        for i in 0..MAX_NOTES {
            std::fs::write(store.dir().join(format!("n{i}.md")), "x").unwrap();
        }
        assert!(matches!(
            store.save("one-more", "x"),
            Err(NoteError::TooMany)
        ));
        // Replacing an existing note is still allowed at the cap.
        assert!(store.save("n0", "y").unwrap().1);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let store = NoteStore::open(link, Scope::User);
        assert!(matches!(store.save("a", "b"), Err(NoteError::Symlink(_))));
        assert!(matches!(store.list(), Err(NoteError::Symlink(_))));

        let store = NoteStore::open(real.clone(), Scope::User);
        let secret = tmp.path().join("secret");
        std::fs::write(&secret, "token").unwrap();
        std::os::unix::fs::symlink(&secret, real.join("leak.md")).unwrap();
        assert!(matches!(
            store.save("leak", "x"),
            Err(NoteError::Symlink(_))
        ));
        assert!(matches!(store.read("leak"), Err(NoteError::Symlink(_))));
        assert!(store.list().unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "token");
    }

    #[test]
    fn prompt_index_empty_and_listing() {
        let (_t1, user) = store(Scope::User);
        let (_t2, ws) = store(Scope::Workspace);
        assert_eq!(prompt_index(Some(&user), &ws), None);
        assert_eq!(prompt_index(None, &ws), None);
        user.save("style", "prefer early returns").unwrap();
        ws.save("build", "make ci").unwrap();
        let index = prompt_index(Some(&user), &ws).unwrap();
        assert!(index.starts_with("# Memory\n\nNotes you saved earlier."));
        let user_at = index.find("## User notes (").unwrap();
        let ws_at = index.find("## Workspace notes (").unwrap();
        assert!(user_at < ws_at);
        assert!(index.contains("- style: prefer early returns\n"));
        assert!(index.contains("- build: make ci\n"));
        let ws_only = prompt_index(None, &ws).unwrap();
        assert!(!ws_only.contains("User notes"));
    }

    #[test]
    fn prompt_index_is_capped() {
        let (_t1, user) = store(Scope::User);
        let (_t2, ws) = store(Scope::Workspace);
        std::fs::create_dir_all(user.dir()).unwrap();
        std::fs::create_dir_all(ws.dir()).unwrap();
        for i in 0..MAX_NOTES {
            let body = format!("{i} {}", "z".repeat(150));
            std::fs::write(user.dir().join(format!("u{i:03}.md")), &body).unwrap();
            std::fs::write(ws.dir().join(format!("w{i:03}.md")), &body).unwrap();
        }
        let index = prompt_index(Some(&user), &ws).unwrap();
        assert!(index.len() <= PROMPT_INDEX_MAX_BYTES, "{}", index.len());
        let shown = index.lines().filter(|l| l.starts_with("- ")).count();
        let marker = format!("({} more — use memory_list)", 2 * MAX_NOTES - shown);
        assert!(index.trim_end().ends_with(&marker), "{index}");
    }
}

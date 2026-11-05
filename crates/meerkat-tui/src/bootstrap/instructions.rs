//! Instruction files beyond the workspace `AGENTS.md`.
//!
//! Meerkat itself loads the workspace `AGENTS.md` (or `.rkat/AGENTS.md`)
//! into the system prompt: the factory's context root is the workspace. The
//! TUI adds, as `SessionBuildOptions.additional_instructions` sections after
//! the persona and in this order:
//!
//! 1. the user-global `AGENTS.md` in the [`GlobalDirs`] config directory,
//! 2. ancestor `AGENTS.md` (or `.rkat/AGENTS.md`) files from the git root
//!    down to the workspace's parent directory,
//! 3. the workspace `AGENTS.local.md` (or `.rkat/AGENTS.local.md`).
//!
//! Each file is capped at [`AGENTS_MD_MAX_BYTES`] (upstream's
//! [`normalize_agents_md_content`]), empty and unreadable files are skipped,
//! and sections stop being added once their rendered total would pass
//! [`TOTAL_MAX_BYTES`]. Discovery runs on every session build, so `/new`
//! and `/resume` see edited files (a resumed session keeps the system prompt
//! it was created with: a bare resume does not reassemble it upstream).
//!
//! It also lets a user-global `SYSTEM.md` replace the base system prompt
//! ([`apply_system_md`]) and appends `/remember` lines to the workspace
//! `AGENTS.md` ([`append_remember_line`]).

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use meerkat_core::Config;
use meerkat_core::prompt::{AGENTS_MD_MAX_BYTES, normalize_agents_md_content};

use super::dirs::GlobalDirs;

/// Cap on the combined rendered size of all discovered sections.
pub const TOTAL_MAX_BYTES: usize = 96 * 1024;

/// File name of the workspace-local, uncommitted instruction file.
pub const LOCAL_FILE_NAME: &str = "AGENTS.local.md";

/// File name of a project instruction file.
pub const AGENTS_FILE_NAME: &str = "AGENTS.md";

/// Heading of the runtime system context `/remember` adds (the persona
/// refers to it).
pub const REMEMBERED_HEADING: &str = "# Remembered instruction";

/// Where an instruction section came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// The user-global `AGENTS.md`.
    UserGlobal,
    /// An `AGENTS.md` in a directory between the git root and the workspace.
    Ancestor,
    /// The workspace `AGENTS.local.md`.
    Local,
}

/// One instruction file, ready to render into the system prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Where the file came from.
    pub kind: SectionKind,
    /// The file's path.
    pub path: PathBuf,
    /// The file's content (possibly truncated, trailing whitespace trimmed).
    pub body: String,
    /// Whether `body` was cut at [`AGENTS_MD_MAX_BYTES`].
    pub truncated: bool,
}

impl Section {
    /// `# Instructions (from <path>)`.
    #[must_use]
    pub fn heading(&self) -> String {
        format!("# Instructions (from {})", self.path.display())
    }

    /// Heading, body and a truncation marker when the body was cut.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = self.heading();
        out.push_str("\n\n");
        out.push_str(&self.body);
        if self.truncated {
            let _ = write!(out, "\n\n[truncated at {} KiB]", AGENTS_MD_MAX_BYTES / 1024);
        }
        out
    }
}

/// The runtime system context for a `/remember` line:
/// [`REMEMBERED_HEADING`], a blank line and `text` trimmed.
#[must_use]
pub fn remembered_context(text: &str) -> String {
    format!("{REMEMBERED_HEADING}\n\n{}", text.trim())
}

/// Where instruction files are looked for.
#[derive(Debug, Clone)]
pub struct InstructionSources {
    /// The workspace (the factory's context root).
    pub workspace: PathBuf,
    /// The user-global config directory, if any.
    pub global: Option<GlobalDirs>,
    /// The home directory: never adopted as a git root (a dotfiles
    /// repository in `$HOME` must not pull `~/AGENTS.md` into every project).
    pub home: Option<PathBuf>,
}

impl InstructionSources {
    /// Discover instruction sections (see the module docs for the order and
    /// caps). The workspace's own `AGENTS.md` is loaded natively and is not
    /// part of the result.
    #[must_use]
    pub fn discover(&self) -> Vec<Section> {
        let mut candidates: Vec<(SectionKind, PathBuf)> = Vec::new();
        if let Some(global) = &self.global {
            let path = global.agents_md();
            if path.is_file() {
                candidates.push((SectionKind::UserGlobal, path));
            }
        }
        if let Some(root) = git_root(&self.workspace, self.home.as_deref()) {
            let ancestors: Vec<&Path> = self
                .workspace
                .ancestors()
                .skip(1)
                .take_while(|dir| dir.starts_with(&root))
                .collect();
            for dir in ancestors.into_iter().rev() {
                if let Some(path) = find_in(dir, AGENTS_FILE_NAME) {
                    candidates.push((SectionKind::Ancestor, path));
                }
            }
        }
        if let Some(path) = find_in(&self.workspace, LOCAL_FILE_NAME) {
            candidates.push((SectionKind::Local, path));
        }
        cap_total(
            candidates
                .into_iter()
                .filter_map(|(kind, path)| load_section(kind, path)),
        )
    }

    /// One rendered string per discovered section, for
    /// `SessionOptions.additional_instructions`.
    #[must_use]
    pub fn render(&self) -> Vec<String> {
        self.discover().iter().map(Section::render).collect()
    }
}

/// Keep sections while their rendered total stays within
/// [`TOTAL_MAX_BYTES`]; the first one that would pass it ends the list.
fn cap_total(sections: impl Iterator<Item = Section>) -> Vec<Section> {
    let mut kept = Vec::new();
    let mut total = 0usize;
    for section in sections {
        let size = section.render().len();
        if total + size > TOTAL_MAX_BYTES {
            tracing::warn!(
                path = %section.path.display(),
                limit = TOTAL_MAX_BYTES,
                "instruction files exceed the total size cap; skipping this and later files"
            );
            break;
        }
        total += size;
        kept.push(section);
    }
    kept
}

/// `<dir>/<name>`, else `<dir>/.rkat/<name>`, whichever is a file (the
/// candidates upstream's prompt assembly reads for the workspace).
fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    [dir.join(name), dir.join(".rkat").join(name)]
        .into_iter()
        .find(|path| path.is_file())
}

/// Read one instruction file; `None` when empty or unreadable.
fn load_section(kind: SectionKind, path: PathBuf) -> Option<Section> {
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "cannot read instruction file; skipping");
            return None;
        }
    };
    let trimmed = content.trim_end();
    let body = normalize_agents_md_content(trimmed)?;
    let truncated = body.len() < trimmed.len();
    Some(Section {
        kind,
        path,
        body,
        truncated,
    })
}

/// The nearest ancestor of `start` (inclusive) containing `.git`, as a
/// directory or as a file (worktrees, submodules). The search stops below
/// `home`: the home directory and its ancestors are never a git root here.
#[must_use]
pub fn git_root(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    start
        .ancestors()
        .take_while(|dir| home.is_none_or(|home| !home.starts_with(dir)))
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Point `config.agent.system_prompt_file` at the user-global `SYSTEM.md`.
///
/// Applies only when `SYSTEM.md` exists and is non-empty and the config sets
/// neither `system_prompt_file` nor a non-empty `system_prompt`. Returns the
/// applied path.
pub fn apply_system_md(config: &mut Config, global: Option<&GlobalDirs>) -> Option<PathBuf> {
    let path = global?.system_md();
    let file_set = config
        .agent
        .system_prompt_file
        .as_ref()
        .is_some_and(|p| !p.as_os_str().is_empty());
    let inline_set = config
        .agent
        .system_prompt
        .as_ref()
        .is_some_and(|p| !p.trim().is_empty());
    if file_set || inline_set {
        return None;
    }
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => {}
        Ok(_) => return None,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), %err, "cannot read SYSTEM.md; ignoring");
            }
            return None;
        }
    }
    config.agent.system_prompt_file = Some(path.clone());
    Some(path)
}

/// The file `/remember` appends to: the workspace `AGENTS.md` Meerkat loads
/// (`AGENTS.md`, else `.rkat/AGENTS.md`), or `<workspace>/AGENTS.md` when
/// neither exists.
#[must_use]
pub fn remember_target(workspace: &Path) -> PathBuf {
    find_in(workspace, AGENTS_FILE_NAME).unwrap_or_else(|| workspace.join(AGENTS_FILE_NAME))
}

/// The line `/remember` writes for `text`: line breaks collapsed to single
/// spaces, trimmed. `None` when nothing is left.
#[must_use]
pub fn remember_line(text: &str) -> Option<String> {
    let line = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!line.is_empty()).then_some(line)
}

/// Append `- <line>` ([`remember_line`]) to `path`.
///
/// A missing file is created with an `# AGENTS.md` header; an existing file
/// without a trailing newline gets one first.
///
/// # Errors
///
/// [`std::io::ErrorKind::InvalidInput`] when `text` is blank, or any I/O
/// error reading or writing `path`.
pub fn append_remember_line(path: &Path, text: &str) -> std::io::Result<()> {
    let line = remember_line(text).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "nothing to remember")
    })?;
    let existing = match std::fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let mut out = String::new();
    match existing.as_deref() {
        None => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            out.push_str("# AGENTS.md\n\n");
        }
        Some(content) if !content.is_empty() && !content.ends_with('\n') => out.push('\n'),
        Some(_) => {}
    }
    let _ = writeln!(out, "- {line}");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(out.as_bytes())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    struct Tree {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        a: PathBuf,
        ws: PathBuf,
        global: GlobalDirs,
    }

    impl Tree {
        fn sources(&self, global: bool) -> InstructionSources {
            InstructionSources {
                workspace: self.ws.clone(),
                global: global.then(|| self.global.clone()),
                home: None,
            }
        }
    }

    /// `<tmp>/root/.git/`, `root/AGENTS.md`, `root/a/.rkat/AGENTS.md`,
    /// workspace `root/a/b` with `AGENTS.md` and `AGENTS.local.md`, and a
    /// global `AGENTS.md`.
    fn tree() -> Tree {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("root");
        let a = root.join("a");
        let ws = a.join("b");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        write(&root.join("AGENTS.md"), "root rules\n");
        write(&a.join(".rkat/AGENTS.md"), "a rules\n");
        write(&ws.join("AGENTS.md"), "workspace rules\n");
        write(&ws.join(LOCAL_FILE_NAME), "local rules\n");
        let global = GlobalDirs {
            config: base.join("config"),
            cache: base.join("cache"),
        };
        write(&global.agents_md(), "global rules\n");
        Tree {
            _tmp: tmp,
            root,
            a,
            ws,
            global,
        }
    }

    #[test]
    fn discovers_in_order_and_excludes_workspace_agents_md() {
        let t = tree();
        let sections = t.sources(true).discover();
        let found: Vec<_> = sections.iter().map(|s| (s.kind, s.path.clone())).collect();
        assert_eq!(
            found,
            [
                (SectionKind::UserGlobal, t.global.agents_md()),
                (SectionKind::Ancestor, t.root.join("AGENTS.md")),
                (SectionKind::Ancestor, t.a.join(".rkat/AGENTS.md")),
                (SectionKind::Local, t.ws.join(LOCAL_FILE_NAME)),
            ]
        );
        assert!(sections.iter().all(|s| !s.truncated));
        assert_eq!(sections[1].body, "root rules");
        let rendered = t.sources(true).render();
        assert!(rendered.iter().all(|r| !r.contains("workspace rules")));
        assert_eq!(
            rendered[3],
            format!(
                "# Instructions (from {})\n\nlocal rules",
                t.ws.join(LOCAL_FILE_NAME).display()
            )
        );
    }

    #[test]
    fn rkat_local_file_is_accepted() {
        let t = tree();
        std::fs::remove_file(t.ws.join(LOCAL_FILE_NAME)).unwrap();
        write(&t.ws.join(".rkat").join(LOCAL_FILE_NAME), "hidden local\n");
        let sections = t.sources(false).discover();
        let last = sections.last().unwrap();
        assert_eq!(last.kind, SectionKind::Local);
        assert_eq!(last.body, "hidden local");
    }

    #[test]
    fn oversized_file_is_truncated_with_marker() {
        let t = tree();
        // Multi-byte characters straddling the cap.
        write(&t.global.agents_md(), &"é".repeat(AGENTS_MD_MAX_BYTES));
        let sections = t.sources(true).discover();
        let global = &sections[0];
        assert!(global.truncated);
        assert!(global.body.len() <= AGENTS_MD_MAX_BYTES);
        assert!(global.render().ends_with("\n\n[truncated at 32 KiB]"));
    }

    #[test]
    fn total_cap_stops_adding_sections() {
        let t = tree();
        let big = "x".repeat(AGENTS_MD_MAX_BYTES - 10);
        write(&t.global.agents_md(), &big);
        write(&t.root.join("AGENTS.md"), &big);
        write(&t.a.join(".rkat/AGENTS.md"), &big);
        write(&t.ws.join(LOCAL_FILE_NAME), &big);
        let sections = t.sources(true).discover();
        assert_eq!(sections.len(), 2, "a third ~32 KiB section passes the cap");
        let total: usize = sections.iter().map(|s| s.render().len()).sum();
        assert!(total <= TOTAL_MAX_BYTES);
    }

    #[test]
    fn git_file_worktree_is_detected() {
        let t = tree();
        std::fs::remove_dir_all(t.root.join(".git")).unwrap();
        write(&t.a.join(".git"), "gitdir: /elsewhere\n");
        assert_eq!(git_root(&t.ws, None), Some(t.a.clone()));
        let found: Vec<_> = t
            .sources(false)
            .discover()
            .into_iter()
            .map(|s| (s.kind, s.path))
            .collect();
        assert_eq!(
            found,
            [
                (SectionKind::Ancestor, t.a.join(".rkat/AGENTS.md")),
                (SectionKind::Local, t.ws.join(LOCAL_FILE_NAME)),
            ]
        );
    }

    #[test]
    fn home_is_never_the_git_root() {
        let t = tree();
        // `root` plays the home directory holding a dotfiles repository.
        assert_eq!(git_root(&t.ws, Some(&t.root)), None);
        let mut sources = t.sources(false);
        sources.home = Some(t.root.clone());
        assert!(
            sources
                .discover()
                .iter()
                .all(|s| s.kind != SectionKind::Ancestor)
        );
        // A repository below home is found.
        std::fs::create_dir_all(t.a.join(".git")).unwrap();
        assert_eq!(git_root(&t.ws, Some(&t.root)), Some(t.a.clone()));
    }

    #[test]
    fn workspace_at_git_root_has_no_ancestors() {
        let t = tree();
        std::fs::create_dir_all(t.ws.join(".git")).unwrap();
        assert!(
            t.sources(false)
                .discover()
                .iter()
                .all(|s| s.kind != SectionKind::Ancestor)
        );
    }

    #[test]
    fn empty_files_are_skipped() {
        let t = tree();
        write(&t.root.join("AGENTS.md"), "  \n\t\n");
        write(&t.global.agents_md(), "");
        let sections = t.sources(true).discover();
        let kinds: Vec<_> = sections.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [SectionKind::Ancestor, SectionKind::Local]);
        assert_eq!(sections[0].path, t.a.join(".rkat/AGENTS.md"));
    }

    #[test]
    fn system_md_applies_only_without_configured_prompt() {
        let t = tree();
        assert_eq!(
            apply_system_md(&mut Config::default(), Some(&t.global)),
            None
        );
        write(&t.global.system_md(), "You are X.\n");

        let mut config = Config::default();
        assert_eq!(apply_system_md(&mut config, None), None);
        assert_eq!(
            apply_system_md(&mut config, Some(&t.global)),
            Some(t.global.system_md())
        );
        assert_eq!(config.agent.system_prompt_file, Some(t.global.system_md()));

        let mut config = Config::default();
        config.agent.system_prompt = Some("inline".into());
        assert_eq!(apply_system_md(&mut config, Some(&t.global)), None);
        assert_eq!(config.agent.system_prompt_file, None);

        let mut config = Config::default();
        config.agent.system_prompt = Some("   ".into());
        assert!(apply_system_md(&mut config, Some(&t.global)).is_some());

        let mut config = Config::default();
        config.agent.system_prompt_file = Some(PathBuf::from("/custom.md"));
        assert_eq!(apply_system_md(&mut config, Some(&t.global)), None);

        write(&t.global.system_md(), " \n");
        assert_eq!(
            apply_system_md(&mut Config::default(), Some(&t.global)),
            None
        );
    }

    #[test]
    fn remember_target_resolution() {
        let t = tree();
        assert_eq!(remember_target(&t.ws), t.ws.join("AGENTS.md"));
        assert_eq!(remember_target(&t.a), t.a.join(".rkat/AGENTS.md"));
        let empty = t.root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(remember_target(&empty), empty.join("AGENTS.md"));
    }

    #[test]
    fn append_remember_line_creates_and_appends() {
        let t = tree();
        let path = t.root.join("new/AGENTS.md");
        append_remember_line(&path, "  use tabs\n  not spaces  ").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# AGENTS.md\n\n- use tabs not spaces\n"
        );
        append_remember_line(&path, "second").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# AGENTS.md\n\n- use tabs not spaces\n- second\n"
        );
        let existing = t.root.join("AGENTS.md");
        std::fs::write(&existing, "no newline").unwrap();
        append_remember_line(&existing, "x\r\ny").unwrap();
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "no newline\n- x y\n"
        );
        let err = append_remember_line(&existing, " \n ").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            remembered_context(" prefer tabs "),
            "# Remembered instruction\n\nprefer tabs"
        );
    }
}

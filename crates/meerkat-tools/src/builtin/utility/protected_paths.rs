//! The one rule for where the in-process file tools never write.
//!
//! `apply_patch` and `blob_save_file` run in the host process, so no OS
//! process confinement stands between a model's call and the file. Two kinds
//! of location inside the project root are the host's, not the project's:
//!
//! - the engine's own directory, `.rkat`, at the project root and in any
//!   directory under it, with everything it holds: the project's MCP
//!   servers, hooks and skills, and by default the realm's state and config
//!   documents. A host reads the `.rkat` of the directory it is started in
//!   at its next start and runs what it names, so a session that can write
//!   one decides what a host started there launches. This part of the rule
//!   needs nothing from the host and is always on;
//! - every path the host declares as its own state or input
//!   ([`ProtectedPaths::with_host_paths`]) that lies strictly inside the
//!   project root: a realm state root, a config document, a convention file
//!   or a credential store. It is protected with everything under it. A
//!   declared path that contains or equals the project root is not applied:
//!   a host that roots its tools inside its own state (a server started
//!   without a context root) declares the documents it must keep, and every
//!   other file there stays writable.
//!
//! A write, a delete and both ends of a move are refused when the target
//! lies in such a location, judged twice: on the path's own components (a
//! `.rkat` that does not exist yet would be created by the write) and on
//! the real path of its deepest existing ancestor (a symbolic link into the
//! location). Where the project root's `.rkat` or a declared path is itself
//! a symbolic link, the directory it leads to is the same location and is
//! refused under its own name; a link deeper inside such a location is not
//! followed. A path with a symbolic link that leads nowhere is refused too:
//! the write would create the link's target, wherever it points. Reads are
//! not judged here.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// The engine directory's name under a project root.
const ENGINE_DIRECTORY: &str = ".rkat";

/// Whether this platform's usual filesystem treats names that differ only
/// in letter case as one name.
const CASE_INSENSITIVE_NAMES: bool = cfg!(any(target_os = "macos", windows));

/// Why a file tool refused its target path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteRefusal {
    /// The target lies in the engine's own directory or in a location the
    /// host declared as its own.
    Protected,
    /// A component of the path is a symbolic link that leads nowhere.
    DanglingLink,
}

impl WriteRefusal {
    /// The reason a model reads. It names the path as the model gave it and
    /// nothing of the host's layout.
    pub(crate) fn reason(self, given: &Path) -> String {
        match self {
            Self::Protected => format!(
                "path '{}' is in the engine's own directory (a {ENGINE_DIRECTORY} directory anywhere under the project root) or in another location the host keeps its state and configuration in; the file tools do not write there. Engine configuration is changed with the rkat CLI or the host's config commands",
                given.display()
            ),
            Self::DanglingLink => format!(
                "path '{}' goes through a symbolic link whose target does not exist; the file tools do not write through it",
                given.display()
            ),
        }
    }
}

/// The locations a file tool rooted at one project root never writes.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProtectedPaths {
    /// Absolute paths the host declared. One that lies strictly inside the
    /// project root is protected with everything under it; one that
    /// contains or equals the project root is not applied.
    host: Arc<[PathBuf]>,
}

impl ProtectedPaths {
    /// The rule with the host's declared paths added to the engine
    /// directory. A path that is not absolute is ignored: it names no
    /// location without a working directory, which a tool never reads.
    pub(crate) fn with_host_paths(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            host: paths
                .into_iter()
                .filter(|path| path.is_absolute())
                .map(|path| normalize(&path))
                .collect(),
        }
    }

    /// Admit `target` as a path a file tool may create, change, remove or
    /// move. `target` is absolute, without `.` or `..`, and inside
    /// `project_root` by its components (the tools' own containment checks
    /// established that).
    pub(crate) fn admit_write(
        &self,
        project_root: &Path,
        target: &Path,
    ) -> Result<(), WriteRefusal> {
        if in_engine_directory(project_root, target) {
            return Err(WriteRefusal::Protected);
        }
        let real_target = real_path(target)?;
        // A root that cannot be resolved leaves the judgement on components,
        // and the tool's own root check refuses the call.
        let real_root = real_path(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        if in_engine_directory(&real_root, &real_target) {
            return Err(WriteRefusal::Protected);
        }
        // The project root's engine directory may itself be a symbolic
        // link: the host reads it where the link leads, so that directory
        // is the engine's too, under its own name.
        if real_path(&project_root.join(ENGINE_DIRECTORY))
            .is_ok_and(|real_engine| strip_prefix(&real_target, &real_engine).is_some())
        {
            return Err(WriteRefusal::Protected);
        }
        for declared in self.host.iter() {
            // The declared path as the host named it and where it really
            // lies: the host may name its state through a symbolic link.
            let real_declared = real_path(declared).ok();
            let forms = || std::iter::once(declared.as_path()).chain(real_declared.as_deref());
            let inside_root = forms().any(|declared| {
                strictly_inside(declared, project_root) || strictly_inside(declared, &real_root)
            });
            if inside_root
                && forms().any(|declared| {
                    strip_prefix(target, declared).is_some()
                        || strip_prefix(&real_target, declared).is_some()
                })
            {
                return Err(WriteRefusal::Protected);
            }
        }
        Ok(())
    }
}

/// Whether `target` is an engine directory under `root` or lies in one, by
/// components: a component with the engine directory's name at any depth
/// below the root. A host started in a subdirectory reads that directory's
/// `.rkat`, so each one is an engine directory.
fn in_engine_directory(root: &Path, target: &Path) -> bool {
    strip_prefix(target, root).is_some_and(|below| {
        below.components().any(|component| {
            matches!(
                component,
                Component::Normal(name) if is_engine_directory_name(name, cfg!(windows))
            )
        })
    })
}

/// Whether `path` lies under `base` and is not `base` itself.
fn strictly_inside(path: &Path, base: &Path) -> bool {
    strip_prefix(path, base).is_some_and(|below| below.components().next().is_some())
}

/// Whether `name`, a path component, opens an engine directory. Letter
/// case is ignored on every platform: a case-insensitive filesystem (the
/// default on macOS and Windows) opens `.rkat` for `.RKAT`.
fn is_engine_directory_name(name: &OsStr, windows_names: bool) -> bool {
    same_name(name, OsStr::new(ENGINE_DIRECTORY), true, windows_names)
}

/// `name` as a comparison that ignores letter case reads it: ASCII letters
/// in lower case, and the other spellings a case-insensitive filesystem
/// opens the same entry for folded as well. Those are the two characters
/// whose case folding is an ASCII letter (the KELVIN SIGN, U+212A, for `k`
/// and LATIN SMALL LETTER LONG S, U+017F, for `s`) and the code points HFS+
/// ignores in a name. No other Unicode equivalence is applied.
fn fold_name(name: &str) -> String {
    name.chars()
        .filter(|character| !is_hfs_ignorable(*character))
        .map(|character| match character {
            '\u{212A}' => 'k',
            '\u{017F}' => 's',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// The code points HFS+ ignores when it compares names.
const fn is_hfs_ignorable(character: char) -> bool {
    matches!(
        character,
        '\u{200C}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{206A}'..='\u{206F}' | '\u{FEFF}'
    )
}

/// Whether two path component names open the same directory entry. With
/// `ignore_case`, both names are folded first ([`fold_name`]). With
/// `windows_names`, each name is then reduced the way Windows reduces it
/// before it opens a file: an alternate data stream suffix (`:name`) and
/// trailing dots and spaces are dropped.
fn same_name(left: &OsStr, right: &OsStr, ignore_case: bool, windows_names: bool) -> bool {
    let (Some(left), Some(right)) = (left.to_str(), right.to_str()) else {
        return left == right;
    };
    let reduce = |name: &str| -> String {
        let name = if ignore_case {
            fold_name(name)
        } else {
            name.to_owned()
        };
        if windows_names {
            name.split(':')
                .next()
                .unwrap_or(&name)
                .trim_end_matches(['.', ' '])
                .to_owned()
        } else {
            name
        }
    };
    reduce(left) == reduce(right)
}

/// `path` below `base` when `base` is a prefix of it by whole components
/// (the path itself included), comparing names the way the platform's
/// filesystem does.
fn strip_prefix(path: &Path, base: &Path) -> Option<PathBuf> {
    let mut rest = path.components();
    for expected in base.components() {
        let found = rest.next()?;
        if !same_component(found, expected) {
            return None;
        }
    }
    Some(rest.as_path().to_path_buf())
}

fn same_component(left: Component<'_>, right: Component<'_>) -> bool {
    match (left, right) {
        (Component::Normal(left), Component::Normal(right)) => {
            same_name(left, right, CASE_INSENSITIVE_NAMES, cfg!(windows))
        }
        _ => left == right,
    }
}

/// `path` without `.` components and with each `..` applied to what
/// precedes it.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// The real location of `path`: its deepest existing ancestor with every
/// symbolic link followed, then the components that do not exist yet. A
/// component that exists as a symbolic link but cannot be followed (its
/// target is missing, or the links loop) is refused: creating the path
/// would write wherever that link points.
fn real_path(path: &Path) -> Result<PathBuf, WriteRefusal> {
    let mut ancestor = path.to_path_buf();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        if let Ok(mut real) = std::fs::canonicalize(&ancestor) {
            real.extend(missing.iter().rev());
            return Ok(real);
        }
        if std::fs::symlink_metadata(&ancestor).is_ok_and(|entry| entry.file_type().is_symlink()) {
            return Err(WriteRefusal::DanglingLink);
        }
        let Some(name) = ancestor.file_name().map(OsStr::to_os_string) else {
            return Ok(path.to_path_buf());
        };
        missing.push(name);
        if !ancestor.pop() {
            return Ok(path.to_path_buf());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        (dir, root)
    }

    /// The engine directory's name is matched without letter case on every
    /// platform, and only as the whole name.
    #[test]
    fn engine_directory_name_ignores_letter_case_and_matches_whole_names() {
        for name in [".rkat", ".RKAT", ".Rkat", ".rKaT"] {
            assert!(is_engine_directory_name(OsStr::new(name), false), "{name}");
        }
        for name in [
            ".rkatignore",
            ".rkat.rs",
            "rkat",
            ".rkat2",
            "x.rkat",
            ".rkat.",
            ".rkat ",
            ".rkat:stream",
        ] {
            assert!(!is_engine_directory_name(OsStr::new(name), false), "{name}");
        }
    }

    /// Names a filesystem opens `.rkat` for although they are spelled with
    /// other code points: the KELVIN SIGN, which case folding maps to `k`,
    /// and the code points HFS+ ignores in a name. Any other code point
    /// makes a different name.
    #[test]
    fn engine_directory_name_folds_kelvin_and_ignorable_code_points() {
        for name in [
            ".r\u{212A}at",
            ".R\u{212A}AT",
            ".rk\u{200C}at",
            "\u{FEFF}.rkat",
            ".rkat\u{200D}",
            ".\u{200E}r\u{200F}kat",
            ".r\u{202A}k\u{202E}at",
            ".rka\u{206A}t\u{206F}",
        ] {
            assert!(
                is_engine_directory_name(OsStr::new(name), false),
                "{}",
                name.escape_unicode()
            );
        }
        for name in [
            ".rk\u{0301}at",
            ".rkat\u{00A0}",
            ".r\u{212B}at",
            ".rk\u{2060}at",
            ".rkatt",
        ] {
            assert!(
                !is_engine_directory_name(OsStr::new(name), false),
                "{}",
                name.escape_unicode()
            );
        }
    }

    /// The names Windows opens `.rkat` for: trailing dots and spaces, and an
    /// alternate data stream of the directory.
    #[test]
    fn engine_directory_name_follows_windows_name_reduction() {
        for name in [
            ".rkat",
            ".RKAT",
            ".rkat.",
            ".rkat...",
            ".rkat ",
            ".rkat . .",
            ".rkat::$INDEX_ALLOCATION",
            ".rkat:stream",
            ".RKAT.:$DATA",
        ] {
            assert!(is_engine_directory_name(OsStr::new(name), true), "{name}");
        }
        for name in [".rkatignore", ".rkat.rs", "rkat", ".rkat.d", "x:.rkat"] {
            assert!(!is_engine_directory_name(OsStr::new(name), true), "{name}");
        }
    }

    /// `.rkat` is an engine directory at every depth under the project
    /// root: a host started from a subdirectory reads that directory's
    /// `.rkat` as its own (its MCP servers, hooks, skills and realms). A
    /// file or directory whose name only resembles it is the project's own.
    #[test]
    fn engine_directory_is_protected_at_every_depth() {
        let (_dir, root) = project();
        let rule = ProtectedPaths::default();
        for refused in [
            ".rkat",
            ".rkat/mcp.toml",
            ".rkat/realms/r/config.toml",
            ".RKAT/x",
            "packages/app/.rkat",
            "packages/app/.rkat/mcp.toml",
            "packages/app/.rkat/realms/r/config.toml",
            "vendor/tool/.Rkat/notes.txt",
            "docs/.rkat",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::Protected),
                "{refused}"
            );
        }
        for admitted in [
            ".rkatignore",
            "src/.rkat.rs",
            "rkat/config.toml",
            "packages/app/rkat/.rkatrc",
            "packages/.rkat-backup/mcp.toml",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(admitted)),
                Ok(()),
                "{admitted}"
            );
        }
    }

    /// A host-declared path is protected with everything under it, whether
    /// or not it exists yet, and a path beside it is not.
    #[test]
    fn host_declared_paths_are_protected_with_everything_under_them() {
        let (_dir, root) = project();
        std::fs::create_dir_all(root.join("state/realm")).unwrap();
        let rule = ProtectedPaths::with_host_paths([
            root.join("state"),
            root.join("config/../user/.rkat/config.toml"),
            root.join("not-yet/credentials"),
            PathBuf::from("relative/ignored"),
        ]);
        for refused in [
            "state",
            "state/realm/config.toml",
            "state/new-realm/runtime.sqlite3",
            "user/.rkat/config.toml",
            "user/.rkat/mcp.toml",
            "not-yet/credentials/token.json",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::Protected),
                "{refused}"
            );
        }
        for admitted in [
            "state.md",
            "statement/notes.txt",
            "user/notes.txt",
            "not-yet/other.txt",
            "relative/ignored/file.txt",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(admitted)),
                Ok(()),
                "{admitted}"
            );
        }
    }

    /// A host that roots its tools inside its own state root (a server
    /// started without a context root: the project root is the realm
    /// directory) declares the state root and the realm's own documents.
    /// The state root contains the project root, so it is not applied; the
    /// documents are refused, and every other file there stays writable.
    #[test]
    fn a_declared_path_that_contains_the_project_root_is_not_applied() {
        let (_dir, base) = project();
        let state_root = base.join("realms");
        let root = state_root.join("realm-1");
        std::fs::create_dir_all(&root).unwrap();
        let rule = ProtectedPaths::with_host_paths([
            state_root,
            root.clone(),
            root.join("config.toml"),
            root.join("authorization"),
        ]);
        for refused in [
            "config.toml",
            "authorization",
            "authorization/grant-owner.jsonl",
            ".rkat/config.toml",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::Protected),
                "{refused}"
            );
        }
        for admitted in [
            "out.png",
            "notes/config.toml",
            "sessions.sqlite3",
            "config.toml.bak",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(admitted)),
                Ok(()),
                "{admitted}"
            );
        }
    }

    /// Declared names are compared the way the platform's filesystem
    /// compares them: without letter case where it ignores case, and with
    /// Windows' reduction of trailing dots, spaces and stream suffixes.
    #[test]
    fn declared_names_follow_the_platform_name_rules() {
        let name = |text: &'static str| OsStr::new(text);
        assert!(same_name(name("state"), name("state"), false, false));
        assert!(!same_name(name("State"), name("state"), false, false));
        assert!(same_name(name("State"), name("state"), true, false));
        assert!(!same_name(name("state."), name("state"), true, false));
        assert!(same_name(name("state. "), name("STATE"), true, true));
        assert!(same_name(name("state::$DATA"), name("state"), true, true));
        assert!(!same_name(name("statement"), name("state"), true, true));
    }

    /// Every comparison that ignores letter case also folds the spellings a
    /// case-insensitive filesystem opens the same entry for: LATIN SMALL
    /// LETTER LONG S for `s`, the KELVIN SIGN for `k`, and the code points
    /// HFS+ ignores. So a declared name with an `s` or a `k` in it cannot be
    /// spelled around. A comparison that keeps case folds nothing, and
    /// unrelated letters stay different.
    #[test]
    fn case_ignoring_comparisons_fold_long_s_kelvin_and_ignorable_code_points() {
        let name = |text: &'static str| OsStr::new(text);
        for (spelled, declared) in [
            ("\u{017F}tate", "state"),
            ("\u{017F}kill\u{017F}", "skills"),
            ("s\u{212A}ills", "SKILLS"),
            ("config_\u{017F}tate.j\u{017F}on", "config_state.json"),
            ("realm_manife\u{017F}t.json", "realm_manifest.json"),
            ("st\u{200D}ate", "state"),
            ("\u{FEFF}credentials", "Credentials"),
            (".r\u{212A}at", ".rkat"),
        ] {
            assert!(
                same_name(name(spelled), name(declared), true, false),
                "{} opens {declared}",
                spelled.escape_unicode()
            );
            assert!(
                !same_name(name(spelled), name(declared), false, false),
                "{} is another name where case is kept",
                spelled.escape_unicode()
            );
        }
        for (spelled, declared) in [
            ("ztate", "state"),
            ("\u{017F}tate", "ftate"),
            ("\u{212A}eys", "xeys"),
            ("st\u{00E4}te", "state"),
            ("\u{0455}tate", "state"),
            ("st\u{2060}ate", "state"),
        ] {
            assert!(
                !same_name(name(spelled), name(declared), true, false),
                "{} does not open {declared}",
                spelled.escape_unicode()
            );
        }
    }

    /// A symbolic link into a protected location is judged by where it
    /// leads, for the engine directory and for a host-declared path, also
    /// when the declared path is itself reached through a link.
    #[cfg(unix)]
    #[test]
    fn links_into_protected_locations_are_judged_by_where_they_lead() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        std::fs::create_dir_all(root.join(".rkat")).unwrap();
        std::fs::create_dir_all(root.join("real-state/realm")).unwrap();
        symlink(root.join(".rkat"), root.join("settings")).unwrap();
        symlink(root.join("real-state"), root.join("state")).unwrap();
        symlink(root.join("real-state"), root.join("other-way-in")).unwrap();
        // The host names its state root through the link.
        let rule = ProtectedPaths::with_host_paths([root.join("state")]);
        for refused in [
            "settings/mcp.toml",
            "state/realm/config.toml",
            "real-state/realm/config.toml",
            "other-way-in/realm/config.toml",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::Protected),
                "{refused}"
            );
        }
        assert_eq!(rule.admit_write(&root, &root.join("src/main.rs")), Ok(()));
    }

    /// The judgement on the path's own components stands by itself: with
    /// `.rkat` a symbolic link whose target does not exist, a path under it
    /// is refused as the engine's directory, before the link is looked at.
    #[cfg(unix)]
    #[test]
    fn a_path_under_the_engine_name_is_refused_on_its_components_alone() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        symlink(root.join("not-there"), root.join(".rkat")).unwrap();
        let rule = ProtectedPaths::default();
        for refused in [".rkat", ".rkat/mcp.toml", ".rkat/realms/r/config.toml"] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::Protected),
                "{refused}"
            );
        }
    }

    /// A symbolic link into an engine directory deeper in the project is
    /// judged by where it leads: the path names no `.rkat`, the real path
    /// of its deepest existing ancestor does.
    #[cfg(unix)]
    #[test]
    fn a_link_into_a_nested_engine_directory_is_refused_on_its_real_path() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        std::fs::create_dir_all(root.join("packages/app/.rkat/skills")).unwrap();
        symlink("packages/app/.rkat", root.join("alias")).unwrap();
        symlink(root.join("packages/app/.rkat/skills"), root.join("tools")).unwrap();
        let rule = ProtectedPaths::default();
        for refused in [
            "alias",
            "alias/mcp.toml",
            "alias/realms/r/config.toml",
            "tools/planted/SKILL.md",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::Protected),
                "{refused}"
            );
        }
        assert_eq!(
            rule.admit_write(&root, &root.join("packages/app/src/main.rs")),
            Ok(())
        );
    }

    /// An engine directory that is itself a symbolic link is protected
    /// where it leads: the host reads `.rkat` through the link, so a write
    /// to the directory behind it under its own name is refused too.
    #[cfg(unix)]
    #[test]
    fn a_linked_engine_directory_is_protected_where_it_leads() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        std::fs::create_dir_all(root.join("config/engine")).unwrap();
        symlink("config/engine", root.join(".rkat")).unwrap();
        let rule = ProtectedPaths::default();
        for refused in [
            ".rkat/mcp.toml",
            "config/engine",
            "config/engine/mcp.toml",
            "config/engine/realms/r/config.toml",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::Protected),
                "{refused}"
            );
        }
        for admitted in ["config/notes.txt", "config/engine.md"] {
            assert_eq!(
                rule.admit_write(&root, &root.join(admitted)),
                Ok(()),
                "{admitted}"
            );
        }
    }

    /// A link that leads nowhere is refused wherever it stands in the path;
    /// a path that merely does not exist yet is admitted.
    #[cfg(unix)]
    #[test]
    fn a_link_that_leads_nowhere_is_refused_and_a_missing_path_is_not() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        symlink(root.join("missing-target"), root.join("file-link")).unwrap();
        symlink("loop-b", root.join("loop-a")).unwrap();
        symlink("loop-a", root.join("loop-b")).unwrap();
        let rule = ProtectedPaths::default();
        for refused in [
            "file-link",
            "file-link/below.txt",
            "loop-a",
            "loop-a/below.txt",
        ] {
            assert_eq!(
                rule.admit_write(&root, &root.join(refused)),
                Err(WriteRefusal::DanglingLink),
                "{refused}"
            );
        }
        assert_eq!(
            rule.admit_write(&root, &root.join("new/dir/file.txt")),
            Ok(())
        );
    }

    /// The refusal names the path as given and nothing of the host's layout.
    #[test]
    fn refusal_reasons_name_only_the_given_path() {
        let reason = WriteRefusal::Protected.reason(Path::new(".rkat/mcp.toml"));
        assert!(reason.contains("'.rkat/mcp.toml'"));
        assert!(reason.contains("engine's own directory"));
        assert!(reason.contains("rkat CLI"));
        let reason = WriteRefusal::DanglingLink.reason(Path::new("link"));
        assert!(reason.contains("'link'") && reason.contains("symbolic link"));
    }
}

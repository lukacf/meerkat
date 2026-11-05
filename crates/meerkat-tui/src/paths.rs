//! Project-root path resolution shared by the file tools and the approval policy.
//!
//! Both the tool dispatcher and the tier classifier must agree on what a path
//! means, so this module is the single place that turns a model-supplied string
//! into an absolute path and decides whether it is inside the project and whether
//! it is protected.

use globset::{Glob, GlobSet, GlobSetBuilder};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Root-relative glob patterns that are never writable without classifier review,
/// even inside the project: agent configuration and instruction files, VCS
/// internals and attributes, build/toolchain configuration that runs code, editor
/// task configuration, and secrets.
pub const PROTECTED_PATTERNS: &[&str] = &[
    ".rkat/**",
    ".git/**",
    "**/.git/**",
    ".gitattributes",
    ".gitmodules",
    "**/.gitattributes",
    "**/.gitmodules",
    ".claude/**",
    ".env",
    ".env.*",
    ".envrc",
    "**/.env",
    "**/.env.*",
    "**/.envrc",
    "tui.toml",
    ".cargo/**",
    "**/.cargo/**",
    "rust-toolchain",
    "rust-toolchain.toml",
    "**/rust-toolchain*",
    "build.rs",
    "**/build.rs",
    ".vscode/**",
    ".idea/**",
    "AGENTS.md",
    "AGENTS.local.md",
    "**/AGENTS.md",
];

/// Compiled protected patterns. `All` is the fail-closed state used when any
/// pattern fails to compile: every path counts as protected.
#[derive(Debug)]
enum ProtectedSet {
    Globs(GlobSet),
    All,
}

impl ProtectedSet {
    fn build(patterns: &[&str]) -> Self {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            match Glob::new(pattern) {
                Ok(glob) => {
                    builder.add(glob);
                }
                Err(err) => {
                    tracing::error!(%pattern, %err, "invalid protected pattern; protecting every path");
                    return Self::All;
                }
            }
        }
        match builder.build() {
            Ok(set) => Self::Globs(set),
            Err(err) => {
                tracing::error!(%err, "protected patterns failed to compile; protecting every path");
                Self::All
            }
        }
    }

    fn is_match(&self, rel: &Path) -> bool {
        match self {
            Self::Globs(set) => set.is_match(rel),
            Self::All => true,
        }
    }
}

fn protected_set() -> &'static ProtectedSet {
    static SET: OnceLock<ProtectedSet> = OnceLock::new();
    SET.get_or_init(|| ProtectedSet::build(PROTECTED_PATTERNS))
}

/// Whether `name` is an environment file that typically holds secrets:
/// `.env`, `.env.<anything>` or `.envrc`. The approval classifier's
/// read-only tools never read such files, nor anything below a directory of
/// that name.
pub fn is_secret_file_name(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n == ".env" || n == ".envrc" || n.starts_with(".env."))
}

/// Errors from resolving a model-supplied path.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// The supplied path was empty.
    #[error("path is empty")]
    Empty,
    /// The supplied path contained a NUL byte.
    #[error("path contains a NUL byte")]
    NulByte,
    /// `~` was used but the home directory is unknown.
    #[error("cannot expand `~`: home directory is unknown")]
    NoHome,
    /// Canonicalization of an existing ancestor failed.
    #[error("failed to resolve {path}: {source}")]
    Io {
        /// Path being resolved.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// The canonical project root. Constructed once at bootstrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRoot(PathBuf);

impl ProjectRoot {
    /// Canonicalize `path` and use it as the project root.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::Io`] when the directory cannot be canonicalized.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, PathError> {
        let path = path.as_ref();
        let canonical = std::fs::canonicalize(path).map_err(|source| PathError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self(canonical))
    }

    /// The canonical root path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Resolve a model-supplied path against this root.
    ///
    /// Relative paths are joined onto the root (not the process working
    /// directory). `..` is resolved physically, the way the kernel does: the
    /// prefix before it is canonicalized first, so `link/../x` through a
    /// symlinked directory lands next to the link's target. The deepest existing
    /// ancestor of the result is canonicalized so symlinked parents cannot escape
    /// the root unnoticed. The final path does not need to exist.
    ///
    /// # Errors
    ///
    /// Returns a [`PathError`] for empty paths, NUL bytes, an unknown home
    /// directory when `~` is used, or I/O failures during canonicalization.
    pub fn resolve(&self, raw: &str) -> Result<ResolvedPath, PathError> {
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        if raw.contains('\0') {
            return Err(PathError::NulByte);
        }
        let expanded = expand_tilde(raw)?;
        let joined = if expanded.is_absolute() {
            expanded
        } else {
            self.0.join(expanded)
        };
        let abs = resolve_physically(&joined)?;
        let relative = abs.strip_prefix(&self.0).ok().map(Path::to_path_buf);
        let protected = relative
            .as_deref()
            .is_some_and(|rel| protected_set().is_match(rel));
        Ok(ResolvedPath {
            abs,
            relative,
            protected,
        })
    }

    /// Display a resolved path relative to the root when possible.
    #[must_use]
    pub fn display_of<'a>(&self, resolved: &'a ResolvedPath) -> std::path::Display<'a> {
        resolved
            .relative
            .as_deref()
            .unwrap_or(&resolved.abs)
            .display()
    }
}

/// A path resolved against a [`ProjectRoot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Absolute path with symlinked ancestors resolved.
    pub abs: PathBuf,
    /// Path relative to the project root, or `None` when it lies outside.
    pub relative: Option<PathBuf>,
    /// Whether the path matches [`PROTECTED_PATTERNS`] (always `false` outside the root).
    pub protected: bool,
}

impl ResolvedPath {
    /// Whether the path lies inside the project root.
    #[must_use]
    pub fn inside(&self) -> bool {
        self.relative.is_some()
    }

    /// Whether the path may be written without classifier review: inside the
    /// root and not protected.
    #[must_use]
    pub fn freely_writable(&self) -> bool {
        self.inside() && !self.protected
    }
}

fn expand_tilde(raw: &str) -> Result<PathBuf, PathError> {
    if raw == "~" {
        return dirs::home_dir().ok_or(PathError::NoHome);
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or(PathError::NoHome)?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(raw))
}

/// Resolve `.` and `..` the way path lookup does: each `..` applies to the
/// canonicalized (symlink-resolved) prefix before it, then the deepest existing
/// ancestor of the result is canonicalized.
fn resolve_physically(path: &Path) -> Result<PathBuf, PathError> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out = canonicalize_existing_prefix(&out)?;
                // Popping the filesystem root is a no-op, matching `/..` == `/`.
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    canonicalize_existing_prefix(&out)
}

/// Canonicalize the deepest existing ancestor of `path` and re-append the rest.
fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, PathError> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match std::fs::canonicalize(&existing) {
            Ok(canonical) => {
                let mut out = canonical;
                for component in tail.iter().rev() {
                    out.push(component);
                }
                return Ok(out);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(PathError::Io {
                        path: path.to_path_buf(),
                        source: err,
                    });
                };
                tail.push(name.to_os_string());
                if !existing.pop() {
                    return Err(PathError::Io {
                        path: path.to_path_buf(),
                        source: err,
                    });
                }
            }
            Err(source) => {
                return Err(PathError::Io {
                    path: existing,
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn root() -> (tempfile::TempDir, ProjectRoot) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let root = ProjectRoot::new(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn relative_paths_join_onto_root() {
        let (_dir, root) = root();
        let r = root.resolve("src/main.rs").unwrap();
        assert!(r.inside());
        assert_eq!(r.relative.as_deref(), Some(Path::new("src/main.rs")));
        assert!(!r.protected);
        assert!(r.freely_writable());
    }

    #[test]
    fn nonexistent_file_still_resolves_inside() {
        let (_dir, root) = root();
        let r = root.resolve("src/new/deep/file.rs").unwrap();
        assert!(r.inside());
        assert_eq!(
            r.relative.as_deref(),
            Some(Path::new("src/new/deep/file.rs"))
        );
    }

    #[test]
    fn dotdot_escape_is_outside() {
        let (_dir, root) = root();
        let r = root.resolve("../outside.txt").unwrap();
        assert!(!r.inside());
        assert!(!r.protected);
        assert!(!r.freely_writable());
    }

    #[test]
    fn dotdot_within_root_normalizes() {
        let (_dir, root) = root();
        let r = root.resolve("src/../src/./main.rs").unwrap();
        assert_eq!(r.relative.as_deref(), Some(Path::new("src/main.rs")));
    }

    #[test]
    fn absolute_path_inside_root() {
        let (dir, root) = root();
        let abs = dir.path().join("src/main.rs");
        let r = root.resolve(abs.to_str().unwrap()).unwrap();
        assert!(r.inside());
    }

    #[test]
    fn protected_patterns_match() {
        let (_dir, root) = root();
        for p in [
            ".rkat/config.toml",
            ".git/HEAD",
            ".claude/settings.json",
            ".env",
            ".env.local",
            "svc/.env",
            ".envrc",
            "tui.toml",
            ".cargo/config.toml",
            "crates/x/.cargo/config.toml",
            "rust-toolchain",
            "rust-toolchain.toml",
            "sub/rust-toolchain.toml",
            "vendor/dep/.git/config",
            ".gitattributes",
            ".gitmodules",
            "sub/.gitattributes",
            "sub/.gitmodules",
            ".vscode/tasks.json",
            ".idea/workspace.xml",
            "build.rs",
            "crates/x/build.rs",
            "AGENTS.md",
            "AGENTS.local.md",
            "docs/AGENTS.md",
        ] {
            let r = root.resolve(p).unwrap();
            assert!(r.inside(), "{p} should be inside");
            assert!(r.protected, "{p} should be protected");
            assert!(!r.freely_writable());
        }
        for p in [
            "src/env.rs",
            "environment.md",
            "gitignore",
            ".github/ci.yml",
            "src/build.rs.bak",
            "src/builder.rs",
            "cargo/notes.md",
            "AGENTS.txt",
        ] {
            let r = root.resolve(p).unwrap();
            assert!(!r.protected, "{p} should not be protected");
        }
    }

    #[test]
    fn empty_and_nul_are_errors() {
        let (_dir, root) = root();
        assert!(matches!(root.resolve(""), Err(PathError::Empty)));
        assert!(matches!(root.resolve("a\0b"), Err(PathError::NulByte)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_escaping_root_is_outside() {
        let (dir, root) = root();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        let r = root.resolve("link/secret.txt").unwrap();
        assert!(!r.inside(), "symlink escape must resolve outside the root");
        let r2 = root.resolve("link/not-yet-created.txt").unwrap();
        assert!(!r2.inside());
    }

    #[test]
    fn protected_set_fails_closed() {
        let bad = ProtectedSet::build(&["src/**", "[unclosed"]);
        assert!(matches!(bad, ProtectedSet::All));
        assert!(bad.is_match(Path::new("anything.txt")));
        let good = ProtectedSet::build(&["src/**"]);
        assert!(good.is_match(Path::new("src/main.rs")));
        assert!(!good.is_match(Path::new("README.md")));
    }

    #[test]
    fn all_builtin_protected_patterns_compile() {
        assert!(matches!(protected_set(), ProtectedSet::Globs(_)));
    }

    #[cfg(unix)]
    #[test]
    fn dotdot_through_symlinked_directory_is_physical() {
        let (dir, root) = root();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("inner")).unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path().join("inner"), dir.path().join("lnk")).unwrap();
        let r = root.resolve("lnk/../secret").unwrap();
        assert!(!r.inside(), "`..` must apply to the symlink target");
        assert_eq!(r.abs, outside.path().canonicalize().unwrap().join("secret"));
        let r2 = root.resolve("lnk/../not-yet-created").unwrap();
        assert!(!r2.inside());
    }

    #[test]
    fn dotdot_through_missing_directory_is_lexical() {
        let (_dir, root) = root();
        let r = root.resolve("missing/../src/main.rs").unwrap();
        assert_eq!(r.relative.as_deref(), Some(Path::new("src/main.rs")));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_inside_root_stays_inside() {
        let (dir, root) = root();
        std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("alias")).unwrap();
        let r = root.resolve("alias/main.rs").unwrap();
        assert!(r.inside());
        assert_eq!(r.relative.as_deref(), Some(Path::new("src/main.rs")));
    }
}

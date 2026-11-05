//! User-global `rkat-tui` directories.
//!
//! Everything the TUI keeps per user (rather than per workspace) lives in one
//! config directory: user-global `AGENTS.md` and `SYSTEM.md`, the global
//! `tui.toml` rules file, notes, the input history and the host trust
//! records.
//!
//! Resolution order (first match wins):
//!
//! 1. `--config-dir <dir>`
//! 2. `<user config root>/.rkat/tui` when it exists, where the user config
//!    root is `--user-config-root`, else the home directory (the root
//!    Meerkat's own user config `~/.rkat` lives under)
//! 3. a legacy directory of an earlier release, when it exists (used as is,
//!    with a start-up notice; nothing is moved, see
//!    [`legacy_config_notice`]): `<--user-config-root>/.config/rkat-tui`
//!    when that flag is given, else `$XDG_CONFIG_HOME/rkat-tui` (when set
//!    and non-empty) or `~/.config/rkat-tui` (`<platform config
//!    dir>/rkat-tui` on Windows)
//! 4. `<user config root>/.rkat/tui` (created on first write)
//!
//! `$XDG_CONFIG_HOME` no longer decides the default; it only locates a
//! legacy directory.
//!
//! Re-creatable data (the community price list) lives in a cache directory
//! instead ([`GlobalDirs::cache`]):
//!
//! 1. `<--config-dir>/cache` (an explicit flag keeps everything in one place)
//! 2. `<--user-config-root>/.cache/rkat-tui`
//! 3. `$XDG_CACHE_HOME/rkat-tui` when set and non-empty
//! 4. `<platform cache dir>/rkat-tui` (`~/.cache` on Linux, `~/Library/Caches`
//!    on macOS)
//! 5. `<config>/cache` when no cache directory resolves

use std::path::{Path, PathBuf};

/// Name of the application directory inside a legacy config root
/// (`~/.config/rkat-tui`) and a cache root.
pub const APP_DIR: &str = "rkat-tui";

/// The default config directory under the user config root: `.rkat/tui`.
pub const DEFAULT_CONFIG_DIR: [&str; 2] = [".rkat", "tui"];

/// User-global directories of `rkat-tui`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDirs {
    /// The user-global config directory.
    pub config: PathBuf,
    /// The user cache directory (re-creatable data).
    pub cache: PathBuf,
}

/// `<root>/.rkat/tui`.
#[must_use]
pub fn default_config_dir(root: &Path) -> PathBuf {
    DEFAULT_CONFIG_DIR.iter().fold(root.to_path_buf(), |dir, part| dir.join(part))
}

impl GlobalDirs {
    /// Resolve the global config directory from the flags and the platform.
    ///
    /// `config_dir` is `--config-dir`; `user_config_root` is the raw
    /// `--user-config-root` flag (not the home-directory fallback). Returns
    /// `None` when nothing resolves.
    #[must_use]
    pub fn resolve(config_dir: Option<&Path>, user_config_root: Option<&Path>) -> Option<Self> {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let home = dirs::home_dir();
        let platform = dirs::config_dir();
        let mut resolved = Self::resolve_with(
            config_dir,
            user_config_root,
            &LegacyRoots {
                xdg_config_home: xdg.as_deref(),
                home: home.as_deref(),
                platform_config: platform.as_deref(),
            },
            Path::is_dir,
        )?;
        let xdg_cache = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from);
        let platform_cache = dirs::cache_dir();
        if let Some(cache) = cache_dir_with(
            config_dir,
            user_config_root,
            xdg_cache.as_deref(),
            platform_cache.as_deref(),
        ) {
            resolved.cache = cache;
        }
        Some(resolved)
    }

    /// [`GlobalDirs::resolve`] with every platform input supplied explicitly
    /// and `exists` deciding whether a directory exists.
    #[must_use]
    pub fn resolve_with(
        config_dir: Option<&Path>,
        user_config_root: Option<&Path>,
        roots: &LegacyRoots<'_>,
        exists: impl Fn(&Path) -> bool,
    ) -> Option<Self> {
        let config = if let Some(dir) = config_dir {
            dir.to_path_buf()
        } else {
            let default = default_config_dir(user_config_root.or(roots.home)?);
            if exists(&default) {
                default
            } else {
                legacy_candidates(user_config_root, roots)
                    .into_iter()
                    .find(|legacy| exists(legacy))
                    .unwrap_or(default)
            }
        };
        let cache = config.join("cache");
        Some(Self { config, cache })
    }

    /// Global approval rules file (`tui.toml`).
    #[must_use]
    pub fn rules_file(&self) -> PathBuf {
        self.config.join(crate::approval::rules::RULES_FILE_NAME)
    }

    /// User-global `AGENTS.md`.
    #[must_use]
    pub fn agents_md(&self) -> PathBuf {
        self.config.join("AGENTS.md")
    }

    /// User-global `SYSTEM.md` (replaces the base system prompt).
    #[must_use]
    pub fn system_md(&self) -> PathBuf {
        self.config.join("SYSTEM.md")
    }

    /// Directory of user-global notes.
    #[must_use]
    pub fn notes_dir(&self) -> PathBuf {
        self.config.join("notes")
    }

    /// Global input history file.
    #[must_use]
    pub fn history_file(&self) -> PathBuf {
        self.config.join("history")
    }

    /// Cache of the community price list (see [`crate::pricing`]), in the
    /// cache directory.
    #[must_use]
    pub fn pricing_cache(&self) -> PathBuf {
        self.cache.join(crate::pricing::CACHE_FILE)
    }
}

/// Where earlier releases kept the config directory, for
/// [`GlobalDirs::resolve_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct LegacyRoots<'a> {
    /// `$XDG_CONFIG_HOME` (Unix and macOS).
    pub xdg_config_home: Option<&'a Path>,
    /// The home directory; also the user config root without
    /// `--user-config-root`.
    pub home: Option<&'a Path>,
    /// The platform config directory (Windows).
    pub platform_config: Option<&'a Path>,
}

/// The legacy config directories, most specific first.
fn legacy_candidates(user_config_root: Option<&Path>, roots: &LegacyRoots<'_>) -> Vec<PathBuf> {
    if let Some(root) = user_config_root {
        return vec![root.join(".config").join(APP_DIR)];
    }
    if cfg!(windows) {
        return roots
            .platform_config
            .map(|dir| dir.join(APP_DIR))
            .into_iter()
            .collect();
    }
    roots
        .xdg_config_home
        .filter(|xdg| !xdg.as_os_str().is_empty())
        .map(|xdg| xdg.join(APP_DIR))
        .into_iter()
        .chain(roots.home.map(|home| home.join(".config").join(APP_DIR)))
        .collect()
}

/// The one-line start-up notice when `dirs` is a legacy config directory
/// (see the module docs): it names the new default and `--config-dir`.
/// `None` under `--config-dir` (`config_dir`) or when `dirs` is the default.
/// `user_config_root` is the effective root (`--user-config-root`, else the
/// home directory).
#[must_use]
pub fn legacy_config_notice(
    dirs: &GlobalDirs,
    config_dir: Option<&Path>,
    user_config_root: Option<&Path>,
) -> Option<String> {
    if config_dir.is_some() {
        return None;
    }
    let default = default_config_dir(user_config_root?);
    (dirs.config != default).then(|| {
        format!(
            "using the legacy config dir {}; the default is now {} (move the files there, or keep this one with --config-dir {})",
            dirs.config.display(),
            default.display(),
            dirs.config.display()
        )
    })
}

/// The cache directory (see the module docs), `None` when only the
/// `<config>/cache` fallback is left.
#[must_use]
pub fn cache_dir_with(
    config_dir: Option<&Path>,
    user_config_root: Option<&Path>,
    xdg_cache_home: Option<&Path>,
    platform_cache: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(dir) = config_dir {
        Some(dir.join("cache"))
    } else if let Some(root) = user_config_root {
        Some(root.join(".cache").join(APP_DIR))
    } else if let Some(xdg) = xdg_cache_home.filter(|p| !p.as_os_str().is_empty()) {
        Some(xdg.join(APP_DIR))
    } else {
        platform_cache.map(|dir| dir.join(APP_DIR))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    /// The config dir resolved with only `existing` directories present.
    fn resolved(
        flag: Option<&str>,
        root: Option<&str>,
        xdg: Option<&str>,
        home: Option<&str>,
        existing: &[&str],
    ) -> Option<PathBuf> {
        GlobalDirs::resolve_with(
            flag.map(p),
            root.map(p),
            &LegacyRoots {
                xdg_config_home: xdg.map(p),
                home: home.map(p),
                platform_config: Some(p("/p")),
            },
            |dir| existing.iter().any(|e| Path::new(e) == dir),
        )
        .map(|d| d.config)
    }

    #[test]
    fn flag_wins_over_everything() {
        assert_eq!(
            resolved(Some("/f"), Some("/r"), Some("/x"), Some("/h"), &["/h/.rkat/tui"]),
            Some(PathBuf::from("/f"))
        );
    }

    #[test]
    fn the_default_is_under_the_user_config_root() {
        // Nothing exists yet: the new default, whatever XDG says.
        assert_eq!(
            resolved(None, None, Some("/x"), Some("/h"), &[]),
            Some(PathBuf::from("/h/.rkat/tui"))
        );
        // `--user-config-root` replaces the home directory.
        assert_eq!(
            resolved(None, Some("/r"), Some("/x"), Some("/h"), &[]),
            Some(PathBuf::from("/r/.rkat/tui"))
        );
        // An existing default wins over an existing legacy dir.
        assert_eq!(
            resolved(
                None,
                None,
                Some("/x"),
                Some("/h"),
                &["/h/.rkat/tui", "/x/rkat-tui", "/h/.config/rkat-tui"]
            ),
            Some(PathBuf::from("/h/.rkat/tui"))
        );
        assert_eq!(resolved(None, None, Some("/x"), None, &[]), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn an_existing_legacy_dir_is_used_when_the_default_is_missing() {
        assert_eq!(
            resolved(None, None, Some("/x"), Some("/h"), &["/x/rkat-tui", "/h/.config/rkat-tui"]),
            Some(PathBuf::from("/x/rkat-tui"))
        );
        assert_eq!(
            resolved(None, None, Some("/x"), Some("/h"), &["/h/.config/rkat-tui"]),
            Some(PathBuf::from("/h/.config/rkat-tui"))
        );
        assert_eq!(
            resolved(None, None, Some(""), Some("/h"), &["/h/.config/rkat-tui"]),
            Some(PathBuf::from("/h/.config/rkat-tui"))
        );
        // Under `--user-config-root` only that root's legacy dir counts.
        assert_eq!(
            resolved(None, Some("/r"), Some("/x"), Some("/h"), &["/x/rkat-tui"]),
            Some(PathBuf::from("/r/.rkat/tui"))
        );
        assert_eq!(
            resolved(None, Some("/r"), None, Some("/h"), &["/r/.config/rkat-tui"]),
            Some(PathBuf::from("/r/.config/rkat-tui"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_legacy_dir_is_the_platform_config() {
        assert_eq!(
            resolved(None, None, Some("/x"), Some("/h"), &["/p/rkat-tui"]),
            Some(PathBuf::from("/p").join(APP_DIR))
        );
    }

    #[test]
    fn a_legacy_dir_gets_one_notice_naming_the_default_and_the_flag() {
        let legacy = GlobalDirs {
            config: PathBuf::from("/h/.config/rkat-tui"),
            cache: PathBuf::from("/k"),
        };
        let notice = legacy_config_notice(&legacy, None, Some(p("/h"))).unwrap();
        assert!(notice.contains("/h/.rkat/tui"), "{notice}");
        assert!(notice.contains("--config-dir /h/.config/rkat-tui"), "{notice}");
        assert!(!notice.contains('\n'));
        // No notice for the default or under `--config-dir`.
        let default = GlobalDirs {
            config: PathBuf::from("/h/.rkat/tui"),
            cache: PathBuf::from("/k"),
        };
        assert_eq!(legacy_config_notice(&default, None, Some(p("/h"))), None);
        assert_eq!(
            legacy_config_notice(&legacy, Some(p("/h/.config/rkat-tui")), Some(p("/h"))),
            None
        );
    }

    #[test]
    fn the_trust_records_move_with_the_config_dir() {
        // The record lives under the config dir; its key (workspace and
        // realm) does not depend on where that dir is.
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let path = |dir: &Path| {
            crate::approval::trust::HostTrustRecord::open(dir, ws.path(), "tui")
                .record_path()
                .map(Path::to_path_buf)
                .unwrap()
        };
        let (pa, pb) = (path(a.path()), path(b.path()));
        assert!(pa.starts_with(a.path()) && pb.starts_with(b.path()));
        assert_eq!(pa.strip_prefix(a.path()).unwrap(), pb.strip_prefix(b.path()).unwrap());
    }

    #[test]
    fn accessors() {
        let dirs = GlobalDirs {
            config: PathBuf::from("/c"),
            cache: PathBuf::from("/k"),
        };
        assert_eq!(dirs.rules_file(), PathBuf::from("/c/tui.toml"));
        assert_eq!(dirs.agents_md(), PathBuf::from("/c/AGENTS.md"));
        assert_eq!(dirs.system_md(), PathBuf::from("/c/SYSTEM.md"));
        assert_eq!(dirs.notes_dir(), PathBuf::from("/c/notes"));
        assert_eq!(dirs.history_file(), PathBuf::from("/c/history"));
        assert_eq!(dirs.pricing_cache(), PathBuf::from("/k/pricing-cache.json"));
    }

    #[test]
    fn the_price_cache_lives_in_the_cache_dir() {
        let cache = |flag: Option<&str>, root: Option<&str>, xdg: Option<&str>, platform: Option<&str>| {
            cache_dir_with(flag.map(p), root.map(p), xdg.map(p), platform.map(p))
        };
        assert_eq!(
            cache(Some("/f"), Some("/r"), Some("/x"), Some("/p")),
            Some(PathBuf::from("/f/cache"))
        );
        assert_eq!(
            cache(None, Some("/r"), Some("/x"), Some("/p")),
            Some(PathBuf::from("/r/.cache/rkat-tui"))
        );
        assert_eq!(
            cache(None, None, Some("/x"), Some("/p")),
            Some(PathBuf::from("/x/rkat-tui"))
        );
        assert_eq!(
            cache(None, None, Some(""), Some("/p")),
            Some(PathBuf::from("/p/rkat-tui"))
        );
        assert_eq!(cache(None, None, None, None), None);
        // Never the config directory itself.
        let dirs = GlobalDirs::resolve(None, Some(p("/r"))).unwrap();
        assert_eq!(dirs.config, PathBuf::from("/r/.rkat/tui"));
        assert!(!dirs.pricing_cache().starts_with(&dirs.config));
    }
}

//! User-global `rkat-tui` directories.
//!
//! Everything the TUI keeps per user (rather than per workspace) lives in one
//! config directory: user-global `AGENTS.md` and `SYSTEM.md`, the global
//! `tui.toml` rules file, notes and the input history.
//!
//! Resolution order (first match wins):
//!
//! 1. `--config-dir <dir>`
//! 2. `<--user-config-root>/.config/rkat-tui` (an explicit flag; keeps tests hermetic)
//! 3. `$XDG_CONFIG_HOME/rkat-tui` when set and non-empty (Unix and macOS)
//! 4. `~/.config/rkat-tui` (Unix and macOS)
//! 5. `<platform config dir>/rkat-tui` (Windows)

use std::path::{Path, PathBuf};

/// Name of the application directory inside a config root.
pub const APP_DIR: &str = "rkat-tui";

/// User-global directories of `rkat-tui`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalDirs {
    /// The user-global config directory.
    pub config: PathBuf,
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
        Self::resolve_with(
            config_dir,
            user_config_root,
            xdg.as_deref(),
            home.as_deref(),
            platform.as_deref(),
        )
    }

    /// [`GlobalDirs::resolve`] with every platform input supplied explicitly.
    ///
    /// `platform_config` is only consulted on Windows; on other platforms
    /// `xdg_config_home` and `home` are.
    #[must_use]
    pub fn resolve_with(
        config_dir: Option<&Path>,
        user_config_root: Option<&Path>,
        xdg_config_home: Option<&Path>,
        home: Option<&Path>,
        platform_config: Option<&Path>,
    ) -> Option<Self> {
        let config = if let Some(dir) = config_dir {
            dir.to_path_buf()
        } else if let Some(root) = user_config_root {
            root.join(".config").join(APP_DIR)
        } else if cfg!(windows) {
            platform_config?.join(APP_DIR)
        } else if let Some(xdg) = xdg_config_home.filter(|p| !p.as_os_str().is_empty()) {
            xdg.join(APP_DIR)
        } else {
            home?.join(".config").join(APP_DIR)
        };
        Some(Self { config })
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

    /// Cache of the community price list (see [`crate::pricing`]).
    #[must_use]
    pub fn pricing_cache(&self) -> PathBuf {
        self.config.join(crate::pricing::CACHE_FILE)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    fn resolved(
        flag: Option<&str>,
        root: Option<&str>,
        xdg: Option<&str>,
        home: Option<&str>,
        platform: Option<&str>,
    ) -> Option<PathBuf> {
        GlobalDirs::resolve_with(
            flag.map(p),
            root.map(p),
            xdg.map(p),
            home.map(p),
            platform.map(p),
        )
        .map(|d| d.config)
    }

    #[test]
    fn flag_wins_over_everything() {
        assert_eq!(
            resolved(Some("/f"), Some("/r"), Some("/x"), Some("/h"), Some("/p")),
            Some(PathBuf::from("/f"))
        );
    }

    #[test]
    fn explicit_user_root_beats_platform() {
        assert_eq!(
            resolved(None, Some("/r"), Some("/x"), Some("/h"), Some("/p")),
            Some(PathBuf::from("/r/.config/rkat-tui"))
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_precedence() {
        assert_eq!(
            resolved(None, None, Some("/x"), Some("/h"), Some("/p")),
            Some(PathBuf::from("/x/rkat-tui"))
        );
        assert_eq!(
            resolved(None, None, Some(""), Some("/h"), Some("/p")),
            Some(PathBuf::from("/h/.config/rkat-tui"))
        );
        assert_eq!(
            resolved(None, None, None, Some("/h"), Some("/p")),
            Some(PathBuf::from("/h/.config/rkat-tui"))
        );
        assert_eq!(resolved(None, None, None, None, Some("/p")), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_uses_platform_config() {
        assert_eq!(
            resolved(None, None, Some("/x"), Some("/h"), Some("/p")),
            Some(PathBuf::from("/p").join(APP_DIR))
        );
        assert_eq!(resolved(None, None, Some("/x"), Some("/h"), None), None);
    }

    #[test]
    fn accessors() {
        let dirs = GlobalDirs {
            config: PathBuf::from("/c"),
        };
        assert_eq!(dirs.rules_file(), PathBuf::from("/c/tui.toml"));
        assert_eq!(dirs.agents_md(), PathBuf::from("/c/AGENTS.md"));
        assert_eq!(dirs.system_md(), PathBuf::from("/c/SYSTEM.md"));
        assert_eq!(dirs.notes_dir(), PathBuf::from("/c/notes"));
        assert_eq!(dirs.history_file(), PathBuf::from("/c/history"));
    }
}

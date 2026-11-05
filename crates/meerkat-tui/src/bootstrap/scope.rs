//! Workspace and realm resolution.
//!
//! By default all state for a workspace (sessions, realm config, rules, input
//! history, logs) lives inside it, under `<workspace>/.rkat/tui/`, in a realm
//! with the constant id [`WORKSPACE_REALM_ID`]. Nothing is keyed by the
//! workspace's absolute path, so renaming or moving the directory keeps every
//! session reachable with `rkat-tui -c`. A `.gitignore` is written into the
//! state directory so it is never committed.
//!
//! `--realm`, `--isolated` and `--state-root` opt back into the shared realm
//! model `rkat` uses (`resolve_runtime_scope` / `load_config` in
//! `meerkat-cli/src/main.rs`).

use std::path::{Path, PathBuf};

use meerkat_core::{Config, ConfigStore, FileConfigStore, RealmConfig, RealmSelection};
use meerkat_store::{RealmBackend, RealmOrigin, realm_paths_in};

use super::dirs::GlobalDirs;
use crate::cli::{Args, RealmBackendArg};
use crate::error::TuiError;
use crate::paths::{PathError, ProjectRoot};

/// Realm id used for workspace-local state.
pub const WORKSPACE_REALM_ID: &str = "tui";
/// Directory under the workspace that holds realm directories.
pub const WORKSPACE_STATE_DIR: &str = ".rkat";

/// Where this TUI process lives: project, conventions roots and realm.
#[derive(Debug, Clone)]
pub struct RuntimeScope {
    /// Canonical project root (path policy and tool resolution).
    pub project_root: ProjectRoot,
    /// Conventions root for `AGENTS.md`, `.rkat/` hooks, skills and MCP config.
    /// Equal to the project root path.
    pub context_root: PathBuf,
    /// User-global conventions root (`--user-config-root`, else the home
    /// directory). `None` when neither is available.
    pub user_config_root: Option<PathBuf>,
    /// User-global `rkat-tui` directories (`--config-dir`, else
    /// `<user config root>/.rkat/tui`, or an existing legacy directory; see
    /// [`GlobalDirs`]). `None` when nothing resolves.
    pub global: Option<GlobalDirs>,
    /// Realm the sessions and config are stored in.
    pub realm_id: String,
    /// Directory containing realm directories.
    pub state_root: PathBuf,
    /// Backend for a *new* realm (`None`: the store default, SQLite);
    /// existing realms keep their pinned backend, and naming another one
    /// fails the open.
    pub backend_hint: Option<RealmBackend>,
    /// How the realm was selected (recorded in a new realm's manifest).
    pub origin: RealmOrigin,
}

impl RuntimeScope {
    /// Root directory of the active realm (`<state_root>/<realm_id>`).
    #[must_use]
    pub fn realm_root(&self) -> PathBuf {
        realm_paths_in(&self.state_root, &self.realm_id).root
    }

    /// Path of the realm's `config.toml`.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        realm_paths_in(&self.state_root, &self.realm_id).config_path
    }
}

impl From<RealmBackendArg> for RealmBackend {
    fn from(value: RealmBackendArg) -> Self {
        match value {
            RealmBackendArg::Jsonl => Self::Jsonl,
            RealmBackendArg::Sqlite => Self::Sqlite,
            RealmBackendArg::Memory => Self::Memory,
        }
    }
}

/// Resolve the workspace, conventions roots and realm from the arguments.
///
/// The workspace is `--workspace`, else the nearest ancestor of the working
/// directory that already contains `.rkat/tui/` (searching no higher than the
/// git top-level, and never the home directory or above it), else the working
/// directory. The home directory itself (or an ancestor of it) is used only
/// when passed explicitly with `--workspace`.
/// State lives in `<workspace>/.rkat/tui/` unless `--realm`, `--isolated` or
/// `--state-root` select the shared realm model.
///
/// # Errors
///
/// Returns [`TuiError::Realm`] for conflicting/invalid realm flags,
/// [`TuiError::ProjectRoot`] when the root cannot be canonicalized or the
/// working directory is the home directory,
/// [`TuiError::ConfigDirCollision`] when the user-global config directory
/// and the realm directory are the same or nested (see
/// [`check_config_dir_collision`]), and [`TuiError::Io`] when the state
/// directory cannot be created.
pub fn resolve_scope(args: &Args) -> Result<RuntimeScope, TuiError> {
    let context_root = match &args.context_root {
        Some(root) => root.clone(),
        None => {
            let cwd = std::env::current_dir().map_err(|source| TuiError::ProjectRoot {
                path: PathBuf::from("."),
                source,
            })?;
            default_workspace(&cwd, dirs::home_dir().as_deref())?
        }
    };
    let project_root = project_root_from(&context_root)?;
    let context_root = project_root.as_path().to_path_buf();
    let user_config_root = args.user_config_root.clone().or_else(dirs::home_dir);
    let global = GlobalDirs::resolve(args.config_dir.as_deref(), args.user_config_root.as_deref());

    let uses_shared_realm = args.realm.is_some() || args.isolated || args.state_root.is_some();
    if !uses_shared_realm {
        let state_root = context_root.join(WORKSPACE_STATE_DIR);
        check_config_dir_collision(
            global.as_ref(),
            &realm_paths_in(&state_root, WORKSPACE_REALM_ID).root,
        )?;
        ensure_workspace_state_dir(&state_root)?;
        return Ok(RuntimeScope {
            project_root,
            context_root,
            user_config_root,
            global,
            realm_id: WORKSPACE_REALM_ID.to_string(),
            state_root,
            backend_hint: args.realm_backend.map(Into::into),
            origin: RealmOrigin::Workspace,
        });
    }

    let selection = RealmConfig::selection_from_inputs(
        args.realm.clone(),
        args.isolated,
        RealmSelection::WorkspaceDerived {
            root: context_root.clone(),
        },
    )?;
    let origin = match &selection {
        RealmSelection::Explicit { .. } => RealmOrigin::Explicit,
        RealmSelection::Isolated => RealmOrigin::Generated,
        RealmSelection::WorkspaceDerived { .. } => RealmOrigin::Workspace,
    };
    let backend_hint: Option<RealmBackend> = args.realm_backend.map(Into::into);
    let realm_cfg = RealmConfig {
        selection,
        instance_id: None,
        backend_hint: backend_hint.map(|b| b.as_str().to_string()),
        state_root: args.state_root.clone(),
    };
    let locator = realm_cfg.resolve_locator()?;
    check_config_dir_collision(
        global.as_ref(),
        &realm_paths_in(&locator.state_root, locator.realm.as_str()).root,
    )?;

    Ok(RuntimeScope {
        project_root,
        context_root,
        user_config_root,
        global,
        realm_id: locator.realm.as_str().to_string(),
        state_root: locator.state_root,
        // Only an explicit request pins a backend; existing realms are opened
        // with the backend recorded in their manifest.
        backend_hint,
        origin,
    })
}

/// Refuse a user-global config directory that is the realm directory, or
/// holds it, or lies inside it: with `--workspace ~` the realm directory
/// `~/.rkat/tui` is the default config directory, and the realm's
/// `tui.toml` would be the global one (the trust records and notes would
/// mix too). Paths are compared canonicalized as far as they exist.
///
/// # Errors
///
/// [`TuiError::ConfigDirCollision`] naming both paths.
pub fn check_config_dir_collision(
    global: Option<&GlobalDirs>,
    realm_dir: &Path,
) -> Result<(), TuiError> {
    let Some(global) = global else {
        return Ok(());
    };
    let config = canonical_prefix(&global.config);
    let realm = canonical_prefix(realm_dir);
    let relation = if config == realm {
        "are the same directory"
    } else if realm.starts_with(&config) {
        "overlap (the realm dir is inside the config dir)"
    } else if config.starts_with(&realm) {
        "overlap (the config dir is inside the realm dir)"
    } else {
        return Ok(());
    };
    Err(TuiError::ConfigDirCollision {
        config: global.config.clone(),
        realm: realm_dir.to_path_buf(),
        relation,
    })
}

/// `path` with its longest existing ancestor canonicalized and the rest
/// appended (a directory that is not created yet still compares by where
/// it will be).
fn canonical_prefix(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&existing) {
            return rest
                .iter()
                .rev()
                .fold(canonical, |dir, part| dir.join(part));
        }
        match (existing.file_name().map(ToOwned::to_owned), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Message for a working directory that must not become the workspace.
const HOME_WORKSPACE_REFUSED: &str =
    "refusing to use your home directory as the workspace; pass --workspace or cd into a project";

/// The workspace when `--workspace` is not given: a workspace found above
/// `cwd`, else `cwd` itself unless it is `home` (or an ancestor of it).
fn default_workspace(cwd: &Path, home: Option<&Path>) -> Result<PathBuf, TuiError> {
    let home = home.map(canonical_or_self);
    if let Some(found) = find_workspace_above(cwd, home.as_deref()) {
        return Ok(found);
    }
    if home
        .as_deref()
        .is_some_and(|home| home.starts_with(canonical_or_self(cwd)))
    {
        return Err(TuiError::ProjectRoot {
            path: cwd.to_path_buf(),
            source: std::io::Error::other(HOME_WORKSPACE_REFUSED),
        });
    }
    Ok(cwd.to_path_buf())
}

fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The nearest ancestor of `start` (inclusive) that already holds a
/// workspace-local state directory, if any.
///
/// The search stops at the git top-level (a directory containing `.git` is
/// the last one checked) and never yields `home` or an ancestor of it: a
/// stray `~/.rkat/tui/` must not turn the whole home directory into the
/// workspace. `home` is expected canonical.
fn find_workspace_above(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let mut current = canonical_or_self(start);
    loop {
        if home.is_some_and(|home| home.starts_with(&current)) {
            return None;
        }
        if current
            .join(WORKSPACE_STATE_DIR)
            .join(WORKSPACE_REALM_ID)
            .is_dir()
        {
            return Some(current);
        }
        if current.join(".git").exists() || !current.pop() {
            return None;
        }
    }
}

/// Create `<workspace>/.rkat/tui/` and drop a `.gitignore` into it so session
/// state, history, logs and realm rules are never committed.
fn ensure_workspace_state_dir(state_root: &Path) -> Result<(), TuiError> {
    let realm_dir = state_root.join(WORKSPACE_REALM_ID);
    std::fs::create_dir_all(&realm_dir)
        .map_err(|e| TuiError::io("create workspace state directory", e))?;
    let gitignore = realm_dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(&gitignore, "# rkat-tui session state; never commit.\n*\n")
            .map_err(|e| TuiError::io("write workspace state .gitignore", e))?;
    }
    Ok(())
}

fn project_root_from(path: &Path) -> Result<ProjectRoot, TuiError> {
    ProjectRoot::new(path).map_err(|err| match err {
        PathError::Io { path, source } => TuiError::ProjectRoot { path, source },
        other => TuiError::ProjectRoot {
            path: path.to_path_buf(),
            source: std::io::Error::other(other.to_string()),
        },
    })
}

/// Load the realm config (`<state_root>/<realm>/config.toml`) and apply
/// environment overrides, exactly like `rkat`.
///
/// # Errors
///
/// Returns [`TuiError::Io`] when the realm directory cannot be created and
/// [`TuiError::Config`] when the file cannot be parsed or env overrides fail.
pub async fn load_config(scope: &RuntimeScope) -> Result<Config, TuiError> {
    let config_path = scope.config_path();
    if let Some(parent) = config_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| TuiError::io("failed to create realm config directory", e))?;
    }
    let mut config = FileConfigStore::new(config_path, meerkat_models::canonical())
        .get()
        .await
        .map_err(|source| TuiError::Config {
            context: "failed to load config",
            source,
        })?;
    config
        .apply_env_overrides()
        .map_err(|source| TuiError::Config {
            context: "failed to apply env overrides",
            source,
        })?;
    Ok(config)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args(extra: &[&str]) -> Args {
        let mut argv = vec!["rkat-tui"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).unwrap()
    }

    #[test]
    fn default_state_lives_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_str().unwrap();
        let scope = resolve_scope(&args(&["-w", ws])).unwrap();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(scope.realm_id, WORKSPACE_REALM_ID);
        assert_eq!(scope.state_root, canonical.join(".rkat"));
        assert_eq!(scope.realm_root(), canonical.join(".rkat").join("tui"));
        assert_eq!(scope.origin, RealmOrigin::Workspace);
        let gitignore = scope.realm_root().join(".gitignore");
        assert!(gitignore.is_file());
        assert!(std::fs::read_to_string(gitignore).unwrap().contains('*'));
    }

    #[test]
    fn workspace_is_found_from_a_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join(".rkat/tui")).unwrap();
        let sub = root.join("src/deep");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_workspace_above(&sub, None).as_deref(), Some(&*root));
        let other = tempfile::tempdir().unwrap();
        assert!(find_workspace_above(other.path(), None).is_none());
    }

    #[test]
    fn discovery_stops_at_the_git_top_level() {
        let dir = tempfile::tempdir().unwrap();
        let outer = std::fs::canonicalize(dir.path()).unwrap();
        // A workspace above the repository must not be picked up.
        std::fs::create_dir_all(outer.join(".rkat/tui")).unwrap();
        let repo = outer.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let sub = repo.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(find_workspace_above(&sub, None).is_none());
        assert_eq!(default_workspace(&sub, None).unwrap(), sub);
        // A workspace at the repository root is still found.
        std::fs::create_dir_all(repo.join(".rkat/tui")).unwrap();
        assert_eq!(find_workspace_above(&sub, None), Some(repo));
    }

    #[test]
    fn discovery_never_returns_home_or_above() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let home = root.join("home");
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        // Stray state in the home directory and above it.
        std::fs::create_dir_all(home.join(".rkat/tui")).unwrap();
        std::fs::create_dir_all(root.join(".rkat/tui")).unwrap();
        assert!(find_workspace_above(&project, Some(&home)).is_none());
        assert_eq!(default_workspace(&project, Some(&home)).unwrap(), project);
    }

    #[test]
    fn home_directory_is_refused_without_an_explicit_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap();
        let err = default_workspace(&home, Some(&home)).unwrap_err();
        assert!(
            err.to_string()
                .contains("refusing to use your home directory"),
            "{err}"
        );
        // Above the home directory as well.
        let parent = home.parent().unwrap();
        assert!(default_workspace(parent, Some(&home)).is_err());
        // An explicit --workspace is honoured.
        let scope = resolve_scope(&args(&["-w", home.to_str().unwrap()])).unwrap();
        assert_eq!(scope.context_root, home);
    }

    #[test]
    fn realm_flags_opt_into_shared_state() {
        let dir = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let scope = resolve_scope(&args(&[
            "-w",
            dir.path().to_str().unwrap(),
            "--isolated",
            "--state-root",
            state.path().to_str().unwrap(),
        ]))
        .unwrap();
        assert_ne!(scope.realm_id, WORKSPACE_REALM_ID);
        assert_eq!(scope.state_root, state.path());
        assert!(!dir.path().join(".rkat/tui").exists());
    }

    #[test]
    fn global_dirs_follow_flags() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_str().unwrap();
        let cfg = dir.path().join("cfg");
        let scope = resolve_scope(&args(&[
            "-w",
            ws,
            "--config-dir",
            cfg.to_str().unwrap(),
            "--user-config-root",
            ws,
        ]))
        .unwrap();
        assert_eq!(
            scope.global,
            Some(GlobalDirs {
                cache: cfg.join("cache"),
                config: cfg,
            })
        );

        let user = tempfile::tempdir().unwrap();
        let user_root = user.path().to_str().unwrap();
        let scope = resolve_scope(&args(&["-w", ws, "--user-config-root", user_root])).unwrap();
        assert_eq!(
            scope.global.map(|g| g.config),
            Some(user.path().join(".rkat").join("tui"))
        );
    }

    #[test]
    fn a_config_dir_colliding_with_the_realm_dir_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(dir.path()).unwrap();
        let ws = home.to_str().unwrap();
        // `-w ~` with the default config dir `~/.rkat/tui`: the same dir.
        let err = resolve_scope(&args(&["-w", ws, "--user-config-root", ws])).unwrap_err();
        let realm = home.join(".rkat").join("tui");
        let shown = err.to_string();
        assert!(matches!(err, TuiError::ConfigDirCollision { .. }), "{shown}");
        assert_eq!(err.exit_code(), 2);
        assert!(shown.contains("are the same directory"), "{shown}");
        assert!(shown.contains(realm.to_str().unwrap()), "{shown}");
        assert!(shown.contains("--config-dir <other dir>"), "{shown}");
        // Nothing was created before the refusal.
        assert!(!realm.exists());

        // The config dir inside the realm dir.
        let inner = realm.join("cfg");
        let err = resolve_scope(&args(&["-w", ws, "--config-dir", inner.to_str().unwrap()]))
            .unwrap_err();
        assert!(err.to_string().contains("config dir is inside the realm dir"), "{err}");
        // The realm dir inside the config dir.
        let outer = home.join(".rkat");
        let err = resolve_scope(&args(&["-w", ws, "--config-dir", outer.to_str().unwrap()]))
            .unwrap_err();
        assert!(err.to_string().contains("realm dir is inside the config dir"), "{err}");
        // Through a symbolic link to the realm dir as well.
        #[cfg(unix)]
        {
            std::fs::create_dir_all(&realm).unwrap();
            let link = home.join("link");
            std::os::unix::fs::symlink(&realm, &link).unwrap();
            let err = resolve_scope(&args(&["-w", ws, "--config-dir", link.to_str().unwrap()]))
                .unwrap_err();
            assert!(err.to_string().contains("are the same directory"), "{err}");
        }

        // `--config-dir` elsewhere resolves it; so does a sibling whose name
        // merely shares a prefix.
        let other = home.join("cfg");
        let scope =
            resolve_scope(&args(&["-w", ws, "--config-dir", other.to_str().unwrap()])).unwrap();
        assert_eq!(scope.global.map(|g| g.config), Some(other));
        let sibling = home.join(".rkat").join("tui2");
        assert!(
            resolve_scope(&args(&["-w", ws, "--config-dir", sibling.to_str().unwrap()])).is_ok()
        );
    }

    #[test]
    fn collision_check_compares_both_directions() {
        let dirs = |config: &str| GlobalDirs {
            config: PathBuf::from(config),
            cache: PathBuf::from("/k"),
        };
        let realm = Path::new("/nonexistent-rkat/ws/.rkat/tui");
        assert!(check_config_dir_collision(None, realm).is_ok());
        assert!(check_config_dir_collision(Some(&dirs("/nonexistent-rkat/cfg")), realm).is_ok());
        for config in [
            "/nonexistent-rkat/ws/.rkat/tui",
            "/nonexistent-rkat/ws/.rkat",
            "/nonexistent-rkat/ws/.rkat/tui/sub",
        ] {
            assert!(
                matches!(
                    check_config_dir_collision(Some(&dirs(config)), realm),
                    Err(TuiError::ConfigDirCollision { .. })
                ),
                "{config}"
            );
        }
    }
}

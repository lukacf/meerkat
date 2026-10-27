//! Convention-based config discovery for Meta applications.
//!
//! Applications follow a directory convention:
//!
//! ```text
//! config/
//!   mob.toml                    # mob definition (profiles, wiring, skills)
//!   console.toml                # console UI view configuration (optional)
//!   gating.toml                 # gating rules (optional)
//!   access.toml                 # ABAC access control (optional, opt-in)
//!   defaults/
//!     schedules.toml            # default schedule definitions (optional)
//! deployment/
//!   routing.toml                # deployment-specific routing (optional)
//!   schedules.toml              # deployment-specific schedules (optional)
//! .rkat/
//!   config.toml                 # meerkat host config: [self_hosted], [realm], [models] (optional)
//! ```
//!
//! If a file exists at the conventional path, it's loaded. If not, it's skipped.
//! `access.toml` and `gating.toml` are stricter, because their presence opts
//! the deployment into access control and into gating tiers and escalation:
//! each is skipped only when the lookup reports `NotFound` (nothing there,
//! which includes a missing or dangling parent directory). Any other entry (a
//! directory, a dangling link at the file name, an unreadable file) and any
//! other lookup error (such as `ENOTDIR`) is reported for the strict loader,
//! which refuses startup when it cannot open or read the file or rejects its
//! contents. The loaders do not check the file type before reading, so a FIFO
//! at either path blocks startup instead of refusing it.
//! Explicit paths always override convention. The meerkat host config sits at
//! the WORKSPACE root rather than under `config/`, so it is adopted through
//! [`ConventionalPaths::with_meerkat_config_from_workspace`] rather than
//! [`ConventionalPaths::discover`].
//!
//! # Usage
//!
//! ```rust,no_run
//! use meerkat_meta::ConventionalPaths;
//!
//! let paths = ConventionalPaths::discover("config", "deployment");
//! println!("mob: {:?}", paths.mob_toml);
//! println!("gating: {:?}", paths.gating_toml);
//! println!("schedule files: {:?}", paths.schedule_files);
//! ```

use std::path::{Path, PathBuf};

/// Discovered config file paths from conventional directory layout.
///
/// All paths are relative to the working directory. Fields are `Option` —
/// `None` means the file was not found at the conventional location.
#[derive(Debug, Clone)]
pub struct ConventionalPaths {
    /// Mob definition TOML (e.g. `config/mob.toml`).
    pub mob_toml: Option<PathBuf>,
    /// Gating config (e.g. `config/gating.toml`). It sets gating tiers and
    /// escalation, so like [`access_toml`](Self::access_toml) it is `None`
    /// only when the lookup reports `NotFound`; any other entry or lookup
    /// error is reported for the strict loader rather than leaving gating
    /// off.
    pub gating_toml: Option<PathBuf>,
    /// Console UI config (e.g. `config/console.toml`).
    pub console_toml: Option<PathBuf>,
    /// Access-control config (e.g. `config/access.toml`). Presence of the
    /// file opts the deployment into ABAC enforcement plumbing; console
    /// admin edits persist back to it. `None` only when the lookup reports
    /// `NotFound` (nothing there, including a missing or dangling parent);
    /// any other entry or lookup error is reported for the strict loader
    /// rather than leaving access control off.
    pub access_toml: Option<PathBuf>,
    /// Routing config (e.g. `deployment/routing.toml`).
    pub routing_toml: Option<PathBuf>,
    /// Contact directory TOML (e.g. `config/contacts.toml`).
    pub contacts_toml: Option<PathBuf>,
    /// Meerkat host config (`<workspace>/.rkat/config.toml`), adopted only
    /// through [`with_meerkat_config_from_workspace`](Self::with_meerkat_config_from_workspace).
    /// It carries the tables meerkat keeps out of `mob.toml` (`[self_hosted]`,
    /// `[realm]`, `[models]`); a gateway hands it to every agent it builds.
    pub meerkat_config_toml: Option<PathBuf>,
    /// All discovered schedule files, in order:
    /// defaults first (e.g. `config/defaults/schedules.toml`),
    /// then deployment overrides (e.g. `deployment/schedules.toml`).
    pub schedule_files: Vec<PathBuf>,
}

impl ConventionalPaths {
    /// Discover config files from conventional directory layout.
    ///
    /// Checks fixed paths relative to the working directory.
    /// Only includes files that actually exist on disk, except `access.toml`
    /// and `gating.toml`, which are included unless their lookup reports
    /// `NotFound` (see [`access_toml`](Self::access_toml)).
    pub fn discover(config_dir: impl AsRef<Path>, deployment_dir: impl AsRef<Path>) -> Self {
        let config = config_dir.as_ref();
        let deployment = deployment_dir.as_ref();

        let mob_toml = check_file(config.join("mob.toml"));
        let gating_toml = check_entry(config.join("gating.toml"));
        let console_toml = check_file(config.join("console.toml"));
        let access_toml = check_entry(config.join("access.toml"));
        let routing_toml = check_file(deployment.join("routing.toml"));
        let contacts_toml = check_file(config.join("contacts.toml"));

        let mut schedule_files = Vec::new();
        if let Some(p) = check_file(config.join("defaults").join("schedules.toml")) {
            schedule_files.push(p);
        }
        if let Some(p) = check_file(deployment.join("schedules.toml")) {
            schedule_files.push(p);
        }

        Self {
            mob_toml,
            gating_toml,
            console_toml,
            access_toml,
            routing_toml,
            contacts_toml,
            meerkat_config_toml: None,
            schedule_files,
        }
    }

    /// Adopt the workspace's meerkat host config
    /// (`<workspace>/.rkat/config.toml`) when it exists.
    ///
    /// Separate from [`discover`](Self::discover) because the host config
    /// lives at the workspace root, beside `config/` and `deployment/`, not
    /// inside either. Absent, the field stays `None` and a gateway builds
    /// agents from meerkat's default config as before.
    pub fn with_meerkat_config_from_workspace(mut self, workspace_root: impl AsRef<Path>) -> Self {
        self.meerkat_config_toml =
            check_file(workspace_root.as_ref().join(".rkat").join("config.toml"));
        self
    }

    /// Collect schedule file paths as strings (for module args).
    pub fn schedule_file_strings(&self) -> Vec<String> {
        self.schedule_files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }
}

fn check_file(path: PathBuf) -> Option<PathBuf> {
    if path.is_file() { Some(path) } else { None }
}

/// Discovery for a file whose presence opts into a security control. Only a
/// `NotFound` lookup (nothing at the path, including a missing or dangling
/// parent) means "off". Any other entry (a directory, a dangling final link,
/// an unreadable file) and any other lookup error (such as `ENOTDIR`) is
/// reported, so the strict loader handles it instead of the control silently
/// staying off.
fn check_entry(path: PathBuf) -> Option<PathBuf> {
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        _ => Some(path),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_finds_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let deployment = tmp.path().join("deployment");
        fs::create_dir_all(config.join("defaults")).unwrap();
        fs::create_dir_all(&deployment).unwrap();

        fs::write(config.join("mob.toml"), "[mob]\nid = \"test\"").unwrap();
        fs::write(config.join("gating.toml"), "[[rules]]").unwrap();
        fs::write(config.join("console.toml"), "[sidebar]").unwrap();
        fs::write(config.join("access.toml"), "enabled = false").unwrap();
        fs::write(
            config.join("defaults").join("schedules.toml"),
            "[[schedules]]",
        )
        .unwrap();
        fs::write(deployment.join("routing.toml"), "[[routes]]").unwrap();
        fs::write(deployment.join("schedules.toml"), "[[schedules]]").unwrap();

        let paths = ConventionalPaths::discover(&config, &deployment);
        assert!(paths.mob_toml.is_some());
        assert!(paths.gating_toml.is_some());
        assert!(paths.console_toml.is_some());
        assert!(paths.access_toml.is_some());
        assert!(paths.routing_toml.is_some());
        assert_eq!(paths.schedule_files.len(), 2);
    }

    #[test]
    fn discover_handles_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let deployment = tmp.path().join("deployment");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&deployment).unwrap();

        // Only mob.toml exists
        fs::write(config.join("mob.toml"), "[mob]\nid = \"test\"").unwrap();

        let paths = ConventionalPaths::discover(&config, &deployment);
        assert!(paths.mob_toml.is_some());
        assert!(paths.gating_toml.is_none());
        assert!(paths.console_toml.is_none());
        assert!(paths.access_toml.is_none());
        assert!(paths.routing_toml.is_none());
        assert!(paths.schedule_files.is_empty());
    }

    #[test]
    fn discover_handles_nonexistent_dirs() {
        let paths = ConventionalPaths::discover("/nonexistent/config", "/nonexistent/deployment");
        assert!(paths.mob_toml.is_none());
        assert!(paths.gating_toml.is_none());
        assert!(paths.console_toml.is_none());
        assert!(paths.access_toml.is_none());
        assert!(paths.routing_toml.is_none());
        assert!(paths.schedule_files.is_empty());
    }

    /// `access.toml` opts the deployment into access control, so only a
    /// genuinely absent entry leaves it off. Anything else at the path is
    /// reported, and the strict loader then refuses it.
    #[test]
    fn discover_reports_an_unusable_access_entry_for_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let deployment = tmp.path().join("deployment");
        let access = config.join("access.toml");
        fs::create_dir_all(&access).unwrap();
        fs::create_dir_all(&deployment).unwrap();

        let paths = ConventionalPaths::discover(&config, &deployment);
        assert_eq!(
            paths.access_toml.as_deref(),
            Some(access.as_path()),
            "a directory at access.toml must be reported, not read as absent"
        );
        assert!(matches!(
            crate::AccessController::load(&access),
            Err(crate::AccessConfigError::Io(_))
        ));
        // The other conventional files keep their file-only discovery.
        assert!(paths.mob_toml.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn discover_reports_a_dangling_or_unobservable_access_entry_for_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let deployment = tmp.path().join("deployment");
        fs::create_dir_all(&deployment).unwrap();

        // A link whose target is missing is still an entry at the path.
        let linked = tmp.path().join("linked");
        fs::create_dir_all(&linked).unwrap();
        let dangling = linked.join("access.toml");
        std::os::unix::fs::symlink(tmp.path().join("missing.toml"), &dangling).unwrap();
        let paths = ConventionalPaths::discover(&linked, &deployment);
        assert_eq!(paths.access_toml.as_deref(), Some(dangling.as_path()));
        assert!(matches!(
            crate::AccessController::load(&dangling),
            Err(crate::AccessConfigError::Io(_))
        ));

        // A lookup that fails for a reason other than absence (here the
        // config path is a file, so the lookup reports "not a directory") is
        // reported too.
        let blocked = tmp.path().join("blocked");
        fs::write(&blocked, "").unwrap();
        let unobservable = blocked.join("access.toml");
        let paths = ConventionalPaths::discover(&blocked, &deployment);
        assert_eq!(paths.access_toml.as_deref(), Some(unobservable.as_path()));
        assert!(matches!(
            crate::AccessController::load(&unobservable),
            Err(crate::AccessConfigError::Io(_))
        ));
    }

    /// `gating.toml` sets gating tiers and escalation, so like `access.toml`
    /// only a `NotFound` lookup leaves gating off; a directory there is
    /// reported for the strict loader.
    #[test]
    fn discover_reports_an_unusable_gating_entry_for_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let deployment = tmp.path().join("deployment");
        let gating = config.join("gating.toml");
        fs::create_dir_all(&gating).unwrap();
        fs::create_dir_all(&deployment).unwrap();

        let paths = ConventionalPaths::discover(&config, &deployment);
        assert_eq!(
            paths.gating_toml.as_deref(),
            Some(gating.as_path()),
            "a directory at gating.toml must be reported, not read as absent"
        );
        assert!(paths.access_toml.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn discover_reports_a_dangling_or_unobservable_gating_entry_for_refusal() {
        let tmp = tempfile::tempdir().unwrap();
        let deployment = tmp.path().join("deployment");
        fs::create_dir_all(&deployment).unwrap();

        let linked = tmp.path().join("linked");
        fs::create_dir_all(&linked).unwrap();
        let dangling = linked.join("gating.toml");
        std::os::unix::fs::symlink(tmp.path().join("missing.toml"), &dangling).unwrap();
        let paths = ConventionalPaths::discover(&linked, &deployment);
        assert_eq!(paths.gating_toml.as_deref(), Some(dangling.as_path()));

        // With the config path a file, the lookup reports "not a directory".
        let blocked = tmp.path().join("blocked");
        fs::write(&blocked, "").unwrap();
        let unobservable = blocked.join("gating.toml");
        let paths = ConventionalPaths::discover(&blocked, &deployment);
        assert_eq!(paths.gating_toml.as_deref(), Some(unobservable.as_path()));
    }

    /// `NotFound` also covers a missing or dangling parent directory, so the
    /// opt-in files are omitted there exactly like a missing file.
    #[cfg(unix)]
    #[test]
    fn discover_omits_opt_in_files_under_a_missing_or_dangling_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let deployment = tmp.path().join("deployment");
        fs::create_dir_all(&deployment).unwrap();
        let dangling_parent = tmp.path().join("config");
        std::os::unix::fs::symlink(tmp.path().join("missing-dir"), &dangling_parent).unwrap();

        for config in [tmp.path().join("absent"), dangling_parent] {
            let paths = ConventionalPaths::discover(&config, &deployment);
            assert!(paths.access_toml.is_none(), "{}", config.display());
            assert!(paths.gating_toml.is_none(), "{}", config.display());
        }
    }

    /// The meerkat host config is a workspace-root file: `discover` never
    /// adopts it, `with_meerkat_config_from_workspace` adopts it only when it
    /// exists.
    #[test]
    fn meerkat_config_is_adopted_from_the_workspace_root_only_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let deployment = tmp.path().join("deployment");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&deployment).unwrap();

        let absent = ConventionalPaths::discover(&config, &deployment)
            .with_meerkat_config_from_workspace(tmp.path());
        assert!(absent.meerkat_config_toml.is_none());

        fs::create_dir_all(tmp.path().join(".rkat")).unwrap();
        let expected = tmp.path().join(".rkat").join("config.toml");
        fs::write(&expected, "[self_hosted]\n").unwrap();

        assert!(
            ConventionalPaths::discover(&config, &deployment)
                .meerkat_config_toml
                .is_none(),
            "discover alone must not adopt the workspace-root file"
        );
        let present = ConventionalPaths::discover(&config, &deployment)
            .with_meerkat_config_from_workspace(tmp.path());
        assert_eq!(
            present.meerkat_config_toml.as_deref(),
            Some(expected.as_path())
        );
    }

    #[test]
    fn schedule_files_ordered_defaults_first() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        let deployment = tmp.path().join("deployment");
        fs::create_dir_all(config.join("defaults")).unwrap();
        fs::create_dir_all(&deployment).unwrap();

        fs::write(config.join("defaults").join("schedules.toml"), "default").unwrap();
        fs::write(deployment.join("schedules.toml"), "override").unwrap();

        let paths = ConventionalPaths::discover(&config, &deployment);
        assert_eq!(paths.schedule_files.len(), 2);
        assert!(
            paths.schedule_files[0]
                .to_string_lossy()
                .contains("defaults")
        );
        assert!(
            paths.schedule_files[1]
                .to_string_lossy()
                .contains("deployment")
        );
    }
}

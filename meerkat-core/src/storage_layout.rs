//! The single path authority for durable storage roots.
//!
//! `StorageLayout` is resolved once at bootstrap and carried through
//! composition; no crate resolves ambient roots on its own (the Phase 5
//! anti-ambient-resolution gate enforces this outside bootstrap/layout
//! modules). The layout owns **roots and canonical top-level locators**;
//! feature crates own *relative* file names beneath them (blob directories,
//! per-realm databases, projection files) — centralizing every path literal
//! would create a storage god-module, which is its own fragmentation.
//!
//! Field semantics (see `docs/plans/storage-unification-plan.md`, Phase 2):
//!
//! - [`invocation_context`](StorageLayout::invocation_context): the exact
//!   working directory / `--context-root`. **Never walked up.** MCP config
//!   discovery keys off this (its no-walk-up rule is a security boundary).
//! - [`project_root`](StorageLayout::project_root): discovered by walking up
//!   from the invocation context looking for `.rkat` (may equal the
//!   invocation context; `None` outside any project).
//! - [`user_home_root`](StorageLayout::user_home_root): the home-like root
//!   today's `user_config_root` parameters map onto (`--user-config-root`
//!   override, else the platform home). Helpers append `.rkat` themselves —
//!   this is deliberately NOT `~/.rkat`, or existing call sites would
//!   produce `~/.rkat/.rkat`.
//! - [`user_rkat_root`](StorageLayout::user_rkat_root): the derived
//!   `<user_home_root>/.rkat` (config.toml, mcp.toml, skills, trust).
//! - [`credentials_root`](StorageLayout::credentials_root): populated by the
//!   composing layer from the auth crate's unchanged convention
//!   (`config_dir/meerkat/credentials`); the layout carries it so tooling
//!   has one place to read, it does not re-derive it.
//! - [`comms_identity_root`](StorageLayout::comms_identity_root): populated
//!   by the composing layer from the session-comms convention. Durable key
//!   material — the resolution is preserved exactly by its owner; the slot
//!   exists so the anti-ambient gate neither exempts it forever nor lets a
//!   port silently relocate (and thereby rotate) identity keys.
//! - [`state_root`](StorageLayout::state_root): the realm data root, chosen
//!   by realm-id-first dual-root resolution
//!   ([`RealmConfig::resolve_locator_dual_root`]).
//! - [`cache_root`](StorageLayout::cache_root): root for rebuildable caches
//!   (skill git cache); populated by the composing layer (the realm scope
//!   root in realm-backed surfaces).

use std::path::{Path, PathBuf};

use crate::runtime_bootstrap::{
    DualRootResolution, RealmConfig, RealmLocator, RealmRootChoice, RealmRootDefault,
    RuntimeBootstrapError, default_state_root,
};

/// Immutable path authority resolved at bootstrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLayout {
    invocation_context: PathBuf,
    project_root: Option<PathBuf>,
    user_home_root: Option<PathBuf>,
    user_rkat_root: Option<PathBuf>,
    credentials_root: Option<PathBuf>,
    comms_identity_root: Option<PathBuf>,
    state_root: PathBuf,
    cache_root: Option<PathBuf>,
}

/// Bootstrap inputs for [`StorageLayout::resolve`]. Everything ambient a
/// surface consumes is named here; the resolution itself performs no
/// environment reads beyond what these inputs and the realm-root candidates
/// imply.
#[derive(Debug, Clone, Default)]
pub struct StorageLayoutInputs {
    /// The exact context directory (`--context-root`, else the surface's
    /// invocation directory). Never walked up.
    pub invocation_context: PathBuf,
    /// `--state-root` override; wins over probing when present.
    pub explicit_state_root: Option<PathBuf>,
    /// `--user-config-root` override for the home-like root.
    pub user_config_root: Option<PathBuf>,
    /// The surface's documented default root when the realm exists nowhere.
    pub default_root: Option<RealmRootDefault>,
    /// Whether the project-local candidate (`<context>/.rkat/realms`)
    /// participates in probing. The CLI always probes it; server surfaces
    /// only when started with an explicit context root, which keeps their
    /// no-flags behavior unchanged.
    pub probe_local_candidate: bool,
}

/// A layout together with the locator its state root was resolved for.
#[derive(Debug, Clone)]
pub struct ResolvedStorage {
    pub layout: StorageLayout,
    pub locator: RealmLocator,
    /// Provenance of the state root (logged by surfaces, reported by doctor).
    pub root_choice: RealmRootChoice,
}

impl StorageLayout {
    /// Resolve the layout and the realm locator in one step (realm identity
    /// first, then the root — see
    /// [`RealmConfig::resolve_locator_dual_root`]).
    ///
    /// `realm.state_root` (the surface flag) participates as the explicit
    /// root; `inputs.explicit_state_root` is merged into it when the realm
    /// config carries none.
    pub fn resolve(
        inputs: StorageLayoutInputs,
        realm: &RealmConfig,
    ) -> Result<ResolvedStorage, RuntimeBootstrapError> {
        Self::resolve_with_global_candidate(inputs, realm, &default_state_root())
    }

    /// [`Self::resolve`] with an injected user-global candidate root
    /// (testability; production uses [`default_state_root`]).
    pub fn resolve_with_global_candidate(
        inputs: StorageLayoutInputs,
        realm: &RealmConfig,
        global_candidate: &Path,
    ) -> Result<ResolvedStorage, RuntimeBootstrapError> {
        let local_candidate = inputs
            .probe_local_candidate
            .then(|| local_realms_candidate(&inputs.invocation_context));
        let mut realm_config = realm.clone();
        if realm_config.state_root.is_none() {
            realm_config.state_root = inputs.explicit_state_root.clone();
        }
        let default_root = inputs.default_root.unwrap_or(RealmRootDefault::UserGlobal);
        let DualRootResolution { locator, choice } = realm_config.resolve_locator_dual_root_with(
            local_candidate.as_deref(),
            global_candidate,
            default_root,
        )?;

        let project_root = find_project_root(&inputs.invocation_context);
        let user_home_root = inputs.user_config_root.clone().or_else(dirs::home_dir);
        let user_rkat_root = user_home_root.as_ref().map(|home| home.join(".rkat"));

        let layout = StorageLayout {
            invocation_context: inputs.invocation_context,
            project_root,
            user_home_root,
            user_rkat_root,
            credentials_root: None,
            comms_identity_root: None,
            state_root: locator.state_root.clone(),
            cache_root: None,
        };
        Ok(ResolvedStorage {
            layout,
            locator,
            root_choice: choice,
        })
    }

    /// Injected-roots constructor: no ambient reads at all (tests, embedders
    /// that own their environment).
    pub fn with_injected_roots(
        invocation_context: PathBuf,
        project_root: Option<PathBuf>,
        user_home_root: Option<PathBuf>,
        state_root: PathBuf,
    ) -> Self {
        let user_rkat_root = user_home_root.as_ref().map(|home| home.join(".rkat"));
        Self {
            invocation_context,
            project_root,
            user_home_root,
            user_rkat_root,
            credentials_root: None,
            comms_identity_root: None,
            state_root,
            cache_root: None,
        }
    }

    /// The exact cwd/`--context-root`; never walked up (MCP trust boundary).
    pub fn invocation_context(&self) -> &Path {
        &self.invocation_context
    }

    /// The walked-up project root (`.rkat` discovery), when inside one.
    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// The home-like root existing `user_config_root` parameters map onto.
    pub fn user_home_root(&self) -> Option<&Path> {
        self.user_home_root.as_deref()
    }

    /// `<user_home_root>/.rkat` (config.toml, mcp.toml, skills, trust).
    pub fn user_rkat_root(&self) -> Option<&Path> {
        self.user_rkat_root.as_deref()
    }

    /// Credentials root (unchanged auth convention), when composed.
    pub fn credentials_root(&self) -> Option<&Path> {
        self.credentials_root.as_deref()
    }

    /// Session-comms identity root (durable key material), when composed.
    pub fn comms_identity_root(&self) -> Option<&Path> {
        self.comms_identity_root.as_deref()
    }

    /// The realm data root.
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Root for rebuildable caches, when composed.
    pub fn cache_root(&self) -> Option<&Path> {
        self.cache_root.as_deref()
    }

    /// The user-global config document (`<user_rkat_root>/config.toml`) —
    /// the reserved `global` realm's config doc.
    pub fn global_config_path(&self) -> Option<PathBuf> {
        self.user_rkat_root.as_ref().map(|r| r.join("config.toml"))
    }

    /// Compose the credentials root (owner: the auth crate's convention).
    #[must_use]
    pub fn with_credentials_root(mut self, root: PathBuf) -> Self {
        self.credentials_root = Some(root);
        self
    }

    /// Compose the comms identity root (owner: the session-comms
    /// convention; resolution preserved exactly by its owner).
    #[must_use]
    pub fn with_comms_identity_root(mut self, root: PathBuf) -> Self {
        self.comms_identity_root = Some(root);
        self
    }

    /// Compose the rebuildable-cache root (realm scope root in realm-backed
    /// surfaces).
    #[must_use]
    pub fn with_cache_root(mut self, root: PathBuf) -> Self {
        self.cache_root = Some(root);
        self
    }
}

/// The project-local realms candidate for a context root:
/// `<context>/.rkat/realms`.
pub fn local_realms_candidate(context_root: &Path) -> PathBuf {
    context_root.join(".rkat").join("realms")
}

/// Find the project root by walking up from `start_dir` looking for a
/// `.rkat` entry.
///
/// This is the single canonical copy (previously duplicated in
/// `meerkat-core::config` and `meerkat-tools`, with diverging semantics).
/// It adopts the historically *live* semantic — any existing `.rkat` entry
/// counts, matching the `meerkat-tools` copy every production caller used.
pub fn find_project_root(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();
    loop {
        if current.join(".rkat").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::runtime_bootstrap::RealmSelection;

    #[test]
    fn find_project_root_walks_up_and_accepts_any_rkat_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("proj");
        let nested = project.join("a/b");
        std::fs::create_dir_all(&nested).expect("mkdir");
        assert_eq!(find_project_root(&nested), None);
        std::fs::create_dir_all(project.join(".rkat")).expect("mkdir .rkat");
        assert_eq!(
            find_project_root(&nested).as_deref(),
            Some(project.as_path())
        );
        // A plain file also counts (the live historical semantic).
        let file_project = tmp.path().join("file-proj");
        std::fs::create_dir_all(&file_project).expect("mkdir");
        std::fs::write(file_project.join(".rkat"), b"marker").expect("write");
        assert_eq!(find_project_root(&file_project), Some(file_project));
    }

    #[test]
    fn resolve_derives_user_rkat_root_from_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let inputs = StorageLayoutInputs {
            invocation_context: tmp.path().to_path_buf(),
            explicit_state_root: Some(tmp.path().join("state")),
            user_config_root: Some(home.clone()),
            default_root: Some(RealmRootDefault::ProjectLocal),
            probe_local_candidate: true,
        };
        let realm = RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: "team".into(),
            },
            ..RealmConfig::default()
        };
        let resolved = StorageLayout::resolve_with_global_candidate(
            inputs,
            &realm,
            &tmp.path().join("global"),
        )
        .expect("resolve");
        assert_eq!(resolved.root_choice, RealmRootChoice::Explicit);
        assert_eq!(resolved.layout.user_home_root(), Some(home.as_path()));
        assert_eq!(
            resolved.layout.user_rkat_root(),
            Some(home.join(".rkat").as_path())
        );
        assert_eq!(
            resolved.layout.global_config_path(),
            Some(home.join(".rkat").join("config.toml"))
        );
        // The user_home_root/user_rkat_root split: no `.rkat/.rkat`.
        assert!(
            !resolved
                .layout
                .global_config_path()
                .expect("path")
                .to_string_lossy()
                .contains(".rkat/.rkat")
        );
    }

    #[test]
    fn cli_shaped_resolution_defaults_to_local_candidate() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let context = tmp.path().join("ws");
        std::fs::create_dir_all(&context).expect("mkdir");
        let inputs = StorageLayoutInputs {
            invocation_context: context.clone(),
            explicit_state_root: None,
            user_config_root: None,
            default_root: Some(RealmRootDefault::ProjectLocal),
            probe_local_candidate: true,
        };
        let realm = RealmConfig {
            selection: RealmSelection::WorkspaceDerived {
                root: context.clone(),
            },
            ..RealmConfig::default()
        };
        let resolved = StorageLayout::resolve_with_global_candidate(
            inputs,
            &realm,
            &tmp.path().join("global"),
        )
        .expect("resolve");
        assert_eq!(resolved.root_choice, RealmRootChoice::Default);
        assert_eq!(
            resolved.layout.state_root(),
            local_realms_candidate(&context).as_path()
        );
        assert!(resolved.locator.realm.as_str().starts_with("ws-"));
    }

    #[test]
    fn server_shaped_resolution_without_context_never_probes_local() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("cwd");
        // A realm materialized under the cwd-local candidate must NOT be
        // picked up by a server that was not given a context root.
        let local = local_realms_candidate(&cwd);
        std::fs::create_dir_all(local.join("team")).expect("mkdir");
        std::fs::write(
            local
                .join("team")
                .join(crate::runtime_bootstrap::REALM_MANIFEST_FILE_NAME),
            b"{}",
        )
        .expect("manifest");
        let inputs = StorageLayoutInputs {
            invocation_context: cwd,
            explicit_state_root: None,
            user_config_root: None,
            default_root: Some(RealmRootDefault::UserGlobal),
            probe_local_candidate: false,
        };
        let realm = RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: "team".into(),
            },
            ..RealmConfig::default()
        };
        let global = tmp.path().join("global");
        let resolved =
            StorageLayout::resolve_with_global_candidate(inputs, &realm, &global).expect("resolve");
        assert_eq!(resolved.root_choice, RealmRootChoice::Default);
        assert_eq!(resolved.layout.state_root(), global.as_path());
    }
}

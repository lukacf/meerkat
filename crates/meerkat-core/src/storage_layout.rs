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
//!   by realm-id-first resolution over every ancestor's project-local
//!   candidate and the user-global root
//!   ([`RealmConfig::resolve_locator_among_roots`]).
//! - [`cache_root`](StorageLayout::cache_root): root for rebuildable caches
//!   (skill git cache); populated by the composing layer (the realm scope
//!   root in realm-backed surfaces).

use std::path::{Path, PathBuf};

use crate::runtime_bootstrap::{
    RealmConfig, RealmLocator, RealmRootChoice, RealmRootDefault, RealmRootResolution,
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
    /// The roots a first materialization reserves across: the local root
    /// that holds or will hold the realm, and the user-global root (chosen
    /// root included).
    realm_root_candidates: Vec<PathBuf>,
    /// Farther ancestors' candidates that could not be read and were
    /// skipped by resolution.
    unreadable_realm_roots: Vec<PathBuf>,
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
    /// Whether the project-local candidates participate in probing:
    /// `<project-root>/.rkat/realms`, where the project root is walked up
    /// from the invocation context and falls back to the context itself
    /// outside any project, and the same directory of every farther
    /// ancestor that has a `.rkat` directory ([`local_realms_candidates`]).
    /// The CLI always probes them; server surfaces only when started with
    /// an explicit context root, which keeps their no-flags behavior
    /// unchanged.
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
    /// first, then the root: see
    /// [`RealmConfig::resolve_locator_among_roots`]).
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
        // The invocation context is *where the surface was started*, not
        // necessarily the project root: walk up first, so a nested
        // `--context-root` does not pin project persistence to a
        // subdirectory of the project it lives in. Outside any project the
        // context itself is where a fresh `.rkat` would be created.
        let project_root = find_project_root(&inputs.invocation_context);
        let mut realm_config = realm.clone();
        if realm_config.state_root.is_none() {
            realm_config.state_root = inputs.explicit_state_root.clone();
        }
        // A realm that already exists is used where it lies, so every
        // ancestor's candidate is probed for it, not only the project
        // root's. With an explicit state root, or without the local
        // candidate, no ancestor is examined. An entry on the walk that
        // cannot be examined refuses: resolution never goes on to a farther
        // root past a directory it could not look into.
        let local_candidates = if inputs.probe_local_candidate && realm_config.state_root.is_none()
        {
            walk_local_realms_candidates(&inputs.invocation_context, true)?
        } else {
            Vec::new()
        };
        let default_root = inputs.default_root.unwrap_or(RealmRootDefault::UserGlobal);
        let RealmRootResolution {
            locator,
            choice,
            candidate_roots,
            unreadable_roots,
        } = realm_config.resolve_locator_among_roots_with(
            &local_candidates,
            global_candidate,
            default_root,
        )?;

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
            realm_root_candidates: candidate_roots,
            unreadable_realm_roots: unreadable_roots,
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
            realm_root_candidates: vec![state_root.clone()],
            unreadable_realm_roots: Vec::new(),
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

    /// The state roots a first materialization of the realm reserves
    /// across (the store's cross-candidate first-start reservation), the
    /// chosen root included: the local root that holds or will hold the
    /// realm, and the user-global root, so two surfaces with different
    /// default roots cannot each manufacture a manifest for the same realm.
    /// A farther ancestor's candidate is read by resolution and never
    /// written, so it is not listed.
    pub fn realm_root_candidates(&self) -> &[PathBuf] {
        &self.realm_root_candidates
    }

    /// Farther ancestors' candidates that resolution could not read
    /// (permission denied inside their `.rkat`) and skipped: a realm kept
    /// there is not used. A surface reports each once.
    pub fn unreadable_realm_roots(&self) -> &[PathBuf] {
        &self.unreadable_realm_roots
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

/// The project-local realms candidate for a project root:
/// `<root>/.rkat/realms`. Callers pass the walked-up project root (see
/// [`find_project_root`]); the invocation context itself only when outside
/// any project.
pub fn local_realms_candidate(context_root: &Path) -> PathBuf {
    context_root.join(".rkat").join("realms")
}

/// Every project-local realms candidate for an invocation context, nearest
/// first: `<D>/.rkat/realms` for the project root (the nearest ancestor of
/// the context, the context included, with a `.rkat` entry of any kind: see
/// [`find_project_root`]) and for every farther ancestor whose `.rkat` is a
/// directory, which alone can hold a realms root. Outside any project the
/// list is the context's own candidate, where a fresh `.rkat` would be
/// created. Never empty; nothing is created.
///
/// Resolution reads each of them for an existing realm
/// ([`RealmConfig::resolve_locator_among_roots`]); a new realm goes to the
/// first.
///
/// This listing form passes over an entry it cannot examine (read-only
/// tooling such as the storage doctor). [`StorageLayout::resolve`] walks the
/// same directories and refuses on such an entry instead.
pub fn local_realms_candidates(invocation_context: &Path) -> Vec<PathBuf> {
    walk_local_realms_candidates(invocation_context, false)
        .unwrap_or_else(|_| vec![local_realms_candidate(invocation_context)])
}

/// The walk behind [`local_realms_candidates`]. With `refuse_unexaminable`,
/// a `.rkat` entry that cannot be examined (any answer but "found" or "not
/// found": a directory that may not be searched, a link loop) is a typed
/// [`RuntimeBootstrapError::RootProbeFailed`] naming it. "Cannot tell" is
/// not "no entry": the walk must not pass such a directory and offer a
/// farther ancestor's root in its place.
fn walk_local_realms_candidates(
    invocation_context: &Path,
    refuse_unexaminable: bool,
) -> Result<Vec<PathBuf>, RuntimeBootstrapError> {
    let mut candidates = Vec::new();
    let mut current = invocation_context.to_path_buf();
    loop {
        let entry = current.join(".rkat");
        match std::fs::metadata(&entry) {
            // The nearest entry of any kind is the project root's; a
            // farther one counts only as a directory.
            Ok(metadata) => {
                if candidates.is_empty() || metadata.is_dir() {
                    candidates.push(local_realms_candidate(&current));
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) if refuse_unexaminable => {
                return Err(RuntimeBootstrapError::RootProbeFailed {
                    path: entry,
                    source,
                });
            }
            Err(_) => {}
        }
        if !current.pop() {
            break;
        }
    }
    if candidates.is_empty() {
        candidates.push(local_realms_candidate(invocation_context));
    }
    Ok(candidates)
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
    fn nested_invocation_context_resolves_project_root_local_candidate() {
        // A nested `--context-root` (or cwd) inside a project must not pin
        // realm storage to the subdirectory: the local candidate hangs off
        // the walked-up project root.
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = tmp.path().join("proj");
        let nested = project.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::create_dir_all(project.join(".rkat")).expect("mkdir .rkat");
        let inputs = StorageLayoutInputs {
            invocation_context: nested.clone(),
            explicit_state_root: None,
            user_config_root: None,
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
        assert_eq!(resolved.layout.project_root(), Some(project.as_path()));
        assert_eq!(resolved.layout.invocation_context(), nested.as_path());
        assert_eq!(
            resolved.layout.state_root(),
            local_realms_candidate(&project).as_path()
        );
        // The probed candidates surface for the first-start reservation.
        assert_eq!(
            resolved.layout.realm_root_candidates(),
            &[local_realms_candidate(&project), tmp.path().join("global")]
        );
    }

    #[test]
    fn injected_roots_carry_a_single_candidate() {
        let layout = StorageLayout::with_injected_roots(
            PathBuf::from("/ctx"),
            None,
            None,
            PathBuf::from("/state"),
        );
        assert_eq!(
            layout.realm_root_candidates(),
            &[PathBuf::from("/state")],
            "injected layouts resolve single-root: no cross-candidate probing happened"
        );
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

    /// Rows for a realm's state root among every ancestor's `.rkat/realms`.
    mod ancestor_roots {
        use super::*;
        use crate::runtime_bootstrap::{
            REALM_MANIFEST_FILE_NAME, RuntimeBootstrapError, sanitize_realm_id,
        };

        /// A project tree: `<tmp>/proj/a/b`, the user-global root beside it.
        struct Tree {
            _tmp: tempfile::TempDir,
            project: PathBuf,
            middle: PathBuf,
            nested: PathBuf,
            global: PathBuf,
        }

        fn tree() -> Tree {
            let tmp = tempfile::tempdir().expect("tempdir");
            let project = tmp.path().join("proj");
            let middle = project.join("a");
            let nested = middle.join("b");
            std::fs::create_dir_all(&nested).expect("mkdir");
            std::fs::create_dir_all(project.join(".rkat")).expect("mkdir .rkat");
            let global = tmp.path().join("global");
            Tree {
                _tmp: tmp,
                project,
                middle,
                nested,
                global,
            }
        }

        fn realm_dir(realms_root: &Path, realm_id: &str) -> PathBuf {
            realms_root.join(sanitize_realm_id(realm_id))
        }

        fn materialize(realms_root: &Path, realm_id: &str) {
            let dir = realm_dir(realms_root, realm_id);
            std::fs::create_dir_all(&dir).expect("realm dir");
            let manifest = serde_json::json!({
                "realm_id": realm_id,
                "backend": "sqlite",
                "created_at": "0",
            });
            std::fs::write(dir.join(REALM_MANIFEST_FILE_NAME), manifest.to_string())
                .expect("manifest");
        }

        fn workspace_realm(context: &Path) -> RealmConfig {
            RealmConfig {
                selection: RealmSelection::WorkspaceDerived {
                    root: context.to_path_buf(),
                },
                ..RealmConfig::default()
            }
        }

        fn explicit_realm(realm_id: &str) -> RealmConfig {
            RealmConfig {
                selection: RealmSelection::Explicit {
                    realm_id: realm_id.to_string(),
                },
                ..RealmConfig::default()
            }
        }

        /// The inputs the CLI resolves with.
        fn cli_inputs(context: &Path) -> StorageLayoutInputs {
            StorageLayoutInputs {
                invocation_context: context.to_path_buf(),
                explicit_state_root: None,
                user_config_root: None,
                default_root: Some(RealmRootDefault::ProjectLocal),
                probe_local_candidate: true,
            }
        }

        /// The inputs a server started with a context root resolves with.
        fn server_inputs(context: &Path) -> StorageLayoutInputs {
            StorageLayoutInputs {
                default_root: Some(RealmRootDefault::UserGlobal),
                ..cli_inputs(context)
            }
        }

        fn resolve(
            inputs: StorageLayoutInputs,
            realm: &RealmConfig,
            global: &Path,
        ) -> Result<ResolvedStorage, RuntimeBootstrapError> {
            StorageLayout::resolve_with_global_candidate(inputs, realm, global)
        }

        /// The realm of `context`, first resolved and materialized under the
        /// project's root, as a run from `context` leaves it.
        fn realm_kept_under_the_project_root(tree: &Tree, context: &Path) -> String {
            let project_realms = local_realms_candidate(&tree.project);
            let first = resolve(cli_inputs(context), &workspace_realm(context), &tree.global)
                .expect("resolve");
            assert_eq!(first.layout.state_root(), project_realms.as_path());
            assert_eq!(first.root_choice, RealmRootChoice::Default);
            let realm_id = first.locator.realm.as_str().to_string();
            materialize(&project_realms, &realm_id);
            realm_id
        }

        fn assert_split(
            error: &RuntimeBootstrapError,
            realm_id: &str,
            nearer_root: &Path,
            farther_root: &Path,
        ) {
            match error {
                RuntimeBootstrapError::RealmSplitBrain {
                    realm_id: found,
                    local,
                    global,
                } => {
                    assert_eq!(found, realm_id);
                    assert_eq!(local, nearer_root);
                    assert_eq!(global, farther_root);
                }
                other => panic!("wrong error: {other}"),
            }
            let message = error.to_string();
            for root in [nearer_root, farther_root] {
                let dir = realm_dir(root, realm_id);
                assert!(
                    message.contains(&dir.display().to_string()),
                    "the refusal names the realm directory {}: {message}",
                    dir.display()
                );
            }
            // Neither copy is preferred: `--state-root` is given for each,
            // the text says which is nearer and which farther, and the root
            // to keep in the migrate command is left open.
            for root in [nearer_root, farther_root] {
                assert!(
                    message.contains(&format!("--state-root '{}'", root.display())),
                    "the refusal gives --state-root for {}: {message}",
                    root.display()
                );
            }
            assert!(message.contains("nearer"), "{message}");
            assert!(message.contains("farther"), "{message}");
            assert!(message.contains("storage doctor"), "{message}");
            assert!(message.contains("storage migrate"), "{message}");
            assert!(message.contains("--adopt-root <ROOT-TO-KEEP>"), "{message}");
            assert!(!message.contains("--adopt-root '"), "{message}");
        }

        #[test]
        fn realm_under_a_farther_root_is_kept_when_a_directory_between_gets_a_rkat_entry() {
            let tree = tree();
            let project_realms = local_realms_candidate(&tree.project);
            let realm_id = realm_kept_under_the_project_root(&tree, &tree.nested);

            std::fs::create_dir_all(tree.middle.join(".rkat")).expect("nearer .rkat");
            let resolved = resolve(
                cli_inputs(&tree.nested),
                &workspace_realm(&tree.nested),
                &tree.global,
            )
            .expect("resolve");
            assert_eq!(resolved.locator.realm.as_str(), realm_id);
            assert_eq!(resolved.root_choice, RealmRootChoice::ExistingLocal);
            assert_eq!(resolved.layout.state_root(), project_realms.as_path());
            // The first-start reservation covers the root that holds the
            // realm and the user-global root, and no other ancestor's.
            assert_eq!(
                resolved.layout.realm_root_candidates(),
                &[project_realms, tree.global.clone()]
            );
            assert!(
                !tree.middle.join(".rkat").join("realms").exists(),
                "resolution creates nothing under the nearer entry"
            );
        }

        #[test]
        fn realm_under_a_farther_root_is_kept_after_the_context_gets_its_own_project_document() {
            // What `rkat init` leaves in a subdirectory: `<dir>/.rkat` with
            // a `config.toml`.
            let tree = tree();
            let project_realms = local_realms_candidate(&tree.project);
            let realm_id = realm_kept_under_the_project_root(&tree, &tree.nested);

            let own = tree.nested.join(".rkat");
            std::fs::create_dir_all(&own).expect("own .rkat");
            std::fs::write(own.join("config.toml"), b"").expect("project document");
            let resolved = resolve(
                cli_inputs(&tree.nested),
                &workspace_realm(&tree.nested),
                &tree.global,
            )
            .expect("resolve");
            assert_eq!(resolved.locator.realm.as_str(), realm_id);
            assert_eq!(resolved.root_choice, RealmRootChoice::ExistingLocal);
            assert_eq!(resolved.layout.state_root(), project_realms.as_path());
            // The project root the conventions use stays the nearest one.
            assert_eq!(resolved.layout.project_root(), Some(tree.nested.as_path()));
        }

        #[test]
        fn realm_of_a_directory_below_the_context_is_kept_when_that_directory_gets_a_rkat_entry() {
            let tree = tree();
            let project_realms = local_realms_candidate(&tree.project);
            let upper_id = realm_kept_under_the_project_root(&tree, &tree.middle);
            let lower_id = realm_kept_under_the_project_root(&tree, &tree.nested);
            assert_ne!(upper_id, lower_id);

            // The entry appears in a directory strictly below `middle`.
            std::fs::create_dir_all(tree.nested.join(".rkat")).expect("lower .rkat");
            for (context, realm_id) in [(&tree.middle, &upper_id), (&tree.nested, &lower_id)] {
                let resolved =
                    resolve(cli_inputs(context), &workspace_realm(context), &tree.global)
                        .expect("resolve");
                assert_eq!(resolved.locator.realm.as_str(), realm_id);
                assert_eq!(resolved.root_choice, RealmRootChoice::ExistingLocal);
                assert_eq!(resolved.layout.state_root(), project_realms.as_path());
            }
        }

        #[test]
        fn new_realm_goes_to_the_nearest_candidate() {
            let tree = tree();
            std::fs::create_dir_all(tree.middle.join(".rkat")).expect("nearer .rkat");
            let nearest = local_realms_candidate(&tree.middle);
            let resolved = resolve(
                cli_inputs(&tree.nested),
                &workspace_realm(&tree.nested),
                &tree.global,
            )
            .expect("resolve");
            assert_eq!(resolved.root_choice, RealmRootChoice::Default);
            assert_eq!(resolved.layout.state_root(), nearest.as_path());
            assert_eq!(
                resolved.layout.realm_root_candidates(),
                &[nearest, tree.global.clone()]
            );
            assert!(
                !tree.project.join(".rkat").join("realms").exists(),
                "resolution creates nothing under a farther ancestor"
            );
        }

        #[test]
        fn realm_directory_without_a_manifest_under_a_farther_root_is_not_found() {
            let tree = tree();
            std::fs::create_dir_all(tree.middle.join(".rkat")).expect("nearer .rkat");
            let realm = explicit_realm("team");
            let stray = realm_dir(&local_realms_candidate(&tree.project), "team");
            std::fs::create_dir_all(&stray).expect("realm dir");
            std::fs::write(stray.join("config.toml"), b"").expect("realm document");
            let resolved =
                resolve(cli_inputs(&tree.nested), &realm, &tree.global).expect("resolve");
            assert_eq!(resolved.root_choice, RealmRootChoice::Default);
            assert_eq!(
                resolved.layout.state_root(),
                local_realms_candidate(&tree.middle).as_path()
            );
        }

        #[test]
        fn workspace_realm_under_two_ancestor_roots_is_refused_naming_both() {
            let tree = tree();
            let project_realms = local_realms_candidate(&tree.project);
            let realm_id = realm_kept_under_the_project_root(&tree, &tree.nested);
            // The same realm also under a nearer root.
            let nearer_realms = local_realms_candidate(&tree.middle);
            materialize(&nearer_realms, &realm_id);

            let error = resolve(
                cli_inputs(&tree.nested),
                &workspace_realm(&tree.nested),
                &tree.global,
            )
            .expect_err("a realm under two roots is refused");
            assert_split(&error, &realm_id, &nearer_realms, &project_realms);
        }

        #[test]
        fn explicit_realm_under_two_ancestor_roots_is_refused_naming_both() {
            // The same explicit id under an outer directory's root (a home
            // directory with a `.rkat`, for one) and a project below it.
            let tree = tree();
            let outer_realms = local_realms_candidate(&tree.project);
            let inner_realms = local_realms_candidate(&tree.middle);
            materialize(&outer_realms, "dev");
            materialize(&inner_realms, "dev");

            let error = resolve(
                cli_inputs(&tree.middle),
                &explicit_realm("dev"),
                &tree.global,
            )
            .expect_err("a realm under two roots is refused");
            assert_split(&error, "dev", &inner_realms, &outer_realms);
            // From the outer directory only its own root is on the path.
            let outer = resolve(
                cli_inputs(&tree.project),
                &explicit_realm("dev"),
                &tree.global,
            )
            .expect("resolve");
            assert_eq!(outer.root_choice, RealmRootChoice::ExistingLocal);
            assert_eq!(outer.layout.state_root(), outer_realms.as_path());
        }

        #[test]
        fn realm_under_a_farther_root_and_the_user_global_root_is_refused() {
            let tree = tree();
            std::fs::create_dir_all(tree.middle.join(".rkat")).expect("nearer .rkat");
            let project_realms = local_realms_candidate(&tree.project);
            materialize(&project_realms, "team");
            materialize(&tree.global, "team");

            let error = resolve(
                cli_inputs(&tree.nested),
                &explicit_realm("team"),
                &tree.global,
            )
            .expect_err("a realm under two roots is refused");
            assert_split(&error, "team", &project_realms, &tree.global);
        }

        #[test]
        fn explicit_state_root_probes_no_ancestor() {
            let tree = tree();
            std::fs::create_dir_all(tree.middle.join(".rkat")).expect("nearer .rkat");
            materialize(&local_realms_candidate(&tree.project), "team");
            materialize(&local_realms_candidate(&tree.middle), "team");
            let explicit = tree.global.join("explicit");
            let inputs = StorageLayoutInputs {
                explicit_state_root: Some(explicit.clone()),
                ..cli_inputs(&tree.nested)
            };
            let resolved = resolve(inputs, &explicit_realm("team"), &tree.global).expect("resolve");
            assert_eq!(resolved.root_choice, RealmRootChoice::Explicit);
            assert_eq!(resolved.layout.state_root(), explicit.as_path());
            assert_eq!(resolved.layout.realm_root_candidates(), &[explicit]);
        }

        #[test]
        fn server_with_a_context_root_uses_a_realm_under_an_ancestor_project_root() {
            let tree = tree();
            std::fs::create_dir_all(tree.nested.join(".rkat")).expect("context .rkat");
            let project_realms = local_realms_candidate(&tree.project);
            materialize(&project_realms, "team");

            let resolved = resolve(
                server_inputs(&tree.nested),
                &explicit_realm("team"),
                &tree.global,
            )
            .expect("resolve");
            assert_eq!(resolved.root_choice, RealmRootChoice::ExistingLocal);
            assert_eq!(resolved.layout.state_root(), project_realms.as_path());
            assert!(
                !tree.global.exists(),
                "resolution creates nothing in the user-global root"
            );
        }

        #[test]
        fn server_without_a_context_root_probes_no_ancestor() {
            let tree = tree();
            materialize(&local_realms_candidate(&tree.project), "team");
            let inputs = StorageLayoutInputs {
                probe_local_candidate: false,
                ..server_inputs(&tree.nested)
            };
            let resolved = resolve(inputs, &explicit_realm("team"), &tree.global).expect("resolve");
            assert_eq!(resolved.root_choice, RealmRootChoice::Default);
            assert_eq!(resolved.layout.state_root(), tree.global.as_path());
            assert_eq!(
                resolved.layout.realm_root_candidates(),
                std::slice::from_ref(&tree.global)
            );
        }

        #[test]
        fn farther_ancestor_whose_rkat_entry_is_a_file_is_no_candidate() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let outer = tmp.path().join("outer");
            let project = outer.join("proj");
            std::fs::create_dir_all(project.join(".rkat")).expect("mkdir .rkat");
            std::fs::write(outer.join(".rkat"), b"marker").expect("file entry");
            let global = tmp.path().join("global");

            let resolved =
                resolve(cli_inputs(&project), &explicit_realm("team"), &global).expect("resolve");
            assert_eq!(resolved.root_choice, RealmRootChoice::Default);
            assert_eq!(
                resolved.layout.state_root(),
                local_realms_candidate(&project).as_path()
            );
        }

        /// Run `body` with `path` at mode 000, then restore its mode. `None`
        /// when mode bits do not bind this process (the superuser).
        #[cfg(unix)]
        fn with_no_access<T>(path: &Path, body: impl FnOnce() -> T) -> Option<T> {
            use std::os::unix::fs::PermissionsExt;
            let original = std::fs::metadata(path).expect("meta").permissions();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
            let binds = if path.is_dir() {
                std::fs::read_dir(path).is_err()
            } else {
                std::fs::read(path).is_err()
            };
            let outcome = binds.then(body);
            std::fs::set_permissions(path, original).expect("restore");
            outcome
        }

        #[cfg(unix)]
        #[test]
        fn farther_root_that_cannot_be_read_is_skipped() {
            // A permission error from inside a farther ancestor's `.rkat`:
            // the entry itself, its `realms`, the realm directory, the
            // manifest.
            for object in 0..4 {
                let tree = tree();
                std::fs::create_dir_all(tree.middle.join(".rkat")).expect("nearer .rkat");
                let project_realms = local_realms_candidate(&tree.project);
                materialize(&project_realms, "team");
                let team_dir = realm_dir(&project_realms, "team");
                let closed = [
                    tree.project.join(".rkat"),
                    project_realms.clone(),
                    team_dir.clone(),
                    team_dir.join(REALM_MANIFEST_FILE_NAME),
                ][object]
                    .clone();
                let Some(resolved) = with_no_access(&closed, || {
                    resolve(
                        cli_inputs(&tree.nested),
                        &explicit_realm("team"),
                        &tree.global,
                    )
                }) else {
                    return;
                };
                let resolved = resolved.unwrap_or_else(|error| {
                    panic!(
                        "an unreadable farther root does not stop the start ({}): {error}",
                        closed.display()
                    )
                });
                assert_eq!(resolved.root_choice, RealmRootChoice::Default);
                assert_eq!(
                    resolved.layout.state_root(),
                    local_realms_candidate(&tree.middle).as_path()
                );
                assert_eq!(
                    resolved.layout.unreadable_realm_roots(),
                    std::slice::from_ref(&project_realms),
                    "the skipped root is reported"
                );
            }
        }

        #[cfg(unix)]
        #[test]
        fn nearest_root_whose_manifest_cannot_be_read_refuses() {
            let tree = tree();
            let project_realms = local_realms_candidate(&tree.project);
            materialize(&project_realms, "team");
            let manifest = realm_dir(&project_realms, "team").join(REALM_MANIFEST_FILE_NAME);
            let Some(resolved) = with_no_access(&manifest, || {
                resolve(
                    cli_inputs(&tree.nested),
                    &explicit_realm("team"),
                    &tree.global,
                )
            }) else {
                return;
            };
            assert!(matches!(
                resolved,
                Err(RuntimeBootstrapError::RootProbeFailed { .. })
            ));
        }

        #[cfg(unix)]
        fn assert_probe_refusal(
            resolved: Result<ResolvedStorage, RuntimeBootstrapError>,
            examined: &Path,
        ) {
            match resolved {
                Err(RuntimeBootstrapError::RootProbeFailed { path, source }) => {
                    assert_eq!(path, examined);
                    assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
                }
                Err(other) => panic!("wrong error: {other}"),
                Ok(resolved) => panic!(
                    "an entry that cannot be examined must refuse, resolved to {}",
                    resolved.layout.state_root().display()
                ),
            }
        }

        #[cfg(unix)]
        #[test]
        fn context_whose_own_rkat_entry_cannot_be_examined_refuses() {
            // The context directory itself cannot be searched: whether it
            // holds a `.rkat` entry is unknown, which is not "it holds none".
            let tree = tree();
            let Some(resolved) = with_no_access(&tree.nested, || {
                resolve(
                    cli_inputs(&tree.nested),
                    &explicit_realm("team"),
                    &tree.global,
                )
            }) else {
                return;
            };
            assert_probe_refusal(resolved, &tree.nested.join(".rkat"));
        }

        #[cfg(unix)]
        #[test]
        fn directory_above_the_context_that_cannot_be_traversed_refuses() {
            // The first entry examined is the context's own, and it already
            // fails when a directory above the context cannot be searched.
            let tree = tree();
            let Some(resolved) = with_no_access(&tree.middle, || {
                resolve(
                    cli_inputs(&tree.nested),
                    &explicit_realm("team"),
                    &tree.global,
                )
            }) else {
                return;
            };
            assert_probe_refusal(resolved, &tree.nested.join(".rkat"));
        }

        #[cfg(unix)]
        #[test]
        fn explicit_state_root_examines_no_rkat_entry() {
            let tree = tree();
            let explicit = tree.global.join("explicit");
            let inputs = StorageLayoutInputs {
                explicit_state_root: Some(explicit.clone()),
                ..cli_inputs(&tree.nested)
            };
            let Some(resolved) = with_no_access(&tree.nested, || {
                resolve(inputs, &explicit_realm("team"), &tree.global)
            }) else {
                return;
            };
            let resolved = resolved.expect("an explicit state root needs no walk");
            assert_eq!(resolved.root_choice, RealmRootChoice::Explicit);
            assert_eq!(resolved.layout.state_root(), explicit.as_path());
        }
    }
}

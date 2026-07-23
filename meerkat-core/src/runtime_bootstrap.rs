//! Surface/runtime bootstrap contracts shared across interfaces.
//!
//! This module defines how runtimes resolve realm identity and filesystem roots
//! without relying on ambient process state by default.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::connection::{IdentityError, RealmId};

/// Canonical state sharing locator.
///
/// Wave-c C-12 / C-1 follow-up: `realm_id: String` retyped to
/// `realm: RealmId` to match the typed-atom rename C-1 did on
/// `AuthBindingRef`. Consumers must use `self.realm.as_str()` where a
/// `&str` is required (path construction, logging, wire projection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmLocator {
    pub state_root: PathBuf,
    pub realm: RealmId,
}

/// Realm selection mode.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RealmSelection {
    Explicit { realm_id: String },
    Isolated,
    WorkspaceDerived { root: PathBuf },
}

/// Realm/runtime settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RealmConfig {
    pub selection: RealmSelection,
    pub instance_id: Option<String>,
    /// String hint (e.g. "sqlite", "jsonl", "memory"), interpreted by surface/store layers.
    pub backend_hint: Option<String>,
    /// Root directory containing all realm directories.
    pub state_root: Option<PathBuf>,
}

impl Default for RealmConfig {
    fn default() -> Self {
        Self {
            selection: RealmSelection::Isolated,
            instance_id: None,
            backend_hint: None,
            state_root: None,
        }
    }
}

/// Filesystem convention settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ContextConfig {
    pub context_root: Option<PathBuf>,
    pub user_config_root: Option<PathBuf>,
}

/// Top-level runtime bootstrap payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct RuntimeBootstrap {
    pub realm: RealmConfig,
    pub context: ContextConfig,
}

/// Errors resolving runtime bootstrap.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeBootstrapError {
    #[error("`--realm` and `--isolated` cannot be used together")]
    ConflictingSelection,
    #[error("invalid explicit realm id: {0}")]
    InvalidRealmId(String),
    /// The same realm id is materialized under both candidate state roots.
    /// Choosing either silently would deepen the split; resolution is an
    /// operator decision.
    #[error(
        "realm '{realm_id}' exists under both candidate state roots \
         ('{}' and '{}'); refusing to choose. Run `rkat storage doctor` to \
         inspect both copies and `rkat storage migrate` to reconcile them, \
         or pass --state-root to pick one explicitly.",
        .local.display(), .global.display()
    )]
    RealmSplitBrain {
        realm_id: String,
        local: PathBuf,
        global: PathBuf,
    },
    /// A candidate-root probe failed for a reason other than absence.
    /// Only NotFound means "the realm is not here"; every other outcome
    /// (EACCES, ELOOP, ENOTDIR, ...) must refuse typed — mapping it to
    /// absence would route resolution into the *other* root and create the
    /// twin the dual-root invariant exists to prevent.
    #[error("failed probing realm root '{}': {source}", .path.display())]
    RootProbeFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A probed manifest exists but its identity cannot be read. Unknown
    /// identity cannot be ruled in or out, so resolution refuses rather
    /// than guessing a root.
    #[error("realm manifest at '{}' is unreadable: {detail}", .path.display())]
    ManifestUnreadable { path: PathBuf, detail: String },
    /// A candidate root holds a manifest for a *different* realm identity
    /// whose id sanitizes to the same on-disk directory (e.g. `a.b` and
    /// `a_b`). Routing by bare path existence would conflate the two, so
    /// the collision is a typed refusal naming both ids.
    #[error(
        "realm '{requested}' collides with realm '{existing}' under '{}' \
         (both ids sanitize to the same directory name); choose a \
         non-colliding realm id or a different state root",
        .root.display()
    )]
    RealmDirectoryCollision {
        requested: String,
        existing: String,
        root: PathBuf,
    },
}

/// Default global state root shared across surfaces.
pub fn default_state_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("meerkat")
        .join("realms")
}

/// Default user-global instance root for a surface's own (non-realm)
/// state, e.g. `data_dir()/meerkat/rest`. Surfaces call this instead of
/// deriving ambient roots themselves (the storage-ambient gate bans
/// `dirs::*` outside the bootstrap/layout modules).
pub fn default_surface_instance_root(surface: &str) -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("meerkat")
        .join(surface)
}

/// File name of the realm manifest inside a realm directory.
///
/// Vocabulary only: the manifest codec is owned by `meerkat-store`; this
/// constant exists so realm-id-first root probing (below) and read-only
/// diagnostics can recognize a materialized realm without depending on the
/// store crate.
pub const REALM_MANIFEST_FILE_NAME: &str = "realm_manifest.json";

/// Sanitize a realm id into its on-disk directory name.
///
/// (Moved here from `meerkat-store`, which re-exports it: root probing needs
/// the directory-name convention below the store crate in the dependency
/// order.)
pub fn sanitize_realm_id(realm_id: &str) -> String {
    realm_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Which root a surface uses when the realm exists under neither candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealmRootDefault {
    /// The project-local candidate (`<context-root>/.rkat/realms`) — the
    /// CLI's documented default. Falls back to the user-global root when no
    /// local candidate exists.
    ProjectLocal,
    /// The user-global data-dir root — the server surfaces' documented
    /// default.
    UserGlobal,
}

/// Provenance of the resolved state root (surfaces log it; doctor reports it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealmRootChoice {
    /// An explicit `--state-root`/config root won; no probing happened.
    Explicit,
    /// The realm already exists under the project-local candidate.
    ExistingLocal,
    /// The realm already exists under the user-global candidate.
    ExistingGlobal,
    /// Neither candidate holds the realm; the surface default applies.
    Default,
}

/// A resolved locator plus the provenance of its state root.
#[derive(Debug, Clone)]
pub struct DualRootResolution {
    pub locator: RealmLocator,
    pub choice: RealmRootChoice,
    /// Every state-root candidate that participated in resolution (the
    /// chosen root included). First materialization must reserve across all
    /// of them (`meerkat-store`'s cross-candidate first-start reservation)
    /// so two surfaces with different defaults cannot each manufacture a
    /// manifest for the same realm.
    pub candidate_roots: Vec<PathBuf>,
}

/// Minimal identity view of a persisted manifest. The manifest codec is
/// owned by `meerkat-store`; root probing only needs the stable `realm_id`
/// wire field, which every manifest format carries.
#[derive(Deserialize)]
struct ManifestIdentityProbe {
    realm_id: String,
}

/// True when `realms_root` holds a materialized realm for exactly `realm`.
///
/// Fail-closed probe semantics:
/// - only NotFound means absent — any other IO outcome is a typed
///   [`RuntimeBootstrapError::RootProbeFailed`] (an unreadable existing
///   manifest must not route resolution into the other root);
/// - a manifest whose identity differs from `realm` (path-aliased ids —
///   `a.b` and `a_b` sanitize to the same directory) is a typed
///   [`RuntimeBootstrapError::RealmDirectoryCollision`], never a silent hit
///   or miss;
/// - a manifest whose identity cannot be read is a typed
///   [`RuntimeBootstrapError::ManifestUnreadable`].
pub fn realm_exists_under(
    realms_root: &Path,
    realm: &RealmId,
) -> Result<bool, RuntimeBootstrapError> {
    let manifest_path = realms_root
        .join(sanitize_realm_id(realm.as_str()))
        .join(REALM_MANIFEST_FILE_NAME);
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(RuntimeBootstrapError::RootProbeFailed {
                path: manifest_path,
                source,
            });
        }
    };
    let probe: ManifestIdentityProbe = serde_json::from_slice(&bytes).map_err(|err| {
        RuntimeBootstrapError::ManifestUnreadable {
            path: manifest_path.clone(),
            detail: err.to_string(),
        }
    })?;
    if probe.realm_id == realm.as_str() {
        Ok(true)
    } else {
        Err(RuntimeBootstrapError::RealmDirectoryCollision {
            requested: realm.as_str().to_string(),
            existing: probe.realm_id,
            root: realms_root.to_path_buf(),
        })
    }
}

/// True when the two candidate roots are physically one directory (symlink
/// or bind aliases). Canonicalization is best-effort: a candidate that does
/// not exist yet cannot hold a manifest, so literal path equality is the
/// only aliasing left to detect for it.
fn physically_same_root(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(canonical_a), Ok(canonical_b)) => canonical_a == canonical_b,
        _ => false,
    }
}

impl RealmConfig {
    /// Build selection from common CLI inputs, with a provided default mode.
    pub fn selection_from_inputs(
        realm: Option<String>,
        isolated: bool,
        default: RealmSelection,
    ) -> Result<RealmSelection, RuntimeBootstrapError> {
        if realm.is_some() && isolated {
            return Err(RuntimeBootstrapError::ConflictingSelection);
        }
        if let Some(realm_id) = realm {
            validate_explicit_realm_id(&realm_id)?;
            return Ok(RealmSelection::Explicit { realm_id });
        }
        if isolated {
            return Ok(RealmSelection::Isolated);
        }
        Ok(default)
    }

    /// Resolve a concrete `(state_root, realm)` locator.
    pub fn resolve_locator(&self) -> Result<RealmLocator, RuntimeBootstrapError> {
        let state_root = self.state_root.clone().unwrap_or_else(default_state_root);
        let realm = self.resolve_realm_identity()?;
        Ok(RealmLocator { state_root, realm })
    }

    /// Resolve the realm identity exactly once (Isolated selection mints a
    /// fresh id per call, so callers that also probe roots must not resolve
    /// twice).
    pub fn resolve_realm_identity(&self) -> Result<RealmId, RuntimeBootstrapError> {
        let realm_raw = match &self.selection {
            RealmSelection::Explicit { realm_id } => realm_id.clone(),
            RealmSelection::Isolated => generate_realm_id(),
            RealmSelection::WorkspaceDerived { root } => derive_workspace_realm_id(root),
        };
        RealmId::parse(&realm_raw).map_err(|source| match source {
            IdentityError::Empty => RuntimeBootstrapError::InvalidRealmId(realm_raw.clone()),
            IdentityError::InvalidChar(_) => {
                RuntimeBootstrapError::InvalidRealmId(realm_raw.clone())
            }
        })
    }

    /// Realm-id-first dual-root locator resolution.
    ///
    /// Resolves the realm identity *before* choosing a root, then probes for
    /// **that specific realm** under both candidate roots:
    ///
    /// 1. an explicit `state_root` (flag/config) wins — no probing;
    /// 2. else the single candidate root where the realm already exists is
    ///    used where it lies;
    /// 3. both candidates holding the realm is a typed
    ///    [`RuntimeBootstrapError::RealmSplitBrain`] refusal;
    /// 4. neither: the surface's documented default root applies.
    ///
    /// `local_candidate` is the project-local candidate
    /// (`<project-root>/.rkat/realms`; `StorageLayout::resolve` walks the
    /// project root up from the invocation context). The CLI always has
    /// one; server surfaces pass one only when started with an explicit
    /// context root, which keeps their no-flags behavior unchanged.
    ///
    /// The invariant this preserves (the reason probing is realm-id-first
    /// rather than parent-directory-exists): resolution must never route a
    /// realm that exists only under one root into the other and thereby
    /// *create* an empty twin.
    pub fn resolve_locator_dual_root(
        &self,
        local_candidate: Option<&Path>,
        default_root: RealmRootDefault,
    ) -> Result<DualRootResolution, RuntimeBootstrapError> {
        self.resolve_locator_dual_root_with(local_candidate, &default_state_root(), default_root)
    }

    /// [`Self::resolve_locator_dual_root`] with an injected user-global
    /// candidate (testability; production passes [`default_state_root`]).
    pub fn resolve_locator_dual_root_with(
        &self,
        local_candidate: Option<&Path>,
        global_candidate: &Path,
        default_root: RealmRootDefault,
    ) -> Result<DualRootResolution, RuntimeBootstrapError> {
        let realm = self.resolve_realm_identity()?;
        if let Some(explicit) = &self.state_root {
            return Ok(DualRootResolution {
                locator: RealmLocator {
                    state_root: explicit.clone(),
                    realm,
                },
                choice: RealmRootChoice::Explicit,
                candidate_roots: vec![explicit.clone()],
            });
        }
        let global_candidate = global_candidate.to_path_buf();
        // Symlink/bind-aliased candidates are one root, not two copies:
        // treating them as distinct would refuse a split brain that does
        // not exist (and that doctor, which canonicalizes, could never
        // remediate). Collapse to the local spelling.
        if let Some(local) = local_candidate
            && physically_same_root(local, &global_candidate)
        {
            let exists = realm_exists_under(local, &realm)?;
            let root = local.to_path_buf();
            return Ok(DualRootResolution {
                locator: RealmLocator {
                    state_root: root.clone(),
                    realm,
                },
                choice: if exists {
                    RealmRootChoice::ExistingLocal
                } else {
                    RealmRootChoice::Default
                },
                candidate_roots: vec![root],
            });
        }
        let in_local = match local_candidate {
            Some(root) => realm_exists_under(root, &realm)?,
            None => false,
        };
        let in_global = realm_exists_under(&global_candidate, &realm)?;
        let candidate_roots: Vec<PathBuf> = local_candidate
            .map(Path::to_path_buf)
            .into_iter()
            .chain(std::iter::once(global_candidate.clone()))
            .collect();
        let (state_root, choice) = match (in_local, in_global) {
            (true, true) => {
                // The (true, _) arm guarantees the candidate exists here.
                let local = local_candidate.map(Path::to_path_buf).unwrap_or_default();
                return Err(RuntimeBootstrapError::RealmSplitBrain {
                    realm_id: realm.as_str().to_string(),
                    local,
                    global: global_candidate,
                });
            }
            (true, false) => (
                local_candidate.map(Path::to_path_buf).unwrap_or_default(),
                RealmRootChoice::ExistingLocal,
            ),
            (false, true) => (global_candidate, RealmRootChoice::ExistingGlobal),
            (false, false) => {
                let root = match (default_root, local_candidate) {
                    (RealmRootDefault::ProjectLocal, Some(local)) => local.to_path_buf(),
                    _ => global_candidate,
                };
                (root, RealmRootChoice::Default)
            }
        };
        Ok(DualRootResolution {
            locator: RealmLocator { state_root, realm },
            choice,
            candidate_roots,
        })
    }
}

pub fn validate_explicit_realm_id(realm_id: &str) -> Result<(), RuntimeBootstrapError> {
    if realm_id.is_empty()
        || realm_id.len() > 64
        || realm_id.contains(':')
        || realm_id.chars().any(char::is_whitespace)
    {
        return Err(RuntimeBootstrapError::InvalidRealmId(realm_id.to_string()));
    }
    let mut chars = realm_id.chars();
    let first = chars
        .next()
        .ok_or_else(|| RuntimeBootstrapError::InvalidRealmId(realm_id.to_string()))?;
    if !first.is_ascii_alphanumeric() {
        return Err(RuntimeBootstrapError::InvalidRealmId(realm_id.to_string()));
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
        return Err(RuntimeBootstrapError::InvalidRealmId(realm_id.to_string()));
    }
    // Reserve UUID-looking IDs for session ids, preventing locator ambiguity.
    if Uuid::parse_str(realm_id).is_ok() {
        return Err(RuntimeBootstrapError::InvalidRealmId(realm_id.to_string()));
    }
    Ok(())
}

pub fn generate_realm_id() -> String {
    format!("realm-{}", crate::time_compat::new_uuid_v7())
}

pub fn derive_workspace_realm_id(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = canonical.to_string_lossy();
    format!("ws-{}", fnv1a64_hex(&key))
}

pub fn fnv1a64_hex(input: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = OFFSET;
    for b in input.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn selection_conflict_is_rejected() {
        let result = RealmConfig::selection_from_inputs(
            Some("team".to_string()),
            true,
            RealmSelection::Isolated,
        );
        assert!(matches!(
            result,
            Err(RuntimeBootstrapError::ConflictingSelection)
        ));
    }

    #[test]
    fn explicit_realm_id_validation() {
        assert!(validate_explicit_realm_id("team-alpha_1").is_ok());
        assert!(validate_explicit_realm_id("bad:name").is_err());
        assert!(validate_explicit_realm_id("").is_err());
        assert!(validate_explicit_realm_id("550e8400-e29b-41d4-a716-446655440000").is_err());
    }

    #[test]
    fn workspace_selection_is_deterministic() {
        let root = PathBuf::from(".");
        let cfg = RealmConfig {
            selection: RealmSelection::WorkspaceDerived { root },
            ..RealmConfig::default()
        };
        let a = cfg.resolve_locator().map(|locator| locator.realm);
        let b = cfg.resolve_locator().map(|locator| locator.realm);
        assert!(a.is_ok());
        assert_eq!(a.ok(), b.ok());
    }

    mod dual_root {
        use super::*;

        fn materialize_realm(realms_root: &Path, realm_id: &str) {
            let dir = realms_root.join(sanitize_realm_id(realm_id));
            std::fs::create_dir_all(&dir).expect("create realm dir");
            // Identity-checked probing reads the manifest, so the fixture
            // must carry the real `realm_id` wire field.
            let manifest = serde_json::json!({
                "realm_id": realm_id,
                "backend": "sqlite",
                "created_at": "0",
            });
            std::fs::write(dir.join(REALM_MANIFEST_FILE_NAME), manifest.to_string())
                .expect("write manifest");
        }

        fn explicit_cfg(realm_id: &str) -> RealmConfig {
            RealmConfig {
                selection: RealmSelection::Explicit {
                    realm_id: realm_id.to_string(),
                },
                ..RealmConfig::default()
            }
        }

        #[test]
        fn explicit_state_root_wins_without_probing() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let local = tmp.path().join("local");
            materialize_realm(&local, "team");
            let mut cfg = explicit_cfg("team");
            cfg.state_root = Some(tmp.path().join("explicit"));
            let global = tmp.path().join("global");
            let resolved = cfg
                .resolve_locator_dual_root_with(
                    Some(&local),
                    &global,
                    RealmRootDefault::ProjectLocal,
                )
                .expect("resolve");
            assert_eq!(resolved.choice, RealmRootChoice::Explicit);
            assert_eq!(resolved.locator.state_root, tmp.path().join("explicit"));
        }

        #[test]
        fn realm_existing_only_locally_is_used_where_it_lies() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let local = tmp.path().join("local");
            materialize_realm(&local, "team");
            let global = tmp.path().join("global");
            let resolved = explicit_cfg("team")
                .resolve_locator_dual_root_with(Some(&local), &global, RealmRootDefault::UserGlobal)
                .expect("resolve");
            // Even on a surface whose default is the global root.
            assert_eq!(resolved.choice, RealmRootChoice::ExistingLocal);
            assert_eq!(resolved.locator.state_root, local);
        }

        #[test]
        fn absent_realm_falls_to_surface_default() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let local = tmp.path().join("local");
            std::fs::create_dir_all(&local).expect("mkdir");
            let global = tmp.path().join("global");
            let resolved = explicit_cfg("team")
                .resolve_locator_dual_root_with(
                    Some(&local),
                    &global,
                    RealmRootDefault::ProjectLocal,
                )
                .expect("resolve");
            assert_eq!(resolved.choice, RealmRootChoice::Default);
            assert_eq!(resolved.locator.state_root, local);
        }

        #[test]
        fn project_local_default_without_candidate_falls_back_global() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let global = tmp.path().join("global");
            let resolved = explicit_cfg("team")
                .resolve_locator_dual_root_with(None, &global, RealmRootDefault::ProjectLocal)
                .expect("resolve");
            assert_eq!(resolved.choice, RealmRootChoice::Default);
            assert_eq!(resolved.locator.state_root, global);
        }

        #[test]
        fn isolated_identity_is_minted_exactly_once_per_resolution() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let global = tmp.path().join("global");
            let cfg = RealmConfig::default(); // Isolated
            let resolved = cfg
                .resolve_locator_dual_root_with(None, &global, RealmRootDefault::UserGlobal)
                .expect("resolve");
            // A fresh isolated realm never probes true anywhere, so it lands
            // on the default root with a single minted identity.
            assert_eq!(resolved.choice, RealmRootChoice::Default);
            assert!(resolved.locator.realm.as_str().starts_with("realm-"));
        }

        #[test]
        fn probe_recognizes_only_manifested_realms() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().join("realms");
            let realm = RealmId::parse("team").expect("realm id");
            assert!(!realm_exists_under(&root, &realm).expect("probe"));
            // A bare directory without a manifest is not a materialized realm.
            std::fs::create_dir_all(root.join("team")).expect("mkdir");
            assert!(!realm_exists_under(&root, &realm).expect("probe"));
            materialize_realm(&root, "team");
            assert!(realm_exists_under(&root, &realm).expect("probe"));
        }

        #[test]
        fn probe_refuses_identity_colliding_directory() {
            // `a.b` and `a_b` sanitize to the same directory; a probe for
            // `a_b` hitting `a.b`'s manifest must refuse typed, naming both
            // ids — not route (or create) by bare path existence.
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().join("realms");
            materialize_realm(&root, "a.b");
            let realm = RealmId::parse("a_b").expect("realm id");
            let err = realm_exists_under(&root, &realm).expect_err("collision must refuse");
            match &err {
                RuntimeBootstrapError::RealmDirectoryCollision {
                    requested,
                    existing,
                    ..
                } => {
                    assert_eq!(requested, "a_b");
                    assert_eq!(existing, "a.b");
                }
                other => panic!("wrong error: {other}"),
            }
            // The refusal propagates through dual-root resolution.
            let resolved = explicit_cfg("a_b").resolve_locator_dual_root_with(
                Some(&root),
                &tmp.path().join("global"),
                RealmRootDefault::ProjectLocal,
            );
            assert!(matches!(
                resolved,
                Err(RuntimeBootstrapError::RealmDirectoryCollision { .. })
            ));
        }

        #[test]
        fn probe_refuses_unreadable_manifest_identity() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().join("realms");
            let dir = root.join("team");
            std::fs::create_dir_all(&dir).expect("mkdir");
            std::fs::write(dir.join(REALM_MANIFEST_FILE_NAME), b"{ not json").expect("write");
            let realm = RealmId::parse("team").expect("realm id");
            let err = realm_exists_under(&root, &realm).expect_err("must refuse");
            assert!(matches!(
                err,
                RuntimeBootstrapError::ManifestUnreadable { .. }
            ));
        }

        #[cfg(unix)]
        #[test]
        fn probe_io_errors_are_typed_not_absence() {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().join("realms");
            materialize_realm(&root, "team");
            let manifest = root.join("team").join(REALM_MANIFEST_FILE_NAME);
            let original = std::fs::metadata(&manifest).expect("meta").permissions();
            std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o000))
                .expect("chmod");
            if std::fs::read(&manifest).is_ok() {
                // Running as root: permission bits do not bind, so the
                // EACCES branch is untestable here.
                std::fs::set_permissions(&manifest, original).expect("restore");
                return;
            }
            let realm = RealmId::parse("team").expect("realm id");
            let result = realm_exists_under(&root, &realm);
            std::fs::set_permissions(&manifest, original).expect("restore");
            // An unreadable EXISTING manifest is not absence: routing it to
            // the other root would create the twin.
            assert!(matches!(
                result,
                Err(RuntimeBootstrapError::RootProbeFailed { .. })
            ));
        }

        #[cfg(unix)]
        #[test]
        fn aliased_candidate_roots_are_one_root_not_split_brain() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let global = tmp.path().join("global");
            materialize_realm(&global, "team");
            // The "local" candidate is a symlink alias of the global root.
            let local = tmp.path().join("local-alias");
            std::os::unix::fs::symlink(&global, &local).expect("symlink");
            let resolved = explicit_cfg("team")
                .resolve_locator_dual_root_with(
                    Some(&local),
                    &global,
                    RealmRootDefault::ProjectLocal,
                )
                .expect("aliased roots must not refuse as split brain");
            assert_eq!(resolved.choice, RealmRootChoice::ExistingLocal);
            assert_eq!(resolved.locator.state_root, local);
            assert_eq!(resolved.candidate_roots, vec![local]);
        }

        #[test]
        fn resolution_exposes_probed_candidate_roots() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let local = tmp.path().join("local");
            std::fs::create_dir_all(&local).expect("mkdir");
            let global = tmp.path().join("global");
            let resolved = explicit_cfg("team")
                .resolve_locator_dual_root_with(
                    Some(&local),
                    &global,
                    RealmRootDefault::ProjectLocal,
                )
                .expect("resolve");
            assert_eq!(resolved.candidate_roots, vec![local, global.clone()]);
            // Explicit state roots resolve single-candidate (no probing).
            let mut cfg = explicit_cfg("team");
            cfg.state_root = Some(tmp.path().join("explicit"));
            let explicit = cfg
                .resolve_locator_dual_root_with(None, &global, RealmRootDefault::UserGlobal)
                .expect("resolve");
            assert_eq!(explicit.candidate_roots, vec![tmp.path().join("explicit")]);
        }

        #[test]
        fn split_brain_is_a_typed_refusal() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let local = tmp.path().join("local");
            let global = tmp.path().join("global");
            materialize_realm(&local, "team");
            materialize_realm(&global, "team");
            let err = explicit_cfg("team")
                .resolve_locator_dual_root_with(
                    Some(&local),
                    &global,
                    RealmRootDefault::ProjectLocal,
                )
                .expect_err("split brain must refuse");
            match &err {
                RuntimeBootstrapError::RealmSplitBrain {
                    realm_id,
                    local: found_local,
                    global: found_global,
                } => {
                    assert_eq!(realm_id, "team");
                    assert_eq!(found_local, &local);
                    assert_eq!(found_global, &global);
                }
                other => panic!("wrong error: {other}"),
            }
            let message = err.to_string();
            assert!(message.contains("rkat storage doctor"), "{message}");
        }

        #[test]
        fn realm_existing_only_globally_is_used_where_it_lies() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let local = tmp.path().join("local");
            let global = tmp.path().join("global");
            std::fs::create_dir_all(&local).expect("mkdir");
            materialize_realm(&global, "team");
            let resolved = explicit_cfg("team")
                .resolve_locator_dual_root_with(
                    Some(&local),
                    &global,
                    RealmRootDefault::ProjectLocal,
                )
                .expect("resolve");
            // Even on a surface whose default is the local root: the probe
            // must not create an empty local twin of a global realm.
            assert_eq!(resolved.choice, RealmRootChoice::ExistingGlobal);
            assert_eq!(resolved.locator.state_root, global);
        }
    }
}

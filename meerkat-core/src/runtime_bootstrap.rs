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
}

/// True when `realms_root` holds a materialized realm directory for `realm`.
///
/// The probe is manifest-file existence under the sanitized directory name.
/// Path-aliased ids (`a.b` vs `a_b` sanitize identically) can therefore
/// probe true for a sibling identity; that is safe for *root routing*
/// because the store's manifest open stays fail-closed on identity mismatch
/// — the invariant here is only that the resolver never *creates* a twin.
pub fn realm_exists_under(realms_root: &Path, realm: &RealmId) -> bool {
    realms_root
        .join(sanitize_realm_id(realm.as_str()))
        .join(REALM_MANIFEST_FILE_NAME)
        .is_file()
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
    /// (`<context-root>/.rkat/realms`). The CLI always has one; server
    /// surfaces pass one only when started with an explicit context root,
    /// which keeps their no-flags behavior unchanged.
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
            });
        }
        let global_candidate = global_candidate.to_path_buf();
        let in_local = local_candidate.is_some_and(|root| realm_exists_under(root, &realm));
        let in_global = realm_exists_under(&global_candidate, &realm);
        let (state_root, choice) = match (in_local, in_global) {
            (true, true) => {
                // `is_some_and` above guarantees the candidate exists here.
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
            std::fs::write(dir.join(REALM_MANIFEST_FILE_NAME), b"{}").expect("write manifest");
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
            assert!(!realm_exists_under(&root, &realm));
            // A bare directory without a manifest is not a materialized realm.
            std::fs::create_dir_all(root.join("team")).expect("mkdir");
            assert!(!realm_exists_under(&root, &realm));
            materialize_realm(&root, "team");
            assert!(realm_exists_under(&root, &realm));
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

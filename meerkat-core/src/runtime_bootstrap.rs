//! Surface/runtime bootstrap contracts shared across interfaces.
//!
//! This module defines how runtimes resolve realm identity and filesystem roots
//! without relying on ambient process state by default.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Canonical state sharing locator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmLocator {
    pub state_root: PathBuf,
    pub realm_id: String,
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
    /// String hint (e.g. "redb", "jsonl"), interpreted by surface/store layers.
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
}

/// Default global state root shared across surfaces.
pub fn default_state_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("meerkat")
        .join("realms")
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

    /// Resolve a concrete `(state_root, realm_id)` locator.
    pub fn resolve_locator(&self) -> Result<RealmLocator, RuntimeBootstrapError> {
        let state_root = self.state_root.clone().unwrap_or_else(default_state_root);
        let realm_id = match &self.selection {
            RealmSelection::Explicit { realm_id } => realm_id.clone(),
            RealmSelection::Isolated => generate_realm_id(),
            RealmSelection::WorkspaceDerived { root } => derive_workspace_realm_id(root),
        };
        Ok(RealmLocator {
            state_root,
            realm_id,
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
    format!("realm-{}", Uuid::now_v7())
}

pub fn derive_workspace_realm_id(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let key = canonical.to_string_lossy();
    format!("ws-{}", fnv1a64_hex(&key))
}

pub fn fnv1a64_hex(input: &str) -> String {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for b in input.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
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
        let a = cfg.resolve_locator().map(|locator| locator.realm_id);
        let b = cfg.resolve_locator().map(|locator| locator.realm_id);
        assert!(a.is_ok());
        assert_eq!(a.ok(), b.ok());
    }
}

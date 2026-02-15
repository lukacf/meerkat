//! Surface/runtime bootstrap contracts shared across interfaces.
//!
//! This module defines how runtimes resolve realm identity and filesystem roots
//! without relying on ambient process state by default.

use crate::SessionId;
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

/// Parsed session locator input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLocator {
    pub realm_id: Option<String>,
    pub session_id: SessionId,
}

/// Errors parsing or validating session locators.
#[derive(Debug, thiserror::Error)]
pub enum SessionLocatorError {
    #[error("invalid session locator: expected <session_id> or <realm_id>:<session_id>")]
    InvalidFormat,
    #[error("invalid realm id in session locator: {0}")]
    InvalidRealmId(String),
    #[error("invalid session id in session locator: {0}")]
    InvalidSessionId(String),
    #[error("session locator realm '{provided}' does not match active realm '{active}'")]
    RealmMismatch { provided: String, active: String },
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

pub fn format_session_ref(realm_id: &str, session_id: &SessionId) -> String {
    format!("{realm_id}:{session_id}")
}

impl SessionLocator {
    /// Parse either a bare `<session_id>` or `<realm_id>:<session_id>`.
    pub fn parse(input: &str) -> Result<Self, SessionLocatorError> {
        if let Some((realm_part, session_part)) = input.split_once(':') {
            if realm_part.is_empty() || session_part.is_empty() {
                return Err(SessionLocatorError::InvalidFormat);
            }
            validate_explicit_realm_id(realm_part)
                .map_err(|_| SessionLocatorError::InvalidRealmId(realm_part.to_string()))?;
            let session_id = SessionId::parse(session_part)
                .map_err(|_| SessionLocatorError::InvalidSessionId(session_part.to_string()))?;
            return Ok(Self {
                realm_id: Some(realm_part.to_string()),
                session_id,
            });
        }
        let session_id = SessionId::parse(input)
            .map_err(|_| SessionLocatorError::InvalidSessionId(input.to_string()))?;
        Ok(Self {
            realm_id: None,
            session_id,
        })
    }

    /// Resolve a locator against an active realm id, ensuring any explicit
    /// locator realm matches.
    pub fn resolve_for_realm(
        input: &str,
        active_realm_id: &str,
    ) -> Result<SessionId, SessionLocatorError> {
        let locator = Self::parse(input)?;
        if let Some(provided) = locator.realm_id {
            if provided != active_realm_id {
                return Err(SessionLocatorError::RealmMismatch {
                    provided,
                    active: active_realm_id.to_string(),
                });
            }
        }
        Ok(locator.session_id)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
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

    #[test]
    fn session_locator_parsing_is_unambiguous() {
        let sid = SessionId::new();
        let bare = SessionLocator::parse(&sid.to_string()).expect("bare locator should parse");
        assert!(bare.realm_id.is_none());
        assert_eq!(bare.session_id, sid);

        let explicit = SessionLocator::parse(&format!("team-alpha:{sid}"))
            .expect("session_ref locator should parse");
        assert_eq!(explicit.realm_id.as_deref(), Some("team-alpha"));
        assert_eq!(explicit.session_id, sid);
    }

    #[test]
    fn session_locator_rejects_invalid_shapes() {
        assert!(matches!(
            SessionLocator::parse("team-alpha:"),
            Err(SessionLocatorError::InvalidFormat)
        ));
        assert!(matches!(
            SessionLocator::parse("not-a-session"),
            Err(SessionLocatorError::InvalidSessionId(_))
        ));
    }

    #[test]
    fn session_locator_realm_mismatch_is_rejected() {
        let sid = SessionId::new();
        let result = SessionLocator::resolve_for_realm(&format!("alpha:{sid}"), "beta");
        assert!(matches!(
            result,
            Err(SessionLocatorError::RealmMismatch { .. })
        ));
    }
}

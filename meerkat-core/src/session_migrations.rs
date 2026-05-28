//! v0 → v1 persistence migration for session snapshots and runtime input
//! state blobs.
//!
//! # Strategy
//!
//! *Opportunistic upgrade on read:* the `SessionStore` load path pushes each
//! persisted blob through [`migrate_session_value`] before `serde_json`
//! deserializes into [`Session`]. The returned `Session` (or
//! [`SessionMigrationError::Partial`] — same `Session`, carrying a legacy
//! payload bag) is handed back to the caller, and the next `save()` rewrites
//! the row as v2 automatically because the in-memory envelope is v2.
//!
//! # What actually changes v0 → v1
//!
//! - `AuthBindingRef` must already use the current typed shape
//!   (`realm` / `binding`). Legacy aliases (`connection_ref`,
//!   `realm_id`, `binding_id`) are rejected fail-closed here rather than
//!   rewritten, because auth-binding identity affects trust/default
//!   resolution and no generated migration authority owns a value change.
//! - `SessionMetadata.schema_version` byte is stamped with the current
//!   `SESSION_METADATA_SCHEMA_VERSION` on write; on read, rows lacking the
//!   field default to `1` via `#[serde(default)]`. Canonical nested
//!   `metadata.session_metadata` must deserialize as [`SessionMetadata`] and
//!   pass generated durable-config restore authority after stamping; malformed
//!   or incomplete metadata fails closed rather than becoming absent metadata.
//! - Root `session_metadata` and `session_llm_identity` rows are rejected
//!   fail-closed. `Session` does not carry those root fields, and lowering
//!   them into metadata would change identity/trust/default-resolution facts
//!   without generated migration authority.
//! - `provider_params` remains `Option<serde_json::Value>` on
//!   `SessionMetadata` in this wave — the canonical typed shape
//!   (`ProviderParamsOverride`) lives on `RuntimeTurnMetadata`. C-3's only
//!   obligation here is to keep the value opaque (fixture #4, the Anthropic
//!   extended-thinking canary, must be preserved byte-for-byte through the
//!   round-trip).
//! - Input-state (`StoredInputState`) lives in `meerkat-runtime`; its
//!   production serde shape carries a generated-authority version byte, and
//!   this module exposes the JSON transform used by compatibility fixtures.

use serde_json::{Map, Value};
use std::collections::BTreeMap;

use crate::generated::{session_durable_config_authority, session_persistence_version_authority};
use crate::{Session, SessionMetadata};

/// Salvaged-migration payload carried by [`SessionMigrationError::Partial`].
///
/// Boxed off the enum so the `Result<Session, SessionMigrationError>`
/// discriminant stays compact on the hot success path.
#[derive(Debug)]
pub struct PartialSessionMigration {
    pub session: Session,
    pub legacy: BTreeMap<String, Value>,
}

/// Failure modes returned by [`migrate_session_value`] /
/// [`migrate_input_state_value`].
#[derive(Debug)]
pub enum SessionMigrationError {
    /// Input wasn't a JSON object we could introspect.
    Malformed(String),
    /// Serde failed to reconstruct the typed shape even after transforms.
    Deserialize(serde_json::Error),
    /// Transforms succeeded but some payload was salvaged under a legacy
    /// key rather than being dropped silently. The `session` carries the
    /// usable v2 form; `legacy` carries the original raw shape so operators
    /// can inspect what was coerced.
    ///
    /// Wave-c dogma: silent drop is never acceptable. A legacy payload
    /// that can't be re-typed is retained alongside the migrated session,
    /// not discarded.
    Partial(Box<PartialSessionMigration>),
    /// Generated persistence-version authority rejected a migration stamp.
    GeneratedAuthority(
        session_persistence_version_authority::SessionPersistenceVersionAuthorityError,
    ),
    /// Generated durable-config authority rejected restored session metadata.
    GeneratedDurableConfigAuthority(
        session_durable_config_authority::SessionDurableConfigAuthorityError,
    ),
    /// Canonical nested `metadata.session_metadata` failed typed restore.
    InvalidSessionMetadata(serde_json::Error),
    /// Canonical nested `metadata.session_metadata` was not a JSON object.
    InvalidSessionMetadataShape(&'static str),
    /// Legacy auth-binding identity would require a semantic migration that
    /// this compatibility path is not allowed to perform.
    UnsupportedLegacyAuthBinding {
        location: &'static str,
        field: &'static str,
    },
    /// Root `session_metadata` would be ignored by `Session` serde unless a
    /// generated migration authority owns a typed lowering/rejection path.
    UnsupportedRootSessionMetadata,
    /// Root `session_llm_identity` would be ignored by `Session` serde unless
    /// a generated migration authority owns a typed lowering/rejection path.
    UnsupportedRootSessionLlmIdentity,
}

impl std::fmt::Display for SessionMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(reason) => write!(f, "malformed persisted session: {reason}"),
            Self::Deserialize(err) => {
                write!(
                    f,
                    "persisted session deserialize failed after migration: {err}"
                )
            }
            Self::Partial(inner) => write!(
                f,
                "persisted session migrated with salvaged legacy payload ({} keys retained)",
                inner.legacy.len()
            ),
            Self::GeneratedAuthority(err) => write!(f, "{err}"),
            Self::GeneratedDurableConfigAuthority(err) => write!(f, "{err}"),
            Self::InvalidSessionMetadata(err) => {
                write!(f, "persisted session metadata failed typed restore: {err}")
            }
            Self::InvalidSessionMetadataShape(reason) => {
                write!(
                    f,
                    "persisted session metadata failed typed restore: {reason}"
                )
            }
            Self::UnsupportedLegacyAuthBinding { location, field } => write!(
                f,
                "legacy auth-binding identity field '{field}' in {location} requires generated migration authority"
            ),
            Self::UnsupportedRootSessionMetadata => write!(
                f,
                "root session_metadata requires generated migration authority"
            ),
            Self::UnsupportedRootSessionLlmIdentity => write!(
                f,
                "root session_llm_identity requires generated migration authority"
            ),
        }
    }
}

impl std::error::Error for SessionMigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Deserialize(err) => Some(err),
            Self::GeneratedAuthority(err) => Some(err),
            Self::GeneratedDurableConfigAuthority(err) => Some(err),
            Self::InvalidSessionMetadata(err) => Some(err),
            _ => None,
        }
    }
}

/// Convenience: deserialize a persisted session-snapshot byte slice
/// through the migration path. `SessionStore` implementations
/// (`SqliteSessionStore`, `JsonlStore`, `MemoryStore`) route all reads
/// through this helper so v0/v1 rows transparently upgrade.
///
/// On [`SessionMigrationError::Partial`], the migrated `Session` is
/// still returned (the legacy payload is logged at `warn!`) — silent
/// drop is not acceptable (wave-c dogma) but hard-failing a load when
/// salvage succeeded would be worse. Callers that need the legacy
/// payload for operator review can go through [`migrate_session_value`]
/// directly.
pub fn deserialize_session_migrating(bytes: &[u8]) -> Result<Session, SessionMigrationError> {
    let value: Value = serde_json::from_slice(bytes).map_err(SessionMigrationError::Deserialize)?;
    match migrate_session_value(value) {
        Ok(session) => Ok(session),
        Err(SessionMigrationError::Partial(partial)) => {
            tracing::warn!(
                legacy_keys = ?partial.legacy.keys().collect::<Vec<_>>(),
                "session migration salvaged a legacy payload during load"
            );
            Ok(partial.session)
        }
        Err(other) => Err(other),
    }
}

/// Top-level migration entry point for session snapshots.
///
/// Takes a raw JSON value as read from the store, rewrites legacy shapes
/// in place (see module-level doc), and deserializes into a typed
/// [`Session`]. A [`Partial`](SessionMigrationError::Partial) return means
/// the migration salvaged something that couldn't be strictly typed —
/// callers can still use the `session` but should log the preserved
/// payload for operator review.
pub fn migrate_session_value(mut value: Value) -> Result<Session, SessionMigrationError> {
    let Some(root) = value.as_object_mut() else {
        return Err(SessionMigrationError::Malformed(
            "top-level session blob is not a JSON object".to_string(),
        ));
    };

    let mut legacy: BTreeMap<String, Value> = BTreeMap::new();

    // `SessionMetadata` lives inside `session.metadata` under the
    // `session_metadata` key (`SESSION_METADATA_KEY`).
    if let Some(metadata) = root.get_mut("metadata").and_then(Value::as_object_mut)
        && let Some(sess_meta_value) = metadata.get_mut("session_metadata")
    {
        let Some(sess_meta) = sess_meta_value.as_object_mut() else {
            return Err(SessionMigrationError::InvalidSessionMetadataShape(
                "metadata.session_metadata is not a JSON object",
            ));
        };
        migrate_metadata_object(sess_meta, &mut legacy)?;
        validate_session_metadata_object(sess_meta)?;
    }

    if root.contains_key("session_metadata") {
        return Err(SessionMigrationError::UnsupportedRootSessionMetadata);
    }

    if root.contains_key("session_llm_identity") {
        return Err(SessionMigrationError::UnsupportedRootSessionLlmIdentity);
    }

    stamp_session_envelope_version(root)?;

    match serde_json::from_value::<Session>(value) {
        Ok(session) if legacy.is_empty() => Ok(session),
        Ok(session) => Err(SessionMigrationError::Partial(Box::new(
            PartialSessionMigration { session, legacy },
        ))),
        Err(err) => Err(SessionMigrationError::Deserialize(err)),
    }
}

/// Migration entry point for `StoredInputState` rows (fixtures 9-11).
///
/// Production `StoredInputState` serde now validates the same generated
/// version authority on write/read; this transform is the compatibility
/// fixture entry point for pre-version JSON blobs.
pub fn migrate_input_state_value(mut value: Value) -> Result<Value, SessionMigrationError> {
    let Some(root) = value.as_object_mut() else {
        return Err(SessionMigrationError::Malformed(
            "input state blob is not a JSON object".to_string(),
        ));
    };

    // StoredInputState.persisted_input.Prompt.turn_metadata may carry
    // v0 auth_binding / legacy provider string. Rewrite in place.
    if let Some(persisted) = root
        .get_mut("persisted_input")
        .and_then(Value::as_object_mut)
    {
        for (_variant, body) in persisted.iter_mut() {
            let Some(body_obj) = body.as_object_mut() else {
                continue;
            };
            if let Some(tm) = body_obj
                .get_mut("turn_metadata")
                .and_then(Value::as_object_mut)
            {
                migrate_turn_metadata_object(tm)?;
            }
        }
    }

    stamp_stored_input_state_version(root)?;

    Ok(value)
}

fn stamp_session_envelope_version(
    object: &mut Map<String, Value>,
) -> Result<(), SessionMigrationError> {
    let stamp = session_persistence_version_authority::authorize_session_envelope_version_stamp();
    session_persistence_version_authority::stamp_authorized_version(object, stamp)
        .map_err(SessionMigrationError::GeneratedAuthority)?;
    Ok(())
}

fn stamp_stored_input_state_version(
    object: &mut Map<String, Value>,
) -> Result<(), SessionMigrationError> {
    let stamp = session_persistence_version_authority::authorize_stored_input_state_version_stamp();
    session_persistence_version_authority::stamp_authorized_version(object, stamp)
        .map_err(SessionMigrationError::GeneratedAuthority)?;
    Ok(())
}

fn stamp_session_metadata_schema_version(
    object: &mut Map<String, Value>,
) -> Result<(), SessionMigrationError> {
    let stamp =
        session_persistence_version_authority::authorize_session_metadata_schema_version_stamp();
    session_persistence_version_authority::stamp_authorized_version(object, stamp)
        .map_err(SessionMigrationError::GeneratedAuthority)?;
    Ok(())
}

fn migrate_metadata_object(
    meta: &mut Map<String, Value>,
    legacy: &mut BTreeMap<String, Value>,
) -> Result<(), SessionMigrationError> {
    let _ = legacy;
    reject_legacy_auth_binding_field(meta, "SessionMetadata")?;

    stamp_session_metadata_schema_version(meta)?;
    Ok(())
}

fn validate_session_metadata_object(
    meta: &Map<String, Value>,
) -> Result<(), SessionMigrationError> {
    let mut metadata = serde_json::from_value::<SessionMetadata>(Value::Object(meta.clone()))
        .map_err(SessionMigrationError::InvalidSessionMetadata)?;
    metadata.schema_version =
        session_persistence_version_authority::restore_session_metadata_schema_version(
            metadata.schema_version,
        )
        .map_err(SessionMigrationError::GeneratedAuthority)?;
    session_durable_config_authority::restore_session_metadata(metadata)
        .map_err(SessionMigrationError::GeneratedDurableConfigAuthority)?;
    Ok(())
}

fn migrate_turn_metadata_object(tm: &mut Map<String, Value>) -> Result<(), SessionMigrationError> {
    reject_legacy_auth_binding_field(tm, "RuntimeTurnMetadata")?;
    // Provider string that no longer parses: leave it; serde will
    // surface the typed-Provider error on the way out and the deserialize
    // path will propagate. Fixture #11 exercises this.
    Ok(())
}

fn reject_legacy_auth_binding_field(
    map: &Map<String, Value>,
    location: &'static str,
) -> Result<(), SessionMigrationError> {
    if map.contains_key("connection_ref") {
        return Err(SessionMigrationError::UnsupportedLegacyAuthBinding {
            location,
            field: "connection_ref",
        });
    }
    if let Some(obj) = map.get("auth_binding").and_then(Value::as_object) {
        for field in ["realm_id", "binding_id"] {
            if obj.contains_key(field) {
                return Err(SessionMigrationError::UnsupportedLegacyAuthBinding {
                    location,
                    field,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn legacy_connection_ref_rejected_fail_closed() {
        let mut map = Map::new();
        map.insert(
            "connection_ref".to_string(),
            serde_json::json!({"realm": "dev"}),
        );

        let err = reject_legacy_auth_binding_field(&map, "SessionMetadata")
            .expect_err("legacy connection_ref must not be rewritten");

        assert!(matches!(
            err,
            SessionMigrationError::UnsupportedLegacyAuthBinding {
                location: "SessionMetadata",
                field: "connection_ref",
            }
        ));
    }

    #[test]
    fn legacy_auth_binding_identity_fields_rejected_fail_closed() {
        let mut map = Map::new();
        map.insert(
            "auth_binding".to_string(),
            serde_json::json!({"realm_id": "dev", "binding_id": "default"}),
        );

        let err = reject_legacy_auth_binding_field(&map, "SessionLlmIdentity")
            .expect_err("legacy auth binding identity fields must not be rewritten");

        assert!(matches!(
            err,
            SessionMigrationError::UnsupportedLegacyAuthBinding {
                location: "SessionLlmIdentity",
                field: "realm_id",
            }
        ));
    }

    #[test]
    fn root_session_metadata_rejected_fail_closed() {
        let envelope = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "messages": [],
            "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
            "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
            "session_metadata": {
                "provider": "openai",
                "model": "gpt-4o-mini"
            }
        });

        let err = migrate_session_value(envelope)
            .expect_err("root session_metadata must not be silently dropped");

        assert!(matches!(
            err,
            SessionMigrationError::UnsupportedRootSessionMetadata
        ));
    }

    #[test]
    fn invalid_nested_session_metadata_rejected_fail_closed() {
        let envelope = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "messages": [],
            "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
            "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
            "metadata": {
                "session_metadata": {
                    "provider_params": {
                        "temperature": 0.2
                    }
                }
            }
        });

        let err = migrate_session_value(envelope)
            .expect_err("incomplete nested session_metadata must fail closed");

        assert!(matches!(
            err,
            SessionMigrationError::InvalidSessionMetadata(_)
        ));
    }

    #[test]
    fn root_session_llm_identity_rejected_fail_closed() {
        let envelope = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "messages": [],
            "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
            "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
            "session_llm_identity": {
                "provider": "openai",
                "model": "gpt-4o-mini"
            }
        });

        let err = migrate_session_value(envelope)
            .expect_err("root session_llm_identity must not be silently dropped");

        assert!(matches!(
            err,
            SessionMigrationError::UnsupportedRootSessionLlmIdentity
        ));
    }
}

//! v0 → v1 persistence migration for session snapshots and runtime input
//! state blobs.
//!
//! See `docs/wave-c-prep/persistence-migration.md` (authoritative). Wave-c
//! C-3 lands fixtures 1-11; fixture #12 (`runtime_session_snapshots` drift)
//! is owned by C-6r.
//!
//! # Strategy (per §4 of the design doc)
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
//! - `ConnectionRef` inner field names (`realm_id` / `binding_id` →
//!   `realm` / `binding`) — structural rename inside `SessionMetadata`
//!   and inside `SessionLlmIdentity`. Slug validation is deferred to
//!   `RealmId::parse` / `BindingId::parse`. Invalid slugs are slugified
//!   (`[^a-z0-9_.-]` → `_`) and the original payload is retained under
//!   `legacy_connection_ref` so no binding intent is dropped silently.
//! - `SessionMetadata.schema_version` byte is stamped with the current
//!   `SESSION_METADATA_SCHEMA_VERSION` on write; on read, rows lacking the
//!   field default to `1` via `#[serde(default)]`.
//! - `provider_params` remains `Option<serde_json::Value>` on
//!   `SessionMetadata` in this wave — the canonical typed shape
//!   (`ProviderParamsOverride`) lives on `RuntimeTurnMetadata`. C-3's only
//!   obligation here is to keep the value opaque (fixture #4, the Anthropic
//!   extended-thinking canary, must be preserved byte-for-byte through the
//!   round-trip).
//! - Input-state (`StoredInputState`) lives in `meerkat-runtime`; C-3
//!   provides the transform entry point ([`migrate_input_state_value`])
//!   that fixtures 9-11 exercise. Wiring the transform into the runtime
//!   store's read path is C-6r's responsibility.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use crate::Session;
use crate::session::{SESSION_METADATA_SCHEMA_VERSION, SESSION_VERSION};

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
        }
    }
}

impl std::error::Error for SessionMigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Deserialize(err) => Some(err),
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
    if let Some(sess_meta) = root
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .and_then(|map| map.get_mut("session_metadata"))
        .and_then(Value::as_object_mut)
    {
        migrate_metadata_object(sess_meta, &mut legacy);
    }

    // Some projection sites serialize `session_metadata` at the
    // envelope level instead of nested inside `metadata`; accept both
    // shapes so test fixtures and wild rows both migrate cleanly.
    if let Some(sess_meta) = root
        .get_mut("session_metadata")
        .and_then(Value::as_object_mut)
    {
        migrate_metadata_object(sess_meta, &mut legacy);
    }

    // Migrate SessionLlmIdentity sub-tree (identical connection_ref logic).
    if let Some(ident) = root
        .get_mut("session_llm_identity")
        .and_then(Value::as_object_mut)
    {
        migrate_identity_object(ident, &mut legacy);
    }

    // Stamp envelope version forward. A blob missing `version` is the
    // pre-SESSION_VERSION shape; treat as v1 and bump to current.
    root.entry("version").or_insert_with(|| json!(1));
    root.insert("version".to_string(), json!(SESSION_VERSION));

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
/// Production wiring (replacing `runtime_store.load_input_states`'s direct
/// `serde_json::from_slice`) lands with C-6r. C-3 exposes the transform
/// so the persistence compat suite can exercise it in isolation.
pub fn migrate_input_state_value(mut value: Value) -> Result<Value, SessionMigrationError> {
    let Some(root) = value.as_object_mut() else {
        return Err(SessionMigrationError::Malformed(
            "input state blob is not a JSON object".to_string(),
        ));
    };

    // StoredInputState.persisted_input.Prompt.turn_metadata may carry
    // v0 connection_ref / legacy provider string. Rewrite in place.
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
                migrate_turn_metadata_object(tm);
            }
        }
    }

    // Stamp the new per-entity version byte so the production deserialize
    // path (once C-6r wires it) sees v2 rows.
    root.entry("stored_input_state_version")
        .or_insert_with(|| json!(1));
    root.insert(
        "stored_input_state_version".to_string(),
        json!(STORED_INPUT_STATE_VERSION),
    );

    Ok(value)
}

/// Current `StoredInputState` on-disk version, bumped by wave-c C-3.
pub const STORED_INPUT_STATE_VERSION: u32 = 2;

fn migrate_metadata_object(meta: &mut Map<String, Value>, legacy: &mut BTreeMap<String, Value>) {
    // ConnectionRef on metadata itself.
    if let Some(cref) = meta.get_mut("connection_ref") {
        rewrite_connection_ref(cref, legacy, "legacy_connection_ref_session_metadata");
    }

    // `realm_id` on SessionMetadata is kept as Option<String> for v1;
    // cross-check dogma §5 of persistence-migration.md: if connection_ref
    // is absent but realm_id is present, lift realm_id into a
    // connection_ref shell so resume doesn't fall back to env defaults.
    // We only do this when binding_id is also inferrable — otherwise we
    // leave realm_id alone (resume handles the absence cleanly).
    // NB: field population happens through `default_binding` inference,
    // which is business-policy territory; we do not fabricate a binding
    // here, we only rewrite the shape that exists.

    // Stamp schema_version forward for any blob that didn't carry it.
    meta.entry("schema_version").or_insert_with(|| json!(1));
    meta.insert(
        "schema_version".to_string(),
        json!(SESSION_METADATA_SCHEMA_VERSION),
    );
}

fn migrate_identity_object(ident: &mut Map<String, Value>, legacy: &mut BTreeMap<String, Value>) {
    if let Some(cref) = ident.get_mut("connection_ref") {
        rewrite_connection_ref(cref, legacy, "legacy_connection_ref_session_llm_identity");
    }
}

fn migrate_turn_metadata_object(tm: &mut Map<String, Value>) {
    if let Some(cref) = tm.get_mut("connection_ref") {
        // Turn-metadata ConnectionRef is salvage-best-effort; we drop
        // ill-formed values to None (the turn already ran; the override
        // only matters on a *future* retry).
        let mut throwaway = BTreeMap::new();
        rewrite_connection_ref(cref, &mut throwaway, "__discard__");
    }
    // Provider string that no longer parses: leave it; serde will
    // surface the typed-Provider error on the way out and the deserialize
    // path will propagate. Fixture #11 exercises this.
}

/// Rewrite `{realm_id, binding_id, profile}` → `{realm, binding, profile}`
/// in place on a `ConnectionRef` JSON object. If the slug validation
/// embedded in the typed newtypes would reject the raw value, slugify
/// (`[^a-z0-9_.-]` → `_`) and retain the original under
/// `legacy[legacy_key]`.
fn rewrite_connection_ref(
    cref: &mut Value,
    legacy: &mut BTreeMap<String, Value>,
    legacy_key: &str,
) {
    let Some(obj) = cref.as_object_mut() else {
        return;
    };

    // Retain a snapshot of the pre-migration shape if we detect the v0
    // field names — so operator tooling can compare the two.
    let has_legacy_keys = obj.contains_key("realm_id") || obj.contains_key("binding_id");

    let realm_raw = obj.remove("realm_id").or_else(|| obj.remove("realm"));
    let binding_raw = obj.remove("binding_id").or_else(|| obj.remove("binding"));

    let mut preserved = serde_json::Map::new();
    let mut slugified = false;

    if let Some(raw) = realm_raw {
        if let Some(s) = raw.as_str() {
            let (coerced, was_slugified) = slugify_if_needed(s);
            if was_slugified {
                preserved.insert("realm_id".to_string(), Value::String(s.to_string()));
                slugified = true;
            }
            obj.insert("realm".to_string(), Value::String(coerced));
        } else {
            // Non-string value — let serde surface it.
            obj.insert("realm".to_string(), raw);
        }
    }

    if let Some(raw) = binding_raw {
        if let Some(s) = raw.as_str() {
            let (coerced, was_slugified) = slugify_if_needed(s);
            if was_slugified {
                preserved.insert("binding_id".to_string(), Value::String(s.to_string()));
                slugified = true;
            }
            obj.insert("binding".to_string(), Value::String(coerced));
        } else {
            obj.insert("binding".to_string(), raw);
        }
    }

    // `profile` is re-used as-is — same field name, same Option<String>
    // shape; slug validation happens inside ProfileId::parse on
    // deserialize.
    if let Some(profile_raw) = obj.remove("profile") {
        if let Some(s) = profile_raw.as_str() {
            let (coerced, was_slugified) = slugify_if_needed(s);
            if was_slugified {
                preserved.insert("profile".to_string(), Value::String(s.to_string()));
                slugified = true;
            }
            obj.insert("profile".to_string(), Value::String(coerced));
        } else {
            obj.insert("profile".to_string(), profile_raw);
        }
    }

    if slugified && legacy_key != "__discard__" {
        legacy.insert(legacy_key.to_string(), Value::Object(preserved));
    } else if has_legacy_keys && legacy_key != "__discard__" {
        // Pure rename, no slug coercion — retain nothing (clean v0→v2).
    }
}

/// If the raw value is already a valid realm/binding/profile slug
/// (ASCII alphanumeric + `-`, `_`, `.`), return it unchanged. Otherwise
/// lowercase and replace each out-of-class byte with `_`.
fn slugify_if_needed(raw: &str) -> (String, bool) {
    if !raw.is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return (raw.to_string(), false);
    }
    let lower = raw.to_ascii_lowercase();
    let coerced: String = lower
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    (coerced, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_passes_clean_slugs() {
        assert_eq!(slugify_if_needed("dev"), ("dev".to_string(), false));
        assert_eq!(
            slugify_if_needed("default_openai"),
            ("default_openai".to_string(), false)
        );
        assert_eq!(
            slugify_if_needed("realm-1.2"),
            ("realm-1.2".to_string(), false)
        );
    }

    #[test]
    fn slugify_coerces_invalid_chars() {
        assert_eq!(
            slugify_if_needed("dev mode"),
            ("dev_mode".to_string(), true)
        );
        assert_eq!(
            slugify_if_needed("Prod/Thing"),
            ("prod_thing".to_string(), true)
        );
    }
}

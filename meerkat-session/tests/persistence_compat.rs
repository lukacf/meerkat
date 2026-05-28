#![allow(clippy::panic, clippy::unnecessary_debug_formatting)]

//! Tripwire for wave-c (Section 1.5 #2). Flipped green by **C-3**
//! (fixtures 1-11) + **C-6r** (fixture #12, runtime-side snapshot
//! table).
//!
//! Invariant: compatible v0 session/runtime-store fixtures must load
//! under the v1 typed schema with lossless round-trip semantics. The
//! canary is fixture #4 (Anthropic extended-thinking
//! `thinking: {type:"enabled", budget_tokens:32000}`), which is the
//! production shape most at risk of being silently dropped by an eager
//! typed retype. Auth-binding identity aliases, root
//! `session_metadata`, and root `session_llm_identity` are intentionally
//! not compatible migration shapes: changing or lowering those facts
//! affects trust/default resolution, so the migration shell must fail
//! closed unless a generated authority owns the value change.
//!
//! At c.0 the stub used fixtures as inline blobs. C-3 lands:
//! - disk fixtures under `meerkat-session/tests/fixtures/pre_wave_b/`
//!   so helper drift cannot mask regressions;
//! - the migration entry points in
//!   `meerkat_session::persistent::migrations`;
//! - per-fixture typed assertions (the Anthropic `thinking` canary
//!   verifies the typed bag preserves the nested shape byte-for-byte).
//!
//! Fixture #12 (`runtime_session_snapshot_drift`) was un-ignored by
//! C-6r, which routes the `runtime_session_snapshots` read path in
//! `PersistentSessionService::load_authoritative_session_base`
//! through `deserialize_session_migrating` — the same migration entry
//! point used for the primary `SessionStore::load` path. Now green.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use meerkat_core::generated::session_persistence_version_authority::STORED_INPUT_STATE_VERSION;
use meerkat_core::{SESSION_METADATA_SCHEMA_VERSION, SESSION_VERSION};
use meerkat_session::migrations::{
    SessionMigrationError, migrate_input_state_value, migrate_session_value,
};
use serde_json::{Value, json};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("pre_wave_b")
}

fn load_fixture(name: &str) -> Value {
    let path = fixture_dir().join(format!("{name}.json"));
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|err| panic!("fixture {name} missing at {path:?}: {err}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("fixture {name} parse error: {err}"))
}

/// Call the session-envelope migrator for a fixture where the scenario
/// blob is stored under root `session_metadata`; we merge it into a
/// complete, supported nested SessionMetadata shell before handing it
/// to the migrator so the round-trip exercises the full typed path.
/// Direct root `session_metadata` is a separate fail-closed contract
/// below.
fn migrate_metadata_scenario(name: &str) -> Result<meerkat_core::Session, SessionMigrationError> {
    let raw = load_fixture(name);
    let metadata_root = raw
        .as_object()
        .and_then(|obj| obj.get("session_metadata").cloned())
        .unwrap_or(Value::Null);
    let mut session_metadata = json!({
        "model": "gpt-4o-mini",
        "max_tokens": 4096,
        "provider": "open_a_i",
        "tooling": {},
        "comms_name": null,
    });
    if let (Some(target), Some(source)) =
        (session_metadata.as_object_mut(), metadata_root.as_object())
    {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    let session_llm_identity = raw
        .as_object()
        .and_then(|obj| obj.get("session_llm_identity").cloned());
    let id = raw
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("00000000-0000-0000-0000-000000000001")
        .to_string();

    let mut envelope = json!({
        "id": id,
        "messages": [],
        "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "metadata": {
            "session_metadata": session_metadata,
        },
    });
    if let Some(ident) = session_llm_identity {
        envelope
            .as_object_mut()
            .unwrap()
            .insert("session_llm_identity".to_string(), ident);
    }
    migrate_session_value(envelope)
}

/// Fixture #1: empty metadata. No `session_metadata` key at all — the
/// migrator should be a no-op on the metadata sub-tree.
#[test]
fn fixture_01_session_empty_metadata() {
    // Synthesize a minimal v0 Session blob — no metadata at all.
    let envelope = json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "messages": [],
        "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
    });
    let session = migrate_session_value(envelope).expect("empty metadata must migrate cleanly");
    assert_eq!(session.version(), SESSION_VERSION);
    // Ensure we didn't fabricate a SessionMetadata entry out of thin air.
    assert!(
        session.metadata().get("session_metadata").is_none(),
        "migrator must not synthesize SessionMetadata when none was persisted"
    );
}

#[test]
fn migration_rejects_unsupported_session_envelope_version() {
    let envelope = json!({
        "version": SESSION_VERSION + 100,
        "id": "00000000-0000-0000-0000-000000000001",
        "messages": [],
        "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
    });
    let err =
        migrate_session_value(envelope).expect_err("future envelope version must fail closed");
    assert!(
        matches!(err, SessionMigrationError::GeneratedAuthority(_)),
        "unexpected error: {err:?}"
    );
}

#[test]
fn migration_rejects_unsupported_session_metadata_schema_version() {
    let envelope = json!({
        "id": "00000000-0000-0000-0000-000000000001",
        "messages": [],
        "created_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "updated_at": { "secs_since_epoch": 0, "nanos_since_epoch": 0 },
        "metadata": {
            "session_metadata": {
                "schema_version": SESSION_METADATA_SCHEMA_VERSION + 100,
            },
        },
    });
    let err = migrate_session_value(envelope).expect_err("future metadata schema must fail closed");
    assert!(
        matches!(err, SessionMigrationError::GeneratedAuthority(_)),
        "unexpected error: {err:?}"
    );
}

#[test]
fn migration_rejects_root_session_metadata() {
    let err = migrate_session_value(load_fixture("session_provider_params_openai"))
        .expect_err("root session_metadata must fail closed");
    assert!(
        matches!(err, SessionMigrationError::UnsupportedRootSessionMetadata),
        "unexpected error: {err:?}"
    );
}

#[test]
fn migration_rejects_unsupported_stored_input_state_version() {
    let raw = json!({
        "stored_input_state_version": STORED_INPUT_STATE_VERSION + 100,
    });
    let err = migrate_input_state_value(raw)
        .expect_err("future stored input-state version must fail closed");
    assert!(
        matches!(err, SessionMigrationError::GeneratedAuthority(_)),
        "unexpected error: {err:?}"
    );
}

/// Fixture #2: OpenAI provider_params — typed temperature + reasoning +
/// unknown (encrypted_content).
#[test]
fn fixture_02_session_provider_params_openai() {
    let session =
        migrate_metadata_scenario("session_provider_params_openai").expect("must migrate");
    let metadata_json = session
        .metadata()
        .get("session_metadata")
        .expect("session_metadata must be present after migration");
    let params = metadata_json
        .get("provider_params")
        .expect("provider_params preserved");

    // provider_params stays opaque Value at SessionMetadata level in
    // this wave (retype lives on RuntimeTurnMetadata, not on metadata).
    // Canary: temperature, reasoning, and the opaque encrypted_content
    // all survive round-trip.
    assert_eq!(params.get("temperature").and_then(Value::as_f64), Some(0.2));
    assert_eq!(
        params.get("reasoning").and_then(Value::as_str),
        Some("silent")
    );
    assert_eq!(
        params.get("encrypted_content").and_then(Value::as_str),
        Some("Zm9v")
    );
    // Schema version byte stamped forward.
    assert_eq!(
        metadata_json.get("schema_version").and_then(Value::as_u64),
        Some(u64::from(SESSION_METADATA_SCHEMA_VERSION))
    );
}

/// Fixture #3: Anthropic signature passthrough.
#[test]
fn fixture_03_session_provider_params_anthropic_signature() {
    let session = migrate_metadata_scenario("session_provider_params_anthropic_signature")
        .expect("must migrate");
    let params = session
        .metadata()
        .get("session_metadata")
        .and_then(|m| m.get("provider_params"))
        .expect("provider_params preserved");
    assert_eq!(
        params.get("signature").and_then(Value::as_str),
        Some("abc123")
    );
}

/// Fixture #4 — THE CANARY. Anthropic extended-thinking shape
/// `{thinking: {type:"enabled", budget_tokens:32000}}`. Must survive
/// byte-for-byte through the typed round-trip; this is the production
/// payload most at risk of silent drop.
#[test]
fn fixture_04_anthropic_thinking_canary() {
    let session = migrate_metadata_scenario("session_provider_params_anthropic_thinking")
        .expect("must migrate");
    let params = session
        .metadata()
        .get("session_metadata")
        .and_then(|m| m.get("provider_params"))
        .expect("provider_params preserved");
    let thinking = params
        .get("thinking")
        .expect("thinking bag MUST survive — silent-drop canary");

    // Byte-for-byte shape preservation, not just "is_ok()".
    assert_eq!(
        thinking.get("type").and_then(Value::as_str),
        Some("enabled"),
        "thinking.type dropped by migration — regression of silent-drop canary"
    );
    assert_eq!(
        thinking.get("budget_tokens").and_then(Value::as_u64),
        Some(32000),
        "thinking.budget_tokens dropped — regression of silent-drop canary"
    );

    // Round-trip: re-serialise migrated Session, re-run migrator, assert
    // idempotent (no further transforms applied).
    let reserialized = serde_json::to_value(&session).unwrap();
    let again = migrate_session_value(reserialized).expect("idempotent on v2 blob");
    let thinking_again = again
        .metadata()
        .get("session_metadata")
        .and_then(|m| m.get("provider_params"))
        .and_then(|p| p.get("thinking"))
        .expect("thinking survives second migration pass");
    assert_eq!(
        thinking_again.get("budget_tokens").and_then(Value::as_u64),
        Some(32000)
    );
}

/// Fixture #5: non-object provider_params (scalar). Must migrate
/// cleanly — the Value-opaque carrier preserves scalars unchanged.
#[test]
fn fixture_05_session_provider_params_unknown_scalar() {
    let session =
        migrate_metadata_scenario("session_provider_params_unknown").expect("must migrate");
    let params = session
        .metadata()
        .get("session_metadata")
        .and_then(|m| m.get("provider_params"))
        .expect("provider_params preserved");
    assert_eq!(params.as_u64(), Some(42));
}

/// Fixture #6: legacy AuthBindingRef aliases are identity/trust facts,
/// so the migration shell rejects them fail-closed instead of rewriting
/// `connection_ref.{realm_id,binding_id}` to `auth_binding.{realm,binding}`.
#[test]
fn fixture_06_auth_binding_legacy_identity_rejected_fail_closed() {
    let err = migrate_metadata_scenario("session_auth_binding_slug_valid")
        .expect_err("legacy auth-binding identity aliases must fail closed");

    assert!(
        matches!(
            err,
            SessionMigrationError::UnsupportedLegacyAuthBinding {
                location: "SessionMetadata",
                field: "connection_ref",
            }
        ),
        "unexpected error: {err:?}"
    );
}

/// Fixture #7: invalid legacy auth-binding slugs are not slugified or
/// salvaged by compatibility code; the identity alias itself is
/// rejected before any value rewrite can occur.
#[test]
fn fixture_07_auth_binding_invalid_legacy_identity_rejected_fail_closed() {
    let err = migrate_metadata_scenario("session_auth_binding_slug_invalid")
        .expect_err("legacy auth-binding identity aliases must fail closed");

    assert!(
        matches!(
            err,
            SessionMigrationError::UnsupportedLegacyAuthBinding {
                location: "SessionMetadata",
                field: "connection_ref",
            }
        ),
        "unexpected error: {err:?}"
    );
}

/// Fixture #8: root `session_llm_identity` is an identity/trust fact
/// that `Session` serde does not carry, so migration rejects it
/// fail-closed instead of silently dropping or lowering it.
#[test]
fn fixture_08_hot_swap_identity_mixed() {
    let err = migrate_metadata_scenario("session_hot_swap_identity_mixed")
        .expect_err("root session_llm_identity must fail closed");

    assert!(
        matches!(
            err,
            SessionMigrationError::UnsupportedRootSessionLlmIdentity
        ),
        "unexpected error: {err:?}"
    );
}

/// Fixture #9: v0 input-state with full turn_metadata (provider_params,
/// additional_instructions list of strings, model).
#[test]
fn fixture_09_input_state_full_turn_metadata() {
    let raw = load_fixture("input_state_prompt_full_turn_metadata");
    let migrated = migrate_input_state_value(raw).expect("input state migrates");
    let tm = migrated
        .get("persisted_input")
        .and_then(|p| p.get("Prompt"))
        .and_then(|p| p.get("turn_metadata"))
        .expect("turn_metadata survives migration");
    assert_eq!(
        tm.get("provider_params")
            .and_then(|p| p.get("temperature"))
            .and_then(Value::as_f64),
        Some(0.7)
    );
    // Bare-string additional_instructions preserved as-is — the typed
    // lift into `TurnInstruction` happens at read-time inside
    // RuntimeTurnMetadata serde (C-6r wires the production read path).
    let instr = tm
        .get("additional_instructions")
        .and_then(Value::as_array)
        .expect("additional_instructions preserved");
    assert_eq!(instr.len(), 2);
    assert_eq!(instr[0].as_str(), Some("foo"));

    // Version byte stamped.
    assert_eq!(
        migrated
            .get("stored_input_state_version")
            .and_then(Value::as_u64),
        Some(u64::from(STORED_INPUT_STATE_VERSION))
    );
}

/// Fixture #10: minimal input state — no turn_metadata. Migration must
/// be an identity on the absent field (no fabrication).
#[test]
fn fixture_10_input_state_continuation_minimal() {
    let raw = load_fixture("input_state_continuation_minimal");
    let migrated = migrate_input_state_value(raw).expect("input state migrates");
    // No turn_metadata synthesis.
    assert!(
        migrated
            .get("persisted_input")
            .and_then(|p| p.get("Continuation"))
            .and_then(|c| c.get("turn_metadata"))
            .is_none(),
        "migration must not fabricate turn_metadata"
    );
    assert_eq!(
        migrated
            .get("stored_input_state_version")
            .and_then(Value::as_u64),
        Some(u64::from(STORED_INPUT_STATE_VERSION))
    );
}

/// Fixture #11: retired provider string. The transform preserves the
/// raw string — typed provider parse happens downstream inside
/// `Provider::deserialize` (read-time surface in C-6r).
#[test]
fn fixture_11_input_state_provider_unknown_string() {
    let raw = load_fixture("input_state_provider_unknown_string");
    let migrated = migrate_input_state_value(raw).expect("input state migrates");
    let provider = migrated
        .get("persisted_input")
        .and_then(|p| p.get("Prompt"))
        .and_then(|p| p.get("turn_metadata"))
        .and_then(|tm| tm.get("provider"))
        .and_then(Value::as_str);
    assert_eq!(
        provider,
        Some("retired_backend_v0"),
        "retired provider string preserved — downstream Provider::deserialize \
         surfaces the typed error without silent-dropping the legacy value"
    );
}

/// Fixture #12: runtime-snapshot drift — owned by C-6r. Un-ignored
/// once the runtime-side `runtime_session_snapshots` load path in
/// `meerkat-session::persistent::SessionService::restore_session`
/// routes the raw blob through `deserialize_session_migrating`,
/// which wraps `migrate_session_value` directly. The legacy payload
/// canary (`thinking.budget_tokens`) now survives the round-trip
/// without the shell needing to re-hydrate `provider_params` by
/// hand, honoring the wave-c C-3 persistence contract.
#[test]
fn fixture_12_runtime_session_snapshot_drift() {
    let raw = load_fixture("runtime_session_snapshot_drift");
    let snapshot = raw
        .get("snapshot")
        .cloned()
        .expect("fixture contains a `snapshot` wrapper");
    let migrated =
        migrate_session_value(snapshot).expect("runtime snapshot must round-trip via migration");
    // Canary survives through the runtime-side path as well.
    let thinking = migrated
        .metadata()
        .get("session_metadata")
        .and_then(|m| m.get("provider_params"))
        .and_then(|p| p.get("thinking"))
        .expect("thinking survives runtime snapshot migration");
    assert_eq!(
        thinking.get("budget_tokens").and_then(Value::as_u64),
        Some(32000)
    );
}

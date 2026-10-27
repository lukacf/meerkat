#![allow(
    clippy::all,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unused_imports,
    redundant_semicolons,
    clippy::redundant_clone
)]
//! External boundary tests for identity-first continuity.
//!
//! These tests verify assumptions about Meerkat crate interfaces that the
//! identity-first model depends on. They are marked `#[ignore]` to keep
//! them out of the default fast suite and MUST be run at Gate 0 via
//! `--run-ignored all`.

use std::sync::Arc;

// ---------------------------------------------------------------------------
// Task 0.23 — External boundary: Meerkat SessionStore interface
//
// Proves Meta can call SessionStore::load/save with real types from
// meerkat_store crate, round-trip a session through SqliteSessionStore.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn identity_first_boundary_session_store_roundtrip() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let db_path = tmp.path().join("sessions.db");

    let store: Arc<dyn meerkat_store::SessionStore> =
        Arc::new(meerkat_store::SqliteSessionStore::open(&db_path).expect("open store"));

    // Create a session and save it
    let session = meerkat_core::Session::new();
    let session_id = session.id().clone();

    store.save(&session).await.expect("save session");

    // Load it back
    let loaded = store
        .load(&session_id)
        .await
        .expect("load session")
        .expect("session should exist");

    assert_eq!(loaded.id(), &session_id);
    assert_eq!(loaded.messages().len(), session.messages().len());

    // Verify exists
    let exists = store.exists(&session_id).await.expect("exists check");
    assert!(exists);

    // Prove we can serialize a Session to bytes (for SessionSnapshot)
    let serialized = serde_json::to_vec(&session).expect("serialize session");
    assert!(!serialized.is_empty());

    // And deserialize back
    let deserialized: meerkat_core::Session =
        serde_json::from_slice(&serialized).expect("deserialize session");
    assert_eq!(deserialized.id(), &session_id);
}

// ---------------------------------------------------------------------------
// Task 0.24 — External boundary: Meerkat session restore path
//
// Proves CreateSessionRequest.build.resume_session accepts a loaded
// session and the session pipeline restores it.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Task 0.25 — External boundary: MobDefinition compatibility
//
// Proves MobDefinition::from_toml() works for a definition containing
// profiles used by DurableAgentSpec, ProfileName type matches.
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn identity_first_boundary_mob_definition_compatibility() {
    let toml_str = r#"
[mob]
id = "test-mob"

[profiles.lead]
model = "claude-sonnet-4-6"
system_prompt = "You are the lead agent."

[profiles.worker]
model = "claude-haiku-4-5-20251001"
system_prompt = "You are a worker agent."
"#;

    let def = meerkat_mob::MobDefinition::from_toml(toml_str).expect("parse mob definition");

    // Verify profiles are accessible
    assert!(
        def.profiles
            .contains_key(&meerkat_mob::ProfileName::from("lead"))
    );
    assert!(
        def.profiles
            .contains_key(&meerkat_mob::ProfileName::from("worker"))
    );

    // Verify ProfileName from MobDefinition is the same type used in DurableAgentSpec
    let profile_name = meerkat_mob::ProfileName::from("lead");
    let spec = meerkat_meta::identity_first::DurableAgentSpec {
        identity: meerkat_meta::identity_first::AgentIdentity::parse("triage:main").expect("parse"),
        profile: profile_name.clone(),
        addressability: meerkat_meta::identity_first::AgentAddressability::Addressable,
        display_name: None,
        labels: std::collections::BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };

    // The fact that this compiles proves ProfileName type compatibility
    assert!(def.profiles.contains_key(&spec.profile));

    // Verify RosterContext can hold a MobDefinition
    let roster_ctx = meerkat_meta::identity_first::RosterContext {
        mob_definition: Some(def),
        previous_identities: vec![],
    };
    assert!(roster_ctx.mob_definition.is_some());
}

//! Acceptance pin for OB3's exact Meerkat 0.8.10 recovery-migration session.
//!
//! The fixture contains one system message and no user/provider transcript.
//! It proves the frozen `RecoveryMigration` + legacy authority-base stamp only
//! at the explicit importer boundary; the resulting domain Session carries no
//! checkpoint vocabulary.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_core::{
    Released0810ImportError, Released0810ImportEvidence, SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
    Session, import_released_0810_session,
};
use sha2::{Digest, Sha256};

const FIXTURE: &[u8] = include_bytes!("fixtures/v0_8_10_ob3_recovery_migration_session.json");
const PROVENANCE: &[u8] =
    include_bytes!("fixtures/v0_8_10_ob3_recovery_migration_session.provenance.json");
const FIXTURE_SHA256: &str = "43e49a7b216cf61f6ba8f289824c9d6e24a64a81d873f9eb4a09c5b3f6f0cd98";
const RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY: &str = "session_runtime_checkpoint_provenance_v1";

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").unwrap();
    }
    encoded
}

fn released_stamp_key(metadata: &serde_json::Map<String, serde_json::Value>) -> Option<&str> {
    metadata.iter().find_map(|(key, value)| {
        let fields = value.as_object()?;
        (fields.contains_key("schema_version")
            && fields.contains_key("checkpoint_revision")
            && fields.contains_key("authority_base")
            && fields.contains_key("provenance"))
        .then_some(key.as_str())
    })
}

#[test]
fn ob3_released_recovery_migration_runs_only_through_one_time_importer() {
    assert_eq!(FIXTURE.len(), 52_693);
    assert_eq!(sha256_hex(FIXTURE), FIXTURE_SHA256);

    let provenance: serde_json::Value =
        serde_json::from_slice(PROVENANCE).expect("fixture provenance JSON");
    assert_eq!(provenance["bytes"], FIXTURE.len());
    assert_eq!(provenance["sha256"], FIXTURE_SHA256);
    assert_eq!(provenance["producer"]["product"], "meerkat");
    assert_eq!(provenance["producer"]["version"], "0.8.10");
    assert_eq!(
        provenance["delivery"]["classification"],
        "authorized_system_only_session"
    );
    let raw: serde_json::Value = serde_json::from_slice(FIXTURE).expect("released fixture is JSON");
    assert_eq!(raw["messages"].as_array().map(Vec::len), Some(1));
    assert_eq!(raw["messages"][0]["role"], "system");
    let raw_metadata = raw["metadata"]
        .as_object()
        .expect("fixture metadata object");
    assert!(!raw_metadata.contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY));

    let stamp_key = released_stamp_key(raw_metadata).expect("released stamp carrier");
    let raw_stamp = raw_metadata
        .get(stamp_key)
        .and_then(serde_json::Value::as_object)
        .expect("exact released fixture carries its frozen stamp");
    assert_eq!(raw_stamp["schema_version"], 1);
    assert_eq!(raw_stamp["provenance"], "recovery_migration");
    assert_eq!(raw_stamp["generation"], 0);
    assert_eq!(raw_stamp["checkpoint_revision"], 6);
    assert_eq!(
        raw_stamp["digest"],
        provenance["expected"]["checkpoint_digest"]
    );
    assert_eq!(raw_stamp["authority_base"]["kind"], "legacy");
    assert_eq!(
        raw_stamp["authority_base"]["source_blob_digest"],
        "sha256:1645c5370dc3c27cb9194bef236c0dfc5f441cc6dde7c6524270620b97dbde62"
    );
    assert_eq!(raw_stamp["authority_base"]["observed_generation"], 0);
    assert_eq!(
        raw_stamp["authority_base"]["observed_checkpoint_revision"],
        6
    );

    let imported = import_released_0810_session(FIXTURE)
        .expect("the explicit importer must verify this exact released document once");
    assert_eq!(
        imported.receipt().evidence(),
        Released0810ImportEvidence::FrozenCheckpointVerified
    );
    assert_eq!(
        sha256_hex(imported.receipt().source_document_sha256()),
        FIXTURE_SHA256
    );
    let (session, _single_use_receipt) = imported.into_parts();
    assert_eq!(
        session.id().to_string(),
        provenance["expected"]["session_id"]
    );
    assert_eq!(session.messages().len(), 1);
    assert!(
        released_stamp_key(session.metadata()).is_none(),
        "current domain Session must not retain released proof metadata"
    );

    // This exact artifact has no transcript-history graph or witness. The
    // importer preserves that absence and strips all released proof carriers.
    assert!(
        session
            .metadata()
            .get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .is_none()
    );
}

#[test]
fn unstamped_released_envelope_requires_store_authorization_receipt() {
    let mut raw: serde_json::Value =
        serde_json::from_slice(FIXTURE).expect("released fixture is JSON");
    let raw_metadata = raw["metadata"]
        .as_object_mut()
        .expect("fixture metadata object");
    let stamp_key = released_stamp_key(raw_metadata)
        .expect("released stamp carrier")
        .to_string();
    raw_metadata.remove(&stamp_key);
    let unstamped = serde_json::to_vec(&raw).expect("unstamped released envelope");

    let imported = import_released_0810_session(&unstamped)
        .expect("strict released unstamped shape crosses only the explicit importer");
    assert_eq!(
        imported.receipt().evidence(),
        Released0810ImportEvidence::StoreAuthorizationRequired
    );
    assert_eq!(
        imported.receipt().session_id().to_string(),
        raw["id"].as_str().expect("fixture session id")
    );
    assert_eq!(
        sha256_hex(imported.receipt().source_document_sha256()),
        sha256_hex(&unstamped)
    );
    assert!(released_stamp_key(imported.session().metadata()).is_none());
}

#[test]
fn released_importer_requires_json_eof() {
    let mut trailing = FIXTURE.to_vec();
    trailing.extend_from_slice(b"\n{}");
    assert!(matches!(
        import_released_0810_session(&trailing),
        Err(Released0810ImportError::Malformed(_))
    ));

    let mut trailing_whitespace = FIXTURE.to_vec();
    trailing_whitespace.extend_from_slice(b"\n\t ");
    import_released_0810_session(&trailing_whitespace)
        .expect("JSON trailing whitespace remains part of the exact accepted document");
}

#[test]
fn released_runtime_checkpoint_provenance_is_verified_stripped_and_refused_current() {
    let mut released: serde_json::Value =
        serde_json::from_slice(FIXTURE).expect("released fixture is JSON");
    released["metadata"]
        .as_object_mut()
        .expect("fixture metadata object")
        .insert(
            RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY.to_string(),
            serde_json::Value::Bool(true),
        );
    let released = serde_json::to_vec(&released).expect("released envelope with legacy carrier");
    let imported = import_released_0810_session(&released)
        .expect("frozen digest excludes the released runtime-provenance carrier");
    assert!(
        !imported
            .session()
            .metadata()
            .contains_key(RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY)
    );

    let mut current: serde_json::Value =
        serde_json::from_slice(FIXTURE).expect("released fixture is JSON");
    current["version"] = serde_json::json!(3);
    let current_metadata = current["metadata"]
        .as_object_mut()
        .expect("fixture metadata object");
    let stamp_key = released_stamp_key(current_metadata)
        .expect("released stamp carrier")
        .to_string();
    current_metadata.remove(&stamp_key);
    current_metadata.insert(
        RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY.to_string(),
        serde_json::Value::Bool(true),
    );
    serde_json::from_value::<Session>(current)
        .expect_err("current Session must refuse every released checkpoint authority carrier");
}

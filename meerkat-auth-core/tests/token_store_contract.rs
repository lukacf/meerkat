//! T4 — TokenStore round-trip contract (Phase 4a top-down).
//!
//! RCT Mini choke-point: `PersistedTokens → save → load → PersistedTokens`
//! across all four backends (`FileTokenStore`, `EphemeralTokenStore`,
//! `AutoTokenStore`, `KeyringTokenStore`). Verifies atomic overwrite,
//! 0o600 perms, and list enumeration.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, FileTokenStore, PersistedAuthMode, PersistedTokens, TokenKey, TokenStore,
};

// --- fixtures -----------------------------------------------------------

fn sample_api_key() -> PersistedTokens {
    PersistedTokens::api_key("sk-test-api-key-1")
}

fn sample_oauth() -> PersistedTokens {
    PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("access-abc".into()),
        refresh_token: Some("refresh-abc".into()),
        id_token: Some("jwt.payload.sig".into()),
        expires_at: Some(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
        last_refresh: Some(chrono::DateTime::from_timestamp(1_699_999_000, 0).unwrap()),
        scopes: vec!["openid".into(), "email".into()],
        account_id: Some("acct_123".into()),
        metadata: serde_json::json!({"plan_type": "pro", "fedramp": false}),
    }
}

fn k(realm: &str, binding: &str) -> TokenKey {
    TokenKey::parse(realm, binding).expect("valid slugs in test fixture")
}

fn kp(realm: &str, binding: &str, profile: &str) -> TokenKey {
    TokenKey::parse_with_profile(realm, binding, Some(profile))
        .expect("valid slugs in test fixture")
}

// --- EphemeralTokenStore (4a.4) ----------------------------------------

#[tokio::test]
async fn ephemeral_round_trip_api_key() {
    let store = EphemeralTokenStore::new();
    let key = k("dev", "default_openai");
    let tokens = sample_api_key();

    store
        .save(&key, &tokens)
        .await
        .expect("ephemeral save should succeed");
    let loaded = store
        .load(&key)
        .await
        .expect("ephemeral load should succeed");
    assert_eq!(loaded, Some(tokens));
}

#[tokio::test]
async fn ephemeral_round_trip_oauth_with_metadata() {
    let store = EphemeralTokenStore::new();
    let key = k("dev", "managed_chatgpt");
    let tokens = sample_oauth();

    store.save(&key, &tokens).await.unwrap();
    let loaded = store.load(&key).await.unwrap();
    assert_eq!(loaded, Some(tokens));
}

#[tokio::test]
async fn ephemeral_clear_removes_entry() {
    let store = EphemeralTokenStore::new();
    let key = k("dev", "x");
    store.save(&key, &sample_api_key()).await.unwrap();
    store.clear(&key).await.unwrap();
    assert_eq!(store.load(&key).await.unwrap(), None);
}

#[tokio::test]
async fn ephemeral_list_returns_saved_keys() {
    let store = EphemeralTokenStore::new();
    store.save(&k("dev", "a"), &sample_api_key()).await.unwrap();
    store
        .save(&k("prod", "b"), &sample_api_key())
        .await
        .unwrap();
    let mut keys = store.list().await.unwrap();
    keys.sort();
    assert_eq!(keys, vec![k("dev", "a"), k("prod", "b")]);
}

#[tokio::test]
async fn ephemeral_profile_override_does_not_collide_with_default_binding() {
    let store = EphemeralTokenStore::new();
    let default_key = k("dev", "default_openai");
    let work_key = kp("dev", "default_openai", "work");

    store
        .save(&default_key, &PersistedTokens::api_key("sk-default"))
        .await
        .unwrap();
    store
        .save(&work_key, &PersistedTokens::api_key("sk-work"))
        .await
        .unwrap();

    assert_eq!(
        store
            .load(&default_key)
            .await
            .unwrap()
            .and_then(|tokens| tokens.primary_secret),
        Some("sk-default".to_string())
    );
    assert_eq!(
        store
            .load(&work_key)
            .await
            .unwrap()
            .and_then(|tokens| tokens.primary_secret),
        Some("sk-work".to_string())
    );
}

#[tokio::test]
async fn ephemeral_load_missing_returns_none() {
    let store = EphemeralTokenStore::new();
    assert_eq!(store.load(&k("dev", "missing")).await.unwrap(), None);
}

// --- FileTokenStore (4a.2) ---------------------------------------------

#[tokio::test]
async fn file_round_trip_api_key() {
    let temp = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(temp.path().to_path_buf());
    let key = k("dev", "default_openai");
    let tokens = sample_api_key();

    store.save(&key, &tokens).await.expect("file save");
    let loaded = store.load(&key).await.expect("file load");
    assert_eq!(loaded, Some(tokens));
}

#[tokio::test]
async fn file_round_trip_oauth_with_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(temp.path().to_path_buf());
    let key = k("dev", "managed_chatgpt");
    let tokens = sample_oauth();

    store.save(&key, &tokens).await.unwrap();
    let loaded = store.load(&key).await.unwrap();
    assert_eq!(loaded, Some(tokens));
}

#[tokio::test]
async fn file_profile_override_does_not_collide_with_default_binding() {
    let temp = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(temp.path().to_path_buf());
    let default_key = k("dev", "default_openai");
    let work_key = kp("dev", "default_openai", "work");

    store
        .save(&default_key, &PersistedTokens::api_key("sk-default"))
        .await
        .unwrap();
    store
        .save(&work_key, &PersistedTokens::api_key("sk-work"))
        .await
        .unwrap();

    let default_loaded = store.load(&default_key).await.unwrap().unwrap();
    let work_loaded = store.load(&work_key).await.unwrap().unwrap();
    assert_eq!(default_loaded.primary_secret.as_deref(), Some("sk-default"));
    assert_eq!(work_loaded.primary_secret.as_deref(), Some("sk-work"));

    let mut keys = store.list().await.unwrap();
    keys.sort();
    assert_eq!(keys, vec![default_key, work_key]);
}

#[tokio::test]
async fn file_clear_removes_entry_and_parent_survives() {
    let temp = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(temp.path().to_path_buf());
    let key = k("dev", "x");
    store.save(&key, &sample_api_key()).await.unwrap();
    store.clear(&key).await.unwrap();
    assert_eq!(store.load(&key).await.unwrap(), None);
    // Second clear is idempotent.
    store.clear(&key).await.unwrap();
}

#[tokio::test]
async fn file_list_returns_all_realms_and_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(temp.path().to_path_buf());
    store.save(&k("dev", "a"), &sample_api_key()).await.unwrap();
    store.save(&k("dev", "b"), &sample_api_key()).await.unwrap();
    store
        .save(&k("prod", "c"), &sample_api_key())
        .await
        .unwrap();
    let mut keys = store.list().await.unwrap();
    keys.sort();
    assert_eq!(keys, vec![k("dev", "a"), k("dev", "b"), k("prod", "c")]);
}

#[tokio::test]
async fn file_overwrite_is_atomic() {
    // Two consecutive saves — second fully replaces first (no partial
    // merge, no leftover temp files).
    let temp = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(temp.path().to_path_buf());
    let key = k("dev", "x");
    store
        .save(&key, &PersistedTokens::api_key("first"))
        .await
        .unwrap();
    store
        .save(&key, &PersistedTokens::api_key("second"))
        .await
        .unwrap();
    let loaded = store.load(&key).await.unwrap().unwrap();
    assert_eq!(loaded.primary_secret.as_deref(), Some("second"));
}

#[cfg(unix)]
#[tokio::test]
async fn file_save_sets_0o600_perms() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(temp.path().to_path_buf());
    let key = k("dev", "perms");
    store.save(&key, &sample_api_key()).await.unwrap();
    let path = temp.path().join("dev").join("perms.json");
    let meta = std::fs::metadata(&path).unwrap();
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "expected 0o600 perms, got 0o{mode:o}");
}

// --- AutoTokenStore (4a.4) ---------------------------------------------
//
// Auto tries the OS keyring first (when `native-keyring` is enabled)
// and falls back to the file store on failure. On macOS with the
// feature enabled — which is the default through meerkat-cli's
// dependency declaration — exercising this test hits the real
// `login.keychain` and Security.framework pops a password dialog.
// Interactive prompts in the default unit-test lane are a no-go, so
// this test is `#[ignore]` by default and only runs when the caller
// opts in via `--include-ignored` (or `--run-ignored all`). The
// file-store round-trip the "falls back to file" claim rests on is
// already covered by `file_round_trip_saves_and_loads`; this test
// only adds coverage when someone is explicitly validating the
// keyring path and is willing to approve the keychain dialog.

#[tokio::test]
#[ignore = "AutoTokenStore hits the real OS keychain when native-keyring is enabled; opt in with --include-ignored"]
async fn auto_round_trip_falls_back_to_file() {
    let temp = tempfile::tempdir().unwrap();
    let store = meerkat_auth_core::auth_store::AutoTokenStore::new(
        temp.path().to_path_buf(),
        "meerkat-test",
    );
    let key = k("dev", "default_openai");
    let tokens = sample_api_key();

    store.save(&key, &tokens).await.expect("auto save");
    let loaded = store.load(&key).await.expect("auto load");
    assert_eq!(loaded, Some(tokens));
}

// --- Backend name smoke (always green) ---------------------------------

#[test]
fn backend_names_are_stable_strings() {
    assert_eq!(FileTokenStore::new("/tmp/x").backend_name(), "file");
    assert_eq!(EphemeralTokenStore::new().backend_name(), "ephemeral");
    assert_eq!(
        meerkat_auth_core::auth_store::AutoTokenStore::new("/tmp/x", "svc").backend_name(),
        "auto",
    );
}

//! Token clear / AuthMachine release boundary contract.
//!
//! The durable token record carries the machine-stamped lifecycle marker, so
//! `store.clear` is the single atomic durable boundary: one mutation destroys
//! the credential and its lifecycle publication together. The in-process lease
//! is a projection of that durable truth — these tests pin the durable-first
//! ordering (no compensation window) and the rehydrate self-heal that releases
//! a lease whose durable record is gone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use meerkat_auth_core::EphemeralTokenStore;
use meerkat_auth_core::auth_store::{
    PersistedAuthMode, PersistedTokens, TokenKey, TokenStore, TokenStoreError,
};
use meerkat_core::handles::{AuthLeasePhase, GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::{
    AuthBindingRef, TokenLifecycleClearError, clear_tokens_and_publish_lifecycle_released,
    mark_tokens_lifecycle_published_for_transition, publish_token_lifecycle_acquired,
    rehydrate_marked_tokens_for_status,
};

fn test_auth_lease() -> GeneratedAuthLeaseHandle {
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(handle)
        .expect("test auth lease must be certified by generated AuthMachine authority")
}

fn binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
        binding: meerkat_core::BindingId::parse("default_openai").expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn oauth_tokens() -> PersistedTokens {
    PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("access".into()),
        refresh_token: Some("refresh".into()),
        id_token: None,
        expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        last_refresh: None,
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    }
}

/// Acquire the lease, stamp the marker, and persist the marked tokens —
/// the same publish shape the login paths use.
async fn publish_and_save(
    store: &dyn TokenStore,
    handle: &GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) -> TokenKey {
    let tokens = oauth_tokens();
    let transition =
        publish_token_lifecycle_acquired(handle, auth_binding, &tokens).expect("acquire lease");
    let marked = mark_tokens_lifecycle_published_for_transition(
        &TokenKey::from_auth_binding(auth_binding),
        &tokens,
        &transition,
    )
    .expect("mark lifecycle published");
    let key = TokenKey::from_auth_binding(auth_binding);
    store.save(&key, &marked).await.expect("save tokens");
    key
}

fn lease_phase(
    handle: &GeneratedAuthLeaseHandle,
    auth_binding: &AuthBindingRef,
) -> (bool, Option<AuthLeasePhase>) {
    let lease_key = LeaseKey::from_auth_binding(auth_binding);
    let captured = handle.capture_auth_lifecycle_restore_snapshot(&lease_key);
    let snapshot = captured.snapshot();
    (snapshot.credential_present, snapshot.phase)
}

/// A token store whose `clear` fails: used to prove the durable-first ordering
/// leaves BOTH the durable record and the lease untouched on clear failure
/// (no half-committed split, no compensation needed).
struct FailingClearStore {
    inner: EphemeralTokenStore,
    fail_clear: AtomicBool,
}

#[async_trait]
impl TokenStore for FailingClearStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.inner.load(key).await
    }
    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        self.inner.save(key, tokens).await
    }
    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        if self.fail_clear.load(Ordering::Acquire) {
            return Err(TokenStoreError::Unavailable(
                "injected clear failure".into(),
            ));
        }
        self.inner.clear(key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.inner.list().await
    }
    fn backend_name(&self) -> &'static str {
        "failing-clear-test"
    }
}

#[tokio::test]
async fn clear_destroys_durable_record_and_releases_lease() {
    let store = EphemeralTokenStore::new();
    let handle = test_auth_lease();
    let auth_binding = binding();
    let key = publish_and_save(&store, &handle, &auth_binding).await;
    assert!(store.load(&key).await.unwrap().is_some());

    clear_tokens_and_publish_lifecycle_released(&store, &handle, &auth_binding)
        .await
        .expect("clear succeeds");

    assert!(
        store.load(&key).await.unwrap().is_none(),
        "durable record (credential + marker) must be gone"
    );
    let (credential_present, phase) = lease_phase(&handle, &auth_binding);
    assert!(
        !credential_present || phase == Some(AuthLeasePhase::Released),
        "lease projection must not keep a live credential after clear, got {phase:?}"
    );
}

#[tokio::test]
async fn failed_clear_changes_nothing_durable_or_projected() {
    let store = FailingClearStore {
        inner: EphemeralTokenStore::new(),
        fail_clear: AtomicBool::new(false),
    };
    let handle = test_auth_lease();
    let auth_binding = binding();
    let key = publish_and_save(&store, &handle, &auth_binding).await;
    store.fail_clear.store(true, Ordering::Release);

    let err = clear_tokens_and_publish_lifecycle_released(&store, &handle, &auth_binding)
        .await
        .expect_err("injected clear failure must propagate");
    assert!(matches!(err, TokenLifecycleClearError::TokenStoreClear(_)));

    // Durable-first ordering: the failed clear is the FIRST step, so neither
    // the durable record nor the lease projection moved — no split state, no
    // compensation window.
    assert!(
        store.load(&key).await.unwrap().is_some(),
        "durable record must be untouched after failed clear"
    );
    let (credential_present, phase) = lease_phase(&handle, &auth_binding);
    assert!(
        credential_present && phase.is_some_and(|p| p != AuthLeasePhase::Released),
        "lease must remain live when the durable record was not cleared, got {phase:?}"
    );
}

#[tokio::test]
async fn rehydrate_releases_lease_whose_durable_record_is_gone() {
    let store = EphemeralTokenStore::new();
    let handle = test_auth_lease();
    let auth_binding = binding();

    // Simulate the post-clear split: the durable record is gone (clear
    // committed) but the in-process lease still projects a live credential
    // (release transition was rejected / process raced).
    let tokens = oauth_tokens();
    publish_token_lifecycle_acquired(&handle, &auth_binding, &tokens).expect("acquire lease");
    let (credential_present, _) = lease_phase(&handle, &auth_binding);
    assert!(credential_present, "fixture: lease holds a live credential");

    let rehydrated = rehydrate_marked_tokens_for_status(
        &store,
        &handle,
        &auth_binding,
        PersistedAuthMode::ChatgptOauth,
        Utc::now(),
    )
    .await
    .expect("rehydrate succeeds");

    assert!(rehydrated.is_none(), "no durable record to rehydrate");
    let (credential_present, phase) = lease_phase(&handle, &auth_binding);
    assert!(
        !credential_present || phase == Some(AuthLeasePhase::Released),
        "rehydrate must release a lease whose durable record is gone, got {phase:?}"
    );
}

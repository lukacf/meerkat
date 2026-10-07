//! Actual lease/store serialization controls. No network or native admission.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]
use async_trait::async_trait;
use chrono::Utc;
use meerkat_auth_core::{EphemeralTokenStore, auth_store::InMemoryCoordinator};
use meerkat_core::auth::token_store::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore,
    TokenStoreError,
};
use meerkat_core::handles::{AuthLeasePhase, GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::{AuthBindingRef, AuthCredentialIdentity, BindingId, BindingOrigin, RealmId};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;

fn binding(name: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::global(),
        binding: BindingId::parse(name).expect("binding"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}
fn handle() -> GeneratedAuthLeaseHandle {
    meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
        Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new()),
    )
    .expect("actual generated handle")
}
struct Store {
    inner: EphemeralTokenStore,
    loads: AtomicUsize,
    block_clear: AtomicBool,
    fail_clear: AtomicBool,
    entered: tokio::sync::Semaphore,
    finish: tokio::sync::Semaphore,
}
impl Store {
    fn new() -> Self {
        Self {
            inner: EphemeralTokenStore::new(),
            loads: AtomicUsize::new(0),
            block_clear: AtomicBool::new(false),
            fail_clear: AtomicBool::new(false),
            entered: tokio::sync::Semaphore::new(0),
            finish: tokio::sync::Semaphore::new(0),
        }
    }
}
#[async_trait]
impl TokenStore for Store {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        self.inner.load(key).await
    }
    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        self.inner.save(key, tokens).await
    }
    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        if self.block_clear.load(Ordering::SeqCst) {
            self.entered.add_permits(1);
            self.finish.acquire().await.expect("finish permit").forget();
        }
        if self.fail_clear.load(Ordering::SeqCst) {
            return Err(TokenStoreError::Unavailable("fixture clear failure".into()));
        }
        self.inner.clear(key).await
    }
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        self.inner.list().await
    }
    fn backend_name(&self) -> &'static str {
        "lifecycle-serialization-fixture"
    }
}
async fn seed(
    store: &Store,
    handle: &GeneratedAuthLeaseHandle,
    binding: &AuthBindingRef,
) -> PersistedTokens {
    let tokens = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("synthetic-fixture".into()),
        refresh_token: None,
        id_token: None,
        expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        last_refresh: None,
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let transition = meerkat_core::publish_token_lifecycle_acquired(handle, binding, &tokens)
        .expect("actual acquisition");
    let key = TokenKey::from_auth_binding(binding);
    let marked =
        meerkat_core::mark_tokens_lifecycle_published_for_transition(&key, &tokens, &transition)
            .expect("actual marker");
    store.save(&key, &marked).await.expect("seed");
    marked
}

#[tokio::test]
async fn status_waits_for_exact_lease_before_any_store_load() {
    let binding = binding("status_guard");
    let store = Store::new();
    let handle = handle();
    let marked = seed(&store, &handle, &binding).await;
    let lease = LeaseKey::from_auth_binding(&binding);
    let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
    let before = store.loads.load(Ordering::SeqCst);
    let mut status = Box::pin(meerkat_core::rehydrate_marked_tokens_for_status(
        &store,
        &handle,
        &binding,
        PersistedAuthMode::ChatgptOauth,
        Utc::now(),
    ));
    let first = futures::poll!(status.as_mut());
    assert_eq!(
        store.loads.load(Ordering::SeqCst),
        before,
        "a blocked status may not load old durable bytes"
    );
    assert!(
        first.is_pending(),
        "same-lease status must wait for the actual owner"
    );
    drop(guard);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), status)
            .await
            .expect("guard released")
            .expect("status"),
        Some(marked)
    );
}

async fn cancelled_clear(fail: bool, name: &str) {
    let binding = binding(name);
    let store = Arc::new(Store::new());
    let handle = handle();
    let marked = seed(store.as_ref(), &handle, &binding).await;
    store.block_clear.store(true, Ordering::SeqCst);
    store.fail_clear.store(fail, Ordering::SeqCst);
    let persistence =
        ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new()));
    let clear_handle = handle.clone();
    let identity = AuthCredentialIdentity::from_auth_binding(&binding);
    let caller = tokio::spawn(async move {
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
            persistence,
            clear_handle,
            identity,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), store.entered.acquire())
        .await
        .expect("actual clear entered")
        .expect("entered")
        .forget();
    let lease = LeaseKey::from_auth_binding(&binding);
    let released = handle.snapshot(&lease);
    assert!(!released.credential_present);
    assert!(matches!(
        released.phase,
        None | Some(AuthLeasePhase::Released)
    ));
    caller.abort();
    assert!(caller.await.expect_err("caller cancelled").is_cancelled());
    let before = store.loads.load(Ordering::SeqCst);
    let mut status = Box::pin(meerkat_core::rehydrate_marked_tokens_for_status(
        store.as_ref(),
        &handle,
        &binding,
        PersistedAuthMode::ChatgptOauth,
        Utc::now(),
    ));
    let first = futures::poll!(status.as_mut());
    // Release even if the RED assertion fails, so the actual owned task is not
    // left pending inside the test runtime.
    store.finish.add_permits(1);
    assert_eq!(
        store.loads.load(Ordering::SeqCst),
        before,
        "status must not reload the old marker during owned clear"
    );
    assert!(
        first.is_pending(),
        "caller cancellation must not surrender the lease"
    );
    let result = tokio::time::timeout(Duration::from_secs(2), status)
        .await
        .expect("clear or rollback releases lease")
        .expect("status after clear");
    if fail {
        assert_eq!(result, Some(marked));
        assert_eq!(handle.snapshot(&lease).phase, Some(AuthLeasePhase::Valid));
    } else {
        assert!(result.is_none());
        let final_state = handle.snapshot(&lease);
        assert!(
            !final_state.credential_present || final_state.phase == Some(AuthLeasePhase::Released)
        );
    }
}
#[tokio::test]
async fn status_waits_through_cancelled_clear_commit() {
    cancelled_clear(false, "cancelled_clear_commit").await;
}
#[tokio::test]
async fn status_waits_through_cancelled_clear_rollback() {
    cancelled_clear(true, "cancelled_clear_rollback").await;
}

#[tokio::test]
async fn explicit_guard_rejects_other_identity_before_load_and_accepts_same_identity() {
    let selected = binding("exact_guard_selected");
    let other = binding("exact_guard_other");
    let store = Store::new();
    let handle = handle();
    let marked = seed(&store, &handle, &selected).await;
    let selected_lease = LeaseKey::from_auth_binding(&selected);
    let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&selected_lease).await;
    assert_eq!(guard.lease_key(), &selected_lease);
    let before = store.loads.load(Ordering::SeqCst);
    let snapshot = handle.snapshot(&selected_lease);
    assert!(matches!(
        meerkat_core::rehydrate_marked_tokens_for_status_with_guard(
            &store,
            &handle,
            &other,
            PersistedAuthMode::ChatgptOauth,
            Utc::now(),
            &guard
        )
        .await,
        Err(meerkat_core::AuthStatusRehydrateError::LeaseGuardMismatch)
    ));
    assert!(matches!(
        meerkat_core::rehydrate_durable_predecessor_for_mutation(
            &store,
            &handle,
            &other,
            Utc::now(),
            &guard
        )
        .await,
        Err(meerkat_core::AuthStatusRehydrateError::LeaseGuardMismatch)
    ));
    assert_eq!(store.loads.load(Ordering::SeqCst), before);
    assert_eq!(handle.snapshot(&selected_lease), snapshot);
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(2),
            meerkat_core::rehydrate_marked_tokens_for_status_with_guard(
                &store,
                &handle,
                &selected,
                PersistedAuthMode::ChatgptOauth,
                Utc::now(),
                &guard
            )
        )
        .await
        .expect("already-held helper does not relock")
        .expect("matching guard"),
        Some(marked)
    );
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            meerkat_core::rehydrate_marked_tokens_for_status(
                &store,
                &handle,
                &other,
                PersistedAuthMode::ChatgptOauth,
                Utc::now()
            )
        )
        .await
        .expect("unrelated lease is not serialized")
        .expect("other status")
        .is_none()
    );
}

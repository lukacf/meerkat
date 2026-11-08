//! The credential fence C(r) at the credential coordinator and the token
//! store.
//!
//! An admission holder holds C(r) SHARED. While it does, a refresh makes no
//! token request, a credential commit or clear refuses with nothing
//! written, and a logout clear refuses; each lands once the holder releases.
//! A token-store write without a guard covering C(r) is refused with nothing
//! written. Refreshes that coalesce take the fence once. An existing
//! attachment identity key is read without the fence; only a missing one is
//! created, through the fenced coordinator.

#![cfg(all(not(target_arch = "wasm32"), feature = "file-lock"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use meerkat_auth_core::auth_store::{
    CredentialMutationError, CredentialMutationOutcome, FileLockCoordinator, FileTokenStore,
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, RefreshError, TokenKey,
    TokenStore, TokenStoreAccess, TokenStoreError,
};
use meerkat_core::handles::{AuthLeasePhase, GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_core::{
    AdmissionFence, AdmissionGuard, AdmissionHold, AuthBindingRef, RealmId,
    auth::{load_mcp_attachment_identity_key, resolve_or_create_mcp_attachment_identity_key},
    clear_tokens_and_publish_lifecycle_released_coordinated,
    mark_tokens_lifecycle_published_for_transition, publish_token_lifecycle_acquired,
};

const BOUND: Duration = Duration::from_secs(30);

fn realm() -> RealmId {
    RealmId::parse("dev").unwrap()
}

fn key() -> TokenKey {
    TokenKey::parse("dev", "default_openai").unwrap()
}

fn tokens(secret: &str) -> PersistedTokens {
    PersistedTokens::api_key(secret)
}

/// A file store under `root` and its fenced coordinator, as a keychain-capable
/// backend opens them (retained custody): unattended credential work never
/// waits for a held fence, and an interactive mutation waits at most 5 s.
fn persistence(root: &Path) -> ProviderAuthPersistence {
    ProviderAuthPersistence::new(
        Arc::new(FileTokenStore::new(root)),
        Arc::new(FileLockCoordinator::retaining_custody(
            root.join("refresh-locks"),
        )),
    )
}

/// C(`realm`) under `root`, the fence of every credential stored there.
fn fence(root: &Path, realm: &RealmId) -> AdmissionFence {
    AdmissionFence::credentials(root, realm).unwrap()
}

/// An admission holder's hold of C(`realm`): SHARED, so every fenced
/// credential write of the realm is excluded until it is released.
fn hold(root: &Path, realm: &RealmId) -> AdmissionHold {
    let fence = fence(root, realm);
    fence.ensure_exists().unwrap();
    AdmissionHold::try_acquire(vec![fence]).unwrap()
}

/// One unattended credential mutation of `key()` that saves `tokens`, or
/// clears the credential when `tokens` is `None`, counting its runs.
async fn mutate(
    persistence: &ProviderAuthPersistence,
    runs: &Arc<AtomicUsize>,
    tokens: Option<PersistedTokens>,
) -> Result<CredentialMutationOutcome, CredentialMutationError> {
    let store = persistence.token_store();
    let runs = Arc::clone(runs);
    persistence
        .refresh_coordinator()
        .with_exclusive_mutation_for(
            key(),
            TokenStoreAccess::Unattended,
            Box::new(move |guard| {
                Box::pin(async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    match tokens {
                        Some(tokens) => {
                            store
                                .save(&guard, &key(), &tokens)
                                .await
                                .map_err(CredentialMutationError::from_token_store)?;
                            Ok(CredentialMutationOutcome::Persisted(tokens))
                        }
                        None => {
                            store
                                .clear(&guard, &key())
                                .await
                                .map_err(CredentialMutationError::from_token_store)?;
                            Ok(CredentialMutationOutcome::Cleared)
                        }
                    }
                })
            }),
        )
        .await
}

/// A refresh of `key()` through the coordinator. The closure is where a
/// refresh sends its token request: `token_requests` counts them.
async fn refresh(
    persistence: &ProviderAuthPersistence,
    token_requests: &Arc<AtomicUsize>,
) -> Result<PersistedTokens, RefreshError> {
    let requests = Arc::clone(token_requests);
    persistence
        .refresh_coordinator()
        .with_refresh(
            key(),
            Box::new(move |_guard| {
                Box::pin(async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    Ok(tokens("refreshed"))
                })
            }),
        )
        .await
}

#[tokio::test]
async fn a_refresh_under_a_held_fence_makes_no_token_request() {
    let root = tempfile::tempdir().unwrap();
    let persistence = persistence(root.path());
    let token_requests = Arc::new(AtomicUsize::new(0));

    let held = hold(root.path(), &realm());
    let refused = refresh(&persistence, &token_requests).await;
    assert!(
        matches!(refused, Err(RefreshError::AdmissionInProgress)),
        "{refused:?}"
    );
    assert_eq!(
        token_requests.load(Ordering::SeqCst),
        0,
        "a refused refresh sends no token request"
    );

    drop(held);
    let refreshed = refresh(&persistence, &token_requests).await;
    assert_eq!(refreshed.unwrap(), tokens("refreshed"));
    assert_eq!(token_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn credential_commits_and_clears_refuse_under_a_held_fence_with_nothing_written() {
    let root = tempfile::tempdir().unwrap();
    let persistence = persistence(root.path());
    let store = persistence.token_store();
    let runs = Arc::new(AtomicUsize::new(0));
    mutate(&persistence, &runs, Some(tokens("before")))
        .await
        .expect("with no holder the commit lands");

    let held = hold(root.path(), &realm());
    let committed = mutate(&persistence, &runs, Some(tokens("during"))).await;
    assert!(
        matches!(committed, Err(CredentialMutationError::AdmissionInProgress)),
        "{committed:?}"
    );
    let cleared = mutate(&persistence, &runs, None).await;
    assert!(
        matches!(cleared, Err(CredentialMutationError::AdmissionInProgress)),
        "{cleared:?}"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "neither refused mutation ran"
    );
    assert_eq!(
        store.load(&key()).await.unwrap(),
        Some(tokens("before")),
        "nothing was written"
    );

    drop(held);
    mutate(&persistence, &runs, Some(tokens("after")))
        .await
        .expect("the commit lands once the holder released");
    assert_eq!(store.load(&key()).await.unwrap(), Some(tokens("after")));
    mutate(&persistence, &runs, None)
        .await
        .expect("the clear lands once the holder released");
    assert_eq!(store.load(&key()).await.unwrap(), None);
}

#[tokio::test]
async fn coalesced_refreshes_take_the_fence_once() {
    let root = tempfile::tempdir().unwrap();
    // A caller-runtime coordinator: a refresh that finds one in flight for
    // its key awaits that refresh's result.
    let persistence = ProviderAuthPersistence::new(
        Arc::new(FileTokenStore::new(root.path())),
        Arc::new(FileLockCoordinator::new(root.path().join("refresh-locks"))),
    );
    let token_requests = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());

    // The leader's refresh holds C(r) until released; the others join it.
    let leader = {
        let persistence = persistence.clone();
        let requests = Arc::clone(&token_requests);
        let (entered, release) = (Arc::clone(&entered), Arc::clone(&release));
        let root = root.path().to_path_buf();
        tokio::spawn(async move {
            persistence
                .refresh_coordinator()
                .with_refresh(
                    key(),
                    Box::new(move |guard| {
                        Box::pin(async move {
                            requests.fetch_add(1, Ordering::SeqCst);
                            assert_eq!(
                                guard.covers(&fence(&root, &realm())),
                                Ok(()),
                                "the refresh runs under C(r)"
                            );
                            entered.notify_one();
                            release.notified().await;
                            Ok(tokens("refreshed"))
                        })
                    }),
                )
                .await
        })
    };
    tokio::time::timeout(BOUND, entered.notified())
        .await
        .expect("the leader refreshes under the fence");
    // Joined while the leader holds the fence: no second acquisition, so
    // none of them is refused, and none sends a token request of its own.
    let followers: Vec<_> = (0..4)
        .map(|_| {
            let persistence = persistence.clone();
            let requests = Arc::clone(&token_requests);
            tokio::spawn(async move { refresh(&persistence, &requests).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    release.notify_one();

    assert_eq!(leader.await.unwrap().unwrap(), tokens("refreshed"));
    for follower in followers {
        assert_eq!(follower.await.unwrap().unwrap(), tokens("refreshed"));
    }
    assert_eq!(
        token_requests.load(Ordering::SeqCst),
        1,
        "one refresh ran, under one acquisition of the fence"
    );
}

#[tokio::test]
async fn a_token_store_write_without_a_covering_guard_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let store = FileTokenStore::new(root.path());

    let unfenced = store
        .save(&AdmissionGuard::none(), &key(), &tokens("unfenced"))
        .await;
    assert!(
        matches!(unfenced, Err(TokenStoreError::AdmissionFence(_))),
        "{unfenced:?}"
    );
    let other = AdmissionGuard::acquire(
        vec![fence(root.path(), &RealmId::parse("other").unwrap())],
        BOUND,
    )
    .await
    .unwrap();
    let foreign = store.save(&other, &key(), &tokens("foreign")).await;
    assert!(
        matches!(foreign, Err(TokenStoreError::AdmissionFence(_))),
        "{foreign:?}"
    );
    let foreign_for = store
        .save_for(
            &other,
            &key(),
            &tokens("foreign"),
            TokenStoreAccess::Unattended,
        )
        .await;
    assert!(
        matches!(foreign_for, Err(TokenStoreError::AdmissionFence(_))),
        "{foreign_for:?}"
    );
    assert_eq!(
        store.load(&key()).await.unwrap(),
        None,
        "nothing was written"
    );

    let own = AdmissionGuard::acquire(vec![fence(root.path(), &realm())], BOUND)
        .await
        .unwrap();
    store.save(&own, &key(), &tokens("fenced")).await.unwrap();
    let refused_clear = store.clear(&other, &key()).await;
    assert!(
        matches!(refused_clear, Err(TokenStoreError::AdmissionFence(_))),
        "{refused_clear:?}"
    );
    assert_eq!(
        store.load(&key()).await.unwrap(),
        Some(tokens("fenced")),
        "a refused clear removes nothing"
    );
    store.clear(&own, &key()).await.unwrap();
    assert_eq!(store.load(&key()).await.unwrap(), None);
}

#[tokio::test]
async fn an_existing_attachment_key_is_read_without_a_fence() {
    let root = tempfile::tempdir().unwrap();
    let persistence = persistence(root.path());
    let created = resolve_or_create_mcp_attachment_identity_key(&persistence, &realm())
        .await
        .expect("with no holder a missing key is created");

    let held = hold(root.path(), &realm());
    let read = tokio::time::timeout(
        BOUND,
        resolve_or_create_mcp_attachment_identity_key(&persistence, &realm()),
    )
    .await
    .expect("an existing key is read at once")
    .expect("an existing key is read without the fence");
    assert_eq!(read, created);
    drop(held);

    // A missing key is created only through the fenced coordinator.
    let fresh = RealmId::parse("fresh").unwrap();
    let held = hold(root.path(), &fresh);
    assert!(
        resolve_or_create_mcp_attachment_identity_key(&persistence, &fresh)
            .await
            .is_err(),
        "a missing key is not created under a held fence"
    );
    assert_eq!(
        load_mcp_attachment_identity_key(persistence.token_store().as_ref(), &fresh)
            .await
            .unwrap(),
        None
    );
    drop(held);
    resolve_or_create_mcp_attachment_identity_key(&persistence, &fresh)
        .await
        .expect("created once the holder released");
}

fn test_auth_lease() -> GeneratedAuthLeaseHandle {
    let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
    meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(handle)
        .expect("test auth lease must be certified by generated AuthMachine authority")
}

fn binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: realm(),
        binding: meerkat_core::BindingId::parse("default_openai").unwrap(),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
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

#[tokio::test]
async fn logout_clear_under_a_held_fence_refuses_then_lands_after_release() {
    let root = tempfile::tempdir().unwrap();
    let persistence = persistence(root.path());
    let store = persistence.token_store();
    let handle = test_auth_lease();
    let auth_binding = binding();

    // A login's publish: acquire the lease, stamp the marker, save under C(r).
    let mut login = tokens("access");
    login.auth_mode = PersistedAuthMode::ChatgptOauth;
    login.refresh_token = Some("refresh".into());
    login.expires_at = Some(Utc::now() + chrono::Duration::hours(1));
    let transition =
        publish_token_lifecycle_acquired(&handle, &auth_binding, &login).expect("acquire lease");
    let marked = mark_tokens_lifecycle_published_for_transition(&key(), &login, &transition)
        .expect("mark lifecycle published");
    let guard = AdmissionGuard::acquire(vec![fence(root.path(), &realm())], BOUND)
        .await
        .unwrap();
    store.save(&guard, &key(), &marked).await.unwrap();
    drop(guard);

    let held = hold(root.path(), &realm());
    let refused = clear_tokens_and_publish_lifecycle_released_coordinated(
        persistence.clone(),
        handle.clone(),
        auth_binding.clone(),
    )
    .await;
    assert!(
        matches!(refused, Err(CredentialMutationError::AdmissionInProgress)),
        "{refused:?}"
    );
    assert!(
        store.load(&key()).await.unwrap().is_some(),
        "the refused clear removed nothing"
    );
    let (present, phase) = lease_phase(&handle, &auth_binding);
    assert!(
        present && phase != Some(AuthLeasePhase::Released),
        "the lease stays live: {phase:?}"
    );

    drop(held);
    clear_tokens_and_publish_lifecycle_released_coordinated(
        persistence.clone(),
        handle.clone(),
        auth_binding.clone(),
    )
    .await
    .expect("the clear lands once the holder released");
    assert!(store.load(&key()).await.unwrap().is_none());
    let (present, phase) = lease_phase(&handle, &auth_binding);
    assert!(
        !present || phase == Some(AuthLeasePhase::Released),
        "the lease is released: {phase:?}"
    );
}

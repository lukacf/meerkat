//! Auto-detecting token store with a single authoritative backend per key.
//!
//! Row #278 closure: the keyring and file backends are NOT interchangeable
//! credential authorities. For any given [`TokenKey`] there is exactly one
//! authoritative backend, resolved as a recorded fact by where the key
//! currently lives:
//!
//!   - if the key already exists in the keyring (and keyring is available),
//!     the keyring is authoritative;
//!   - otherwise if the key exists in the file store, the file store is
//!     authoritative;
//!   - for a brand-new key, the authoritative backend is keyring-preferred
//!     when keyring is available, else the file store.
//!
//! A realm's MCP attachment identity key is the one exception: its load,
//! save and clear go to the file store only and never contact the keyring
//! (no lookup, no cross-backend eviction), so keyring availability can never
//! make an existing key look absent, and a key an explicitly selected
//! keyring store holds is never touched.
//!
//! `load`/`save`/`clear` all target the one resolved backend, resolved with a
//! single keychain read per operation, and a keyring `save` additionally
//! evicts any stale file copy so a single key can never leave divergent
//! keyring+file copies. That eviction runs right after the keychain write,
//! under the same owner, even if the caller has gone away.
//!
//! Only a keychain the platform positively reports absent
//! ([`TokenStoreError::KeyringUnavailable`]) hands a key to the file store. A
//! denial, a refused interaction, a locked, read-only or otherwise
//! inaccessible keychain, a pending or refused admission and any other
//! keychain failure keep their original error and their backend: no file
//! write or delete happens in their place.
//!
//! Every write is fenced by one fence whichever backend owns the key: the
//! credential fence C(r) of its realm under this store's root. A guard that
//! does not cover it is refused before any keychain call or file write
//! ([`TokenStoreError::AdmissionFence`]).
//!
//! A status observation never reads the keychain, and a file copy alone never
//! proves the file store owns a key. A file-only key answers from its file;
//! so does any key once a call proved this host has no usable keychain.
//! Otherwise status reports what the keychain owner observed, and never
//! claims absence it has not proven. `list()` stays a discovery/migration
//! read that unions both backends.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use meerkat_core::AdmissionGuard;
use meerkat_core::auth::token_store::{TokenStoreAccess, TokenStoreObservation, TokenStoreRefusal};

use super::file::FileTokenStore;
#[cfg(feature = "keyring")]
use super::keyring::{KeychainStatus, KeyringTokenStore};
use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

pub struct AutoTokenStore {
    file: FileTokenStore,
    #[cfg(feature = "keyring")]
    keyring: KeyringTokenStore,
    #[cfg(not(feature = "keyring"))]
    _service_name: String,
}

impl AutoTokenStore {
    #[cfg(feature = "keyring")]
    pub fn new(root: impl Into<PathBuf>, service_name: impl Into<String>) -> Self {
        Self {
            file: FileTokenStore::new(root),
            keyring: KeyringTokenStore::new(service_name),
        }
    }

    #[cfg(not(feature = "keyring"))]
    pub fn new(root: impl Into<PathBuf>, service_name: impl Into<String>) -> Self {
        Self {
            file: FileTokenStore::new(root),
            _service_name: service_name.into(),
        }
    }

    /// An Auto store over a given keyring store (tests block its keychain).
    #[cfg(all(test, feature = "keyring"))]
    pub(crate) fn with_keyring(root: impl Into<PathBuf>, keyring: KeyringTokenStore) -> Self {
        Self {
            file: FileTokenStore::new(root),
            keyring,
        }
    }

    pub fn root(&self) -> &Path {
        self.file.root()
    }

    /// Whether the keychain holds `key`, from one admitted keychain read.
    /// `None` means this host has no usable keychain; every other keychain
    /// error is returned unchanged.
    #[cfg(feature = "keyring")]
    async fn keyring_holds(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<Option<Option<PersistedTokens>>, TokenStoreError> {
        match self.keyring.get(key, access).await {
            Ok(found) => Ok(Some(found)),
            Err(TokenStoreError::KeyringUnavailable(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

fn refused_status_write(access: TokenStoreAccess) -> TokenStoreError {
    TokenStoreError::refused(
        "auto",
        access,
        TokenStoreRefusal::Unsupported,
        "a status access never writes or deletes",
    )
}

/// Whether `key` lives in the file store only, never contacting the keyring:
/// a realm's MCP attachment identity key must stay one stable key, and its
/// load-or-create cannot tell a missing key from a briefly unavailable
/// keyring.
fn file_only(key: &TokenKey) -> bool {
    key.is_mcp_attachment_identity_key()
}

#[async_trait]
impl TokenStore for AutoTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        self.load_for(key, TokenStoreAccess::Unattended).await
    }

    async fn save(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.save_for(guard, key, tokens, TokenStoreAccess::Unattended)
            .await
    }

    async fn clear(&self, guard: &AdmissionGuard, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.clear_for(guard, key, TokenStoreAccess::Unattended)
            .await
    }

    async fn load_for(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        if file_only(key) {
            return self.file.load(key).await;
        }
        if access == TokenStoreAccess::Status {
            return match self.observe_status(key).await? {
                TokenStoreObservation::Present(tokens) => Ok(Some(*tokens)),
                TokenStoreObservation::Missing => Ok(None),
                observation => Err(TokenStoreError::refused(
                    "auto",
                    access,
                    observation
                        .refusal()
                        .unwrap_or(TokenStoreRefusal::NotObserved),
                    "the status observation did not answer",
                )),
            };
        }
        // One keychain read decides: a keyring-resident key is authoritative
        // there; otherwise the file store answers.
        #[cfg(feature = "keyring")]
        if let Some(Some(tokens)) = self.keyring_holds(key, access).await? {
            return Ok(Some(tokens));
        }
        self.file.load(key).await
    }

    async fn save_for(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
        access: TokenStoreAccess,
    ) -> Result<(), TokenStoreError> {
        if access == TokenStoreAccess::Status {
            return Err(refused_status_write(access));
        }
        // One fence covers the key in either backend: C of its realm under
        // this store's root. Refused before any keychain call or file write.
        self.file.require_fence(guard, key)?;
        if file_only(key) {
            return self.file.write(key, tokens).await;
        }
        #[cfg(feature = "keyring")]
        match self.keyring_holds(key, access).await? {
            // The keychain holds the key, or the key is brand new: the
            // keychain owns it. The stale file copy is evicted right after
            // the keychain write, under the same owner.
            Some(Some(_)) => {
                return self
                    .keyring
                    .save_then(
                        key,
                        tokens,
                        access,
                        Some(Box::new(self.file.evict_blocking(key))),
                    )
                    .await;
            }
            Some(None) if self.file.load(key).await?.is_none() => {
                return self
                    .keyring
                    .save_then(
                        key,
                        tokens,
                        access,
                        Some(Box::new(self.file.evict_blocking(key))),
                    )
                    .await;
            }
            // The file store owns the key; the keychain holds no copy to evict.
            Some(None) => {}
            // No usable keychain on this host: the file store owns every key.
            None => {}
        }
        self.file.write(key, tokens).await
    }

    async fn clear_for(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<(), TokenStoreError> {
        if access == TokenStoreAccess::Status {
            return Err(refused_status_write(access));
        }
        self.file.require_fence(guard, key)?;
        if file_only(key) {
            return self.file.remove(key).await;
        }
        // Clear from BOTH backends so no cached copy lingers regardless of
        // which backend currently owns the key. The file copy is removed after
        // the keychain delete under the same owner; a keychain refusal deletes
        // nothing.
        #[cfg(feature = "keyring")]
        match self
            .keyring
            .clear_then(key, access, Some(Box::new(self.file.evict_blocking(key))))
            .await
        {
            Ok(()) => return Ok(()),
            Err(TokenStoreError::KeyringUnavailable(_)) => {}
            Err(error) => return Err(error),
        }
        self.file.remove(key).await
    }

    /// Status never reads the keychain. A file-only key answers from its
    /// file; otherwise the keychain owner's observation stands, and the file
    /// answers only when the keychain was proven absent on this host.
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        if file_only(key) {
            return self.file.observe_status(key).await;
        }
        // A file copy does not prove the file store owns a key the keychain
        // may hold (a keychain write can succeed while evicting the stale
        // file copy fails). The file answers only when the latest call proved
        // this host has no usable keychain, no call is in flight and no
        // denial is remembered; otherwise the keychain owner's observation
        // stands.
        #[cfg(feature = "keyring")]
        {
            match self.keyring.status(key) {
                KeychainStatus::BackendAbsent => self.file.observe_status(key).await,
                KeychainStatus::Observed(observation) => Ok(observation),
            }
        }
        #[cfg(not(feature = "keyring"))]
        self.file.observe_status(key).await
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        // Discovery/migration read: union both backends so a key resident in
        // either is enumerable. Authoritative per-key ownership is resolved at
        // load/save/clear time, not here.
        let mut combined = Vec::new();
        #[cfg(feature = "keyring")]
        {
            match self.keyring.list().await {
                Ok(mut v) => combined.append(&mut v),
                Err(TokenStoreError::KeyringUnavailable(_) | TokenStoreError::Unavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        let mut file_keys = self.file.list().await?;
        combined.append(&mut file_keys);
        combined.sort();
        combined.dedup();
        Ok(combined)
    }

    fn backend_name(&self) -> &'static str {
        "auto"
    }
}

// File-only tests: the AutoTokenStore discovery (`list()`) invariant these
// exercise is a file-backend property. With the `keyring` feature on a host
// with an available keychain, `resolve_backend` routes a fresh key to the
// keyring, which is loaded by exact key but is NOT enumerable — so a
// `list()`-based discovery count is not a meaningful assertion there. The
// no-divergent-copies invariant itself (each save evicts the other backend) is
// exercised by the per-backend save/clear paths regardless of feature.
#[cfg(all(test, not(feature = "keyring")))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn sample_tokens(secret: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: meerkat_core::auth::token_store::PersistedAuthMode::ApiKey,
            primary_secret: Some(secret.to_string()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// Row #278 gate: a save followed by a re-save targets the SAME recorded
    /// backend, and never leaves divergent keyring+file copies for one key.
    /// File-only behavior — the module is gated off keyring builds (see the
    /// module-level comment): a re-save updates the same file-resident key and
    /// `list()` stays singular.
    #[tokio::test]
    async fn save_resave_targets_single_backend_no_divergent_copies() {
        let dir = std::env::temp_dir().join(format!("rkat-auto-{}", uuid::Uuid::new_v4()));
        let store = AutoTokenStore::new(&dir, "rkat-test-auto");
        let key = TokenKey::parse("dev", "default_openai").unwrap();
        let guard = crate::auth_store::covering_guard(&dir, &key);

        store
            .save(&guard, &key, &sample_tokens("first"))
            .await
            .unwrap();
        let loaded = store.load(&key).await.unwrap().unwrap();
        assert_eq!(loaded.primary_secret.as_deref(), Some("first"));

        store
            .save(&guard, &key, &sample_tokens("second"))
            .await
            .unwrap();
        let reloaded = store.load(&key).await.unwrap().unwrap();
        assert_eq!(reloaded.primary_secret.as_deref(), Some("second"));

        // The key must appear exactly once in the discovery union — no
        // divergent per-backend copy was minted by the second save.
        let listed = store.list().await.unwrap();
        assert_eq!(
            listed.iter().filter(|k| **k == key).count(),
            1,
            "key must not have divergent copies across backends after re-save"
        );

        store.clear(&guard, &key).await.unwrap();
        assert!(store.load(&key).await.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(all(test, feature = "keyring"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth_store::keyring::KeychainFailure;
    use crate::auth_store::keyring::test_keychain::{FakeKeychain, store as keyring_store};
    use std::sync::Arc;
    use std::time::Duration;

    fn auto(root: &Path, keychain: &Arc<FakeKeychain>) -> AutoTokenStore {
        AutoTokenStore::with_keyring(root, keyring_store(keychain, 2))
    }

    /// A key no other test uses: the lifecycle owner registry is process
    /// wide, and these tests hold or retain its exclusion.
    fn credential_key(binding: &str) -> TokenKey {
        TokenKey::parse("keychain-auto", binding).unwrap()
    }

    fn tokens(secret: &str) -> PersistedTokens {
        PersistedTokens::api_key(secret)
    }

    fn file_path(root: &Path, key: &TokenKey) -> PathBuf {
        root.join(key.realm().as_str())
            .join(format!("{}.json", key.storage_stem()))
    }

    fn file_bytes(root: &Path, key: &TokenKey) -> Option<Vec<u8>> {
        std::fs::read(file_path(root, key)).ok()
    }

    /// T4: one keychain read per Auto load of a keyring-resident key.
    #[tokio::test]
    async fn an_auto_load_reads_the_keychain_once() {
        let temp = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::open();
        let key = credential_key("single_read");
        keychain.put(
            &key.keyring_account(),
            &serde_json::to_string(&tokens("k")).unwrap(),
        );
        let store = auto(temp.path(), &keychain);
        assert_eq!(
            store
                .load_for(&key, TokenStoreAccess::Unattended)
                .await
                .unwrap(),
            Some(tokens("k"))
        );
        assert_eq!(keychain.calls(), 1, "exactly one keychain read per load");
    }

    /// T5: a denial or a refused interaction on load, save or clear never
    /// falls back to the file store, and keeps the original error and the
    /// keyring backend.
    #[tokio::test]
    async fn a_denial_or_refused_interaction_never_falls_back_to_the_file() {
        for (failure, expected) in [
            (
                KeychainFailure::Denied("errSecUserCanceled".into()),
                TokenStoreRefusal::Denied,
            ),
            (
                KeychainFailure::InteractionNotAllowed("errSecInteractionNotAllowed".into()),
                TokenStoreRefusal::InteractionRequired,
            ),
            (
                KeychainFailure::Inaccessible("errSecReadOnly".into()),
                TokenStoreRefusal::Unavailable,
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let keychain = FakeKeychain::open();
            let store = auto(temp.path(), &keychain);
            let key = credential_key("no_fallback");
            let guard = crate::auth_store::covering_guard(temp.path(), &key);
            store
                .file
                .save(&guard, &key, &tokens("file-copy"))
                .await
                .unwrap();
            let before = file_bytes(temp.path(), &key);
            let original = match &failure {
                KeychainFailure::Denied(detail)
                | KeychainFailure::InteractionNotAllowed(detail)
                | KeychainFailure::Inaccessible(detail) => detail.clone(),
                other => panic!("unexpected failure {other:?}"),
            };
            for op in ["load", "save", "clear"] {
                keychain.fail_next(failure.clone());
                let result = match op {
                    "load" => store
                        .load_for(&key, TokenStoreAccess::Interactive)
                        .await
                        .map(|_| ()),
                    "save" => {
                        store
                            .save_for(&guard, &key, &tokens("new"), TokenStoreAccess::Interactive)
                            .await
                    }
                    _ => {
                        store
                            .clear_for(&guard, &key, TokenStoreAccess::Interactive)
                            .await
                    }
                };
                match result {
                    Err(TokenStoreError::AccessRefused {
                        backend,
                        refusal,
                        detail,
                        ..
                    }) => {
                        assert_eq!(backend, "keyring", "{op}: the backend is kept");
                        assert_eq!(refusal, expected, "{op}: the refusal is typed");
                        assert_eq!(detail, original, "{op}: the original error is kept");
                    }
                    other => panic!("{op}: expected a typed refusal, got {other:?}"),
                }
                assert_eq!(
                    file_bytes(temp.path(), &key),
                    before,
                    "{op}: zero file fallback writes or deletes"
                );
            }
        }
    }

    /// Status never reads the keychain: a file-owned key answers from its
    /// file, an unknown key is not observed, and absence is never claimed.
    #[tokio::test]
    async fn auto_status_answers_from_the_file_and_never_claims_absence() {
        let temp = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::open();
        let store = auto(temp.path(), &keychain);
        let key = credential_key("status_file_side");
        assert_eq!(
            store.observe_status(&key).await.unwrap(),
            TokenStoreObservation::NotObserved
        );
        // Fresh tokens in the keychain, a different stale copy in the file
        // (a keychain write whose file eviction failed): the file copy is not
        // ownership, so a cold status reports nothing it did not observe.
        keychain.put(
            &key.keyring_account(),
            &serde_json::to_string(&tokens("fresh")).unwrap(),
        );
        store
            .file
            .save(
                &crate::auth_store::covering_guard(temp.path(), &key),
                &key,
                &tokens("stale-file"),
            )
            .await
            .unwrap();
        assert_eq!(
            store.observe_status(&key).await.unwrap(),
            TokenStoreObservation::NotObserved,
            "a stale file copy is never reported as the credential"
        );
        assert_eq!(keychain.calls(), 0, "status made no keychain call");
        // After an actual denied read, status reports the denial despite the
        // file copy, still with no call of its own.
        keychain.fail_next(KeychainFailure::Denied("errSecUserCanceled".into()));
        assert!(
            store
                .load_for(&key, TokenStoreAccess::Unattended)
                .await
                .is_err()
        );
        assert_eq!(
            store.observe_status(&key).await.unwrap(),
            TokenStoreObservation::Denied
        );
        assert_eq!(keychain.calls(), 1, "status made no keychain call");
        // A file-only key keeps its positive file answer.
        let identity =
            TokenKey::mcp_attachment_identity_key(meerkat_core::RealmId::parse("status").unwrap());
        store
            .file
            .save(
                &crate::auth_store::covering_guard(temp.path(), &identity),
                &identity,
                &tokens("file-only"),
            )
            .await
            .unwrap();
        assert_eq!(
            store.observe_status(&identity).await.unwrap(),
            TokenStoreObservation::Present(Box::new(tokens("file-only")))
        );
        // Once the latest call proved this host has no usable keychain, the
        // file store owns the key and answers, even after an earlier failure.
        let owned = credential_key("status_no_keychain");
        store
            .file
            .save(
                &crate::auth_store::covering_guard(temp.path(), &owned),
                &owned,
                &tokens("file-owned"),
            )
            .await
            .unwrap();
        keychain.fail_next(KeychainFailure::Failed("platform failure".into()));
        assert!(
            store
                .load_for(&owned, TokenStoreAccess::Unattended)
                .await
                .is_err()
        );
        assert_eq!(
            store.observe_status(&owned).await.unwrap(),
            TokenStoreObservation::Unavailable
        );
        keychain.fail_next(KeychainFailure::NoStorage("no logon session".into()));
        assert_eq!(
            store
                .load_for(&owned, TokenStoreAccess::Unattended)
                .await
                .unwrap(),
            Some(tokens("file-owned"))
        );
        assert_eq!(
            store.observe_status(&owned).await.unwrap(),
            TokenStoreObservation::Present(Box::new(tokens("file-owned"))),
            "an earlier failure never hides newer proof of no keychain"
        );
        assert_eq!(keychain.calls(), 3, "status made no keychain call");
    }

    /// A save or clear whose guard does not cover the credential fence of
    /// the key's realm under the store's root is refused before any keychain
    /// call or file write, whichever backend would own the key.
    #[tokio::test]
    async fn an_auto_write_without_a_covering_guard_is_refused_before_either_backend() {
        let temp = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::open();
        let store = auto(temp.path(), &keychain);
        let credential = credential_key("auto_fenced");
        let identity = TokenKey::mcp_attachment_identity_key(
            meerkat_core::RealmId::parse("auto-fenced").unwrap(),
        );
        for key in [&credential, &identity] {
            let saved = store
                .save_for(
                    &meerkat_core::AdmissionGuard::none(),
                    key,
                    &tokens("new"),
                    TokenStoreAccess::Interactive,
                )
                .await;
            assert!(
                matches!(saved, Err(TokenStoreError::AdmissionFence(_))),
                "{saved:?}"
            );
            let cleared = store
                .clear_for(
                    &meerkat_core::AdmissionGuard::none(),
                    key,
                    TokenStoreAccess::Interactive,
                )
                .await;
            assert!(
                matches!(cleared, Err(TokenStoreError::AdmissionFence(_))),
                "{cleared:?}"
            );
            assert_eq!(file_bytes(temp.path(), key), None, "no file write");
        }
        assert_eq!(keychain.calls(), 0, "no keychain call");
    }

    /// A status access never saves or clears, not even a file-only key whose
    /// operations otherwise go straight to the file store.
    #[tokio::test]
    async fn a_status_access_never_writes_or_deletes_a_file_only_key() {
        let temp = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::open();
        let store = auto(temp.path(), &keychain);
        let identity = TokenKey::mcp_attachment_identity_key(
            meerkat_core::RealmId::parse("status-write").unwrap(),
        );
        let guard = crate::auth_store::covering_guard(temp.path(), &identity);
        store
            .file
            .save(&guard, &identity, &tokens("file-only"))
            .await
            .unwrap();
        let before = file_bytes(temp.path(), &identity);
        let saved = store
            .save_for(
                &guard,
                &identity,
                &tokens("replaced"),
                TokenStoreAccess::Status,
            )
            .await;
        let cleared = store
            .clear_for(&guard, &identity, TokenStoreAccess::Status)
            .await;
        for (op, result) in [("save", saved), ("clear", cleared)] {
            assert!(
                matches!(
                    result,
                    Err(TokenStoreError::AccessRefused {
                        refusal: TokenStoreRefusal::Unsupported,
                        ..
                    })
                ),
                "{op}: got {result:?}"
            );
        }
        assert_eq!(
            file_bytes(temp.path(), &identity),
            before,
            "the file-only key is untouched"
        );
        assert_eq!(keychain.calls(), 0);
    }

    /// Windows can report an absent credential store, then a later call can
    /// be pending or refused. That current observation must beat file fallback.
    #[tokio::test]
    async fn auto_status_uses_current_keychain_observation_after_no_storage() {
        let mut actual = Vec::new();
        let mut expected = Vec::new();
        for (binding, failure, refusal, observation) in [
            (
                "status_absent_then_denied",
                KeychainFailure::Denied("current denial".into()),
                TokenStoreRefusal::Denied,
                TokenStoreObservation::Denied,
            ),
            (
                "status_absent_then_inaccessible",
                KeychainFailure::Inaccessible("current unavailability".into()),
                TokenStoreRefusal::Unavailable,
                TokenStoreObservation::Unavailable,
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let keychain = FakeKeychain::open();
            let store = Arc::new(auto(temp.path(), &keychain));
            let key = credential_key(binding);
            let guard = crate::auth_store::covering_guard(temp.path(), &key);
            store
                .file
                .save(&guard, &key, &tokens("file-owned"))
                .await
                .unwrap();
            keychain.fail_next(KeychainFailure::NoStorage("no logon session".into()));
            assert_eq!(
                store
                    .load_for(&key, TokenStoreAccess::Unattended)
                    .await
                    .unwrap(),
                Some(tokens("file-owned"))
            );
            assert_eq!(
                store.observe_status(&key).await.unwrap(),
                TokenStoreObservation::Present(Box::new(tokens("file-owned")))
            );
            assert_eq!(keychain.calls(), 1);

            keychain.hold();
            keychain.fail_next(failure);
            let loading = tokio::spawn({
                let store = Arc::clone(&store);
                let key = key.clone();
                async move { store.load_for(&key, TokenStoreAccess::Interactive).await }
            });
            keychain.reach_calls(2).await;
            let pending =
                tokio::time::timeout(Duration::from_secs(1), store.observe_status(&key)).await;
            // Release and join before any expected RED assertion so this test
            // never leaves its deliberately held call behind on failure.
            keychain.release();
            assert!(matches!(
                loading.await.unwrap(),
                Err(TokenStoreError::AccessRefused {
                    backend: "keyring",
                    refusal: current,
                    ..
                }) if current == refusal
            ));
            let present_file = store.observe_status(&key).await.unwrap();
            assert_eq!(
                store.file.load(&key).await.unwrap(),
                Some(tokens("file-owned"))
            );
            store.file.clear(&guard, &key).await.unwrap();
            let missing_file = store.observe_status(&key).await.unwrap();
            assert_eq!(keychain.calls(), 2, "status never starts another call");
            actual.push((
                binding,
                pending.expect("status answers at once").unwrap(),
                present_file,
                missing_file,
            ));
            expected.push((
                binding,
                TokenStoreObservation::Pending,
                observation.clone(),
                observation,
            ));
        }
        assert_eq!(
            actual, expected,
            "earlier absence must not hide current keychain state"
        );
    }

    /// Runtime drop with the host process left alive: dropping it is prompt,
    /// the actual lifecycle guard stays held while the keychain call runs,
    /// another mutation and the settlement get typed answers with no call,
    /// and the late completion runs the Auto cleanup under the same owner.
    #[test]
    fn dropping_the_runtime_keeps_the_owner_until_the_keychain_call_settles() {
        let temp = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::blocked();
        let key = credential_key("runtime_drop");
        keychain.put(
            &key.keyring_account(),
            &serde_json::to_string(&tokens("old")).unwrap(),
        );
        let store = Arc::new(auto(temp.path(), &keychain));
        let root = temp.path().to_path_buf();
        std::fs::create_dir_all(root.join(key.realm().as_str())).unwrap();
        std::fs::write(file_path(&root, &key), b"stale file copy").unwrap();
        let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(
            key.credential_identity().unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let clearing = Arc::clone(&store);
        let clearing_key = key.clone();
        let clearing_lease = lease_key.clone();
        let clearing_guard = crate::auth_store::covering_guard(&root, &key);
        runtime.spawn(async move {
            let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&clearing_lease).await;
            clearing
                .clear_for(
                    &clearing_guard,
                    &clearing_key,
                    TokenStoreAccess::Interactive,
                )
                .await
        });
        keychain.wait_for_calls(1);
        let started = std::time::Instant::now();
        drop(runtime);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "dropping the runtime does not wait for the keychain call"
        );
        // The host is alive: the actual guard is still held, nothing is
        // cleaned up yet, and another observer and mutation answer at once.
        assert!(
            meerkat_core::try_acquire_auth_login_lifecycle_guard(&lease_key).is_none(),
            "the lifecycle owner is still held"
        );
        assert!(
            file_path(&root, &key).exists(),
            "no cleanup before completion"
        );
        let other = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        other.block_on(async {
            // The dropped runtime dropped the first clear's caller and its
            // guard; the keychain call keeps its own owner.
            let guard = crate::auth_store::covering_guard(&root, &key);
            // Bounded, so a second call blocked behind the first fails, not hangs.
            let cleared = tokio::time::timeout(
                Duration::from_secs(2),
                store.clear_for(&guard, &key, TokenStoreAccess::Interactive),
            )
            .await
            .expect("another mutation answers at once");
            match cleared {
                Err(TokenStoreError::AccessRefused { refusal, .. }) => {
                    assert_eq!(refusal, TokenStoreRefusal::Pending);
                }
                other => panic!("expected a pending refusal, got {other:?}"),
            }
            match crate::auth_store::settle_in_flight_credential_mutations(Duration::from_millis(
                50,
            ))
            .await
            {
                crate::auth_store::CredentialSettlement::Unsettled { pending } => {
                    assert!(pending >= 1, "the running call is reported");
                }
                crate::auth_store::CredentialSettlement::Settled => {
                    panic!("a running keychain call is never reported as settled")
                }
            }
        });
        assert_eq!(keychain.calls(), 1, "no new keychain call while it runs");
        keychain.release();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while meerkat_core::try_acquire_auth_login_lifecycle_guard(&lease_key).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the owner is never released"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !file_path(&root, &key).exists(),
            "the Auto cleanup ran after the late completion"
        );
        assert_eq!(keychain.entry(&key.keyring_account()), None);
    }

    #[cfg(feature = "file-lock")]
    #[tokio::test]
    async fn the_mcp_attachment_identity_key_never_contacts_the_keyring() {
        let temp = tempfile::tempdir().unwrap();
        let keychain = FakeKeychain::open();
        let store = std::sync::Arc::new(auto(temp.path(), &keychain));
        let realm = meerkat_core::RealmId::parse("home").unwrap();
        // The pair a persisted Auto backend opens: its coordinator takes the
        // credential fences of the store's root.
        let persistence = meerkat_core::auth::ProviderAuthPersistence::new(
            std::sync::Arc::clone(&store) as std::sync::Arc<dyn TokenStore>,
            std::sync::Arc::new(super::super::FileLockCoordinator::retaining_custody(
                temp.path().join("refresh-locks"),
            )),
        );
        // The real helper creates the realm's key through an Auto store.
        let created =
            meerkat_core::auth::resolve_or_create_mcp_attachment_identity_key(&persistence, &realm)
                .await
                .unwrap();
        let key = TokenKey::mcp_attachment_identity_key(realm.clone());
        assert!(
            store.file.load(&key).await.unwrap().is_some(),
            "the key is in the file store"
        );
        let loaded = meerkat_core::auth::load_mcp_attachment_identity_key(store.as_ref(), &realm)
            .await
            .unwrap()
            .expect("the same key loads");
        assert_eq!(loaded.id(), created.id());
        let tokens = store.load(&key).await.unwrap().unwrap();
        let guard = crate::auth_store::covering_guard(temp.path(), &key);
        store.save(&guard, &key, &tokens).await.unwrap();
        store.clear(&guard, &key).await.unwrap();
        assert!(store.file.load(&key).await.unwrap().is_none());
        assert_eq!(
            keychain.calls(),
            0,
            "load, save and clear of the identity key never start a keychain call"
        );
    }
}

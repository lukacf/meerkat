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
//! `load`/`save`/`clear` all target the one resolved backend, and `save`
//! additionally evicts any stale copy from the non-authoritative backend so a
//! single key can never leave divergent keyring+file copies. `list()` stays a
//! discovery/migration read that unions both backends.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::file::FileTokenStore;
#[cfg(feature = "keyring")]
use super::keyring::KeyringTokenStore;
use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

/// The single authoritative backend for a given key. A recorded fact derived
/// from where the key currently lives, not an availability race resolved
/// independently per operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoBackend {
    #[cfg(feature = "keyring")]
    Keyring,
    File,
}

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

    pub fn root(&self) -> &Path {
        self.file.root()
    }

    /// Resolve the single authoritative backend for `key`. Keyring wins only
    /// when keyring is available AND the key already lives there; otherwise the
    /// file store owns the key when it already lives there; a brand-new key is
    /// assigned keyring-preferred (when available) else file. Keyring
    /// unavailability transparently demotes ownership to the file backend.
    #[cfg(feature = "keyring")]
    async fn resolve_backend(&self, key: &TokenKey) -> Result<AutoBackend, TokenStoreError> {
        match self.keyring.load(key).await {
            // The key already lives in the keyring: keyring is authoritative.
            Ok(Some(_)) => return Ok(AutoBackend::Keyring),
            // Keyring is reachable but holds no copy: defer to where the key
            // actually lives (file) or to the new-key default below.
            Ok(None) => {}
            // Keyring is unavailable on this host: the file backend owns the
            // key. This is the explicit file-fallback, not a silent race.
            Err(TokenStoreError::KeyringUnavailable(_)) => return Ok(AutoBackend::File),
            Err(e) => return Err(e),
        }
        if self.file.load(key).await?.is_some() {
            // Existing file-resident key: file is authoritative.
            return Ok(AutoBackend::File);
        }
        // Brand-new key: keyring-preferred since keyring is reachable.
        Ok(AutoBackend::Keyring)
    }

    #[cfg(not(feature = "keyring"))]
    async fn resolve_backend(&self, _key: &TokenKey) -> Result<AutoBackend, TokenStoreError> {
        Ok(AutoBackend::File)
    }
}

#[async_trait]
impl TokenStore for AutoTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        match self.resolve_backend(key).await? {
            #[cfg(feature = "keyring")]
            AutoBackend::Keyring => self.keyring.load(key).await,
            AutoBackend::File => self.file.load(key).await,
        }
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        match self.resolve_backend(key).await? {
            #[cfg(feature = "keyring")]
            AutoBackend::Keyring => {
                self.keyring.save(key, tokens).await?;
                // Evict any stale file copy so the key cannot keep divergent
                // copies across backends. A missing file entry is a no-op.
                self.file.clear(key).await?;
                Ok(())
            }
            AutoBackend::File => {
                self.file.save(key, tokens).await?;
                // Evict any stale keyring copy for the same reason. Keyring
                // unavailability here is benign — there is nothing to evict.
                #[cfg(feature = "keyring")]
                match self.keyring.clear(key).await {
                    Ok(()) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                    Err(e) => return Err(e),
                }
                Ok(())
            }
        }
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        // Clear from BOTH backends so no cached copy lingers regardless of
        // which backend currently owns the key.
        #[cfg(feature = "keyring")]
        {
            match self.keyring.clear(key).await {
                Ok(()) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.file.clear(key).await
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

#[cfg(test)]
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
    ///
    /// Without the keyring feature the only backend is the file store, so the
    /// invariant trivially holds; this test pins the file-only behavior (a
    /// second save updates the same file-resident key, list stays singular).
    #[tokio::test]
    async fn save_resave_targets_single_backend_no_divergent_copies() {
        let dir = std::env::temp_dir().join(format!("rkat-auto-{}", uuid::Uuid::new_v4()));
        let store = AutoTokenStore::new(&dir, "rkat-test-auto");
        let key = TokenKey::parse("dev", "default_openai").unwrap();

        store.save(&key, &sample_tokens("first")).await.unwrap();
        let loaded = store.load(&key).await.unwrap().unwrap();
        assert_eq!(loaded.primary_secret.as_deref(), Some("first"));

        store.save(&key, &sample_tokens("second")).await.unwrap();
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

        store.clear(&key).await.unwrap();
        assert!(store.load(&key).await.unwrap().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

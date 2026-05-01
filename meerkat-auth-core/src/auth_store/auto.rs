//! Auto-detecting token store: tries the OS keyring first (when the
//! `native-keyring` feature is enabled), falls back to the file store on
//! keyring failure, unavailability, or missing entry. Writes go to the
//! keyring when available; fall back to file when not. Reads try keyring
//! first, then file.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::file::FileTokenStore;
#[cfg(feature = "keyring")]
use super::keyring::KeyringTokenStore;
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

    pub fn root(&self) -> &Path {
        self.file.root()
    }

    async fn load_unlocked(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(Some(t)) => return Ok(Some(t)),
                Ok(None) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.file.load_unlocked(key).await
    }

    async fn save_unlocked(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        #[cfg(feature = "keyring")]
        {
            match self.keyring.save_unlocked(key, tokens) {
                Ok(()) => return Ok(()),
                Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.file.save_unlocked(key, tokens).await
    }

    async fn clear_unlocked(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        #[cfg(feature = "keyring")]
        {
            match self.keyring.clear_unlocked(key) {
                Ok(()) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.file.clear_unlocked(key).await
    }
}

#[async_trait]
impl TokenStore for AutoTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;
        self.load_unlocked(key).await
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;
        self.save_unlocked(key, tokens).await
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;
        self.clear_unlocked(key).await
    }

    async fn clear_if_current(
        &self,
        key: &TokenKey,
        expected: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(Some(keyring_tokens)) => {
                    let mut cleared = false;
                    if &keyring_tokens == expected {
                        self.keyring.clear_unlocked(key)?;
                        cleared = true;
                    }
                    if self.file.load_unlocked(key).await?.as_ref() == Some(expected) {
                        self.file.clear_unlocked(key).await?;
                        cleared = true;
                    }
                    return Ok(cleared);
                }
                Ok(None) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }

        if self.file.load_unlocked(key).await?.as_ref() != Some(expected) {
            return Ok(false);
        }
        self.file.clear_unlocked(key).await?;
        Ok(true)
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        let mut combined = Vec::new();
        #[cfg(feature = "keyring")]
        {
            match self.keyring.list().await {
                Ok(mut v) => combined.append(&mut v),
                Err(TokenStoreError::KeyringUnavailable(_)) => {}
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

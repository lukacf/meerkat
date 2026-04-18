//! Auto-detecting token store: tries the OS keyring first (when the
//! `native-keyring` feature is enabled), falls back to the file store on
//! keyring failure, unavailability, or missing entry. Writes go to the
//! keyring when available; fall back to file when not. Reads try keyring
//! first, then file.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::file::FileTokenStore;
#[cfg(feature = "native-keyring")]
use super::keyring::KeyringTokenStore;
use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

pub struct AutoTokenStore {
    file: FileTokenStore,
    #[cfg(feature = "native-keyring")]
    keyring: KeyringTokenStore,
    #[cfg(not(feature = "native-keyring"))]
    _service_name: String,
}

impl AutoTokenStore {
    #[cfg(feature = "native-keyring")]
    pub fn new(root: impl Into<PathBuf>, service_name: impl Into<String>) -> Self {
        Self {
            file: FileTokenStore::new(root),
            keyring: KeyringTokenStore::new(service_name),
        }
    }

    #[cfg(not(feature = "native-keyring"))]
    pub fn new(root: impl Into<PathBuf>, service_name: impl Into<String>) -> Self {
        Self {
            file: FileTokenStore::new(root),
            _service_name: service_name.into(),
        }
    }

    pub fn root(&self) -> &Path {
        self.file.root()
    }
}

#[async_trait]
impl TokenStore for AutoTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        #[cfg(feature = "native-keyring")]
        {
            match self.keyring.load(key).await {
                Ok(Some(t)) => return Ok(Some(t)),
                Ok(None) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.file.load(key).await
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        #[cfg(feature = "native-keyring")]
        {
            match self.keyring.save(key, tokens).await {
                Ok(()) => return Ok(()),
                Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.file.save(key, tokens).await
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        // Clear from BOTH backends so cached copies don't linger.
        #[cfg(feature = "native-keyring")]
        {
            match self.keyring.clear(key).await {
                Ok(()) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }
        self.file.clear(key).await
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        let mut combined = Vec::new();
        #[cfg(feature = "native-keyring")]
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

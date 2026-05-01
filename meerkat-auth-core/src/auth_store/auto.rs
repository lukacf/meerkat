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

    #[cfg(feature = "keyring")]
    async fn save_file_shadow_if_current_or_unreadable(
        &self,
        key: &TokenKey,
        expected: Option<&PersistedTokens>,
        replacement: &PersistedTokens,
    ) {
        let should_save = match self.file.load_unlocked(key).await {
            Ok(present) => present.as_ref() == expected,
            Err(TokenStoreError::Serde(_)) => true,
            Err(_) => false,
        };
        if should_save {
            let _ = self.file.save_unlocked(key, replacement).await;
        }
    }

    #[cfg(feature = "keyring")]
    async fn clear_file_shadow_if_current_or_unreadable(
        &self,
        key: &TokenKey,
        expected: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        let should_clear = match self.file.load_unlocked(key).await {
            Ok(present) => present.as_ref() == Some(expected),
            Err(TokenStoreError::Serde(_)) => true,
            Err(e) => return Err(e),
        };
        if should_clear {
            self.file.clear_unlocked(key).await?;
        }
        Ok(())
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

    async fn save_if_current(
        &self,
        key: &TokenKey,
        expected: &PersistedTokens,
        replacement: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(Some(keyring_tokens)) => {
                    if &keyring_tokens != expected {
                        return Ok(false);
                    }
                    self.keyring.save_unlocked(key, replacement)?;
                    self.save_file_shadow_if_current_or_unreadable(
                        key,
                        Some(expected),
                        replacement,
                    )
                    .await;
                    return Ok(true);
                }
                Ok(None) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }

        if self.file.load_unlocked(key).await?.as_ref() != Some(expected) {
            return Ok(false);
        }
        self.file.save_unlocked(key, replacement).await?;
        Ok(true)
    }

    async fn save_if_current_optional(
        &self,
        key: &TokenKey,
        expected: Option<&PersistedTokens>,
        replacement: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(Some(keyring_tokens)) => {
                    if Some(&keyring_tokens) != expected {
                        return Ok(false);
                    }
                    self.keyring.save_unlocked(key, replacement)?;
                    self.save_file_shadow_if_current_or_unreadable(key, expected, replacement)
                        .await;
                    return Ok(true);
                }
                Ok(None) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(e) => return Err(e),
            }
        }

        if self.file.load_unlocked(key).await?.as_ref() != expected {
            return Ok(false);
        }
        self.file.save_unlocked(key, replacement).await?;
        Ok(true)
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;
        self.clear_unlocked(key).await
    }

    async fn clear_if_unreadable(&self, key: &TokenKey) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(self.file.root(), key).await?;

        #[cfg(feature = "keyring")]
        {
            match self.keyring.load_unlocked(key) {
                Ok(Some(_)) => return Ok(false),
                Ok(None) | Err(TokenStoreError::KeyringUnavailable(_)) => {}
                Err(TokenStoreError::Serde(_)) => {
                    self.clear_unlocked(key).await?;
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
        }

        match self.file.load_unlocked(key).await {
            Ok(_) => Ok(false),
            Err(TokenStoreError::Serde(_)) => {
                self.file.clear_unlocked(key).await?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
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
                        self.clear_file_shadow_if_current_or_unreadable(key, expected)
                            .await?;
                        self.keyring.clear_unlocked(key)?;
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

#[cfg(all(test, feature = "keyring"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn key() -> TokenKey {
        TokenKey::parse("dev", "x").expect("valid test key")
    }

    fn token(value: &str) -> PersistedTokens {
        PersistedTokens::api_key(value)
    }

    fn write_malformed_shadow(root: &Path, key: &TokenKey) {
        let path = root
            .join(key.realm.as_str())
            .join(format!("{}.json", key.binding.as_str()));
        std::fs::create_dir_all(path.parent().expect("token path has parent"))
            .expect("create token dir");
        std::fs::write(path, "{ malformed token json").expect("write malformed token");
    }

    #[tokio::test]
    async fn keyring_save_shadow_replaces_malformed_file_without_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AutoTokenStore::new(temp.path().to_path_buf(), "meerkat-test-shadow-save");
        let key = key();
        let expected = token("expected");
        let replacement = token("replacement");

        write_malformed_shadow(temp.path(), &key);
        store
            .save_file_shadow_if_current_or_unreadable(&key, Some(&expected), &replacement)
            .await;

        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(replacement)
        );
    }

    #[tokio::test]
    async fn keyring_clear_shadow_removes_malformed_file_before_primary_clear() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AutoTokenStore::new(temp.path().to_path_buf(), "meerkat-test-shadow-clear");
        let key = key();
        let expected = token("expected");

        write_malformed_shadow(temp.path(), &key);
        store
            .clear_file_shadow_if_current_or_unreadable(&key, &expected)
            .await
            .expect("malformed shadow clear");

        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            None
        );
    }

    #[tokio::test]
    async fn keyring_shadow_helpers_preserve_different_readable_file_material() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store = AutoTokenStore::new(temp.path().to_path_buf(), "meerkat-test-shadow-stale");
        let key = key();
        let expected = token("expected");
        let other = token("other");
        let replacement = token("replacement");

        store
            .file
            .save_unlocked(&key, &other)
            .await
            .expect("seed file shadow");
        store
            .save_file_shadow_if_current_or_unreadable(&key, Some(&expected), &replacement)
            .await;
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(other.clone())
        );

        store
            .clear_file_shadow_if_current_or_unreadable(&key, &expected)
            .await
            .expect("different material is left alone");
        assert_eq!(
            store.file.load_unlocked(&key).await.expect("file load"),
            Some(other)
        );
    }
}

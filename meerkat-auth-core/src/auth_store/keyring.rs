//! OS keyring-backed token store.
//!
//! Feature-gated behind `native-keyring`. Uses the `keyring` crate (v3).
//! Service name is user-configurable (default `"meerkat-auth"`); account
//! default account format is `<realm>:<binding>` per Codex
//! `codex-rs/login/src/auth/storage.rs:149-220`; profile overrides use
//! `<realm>:<binding>:<profile>`.
//!
//! The keyring crate has no enumeration API; `list()` returns
//! `TokenStoreError::Unavailable` so callers can fall back to a file-store
//! index when they need enumeration.

use async_trait::async_trait;
use keyring::Entry;
use std::path::{Path, PathBuf};

use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

pub struct KeyringTokenStore {
    service_name: String,
    lock_root: PathBuf,
}

impl KeyringTokenStore {
    pub fn new(service_name: impl Into<String>) -> Self {
        let service_name = service_name.into();
        Self {
            lock_root: default_lock_root(&service_name),
            service_name,
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    pub fn lock_root(&self) -> &Path {
        &self.lock_root
    }

    fn entry_for(&self, key: &TokenKey) -> Result<Entry, TokenStoreError> {
        Entry::new(&self.service_name, &key.keyring_account())
            .map_err(|e| TokenStoreError::KeyringUnavailable(e.to_string()))
    }

    pub(crate) fn load_unlocked(
        &self,
        key: &TokenKey,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let entry = self.entry_for(key)?;
        let json = match entry.get_password() {
            Ok(s) => s,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(TokenStoreError::KeyringUnavailable(e.to_string())),
        };
        let tokens: PersistedTokens = serde_json::from_str(&json)?;
        Ok(Some(tokens))
    }

    pub(crate) fn save_unlocked(
        &self,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        let entry = self.entry_for(key)?;
        let json = serde_json::to_string(tokens)?;
        entry
            .set_password(&json)
            .map_err(|e| TokenStoreError::KeyringUnavailable(e.to_string()))?;
        Ok(())
    }

    pub(crate) fn clear_unlocked(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let entry = self.entry_for(key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(TokenStoreError::KeyringUnavailable(e.to_string())),
        }
    }
}

#[async_trait]
impl TokenStore for KeyringTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let _guard = super::lock::lock(&self.lock_root, key).await?;
        self.load_unlocked(key)
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        let _guard = super::lock::lock(&self.lock_root, key).await?;
        self.save_unlocked(key, tokens)
    }

    async fn save_if_current(
        &self,
        key: &TokenKey,
        expected: &PersistedTokens,
        replacement: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(&self.lock_root, key).await?;
        if self.load_unlocked(key)?.as_ref() != Some(expected) {
            return Ok(false);
        }
        self.save_unlocked(key, replacement)?;
        Ok(true)
    }

    async fn save_if_current_optional(
        &self,
        key: &TokenKey,
        expected: Option<&PersistedTokens>,
        replacement: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(&self.lock_root, key).await?;
        if self.load_unlocked(key)?.as_ref() != expected {
            return Ok(false);
        }
        self.save_unlocked(key, replacement)?;
        Ok(true)
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let _guard = super::lock::lock(&self.lock_root, key).await?;
        self.clear_unlocked(key)
    }

    async fn clear_if_current(
        &self,
        key: &TokenKey,
        expected: &PersistedTokens,
    ) -> Result<bool, TokenStoreError> {
        let _guard = super::lock::lock(&self.lock_root, key).await?;
        if self.load_unlocked(key)?.as_ref() != Some(expected) {
            return Ok(false);
        }
        self.clear_unlocked(key)?;
        Ok(true)
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        // keyring crate has no enumeration; callers that need it should
        // pair a `FileTokenStore` index or use `AutoTokenStore` which
        // falls back to the file backend for enumeration.
        Err(TokenStoreError::Unavailable(
            "keyring backend does not support enumeration".into(),
        ))
    }

    fn backend_name(&self) -> &'static str {
        "keyring"
    }
}

fn default_lock_root(_service_name: &str) -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("meerkat")
        .join("credentials")
}

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

use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

pub struct KeyringTokenStore {
    service_name: String,
}

impl KeyringTokenStore {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    fn entry_for(&self, key: &TokenKey) -> Result<Entry, TokenStoreError> {
        Entry::new(&self.service_name, &key.keyring_account())
            .map_err(|e| TokenStoreError::KeyringUnavailable(e.to_string()))
    }
}

#[async_trait]
impl TokenStore for KeyringTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        let entry = self.entry_for(key)?;
        let json = match entry.get_password() {
            Ok(s) => s,
            Err(keyring::Error::NoEntry) => return Ok(None),
            Err(e) => return Err(TokenStoreError::KeyringUnavailable(e.to_string())),
        };
        let tokens: PersistedTokens = serde_json::from_str(&json)?;
        Ok(Some(tokens))
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        let entry = self.entry_for(key)?;
        let json = serde_json::to_string(tokens)?;
        entry
            .set_password(&json)
            .map_err(|e| TokenStoreError::KeyringUnavailable(e.to_string()))?;
        Ok(())
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        let entry = self.entry_for(key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(TokenStoreError::KeyringUnavailable(e.to_string())),
        }
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

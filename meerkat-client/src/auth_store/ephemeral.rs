//! In-memory token store for tests and WASM runtimes that do not persist
//! credentials to disk.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use super::{PersistedTokens, TokenKey, TokenStore, TokenStoreError};

#[derive(Default)]
pub struct EphemeralTokenStore {
    inner: Arc<RwLock<HashMap<TokenKey, PersistedTokens>>>,
}

impl EphemeralTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TokenStore for EphemeralTokenStore {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
        Ok(self.inner.read().get(key).cloned())
    }

    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError> {
        self.inner.write().insert(key.clone(), tokens.clone());
        Ok(())
    }

    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.inner.write().remove(key);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        Ok(self.inner.read().keys().cloned().collect())
    }

    fn backend_name(&self) -> &'static str {
        "ephemeral"
    }
}

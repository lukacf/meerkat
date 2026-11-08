//! In-memory token store for tests and WASM runtimes that do not persist
//! credentials to disk. It is process-local and has no fence: its writes
//! accept any admission guard, [`AdmissionGuard::none`] included.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use meerkat_core::AdmissionGuard;
use meerkat_core::auth::token_store::TokenStoreObservation;

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

    async fn save(
        &self,
        _guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError> {
        self.inner.write().insert(key.clone(), tokens.clone());
        Ok(())
    }

    async fn clear(&self, _guard: &AdmissionGuard, key: &TokenKey) -> Result<(), TokenStoreError> {
        self.inner.write().remove(key);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
        Ok(self.inner.read().keys().cloned().collect())
    }

    fn backend_name(&self) -> &'static str {
        "ephemeral"
    }

    /// An in-memory read never raises operating-system UI: status reads it.
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        Ok(match self.inner.read().get(key).cloned() {
            Some(tokens) => TokenStoreObservation::Present(Box::new(tokens)),
            None => TokenStoreObservation::Missing,
        })
    }
}

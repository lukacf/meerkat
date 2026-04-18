//! TokenStore trait + PersistedTokens/TokenKey types + RefreshCoordinator.
//!
//! Moved from `meerkat-providers::auth_store` (B2 split, 2026-04-18) so
//! the trait surface is reachable without heavy-IO dependencies.
//! Concrete backends (File/Keyring/Auto/Ephemeral/InMemory/FileLock)
//! live in `meerkat-auth-core::auth_store`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Key for a persisted token bundle: realm + binding.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, Ord, PartialOrd)]
pub struct TokenKey {
    pub realm_id: String,
    pub binding_id: String,
}

impl TokenKey {
    pub fn new(realm_id: impl Into<String>, binding_id: impl Into<String>) -> Self {
        Self {
            realm_id: realm_id.into(),
            binding_id: binding_id.into(),
        }
    }

    /// The flat account identifier used by OS keyrings. Format: `<realm>:<binding>`.
    pub fn keyring_account(&self) -> String {
        format!("{}:{}", self.realm_id, self.binding_id)
    }
}

/// Kind of credential material persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedAuthMode {
    ApiKey,
    StaticBearer,
    ChatgptOauth,
    ClaudeAiOauth,
    OauthToApiKey,
    GoogleOauth,
    Adc,
    ComputeAdc,
    Bedrock,
    Vertex,
    Foundry,
    ExternalTokens,
    ExternalAuthorizer,
    Command,
}

/// Serializable token bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PersistedTokens {
    pub auth_mode: PersistedAuthMode,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub primary_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id_token: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "chrono::serde::ts_seconds_option"
    )]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "chrono::serde::ts_seconds_option"
    )]
    pub last_refresh: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl PersistedTokens {
    pub fn api_key(secret: impl Into<String>) -> Self {
        Self {
            auth_mode: PersistedAuthMode::ApiKey,
            primary_secret: Some(secret.into()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn static_bearer(token: impl Into<String>) -> Self {
        Self {
            auth_mode: PersistedAuthMode::StaticBearer,
            primary_secret: Some(token.into()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }
}

/// Errors from the token-store layer.
#[derive(Debug, Error)]
pub enum TokenStoreError {
    #[error("io error: {0}")]
    Io(String),
    #[error("serialization error: {0}")]
    Serde(String),
    #[error("keyring backend unavailable: {0}")]
    KeyringUnavailable(String),
    #[error("no credentials found for {realm}:{binding}")]
    NotFound { realm: String, binding: String },
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("backend unavailable: {0}")]
    Unavailable(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<std::io::Error> for TokenStoreError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied(e.to_string()),
            std::io::ErrorKind::NotFound => Self::Io(e.to_string()),
            _ => Self::Io(e.to_string()),
        }
    }
}

impl From<serde_json::Error> for TokenStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e.to_string())
    }
}

/// Cross-process-safe persistence for tokens.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait TokenStore: Send + Sync {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError>;
    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError>;
    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError>;
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError>;
    fn backend_name(&self) -> &'static str;
}

/// Errors raised by the refresh coordinator.
#[derive(Clone, Debug, Error)]
pub enum RefreshError {
    #[error("refresh function failed: {0}")]
    Refresh(String),
    #[error("refresh in progress was cancelled")]
    Cancelled,
    #[error("cross-process lock acquisition failed: {0}")]
    LockFailed(String),
}

/// Boxed refresh closure.
pub type RefreshFn =
    Box<dyn FnOnce() -> BoxFuture<'static, Result<PersistedTokens, RefreshError>> + Send + 'static>;

/// Coordinator for token-refresh calls. Implementations coalesce
/// concurrent calls for the same `TokenKey`.
#[async_trait]
pub trait RefreshCoordinator: Send + Sync {
    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError>;
}

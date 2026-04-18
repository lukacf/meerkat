//! Token storage for the provider-auth redesign (Phase 4a).
//!
//! `TokenStore` is the persistence boundary between resolved OAuth tokens /
//! API keys / bearer bundles and the provider runtime layer. Four backends
//! ship: `File` (JSON at `$XDG_CONFIG_HOME/meerkat/credentials/`), `Keyring`
//! (native OS keyring, feature-gated), `Auto` (keyring with file fallback),
//! and `Ephemeral` (in-memory HashMap for tests).
//!
//! Reference-CLI shape: modeled after Codex `AuthDotJson` (see
//! `codex-rs/login/src/auth/storage.rs:28-329`), Claude Code `AuthData`
//! (see `claude-code/src/utils/auth.ts:1313-1560`), and Gemini CLI
//! `Oauth2Credential` (see `gemini-cli/packages/core/src/code_assist/oauth-credential-storage.ts`).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod auto;
pub mod command;
pub mod ephemeral;
pub mod file;
#[cfg(feature = "native-keyring")]
pub mod keyring;
pub mod refresh;

pub use auto::AutoTokenStore;
pub use command::{CommandCredentialError, CommandCredentialRunner, CommandCredentialSpec};
pub use ephemeral::EphemeralTokenStore;
pub use file::FileTokenStore;
#[cfg(feature = "native-keyring")]
pub use keyring::KeyringTokenStore;
#[cfg(feature = "refresh-file-lock")]
pub use refresh::FileLockCoordinator;
pub use refresh::{InMemoryCoordinator, RefreshCoordinator, RefreshError, RefreshFn};

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

/// Kind of credential material persisted. Closed set; expanded per
/// provider-method as Phase 4b lands.
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

/// Serializable token bundle. One of `primary_secret` / `refresh_token`
/// will be populated; other fields are provider-specific and optional.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PersistedTokens {
    pub auth_mode: PersistedAuthMode,

    /// Access token (OAuth) or API key (api_key mode). Redacted in Debug.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub primary_secret: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refresh_token: Option<String>,

    /// OAuth ID token (JWT carrying identity claims).
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

    /// Provider-specific metadata (JWT claims, subscription tier, fedramp
    /// flag, etc.). Typed variants land in Phase 4b `ProviderAuthMetadata`.
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
#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Load tokens for the given binding, or `None` if not present.
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError>;

    /// Persist tokens for the given binding (overwrites existing).
    async fn save(&self, key: &TokenKey, tokens: &PersistedTokens) -> Result<(), TokenStoreError>;

    /// Remove stored tokens for the given binding. No-op if absent.
    async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError>;

    /// List all bindings with stored tokens.
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError>;

    /// Backend identifier for diagnostics.
    fn backend_name(&self) -> &'static str;
}

/// Backend selector for `TokenStore::open()`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenStoreBackend {
    File {
        root: std::path::PathBuf,
    },
    #[cfg(feature = "native-keyring")]
    Keyring {
        service_name: String,
    },
    Auto {
        root: std::path::PathBuf,
        service_name: String,
    },
    Ephemeral,
}

impl TokenStoreBackend {
    /// Convenience: build the `Auto` default (keyring if available, file
    /// fallback at `$XDG_CONFIG_HOME/meerkat/credentials/`).
    pub fn default_auto() -> Result<Self, TokenStoreError> {
        let base = dirs::config_dir()
            .ok_or_else(|| TokenStoreError::Unavailable("no XDG_CONFIG_HOME".into()))?;
        Ok(Self::Auto {
            root: base.join("meerkat").join("credentials"),
            service_name: "meerkat-auth".into(),
        })
    }

    /// File-only backend at the default location.
    pub fn default_file() -> Result<Self, TokenStoreError> {
        let base = dirs::config_dir()
            .ok_or_else(|| TokenStoreError::Unavailable("no XDG_CONFIG_HOME".into()))?;
        Ok(Self::File {
            root: base.join("meerkat").join("credentials"),
        })
    }

    /// Open a concrete `TokenStore` from this backend descriptor.
    pub fn open(self) -> Result<Arc<dyn TokenStore>, TokenStoreError> {
        match self {
            Self::File { root } => Ok(Arc::new(FileTokenStore::new(root))),
            #[cfg(feature = "native-keyring")]
            Self::Keyring { service_name } => Ok(Arc::new(KeyringTokenStore::new(service_name))),
            Self::Auto { root, service_name } => {
                Ok(Arc::new(AutoTokenStore::new(root, service_name)))
            }
            Self::Ephemeral => Ok(Arc::new(EphemeralTokenStore::new())),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn persisted_tokens_round_trip_serde() {
        let tokens = PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access-xyz".into()),
            refresh_token: Some("refresh-abc".into()),
            id_token: Some("jwt".into()),
            expires_at: Some(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
            last_refresh: None,
            scopes: vec!["openid".into(), "email".into()],
            account_id: Some("acct_123".into()),
            metadata: serde_json::json!({"plan_type": "pro"}),
        };
        let json = serde_json::to_string(&tokens).unwrap();
        let round: PersistedTokens = serde_json::from_str(&json).unwrap();
        assert_eq!(tokens, round);
    }

    #[test]
    fn token_key_keyring_account() {
        let k = TokenKey::new("dev", "default_openai");
        assert_eq!(k.keyring_account(), "dev:default_openai");
    }
}

//! Token storage backends — concrete implementations of
//! `meerkat_core::auth::TokenStore` and `RefreshCoordinator`.
//!
//! The trait + types moved to `meerkat_core::auth::token_store` in the
//! B2 split (2026-04-18) so that they're reachable from the llm-core
//! provider-runtime registry without heavy-IO deps. This module owns the
//! concrete File/Keyring/Auto/Ephemeral backends.

use std::sync::Arc;

use meerkat_core::CredentialSourceSpec;

pub mod auto;
pub mod command;
pub mod ephemeral;
pub mod file;
#[cfg(feature = "keyring")]
pub mod keyring;
pub mod refresh;

pub use auto::AutoTokenStore;
pub use command::{CommandCredentialError, CommandCredentialRunner, CommandCredentialSpec};
pub use ephemeral::EphemeralTokenStore;
pub use file::FileTokenStore;
#[cfg(feature = "keyring")]
pub use keyring::KeyringTokenStore;
#[cfg(feature = "file-lock")]
pub use refresh::FileLockCoordinator;
pub use refresh::InMemoryCoordinator;

// Re-exports from meerkat-core (trait + types moved there in B2 split).
pub use meerkat_core::auth::token_store::{
    PersistedAuthMode, PersistedTokens, RefreshCoordinator, RefreshError, RefreshFn, TokenKey,
    TokenStore, TokenStoreError,
};

pub fn persisted_auth_mode_for_auth_method(auth_method: &str) -> Option<PersistedAuthMode> {
    match auth_method {
        "api_key" | "api_key_express" | "foundry_api_key" => Some(PersistedAuthMode::ApiKey),
        "static_bearer" | "bearer_api_key" | "bedrock_bearer" => {
            Some(PersistedAuthMode::StaticBearer)
        }
        "managed_chatgpt_oauth" => Some(PersistedAuthMode::ChatgptOauth),
        "external_chatgpt_tokens" => Some(PersistedAuthMode::ExternalTokens),
        "claude_ai_oauth" => Some(PersistedAuthMode::ClaudeAiOauth),
        "oauth_to_api_key" => Some(PersistedAuthMode::OauthToApiKey),
        "google_oauth" => Some(PersistedAuthMode::GoogleOauth),
        _ => None,
    }
}

pub fn credential_source_uses_persisted_store(source: &CredentialSourceSpec) -> bool {
    matches!(
        source,
        CredentialSourceSpec::ManagedStore | CredentialSourceSpec::PlatformDefault
    )
}

pub fn persisted_auth_mode_is_oauth_login(mode: PersistedAuthMode) -> bool {
    meerkat_core::persisted_auth_mode_uses_oauth_login_lifecycle(mode)
}

/// Backend selector for `TokenStore::open()`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenStoreBackend {
    File {
        root: std::path::PathBuf,
    },
    #[cfg(feature = "keyring")]
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
    pub fn default_auto() -> Result<Self, TokenStoreError> {
        // The product-facing default is intentionally file-backed: macOS
        // keychain reads can display app-access prompts during normal CLI
        // runs. Callers that explicitly want OS keyring fallback can build
        // `TokenStoreBackend::Auto` with their chosen service name.
        Self::default_file()
    }

    pub fn default_keyring_auto() -> Result<Self, TokenStoreError> {
        let base = dirs::config_dir()
            .ok_or_else(|| TokenStoreError::Unavailable("no XDG_CONFIG_HOME".into()))?;
        Ok(Self::Auto {
            root: base.join("meerkat").join("credentials"),
            service_name: "meerkat-auth".into(),
        })
    }

    pub fn default_file() -> Result<Self, TokenStoreError> {
        let base = dirs::config_dir()
            .ok_or_else(|| TokenStoreError::Unavailable("no XDG_CONFIG_HOME".into()))?;
        Ok(Self::File {
            root: base.join("meerkat").join("credentials"),
        })
    }

    pub fn open(self) -> Result<Arc<dyn TokenStore>, TokenStoreError> {
        match self {
            Self::File { root } => Ok(Arc::new(FileTokenStore::new(root))),
            #[cfg(feature = "keyring")]
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
    fn default_auto_uses_file_backend() {
        let backend = TokenStoreBackend::default_auto().expect("default token store path");
        assert!(
            matches!(backend, TokenStoreBackend::File { .. }),
            "default token store must not trigger OS keychain prompts"
        );
    }

    #[test]
    fn keyring_auto_stays_explicit() {
        let backend = TokenStoreBackend::default_keyring_auto().expect("default token store path");
        assert!(
            matches!(backend, TokenStoreBackend::Auto { .. }),
            "keyring fallback should remain available as an explicit backend"
        );
    }

    #[test]
    fn token_key_keyring_account() {
        let k = TokenKey::parse("dev", "default_openai").expect("valid slugs");
        assert_eq!(k.keyring_account(), "dev:default_openai");
    }

    #[test]
    fn token_key_profile_override_is_part_of_identity() {
        let default_key = TokenKey::parse("dev", "default_openai").expect("valid slugs");
        let override_key = TokenKey::parse_with_profile("dev", "default_openai", Some("work"))
            .expect("valid slugs");
        assert_ne!(default_key, override_key);
        assert_eq!(override_key.keyring_account(), "dev:default_openai:work");
    }
}

//! Token storage backends — concrete implementations of
//! `meerkat_core::auth::TokenStore` and `RefreshCoordinator`.
//!
//! The trait + types moved to `meerkat_core::auth::token_store` in the
//! B2 split (2026-04-18) so that they're reachable from the llm-core
//! provider-runtime registry without heavy-IO deps. This module owns the
//! concrete File/Keyring/Auto/Ephemeral backends.

use std::sync::Arc;

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

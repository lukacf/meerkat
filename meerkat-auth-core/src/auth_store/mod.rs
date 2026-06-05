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

/// Thin parse-at-boundary shim for genuine string-edge callers (CLI / REST /
/// RPC auth-status projection) that hold only a raw config `auth_method` string
/// and no typed `NormalizedAuthMethod`.
///
/// The auth-method -> persisted-mode mapping itself is owned by the typed
/// per-provider `*AuthMethod::persisted_auth_mode`; this shim only parses the
/// (provider-agnostic, unambiguous) string into one of those typed enums and
/// delegates. Callers that already hold a typed `NormalizedAuthMethod` (e.g.
/// from a `ValidatedBinding`) must call `.persisted_auth_mode()` directly
/// rather than round-tripping through this function.
pub fn persisted_auth_mode_for_auth_method(auth_method: &str) -> Option<PersistedAuthMode> {
    use meerkat_core::provider_matrix::{
        AnthropicAuthMethod, GoogleAuthMethod, OpenAiAuthMethod, SelfHostedAuthMethod,
    };
    OpenAiAuthMethod::parse(auth_method)
        .and_then(OpenAiAuthMethod::persisted_auth_mode)
        .or_else(|| {
            AnthropicAuthMethod::parse(auth_method)
                .and_then(AnthropicAuthMethod::persisted_auth_mode)
        })
        .or_else(|| {
            GoogleAuthMethod::parse(auth_method).and_then(GoogleAuthMethod::persisted_auth_mode)
        })
        .or_else(|| {
            SelfHostedAuthMethod::parse(auth_method)
                .and_then(SelfHostedAuthMethod::persisted_auth_mode)
        })
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
    fn azure_openai_api_key_uses_api_key_token_store_mode() {
        assert_eq!(
            persisted_auth_mode_for_auth_method("azure_api_key"),
            Some(PersistedAuthMode::ApiKey)
        );
    }

    #[test]
    fn shim_delegates_to_typed_owner_for_all_persistable_methods() {
        // The boundary shim must map every direct-secret method string the
        // typed owner recognizes — including the five (`api_key_express`,
        // `foundry_api_key`, `azure_api_key`, `bearer_api_key`,
        // `bedrock_bearer`) beyond the api_key/static_bearer literals — and
        // the OAuth-login methods, exactly as the per-provider enums declare.
        let cases = [
            ("api_key", Some(PersistedAuthMode::ApiKey)),
            ("api_key_express", Some(PersistedAuthMode::ApiKey)),
            ("foundry_api_key", Some(PersistedAuthMode::ApiKey)),
            ("azure_api_key", Some(PersistedAuthMode::ApiKey)),
            ("static_bearer", Some(PersistedAuthMode::StaticBearer)),
            ("bearer_api_key", Some(PersistedAuthMode::StaticBearer)),
            ("bedrock_bearer", Some(PersistedAuthMode::StaticBearer)),
            (
                "managed_chatgpt_oauth",
                Some(PersistedAuthMode::ChatgptOauth),
            ),
            (
                "external_chatgpt_tokens",
                Some(PersistedAuthMode::ExternalTokens),
            ),
            ("claude_ai_oauth", Some(PersistedAuthMode::ClaudeAiOauth)),
            ("oauth_to_api_key", Some(PersistedAuthMode::OauthToApiKey)),
            ("google_oauth", Some(PersistedAuthMode::GoogleOauth)),
            // Authorizer / ADC / SigV4-backed methods hold no persisted secret.
            ("external_authorizer", None),
            ("adc", None),
            ("compute_adc", None),
            ("bedrock_aws_sigv4", None),
            ("vertex_google_auth", None),
            ("foundry_azure_ad", None),
            ("none", None),
            ("totally_unknown", None),
        ];
        for (method, expected) in cases {
            assert_eq!(
                persisted_auth_mode_for_auth_method(method),
                expected,
                "shim mapping for auth_method '{method}'"
            );
        }
    }

    #[test]
    fn direct_secret_methods_beyond_two_literals_are_creatable() {
        use meerkat_core::persisted_auth_mode_is_directly_creatable;
        // The five direct-secret methods that the old 2-literal RPC/REST
        // allowlist silently made unreachable are now creatable.
        for method in [
            "api_key_express",
            "foundry_api_key",
            "azure_api_key",
            "bearer_api_key",
            "bedrock_bearer",
        ] {
            let mode = persisted_auth_mode_for_auth_method(method)
                .unwrap_or_else(|| panic!("'{method}' must resolve a persisted mode"));
            assert!(
                persisted_auth_mode_is_directly_creatable(mode),
                "'{method}' ({mode:?}) must be creatable at create surfaces"
            );
        }
    }

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

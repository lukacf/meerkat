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
pub use keyring::{KEYCHAIN_OPERATION_CAPACITY, KeyringTokenStore};
#[cfg(feature = "file-lock")]
pub use refresh::FileLockCoordinator;
pub use refresh::InMemoryCoordinator;
pub use refresh::{
    CREDENTIAL_ADMISSION_WAIT, CredentialSettlement, settle_in_flight_credential_mutations,
};

// Re-exports from meerkat-core (trait + types moved there in B2 split).
pub use meerkat_core::auth::token_store::{
    CredentialMutationError, CredentialMutationFn, CredentialMutationOutcome,
    CredentialSlotRefusal, PersistedAuthMode, PersistedTokens, ProviderAuthPersistence,
    RefreshCoordinator, RefreshError, RefreshFn, TokenKey, TokenStore, TokenStoreAccess,
    TokenStoreError, TokenStoreObservation, TokenStoreRefusal,
};

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

    /// Canonical cross-process refresh-lock directory for this persisted
    /// credential backend. It is derived from the same backend root so callers
    /// never reconstruct storage conventions independently.
    pub fn refresh_lock_dir(&self) -> Option<std::path::PathBuf> {
        match self {
            Self::File { root } | Self::Auto { root, .. } => Some(root.join("refresh-locks")),
            #[cfg(feature = "keyring")]
            Self::Keyring { .. } => dirs::config_dir().map(|base| {
                base.join("meerkat")
                    .join("credentials")
                    .join("refresh-locks")
            }),
            Self::Ephemeral => None,
        }
    }

    /// Whether this backend's operations can wait on the operating system
    /// (an OS keychain), so its coordinator must retain custody of admitted
    /// work beyond a dropped caller and bound admission.
    pub fn may_wait_on_operating_system(&self) -> bool {
        match self {
            Self::File { .. } | Self::Ephemeral => false,
            #[cfg(feature = "keyring")]
            Self::Keyring { .. } => true,
            Self::Auto { .. } => cfg!(feature = "keyring"),
        }
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

    /// Open the store together with its canonical refresh-mutation authority.
    ///
    /// Persisted stores must never degrade to a process-local coordinator: if
    /// file locking is unavailable, opening the paired capability fails
    /// closed. Only [`TokenStoreBackend::Ephemeral`] receives an
    /// [`InMemoryCoordinator`].
    pub fn open_with_refresh_authority(self) -> Result<ProviderAuthPersistence, TokenStoreError> {
        let refresh_coordinator: Arc<dyn RefreshCoordinator> = match &self {
            Self::Ephemeral => Arc::new(InMemoryCoordinator::new()),
            _ => {
                let lock_dir = self.refresh_lock_dir().ok_or_else(|| {
                    TokenStoreError::Unavailable(
                        "persisted TokenStore has no refresh-lock authority".into(),
                    )
                })?;
                #[cfg(feature = "file-lock")]
                {
                    if self.may_wait_on_operating_system() {
                        Arc::new(FileLockCoordinator::retaining_custody(lock_dir))
                    } else {
                        Arc::new(FileLockCoordinator::new(lock_dir))
                    }
                }
                #[cfg(not(feature = "file-lock"))]
                {
                    let _ = lock_dir;
                    return Err(TokenStoreError::Unavailable(
                        "persisted TokenStore requires the file-lock feature".into(),
                    ));
                }
            }
        };
        let token_store = self.open()?;
        Ok(ProviderAuthPersistence::new(
            token_store,
            refresh_coordinator,
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn direct_secret_methods_beyond_two_literals_are_creatable() {
        use meerkat_core::persisted_auth_mode_is_directly_creatable;
        use meerkat_core::provider_matrix::{
            AnthropicAuthMethod, GoogleAuthMethod, OpenAiAuthMethod,
        };
        // The five direct-secret methods that the old 2-literal RPC/REST
        // allowlist silently made unreachable are creatable — asserted through
        // the typed per-provider owners of the auth-method -> persisted-mode
        // mapping (no string-keyed shim in between).
        let modes = [
            GoogleAuthMethod::ApiKeyExpress.persisted_auth_mode(),
            AnthropicAuthMethod::FoundryApiKey.persisted_auth_mode(),
            OpenAiAuthMethod::AzureApiKey.persisted_auth_mode(),
            GoogleAuthMethod::BearerApiKey.persisted_auth_mode(),
            AnthropicAuthMethod::BedrockBearer.persisted_auth_mode(),
        ];
        for mode in modes {
            let mode = mode.expect("direct-secret method must resolve a persisted mode");
            assert!(
                persisted_auth_mode_is_directly_creatable(mode),
                "({mode:?}) must be creatable at create surfaces"
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
        let expected_lock_dir = match &backend {
            TokenStoreBackend::File { root } => root.join("refresh-locks"),
            other => panic!("unexpected default token store backend: {other:?}"),
        };
        assert!(
            matches!(backend, TokenStoreBackend::File { .. }),
            "default token store must not trigger OS keychain prompts"
        );
        assert_eq!(backend.refresh_lock_dir(), Some(expected_lock_dir));
    }

    #[test]
    fn keyring_auto_stays_explicit() {
        let backend = TokenStoreBackend::default_keyring_auto().expect("default token store path");
        assert!(
            matches!(backend, TokenStoreBackend::Auto { .. }),
            "keyring fallback should remain available as an explicit backend"
        );
    }

    #[cfg(feature = "file-lock")]
    #[test]
    fn persisted_backend_opens_store_and_refresh_authority_as_one_capability() {
        let temp = tempfile::tempdir().unwrap();
        let persistence = TokenStoreBackend::File {
            root: temp.path().join("credentials"),
        }
        .open_with_refresh_authority()
        .unwrap();
        let env = meerkat_llm_core::provider_runtime::ResolverEnvironment::testing()
            .with_provider_auth_persistence(persistence);

        assert!(env.provider_auth_persistence().is_some());
    }

    #[test]
    fn ephemeral_backend_is_explicitly_paired_with_process_local_authority() {
        let persistence = TokenStoreBackend::Ephemeral
            .open_with_refresh_authority()
            .unwrap();
        let env = meerkat_llm_core::provider_runtime::ResolverEnvironment::testing()
            .with_provider_auth_persistence(persistence);

        assert!(env.provider_auth_persistence().is_some());
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

/// Test support for the rows on the modes of the credential directories the
/// stores create: a scenario run under several process umasks.
#[cfg(all(test, unix))]
#[allow(clippy::expect_used)]
pub(crate) mod umask_test_support {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// The umasks a scenario runs under: the common default, a shared-group
    /// default, and the private one.
    const UMASKS: [&str; 3] = ["022", "002", "077"];
    /// Set to its umask in the child process a scenario runs in.
    const UMASK_CHILD: &str = "MEERKAT_AUTH_STORE_TEST_UMASK_CHILD";
    /// Printed by that child, with its umask, once the scenario passed.
    const UMASK_CHILD_DONE: &str = "credential directory scenario passed under umask";

    /// Run `scenario`, the body of the test `test` in the module `module` (the
    /// caller's `module_path!()`), under each of `UMASKS`. The umask is
    /// process-wide, so the shared test process never changes it: each run is
    /// a child process of this test binary, started by a shell that sets the
    /// umask before exec, that runs `test` alone.
    pub(crate) fn under_each_umask(module: &str, test: &str, scenario: impl FnOnce()) {
        if let Some(umask) = std::env::var_os(UMASK_CHILD) {
            scenario();
            println!("\n{UMASK_CHILD_DONE} {}", umask.to_string_lossy());
            return;
        }
        let binary = std::env::current_exe().expect("the test binary");
        let (_, module) = module
            .split_once("::")
            .expect("a module path within the crate");
        let test_path = format!("{module}::{test}");
        for umask in UMASKS {
            #[allow(clippy::disallowed_methods)] // the test re-runs its own binary
            let output = std::process::Command::new("/bin/sh")
                .args([
                    "-c",
                    "umask \"$1\" && shift && exec \"$@\"",
                    "credential-directory-umask",
                    umask,
                ])
                .arg(binary.as_os_str())
                .args([
                    "--exact",
                    test_path.as_str(),
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(UMASK_CHILD, umask)
                .output()
                .expect("start the test under a umask");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let done = format!("{UMASK_CHILD_DONE} {umask}");
            assert!(
                output.status.success() && stdout.contains(done.as_str()),
                "{test} failed under umask {umask} ({}):\n{stdout}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// The permission bits of `path`.
    pub(crate) fn mode_bits(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }
}

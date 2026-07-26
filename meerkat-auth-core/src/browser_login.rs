//! Canonical coordinated browser-OAuth credential commit.
//!
//! Browser hosts own loopback HTTP and browser launch. This module owns the
//! security-sensitive terminal transaction: verify one-time state, serialize
//! against refresh/logout, consume state, acquire AuthMachine lifecycle truth,
//! stamp the durable marker, persist, and compensate on failure.

use std::sync::Arc;

use meerkat_core::auth::token_store::{
    CredentialMutationError, CredentialMutationOutcome, PersistedTokens, ProviderAuthPersistence,
    TokenKey, TokenStore,
};
use meerkat_core::handles::{
    AuthLeasePhase, AuthLeaseRestoreSnapshot, AuthLeaseSnapshot, AuthLeaseTransition,
    GeneratedAuthLeaseHandle, LeaseKey,
};
use meerkat_core::{AuthBindingRef, OAuthProviderIdentity};

use crate::oauth_flow::{OAuthFlowAuthority, OAuthFlowError};

/// One verified browser-flow terminal consume request.
#[derive(Clone)]
pub struct BrowserOAuthFlowCommit {
    pub authority: Arc<dyn OAuthFlowAuthority>,
    pub state: String,
    pub provider: OAuthProviderIdentity,
    pub redirect_uri: String,
}

struct PreparedCommit {
    key: TokenKey,
    lease_key: LeaseKey,
    previous: Option<PersistedTokens>,
    previous_lifecycle: AuthLeaseSnapshot,
    previous_lifecycle_restore: AuthLeaseRestoreSnapshot,
    lifecycle_transition: AuthLeaseTransition,
}

/// Atomically consume a one-time browser OAuth flow and publish its credential.
///
/// The persistence capability supplies both the token vault and its
/// cross-process mutation coordinator. The flow authority must be backed by
/// the same AuthMachine lifecycle authority as `auth_lease`.
///
/// Terminal state remains consumed if lifecycle publication or durable save
/// fails. The authorization code has already been exchanged at this point;
/// restoring browser state would permit replay of a terminal OAuth response.
/// Credential/lifecycle state is compensated back to its predecessor instead.
pub async fn save_oauth_tokens_and_consume_browser_flow(
    persistence: ProviderAuthPersistence,
    auth_lease: GeneratedAuthLeaseHandle,
    auth_binding: AuthBindingRef,
    tokens: PersistedTokens,
    flow: BrowserOAuthFlowCommit,
) -> Result<PersistedTokens, CredentialMutationError> {
    if !flow.authority.terminal_flow_state_is_authmachine_owned() {
        return Err(CredentialMutationError::AuthLifecycle(
            "browser OAuth terminal state is not AuthMachine-owned".to_string(),
        ));
    }
    flow.authority
        .verify(
            &flow.state,
            &auth_binding,
            flow.provider,
            &flow.redirect_uri,
        )
        .map_err(flow_error)?;

    let store = persistence.token_store();
    let coordinator = persistence.refresh_coordinator();
    let key = TokenKey::from_auth_binding(&auth_binding);
    let load_key = key.clone();
    let outcome = coordinator
        .with_exclusive_mutation(
            key,
            Box::new(move || {
                Box::pin(async move {
                    let lease_key = LeaseKey::from_auth_binding(&auth_binding);
                    let _guard =
                        meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
                    let previous = meerkat_core::rehydrate_durable_predecessor_for_mutation(
                        store.as_ref(),
                        &auth_lease,
                        &auth_binding,
                        chrono::Utc::now(),
                    )
                    .await
                    .map_err(|error| {
                        CredentialMutationError::AuthLifecycle(format!(
                            "durable credential predecessor rehydrate failed: {error}"
                        ))
                    })?;
                    flow.authority
                        .consume(
                            &flow.state,
                            &auth_binding,
                            flow.provider,
                            &flow.redirect_uri,
                        )
                        .map_err(flow_error)?;

                    let previous_lifecycle_restore =
                        auth_lease.capture_auth_lifecycle_restore_snapshot(&lease_key);
                    let previous_lifecycle = previous_lifecycle_restore.snapshot().clone();
                    let lifecycle_transition =
                        match meerkat_core::publish_token_lifecycle_acquired(
                            &auth_lease,
                            &auth_binding,
                            &tokens,
                        ) {
                            Ok(transition) => transition,
                            Err(error) => {
                                let cleanup = if previous.is_none() {
                                    auth_lease
                                        .release_credential_lifecycle(&lease_key)
                                        .err()
                                        .map(|cleanup| {
                                            format!(
                                                "; uncredentialed terminal lifecycle cleanup failed: {cleanup}"
                                            )
                                        })
                                        .unwrap_or_default()
                                } else {
                                    String::new()
                                };
                                return Err(CredentialMutationError::AuthLifecycle(format!(
                                    "AuthMachine lifecycle acquire failed after OAuth consume: {error}{cleanup}"
                                )));
                            }
                        };
                    let commit = PreparedCommit {
                        key: load_key.clone(),
                        lease_key,
                        previous,
                        previous_lifecycle,
                        previous_lifecycle_restore,
                        lifecycle_transition,
                    };
                    let marked =
                        match meerkat_core::mark_tokens_lifecycle_published_for_transition(
                            &commit.key,
                            &tokens,
                            &commit.lifecycle_transition,
                        ) {
                            Ok(marked) => marked,
                            Err(error) => {
                                return Err(compensated_error(
                                    store.as_ref(),
                                    &auth_lease,
                                    &commit,
                                    format!(
                                        "AuthMachine lifecycle marker handoff failed after OAuth consume: {error}"
                                    ),
                                )
                                .await);
                            }
                        };
                    if let Err(error) = store.save(&commit.key, &marked).await {
                        return Err(compensated_error(
                            store.as_ref(),
                            &auth_lease,
                            &commit,
                            format!("TokenStore save failed after OAuth consume: {error}"),
                        )
                        .await);
                    }
                    Ok(CredentialMutationOutcome::Persisted(marked))
                })
            }),
        )
        .await?;
    match outcome {
        CredentialMutationOutcome::Persisted(tokens) => Ok(tokens),
        CredentialMutationOutcome::Cleared => Err(CredentialMutationError::Operation(
            "browser-login transaction returned cleared outcome".to_string(),
        )),
    }
}

fn flow_error(error: OAuthFlowError) -> CredentialMutationError {
    CredentialMutationError::AuthLifecycle(error.to_string())
}

async fn compensated_error(
    store: &dyn TokenStore,
    auth_lease: &GeneratedAuthLeaseHandle,
    commit: &PreparedCommit,
    message: String,
) -> CredentialMutationError {
    match rollback_commit(store, auth_lease, commit).await {
        Ok(()) => {
            CredentialMutationError::Operation(format!("{message}; acquired lease rolled back"))
        }
        Err(rollback) => CredentialMutationError::AuthLifecycle(format!(
            "{message}; acquired lease rollback failed: {rollback}"
        )),
    }
}

async fn rollback_commit(
    store: &dyn TokenStore,
    auth_lease: &GeneratedAuthLeaseHandle,
    commit: &PreparedCommit,
) -> Result<(), String> {
    auth_lease
        .release_credential_lifecycle(&commit.lease_key)
        .map_err(|error| format!("AuthMachine lifecycle rollback release failed: {error}"))?;
    match &commit.previous {
        Some(previous) => {
            store
                .save(&commit.key, previous)
                .await
                .map_err(|error| format!("TokenStore rollback save failed: {error}"))?;
            if matches!(
                commit.previous_lifecycle.phase,
                Some(phase) if phase != AuthLeasePhase::Released
            ) && let Some(restored_transition) = meerkat_core::restore_token_lifecycle_snapshot(
                auth_lease,
                &commit.previous_lifecycle_restore,
            )
            .map_err(|error| format!("AuthMachine lifecycle rollback failed: {error}"))?
            {
                let restored = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &commit.key,
                    previous,
                    &restored_transition,
                )
                .map_err(|error| format!("AuthMachine rollback marker handoff failed: {error}"))?;
                store
                    .save(&commit.key, &restored)
                    .await
                    .map_err(|error| format!("TokenStore rollback marker save failed: {error}"))?;
            }
        }
        None => {
            store
                .clear(&commit.key)
                .await
                .map_err(|error| format!("TokenStore rollback clear failed: {error}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Duration;
    use meerkat_core::auth::token_store::{PersistedAuthMode, TokenStoreError};
    use meerkat_core::{BindingId, BindingOrigin, RealmId};
    use std::time::Duration as StdDuration;

    struct AuthMachineOwnedTestFlowAuthority {
        inner: crate::oauth_flow::OAuthFlowRegistry,
    }

    impl AuthMachineOwnedTestFlowAuthority {
        fn new() -> Self {
            Self {
                inner: crate::oauth_flow::OAuthFlowRegistry::new(StdDuration::from_secs(60)),
            }
        }
    }

    impl OAuthFlowAuthority for AuthMachineOwnedTestFlowAuthority {
        fn terminal_flow_state_is_authmachine_owned(&self) -> bool {
            true
        }

        fn start(
            &self,
            target: AuthBindingRef,
            provider: OAuthProviderIdentity,
            redirect_uri: String,
            pkce_verifier: String,
        ) -> Result<String, OAuthFlowError> {
            self.inner
                .start(target, provider, redirect_uri, pkce_verifier)
        }

        fn verify(
            &self,
            state: &str,
            target: &AuthBindingRef,
            provider: OAuthProviderIdentity,
            redirect_uri: &str,
        ) -> Result<crate::oauth_flow::OAuthFlowRecord, OAuthFlowError> {
            self.inner.verify(state, target, provider, redirect_uri)
        }

        fn consume(
            &self,
            state: &str,
            target: &AuthBindingRef,
            provider: OAuthProviderIdentity,
            redirect_uri: &str,
        ) -> Result<crate::oauth_flow::OAuthFlowRecord, OAuthFlowError> {
            self.inner.consume(state, target, provider, redirect_uri)
        }

        fn admit_device_code(
            &self,
            target: AuthBindingRef,
            provider: OAuthProviderIdentity,
            device_code: String,
            expires_in: StdDuration,
        ) -> Result<(), OAuthFlowError> {
            self.inner
                .admit_device_code(target, provider, device_code, expires_in)
        }

        fn verify_device_code(
            &self,
            device_code: &str,
            target: &AuthBindingRef,
            provider: OAuthProviderIdentity,
        ) -> Result<crate::oauth_flow::OAuthDeviceFlowRecord, OAuthFlowError> {
            self.inner.verify_device_code(device_code, target, provider)
        }

        fn begin_device_code_poll(
            &self,
            device_code: &str,
            target: &AuthBindingRef,
            provider: OAuthProviderIdentity,
        ) -> Result<crate::oauth_flow::OAuthDevicePollLease, OAuthFlowError> {
            self.inner
                .begin_device_code_poll(device_code, target, provider)
        }
    }

    fn binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse("global").expect("valid realm"),
            binding: BindingId::parse("openai").expect("valid binding"),
            profile: None,
            origin: BindingOrigin::Configured,
        }
    }

    fn tokens() -> PersistedTokens {
        let now = chrono::Utc::now();
        PersistedTokens {
            auth_mode: PersistedAuthMode::ChatgptOauth,
            primary_secret: Some("access-token".to_string()),
            refresh_token: Some("refresh-token".to_string()),
            id_token: None,
            expires_at: Some(now + Duration::hours(1)),
            last_refresh: Some(now),
            scopes: vec!["openid".to_string()],
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn browser_commit_consumes_state_and_publishes_marked_credential() {
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let authority: Arc<dyn OAuthFlowAuthority> =
            Arc::new(AuthMachineOwnedTestFlowAuthority::new());
        let auth_lease = runtime.generated_auth_lease_handle();
        let store = Arc::new(crate::auth_store::EphemeralTokenStore::new());
        let persistence = ProviderAuthPersistence::new(
            store.clone(),
            Arc::new(crate::auth_store::InMemoryCoordinator::new()),
        );
        let binding = binding();
        let state = authority
            .start(
                binding.clone(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback".to_string(),
                "pkce-verifier".to_string(),
            )
            .expect("start browser flow");

        let committed = save_oauth_tokens_and_consume_browser_flow(
            persistence,
            auth_lease.clone(),
            binding.clone(),
            tokens(),
            BrowserOAuthFlowCommit {
                authority: Arc::clone(&authority),
                state: state.clone(),
                provider: OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback".to_string(),
            },
        )
        .await
        .expect("commit browser flow");

        assert_eq!(committed.primary_secret.as_deref(), Some("access-token"));
        let stored = store
            .load(&TokenKey::from_auth_binding(&binding))
            .await
            .expect("load committed tokens")
            .expect("committed tokens exist");
        assert_eq!(stored, committed);
        assert!(meerkat_core::auth::tokens_lifecycle_published(&stored));
        assert!(matches!(
            authority.verify(
                &state,
                &binding,
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::Missing | OAuthFlowError::RegistryProjectionMissing { .. })
        ));
        assert_eq!(
            auth_lease
                .snapshot(&LeaseKey::from_auth_binding(&binding))
                .phase,
            Some(AuthLeasePhase::Valid)
        );
    }

    struct FailingSaveStore;

    #[async_trait]
    impl TokenStore for FailingSaveStore {
        async fn load(&self, _key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            Ok(None)
        }

        async fn save(
            &self,
            _key: &TokenKey,
            _tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            Err(TokenStoreError::Io("injected save failure".to_string()))
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), TokenStoreError> {
            Ok(())
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(Vec::new())
        }

        fn backend_name(&self) -> &'static str {
            "failing"
        }
    }

    #[tokio::test]
    async fn browser_commit_save_failure_rolls_back_acquired_lifecycle() {
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let authority: Arc<dyn OAuthFlowAuthority> =
            Arc::new(AuthMachineOwnedTestFlowAuthority::new());
        let auth_lease = runtime.generated_auth_lease_handle();
        let persistence = ProviderAuthPersistence::new(
            Arc::new(FailingSaveStore),
            Arc::new(crate::auth_store::InMemoryCoordinator::new()),
        );
        let binding = binding();
        let state = authority
            .start(
                binding.clone(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback".to_string(),
                "pkce-verifier".to_string(),
            )
            .expect("start browser flow");

        let error = save_oauth_tokens_and_consume_browser_flow(
            persistence,
            auth_lease.clone(),
            binding.clone(),
            tokens(),
            BrowserOAuthFlowCommit {
                authority: Arc::clone(&authority),
                state: state.clone(),
                provider: OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback".to_string(),
            },
        )
        .await
        .expect_err("injected token-store save must fail");

        assert!(error.to_string().contains("acquired lease rolled back"));
        assert!(matches!(
            authority.verify(
                &state,
                &binding,
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::Missing | OAuthFlowError::RegistryProjectionMissing { .. })
        ));
        let snapshot = auth_lease.snapshot(&LeaseKey::from_auth_binding(&binding));
        assert!(!snapshot.credential_present);
        assert!(matches!(
            snapshot.phase,
            None | Some(AuthLeasePhase::Released)
        ));
    }
}

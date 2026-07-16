//! Runtime-discovered OAuth for HTTP MCP servers.
//!
//! The MCP config owns only server intent. This module owns OAuth discovery,
//! dynamic client registration, PKCE loopback flow, token persistence, and
//! refresh for MCP resources.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth_oauth::{
    OAuthEndpoints, OAuthError, OAuthTokenRequestFormat, OAuthTokenResult, PkcePair,
    bind_loopback_callback, exchange_authorization_code_with_state, exchange_refresh_token,
    oauth_refresh_observation,
};
use crate::auth_store::{
    CredentialMutationError, CredentialMutationOutcome, PersistedAuthMode, PersistedTokens,
    ProviderAuthPersistence, RefreshCoordinator, RefreshError, TokenKey, TokenStore,
};
use meerkat_core::auth::RefreshFailureDisposition;
use meerkat_core::connection::{AuthBindingRef, BindingId, BindingOrigin, RealmId};
use meerkat_core::generated::auth_lease_durable_lifecycle_marker as durable_marker;
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeaseRestoreSnapshot, AuthLeaseSnapshot,
    CredentialUseDisposition, CredentialUseIntent, GeneratedAuthLeaseHandle, LeaseKey,
};

const MCP_TOKEN_REALM: &str = "mcp-oauth";
const CALLBACK_PATH: &str = "/mcp/oauth/callback";
const CLIENT_NAME: &str = "Meerkat rkat";
pub const MCP_INTERACTIVE_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthMode {
    Stored,
    Interactive,
}

/// Typed canonical identity for an MCP server credential binding.
///
/// Row #349 closure: the `mcp-oauth` realm binding slug is derived from the
/// canonical server config (name + URL) exactly once, here, by the typed
/// identity — not re-hashed independently at every key-derivation site. The
/// `<slug>-<digest>` binding slug is the single owned projection of the
/// `(server_name, server_url)` pair; `token_key` and `lease_key` both delegate
/// to [`binding_slug`](Self::binding_slug) so the token realm key and the
/// `AuthMachine` lease key are guaranteed structurally identical for the same
/// server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct McpServerIdentity {
    server_name: String,
    server_url: String,
}

impl McpServerIdentity {
    /// Build the typed identity from the canonical server config fields.
    pub fn from_server_config(
        server_name: impl Into<String>,
        server_url: impl Into<String>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            server_url: server_url.into(),
        }
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    /// The single owned `<slug>-<digest>` binding slug for this server. The
    /// digest binds name + URL so two servers that differ only in URL get
    /// distinct bindings; the slug prefix keeps the binding human-recognizable.
    fn binding_slug(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.server_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.server_url.as_bytes());
        let digest = URL_SAFE_NO_PAD.encode(hasher.finalize());
        let name_slug = slug_component(&self.server_name);
        format!("{name_slug}-{}", &digest[..16])
    }

    fn key_error(&self, reason: impl std::fmt::Display) -> McpOAuthError {
        McpOAuthError::TokenKey {
            server_name: self.server_name.clone(),
            reason: reason.to_string(),
        }
    }

    /// The one typed auth binding from which both durable token identity and
    /// AuthMachine lifecycle identity are projected.
    pub fn auth_binding_ref(&self) -> Result<AuthBindingRef, McpOAuthError> {
        let realm = RealmId::parse(MCP_TOKEN_REALM).map_err(|error| self.key_error(error))?;
        let binding =
            BindingId::parse(self.binding_slug()).map_err(|error| self.key_error(error))?;
        Ok(AuthBindingRef {
            realm,
            binding,
            profile: None,
            origin: BindingOrigin::Configured,
        })
    }

    /// The durable token-store key for this server's credentials.
    pub fn token_key(&self) -> Result<TokenKey, McpOAuthError> {
        Ok(TokenKey::from_auth_binding(&self.auth_binding_ref()?))
    }

    /// The per-binding `AuthMachine` lease key for this MCP server, structurally
    /// identical to [`token_key`](Self::token_key) (realm `mcp-oauth` + the
    /// `<slug>-<digest>` binding slug). The credential-freshness / reauth
    /// decision for MCP-OAuth bearer tokens is owned by the `AuthMachine` keyed
    /// on this lease — the authority never re-derives expiry policy.
    pub fn lease_key(&self) -> Result<LeaseKey, McpOAuthError> {
        Ok(LeaseKey::from_auth_binding(&self.auth_binding_ref()?))
    }
}

fn slug_component(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        let next = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            last_dash = false;
            Some(ch.to_ascii_lowercase())
        } else if !last_dash {
            last_dash = true;
            Some('-')
        } else {
            None
        };
        if let Some(ch) = next {
            out.push(ch);
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "server".to_string()
    } else {
        trimmed
    }
}

#[async_trait]
pub trait BrowserOpener: Send + Sync {
    async fn open(&self, url: &str) -> Result<(), McpOAuthError>;
}

pub struct SystemBrowserOpener;

#[async_trait]
impl BrowserOpener for SystemBrowserOpener {
    async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
        webbrowser::open(url).map_err(|error| McpOAuthError::Browser(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthError {
    #[error("MCP OAuth token not found for '{server_name}'. Run: rkat mcp login {server_name}")]
    MissingStoredToken { server_name: String },
    #[error("MCP OAuth interactive login requires a TTY")]
    InteractiveRequiresTty,
    #[error("MCP OAuth discovery failed for '{server_name}': {reason}")]
    DiscoveryFailed { server_name: String, reason: String },
    #[error("MCP OAuth dynamic client registration failed for '{server_name}': {reason}")]
    RegistrationFailed { server_name: String, reason: String },
    #[error("MCP OAuth browser open failed: {0}")]
    Browser(String),
    #[error("MCP OAuth token exchange failed for '{server_name}': {reason}")]
    TokenExchangeFailed { server_name: String, reason: String },
    #[error("MCP OAuth token refresh failed for '{server_name}': {reason}")]
    RefreshFailed { server_name: String, reason: String },
    #[error("MCP OAuth token store error: {0}")]
    TokenStore(String),
    #[error("MCP OAuth token key error for '{server_name}': {reason}")]
    TokenKey { server_name: String, reason: String },
    #[error("MCP OAuth metadata missing from stored token for '{server_name}'")]
    MissingStoredMetadata { server_name: String },
    #[error("MCP OAuth stored credentials for '{server_name}' require reauth")]
    ReauthRequired { server_name: String },
    #[error("MCP OAuth credential lifecycle error for '{server_name}': {reason}")]
    AuthLifecycle { server_name: String, reason: String },
}

#[derive(Clone)]
pub struct McpOAuthAuthority {
    http: Client,
    /// Token vault plus same-key refresh serialization authority.
    provider_auth_persistence: ProviderAuthPersistence,
    browser: Arc<dyn BrowserOpener>,
    login_timeout: Duration,
    /// Generated `AuthMachine` lease handle that owns the credential
    /// freshness/refresh/reauth decision for the `mcp-oauth` realm. Injected by
    /// the surface that owns the runtime (the CLI) — `meerkat-auth-core` sits
    /// below `meerkat-runtime` in the dep graph and cannot mint a certified
    /// handle itself.
    auth_lease: GeneratedAuthLeaseHandle,
}

impl McpOAuthAuthority {
    pub fn new(
        provider_auth_persistence: ProviderAuthPersistence,
        browser: Arc<dyn BrowserOpener>,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http: Client::new(),
            provider_auth_persistence,
            browser,
            login_timeout: MCP_INTERACTIVE_LOGIN_TIMEOUT,
            auth_lease,
        }
    }

    pub fn with_http(
        provider_auth_persistence: ProviderAuthPersistence,
        browser: Arc<dyn BrowserOpener>,
        http: Client,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http,
            provider_auth_persistence,
            browser,
            login_timeout: MCP_INTERACTIVE_LOGIN_TIMEOUT,
            auth_lease,
        }
    }

    #[cfg(test)]
    fn with_test_http(
        token_store: Arc<crate::auth_store::EphemeralTokenStore>,
        browser: Arc<dyn BrowserOpener>,
        http: Client,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self::with_http(
            ProviderAuthPersistence::new(
                token_store,
                Arc::new(crate::auth_store::InMemoryCoordinator::new()),
            ),
            browser,
            http,
            auth_lease,
        )
    }

    #[cfg(test)]
    fn with_http_and_login_timeout(
        token_store: Arc<crate::auth_store::EphemeralTokenStore>,
        browser: Arc<dyn BrowserOpener>,
        http: Client,
        login_timeout: Duration,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http,
            provider_auth_persistence: ProviderAuthPersistence::new(
                token_store,
                Arc::new(crate::auth_store::InMemoryCoordinator::new()),
            ),
            browser,
            login_timeout,
            auth_lease,
        }
    }

    fn token_store(&self) -> Arc<dyn TokenStore> {
        self.provider_auth_persistence.token_store()
    }

    fn refresh_coordinator(&self) -> Arc<dyn RefreshCoordinator> {
        self.provider_auth_persistence.refresh_coordinator()
    }

    pub async fn stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        let key = target.token_key()?;
        let lease_key = target.lease_key()?;
        let admitted = {
            let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
            self.load_admitted_stored_credential(target, &key).await?
        };
        let Some(admitted) = admitted else {
            return Ok(None);
        };
        match admitted.disposition {
            CredentialUseDisposition::Authorized => Ok(admitted.tokens.primary_secret),
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing => {
                let authority = self.clone();
                let refresh_target = target.clone();
                let error_target = refresh_target.clone();
                let refresh_key = key.clone();
                let refreshed = self
                    .refresh_coordinator()
                    .with_refresh(
                        key,
                        Box::new(move || {
                            Box::pin(async move {
                                authority
                                    .refresh_stored_credential_under_coordinator(
                                        &refresh_target,
                                        &refresh_key,
                                    )
                                    .await
                            })
                        }),
                    )
                    .await
                    .map_err(|error| map_coordinated_refresh_error(&error_target, error))?;
                Ok(refreshed.primary_secret)
            }
            CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => Err(McpOAuthError::ReauthRequired {
                server_name: target.server_name().to_string(),
            }),
        }
    }

    pub async fn require_stored_bearer_token(
        &self,
        target: &McpServerIdentity,
    ) -> Result<String, McpOAuthError> {
        self.stored_bearer_token(target)
            .await?
            .ok_or_else(|| McpOAuthError::MissingStoredToken {
                server_name: target.server_name().to_string(),
            })
    }

    pub async fn interactive_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        let binding = bind_loopback_callback(CALLBACK_PATH)
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?;
        let redirect_uri = binding.redirect_url.clone();
        let discovery = self
            .discover(target, www_authenticate, &redirect_uri)
            .await?;
        let client = self
            .register_client(target, &discovery, &redirect_uri)
            .await?;
        let pkce = PkcePair::generate_s256();
        let state = URL_SAFE_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes());
        let endpoints = OAuthEndpoints {
            client_id: client.client_id.clone(),
            authorize_url: discovery.authorization_endpoint.clone(),
            token_url: discovery.token_endpoint.clone(),
            device_code_url: None,
            redirect_uri: redirect_uri.clone(),
            scopes: discovery.scopes.clone(),
            extra_authorize_params: vec![("resource".to_string(), discovery.resource.clone())],
            token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
            include_state_in_token_exchange: false,
            extra_token_params: vec![("resource".to_string(), discovery.resource.clone())],
            refresh_scopes: discovery.scopes.clone(),
            extra_headers: Vec::new(),
        };
        let authorize_url = endpoints.authorize_url_with_pkce(&pkce.challenge, &state);
        let callback = binding.expect_state(state.clone());
        self.browser.open(&authorize_url).await?;
        let outcome = callback
            .wait(self.login_timeout)
            .await
            .map_err(|error| map_oauth_exchange_error(target, error))?;
        let token = exchange_authorization_code_with_state(
            &self.http,
            &endpoints,
            &outcome.code,
            pkce.verifier.secret(),
            client.client_secret.as_deref(),
            Some(&outcome.state),
        )
        .await
        .map_err(|error| map_oauth_exchange_error(target, error))?;
        let persisted =
            persisted_tokens_from_result(&token, &discovery, &client, target, Utc::now())?;
        let key = target.token_key()?;
        let authority = self.clone();
        let commit_target = target.clone();
        let commit_key = key.clone();
        let committed = self
            .refresh_coordinator()
            .with_exclusive_mutation(
                key,
                Box::new(move || {
                    Box::pin(async move {
                        authority
                            .commit_interactive_login_under_coordinator(
                                &commit_target,
                                &commit_key,
                                &persisted,
                            )
                            .await
                            .map(CredentialMutationOutcome::Persisted)
                    })
                }),
            )
            .await
            .map_err(|error| map_coordinated_login_error(target, error))?;
        let CredentialMutationOutcome::Persisted(committed) = committed else {
            return Err(McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: "interactive MCP OAuth login unexpectedly cleared credentials".to_string(),
            });
        };
        committed
            .primary_secret
            .ok_or_else(|| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: "committed MCP OAuth credential has no bearer token".to_string(),
            })
    }

    /// Final interactive-login commit transaction. OAuth discovery, browser
    /// interaction, and token exchange intentionally happen before entering the
    /// per-key mutation boundary; only the durable read/publish/save transaction
    /// is serialized with refresh and other login commits.
    async fn commit_interactive_login_under_coordinator(
        &self,
        target: &McpServerIdentity,
        key: &TokenKey,
        persisted: &PersistedTokens,
    ) -> Result<PersistedTokens, CredentialMutationError> {
        let lease_key = target
            .lease_key()
            .map_err(|error| CredentialMutationError::Operation(error.to_string()))?;
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let mut previous_tokens = self
            .token_store()
            .load(key)
            .await
            .map_err(|error| CredentialMutationError::TokenStore(error.to_string()))?;

        // A newly opened CLI authority has no in-memory AuthMachine projection.
        // Restore a valid durable predecessor before capturing compensation so
        // rollback returns token material and lifecycle to the same publication.
        if let Some(tokens) = previous_tokens.as_ref()
            && tokens.auth_mode == PersistedAuthMode::McpOauth
            && tokens.primary_secret.is_some()
            && durable_marker::marker_payload_valid_for_tokens(tokens, key)
            && stored_metadata_for_target(target, tokens).is_ok()
        {
            let auth_binding = target
                .auth_binding_ref()
                .map_err(|error| CredentialMutationError::Operation(error.to_string()))?;
            previous_tokens = meerkat_core::rehydrate_marked_tokens_for_status(
                self.token_store().as_ref(),
                &self.auth_lease,
                &auth_binding,
                PersistedAuthMode::McpOauth,
                Utc::now(),
            )
            .await
            .map_err(|error| CredentialMutationError::AuthLifecycle(error.to_string()))?;
        }
        let previous_snapshot = self
            .auth_lease
            .capture_auth_lifecycle_restore_snapshot(&lease_key);

        // Acquire-first: the AuthMachine transition stamps the durable marker
        // before the single save. Any later failure restores the predecessor
        // captured under this same cross-process transaction.
        let published = match self.publish_login_tokens_via_lease(target, key, persisted) {
            Ok(published) => published,
            Err(error) => {
                let rollback_errors = self
                    .rollback_login_publication(
                        key,
                        &lease_key,
                        previous_tokens.as_ref(),
                        &previous_snapshot,
                    )
                    .await;
                return Err(CredentialMutationError::AuthLifecycle(format!(
                    "{error}{}",
                    rollback_error_suffix(&rollback_errors)
                )));
            }
        };
        if let Err(error) = self.token_store().save(key, &published).await {
            let rollback_errors = self
                .rollback_login_publication(
                    key,
                    &lease_key,
                    previous_tokens.as_ref(),
                    &previous_snapshot,
                )
                .await;
            return Err(CredentialMutationError::TokenStore(format!(
                "{error}{}",
                rollback_error_suffix(&rollback_errors)
            )));
        }
        Ok(published)
    }

    /// Wrap the post-login durable token write in an `AuthMachine` lease-publish
    /// transition (mirrors the provider path's
    /// `publish_managed_store_tokens_refresh_lifecycle` +
    /// `mark_tokens_lifecycle_published`). The shared lease handle is reset for
    /// this key, the lease is acquired at the credential's own expiry, and the
    /// resulting transition stamps the durable lifecycle marker onto the tokens
    /// before they are saved. Fail closed on any transition / marker error.
    fn publish_login_tokens_via_lease(
        &self,
        target: &McpServerIdentity,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<PersistedTokens, McpOAuthError> {
        let lease_key = target.lease_key()?;
        let lifecycle_err =
            |error: meerkat_core::handles::DslTransitionError| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            };
        // Clear the credential side before publishing the freshly minted
        // credential. This transition is fallible and therefore participates
        // in the caller's snapshot-backed transaction; it is never ignored.
        self.auth_lease
            .release_credential_lifecycle(&lease_key)
            .map_err(lifecycle_err)?;
        let expires_at = meerkat_core::persisted_token_expires_at_epoch_secs(tokens);
        let transition = self
            .auth_lease
            .acquire_lease(&lease_key, expires_at)
            .map_err(lifecycle_err)?;
        meerkat_core::mark_tokens_lifecycle_published_for_transition(key, tokens, &transition)
            .map_err(|error| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })
    }

    /// Compensate a failed interactive-login publication back to the exact
    /// durable/token-lifecycle state captured before replacement began.
    async fn rollback_login_publication(
        &self,
        key: &TokenKey,
        lease_key: &LeaseKey,
        previous_tokens: Option<&PersistedTokens>,
        previous_snapshot: &AuthLeaseRestoreSnapshot,
    ) -> Vec<String> {
        let mut rollback_errors = Vec::new();
        if let Err(error) = self.auth_lease.release_credential_lifecycle(lease_key) {
            rollback_errors.push(format!("AuthMachine rollback release failed: {error}"));
        }

        let restored_transition = match meerkat_core::restore_token_lifecycle_snapshot(
            &self.auth_lease,
            previous_snapshot,
        ) {
            Ok(transition) => transition,
            Err(error) => {
                rollback_errors.push(format!("AuthMachine snapshot restore failed: {error}"));
                None
            }
        };

        match previous_tokens {
            Some(previous_tokens) => {
                let mut restored_tokens = previous_tokens.clone();
                if let Some(transition) = restored_transition {
                    match meerkat_core::mark_tokens_lifecycle_published_for_transition(
                        key,
                        previous_tokens,
                        &transition,
                    ) {
                        Ok(marked) => restored_tokens = marked,
                        Err(error) => {
                            rollback_errors.push(format!("marker restore failed: {error}"));
                        }
                    }
                }
                if let Err(error) = self.token_store().save(key, &restored_tokens).await {
                    rollback_errors.push(format!("TokenStore rollback save failed: {error}"));
                }
            }
            None => {
                if let Err(error) = self.token_store().clear(key).await {
                    rollback_errors.push(format!("TokenStore rollback clear failed: {error}"));
                }
            }
        }
        rollback_errors
    }

    /// Load one durable MCP credential through its marker, AuthMachine
    /// projection, freshness observation, and generated use-admission gate.
    /// The caller holds the per-binding lifecycle guard for this whole read.
    async fn load_admitted_stored_credential(
        &self,
        target: &McpServerIdentity,
        key: &TokenKey,
    ) -> Result<Option<AdmittedMcpCredential>, McpOAuthError> {
        let Some(mut tokens) = self
            .token_store()
            .load(key)
            .await
            .map_err(|error| McpOAuthError::TokenStore(error.to_string()))?
        else {
            let lease_key = target.lease_key()?;
            let snapshot = self.auth_lease.snapshot(&lease_key);
            if snapshot.credential_present
                && snapshot
                    .phase
                    .is_some_and(|phase| phase != meerkat_core::handles::AuthLeasePhase::Released)
            {
                self.auth_lease
                    .release_credential_lifecycle(&lease_key)
                    .map_err(|error| McpOAuthError::AuthLifecycle {
                        server_name: target.server_name().to_string(),
                        reason: format!(
                            "durable MCP OAuth credential is absent but lifecycle reconciliation failed: {error}"
                        ),
                    })?;
            }
            return Ok(None);
        };
        if tokens.auth_mode != PersistedAuthMode::McpOauth {
            return Ok(None);
        }
        if tokens.primary_secret.is_none()
            || !durable_marker::marker_payload_valid_for_tokens(&tokens, key)
        {
            return Err(McpOAuthError::ReauthRequired {
                server_name: target.server_name().to_string(),
            });
        }
        let mut metadata = stored_metadata_for_target(target, &tokens)?;
        let auth_binding = target.auth_binding_ref()?;
        let lease_key = LeaseKey::from_auth_binding(&auth_binding);
        let lifecycle_err =
            |error: meerkat_core::handles::DslTransitionError| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            };

        let snapshot = self.auth_lease.snapshot(&lease_key);
        let restore_from_durable_marker = lifecycle_snapshot_is_absent(&snapshot)
            || matches!(
                durable_marker::marker_relation_for_tokens_and_snapshot(&tokens, &snapshot, key),
                durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
            );
        if restore_from_durable_marker {
            tokens = meerkat_core::rehydrate_marked_tokens_for_status(
                self.token_store().as_ref(),
                &self.auth_lease,
                &auth_binding,
                PersistedAuthMode::McpOauth,
                Utc::now(),
            )
            .await
            .map_err(|error| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .ok_or_else(|| McpOAuthError::ReauthRequired {
                server_name: target.server_name().to_string(),
            })?;
            if tokens.primary_secret.is_none()
                || !durable_marker::marker_payload_valid_for_tokens(&tokens, key)
            {
                return Err(McpOAuthError::ReauthRequired {
                    server_name: target.server_name().to_string(),
                });
            }
            metadata = stored_metadata_for_target(target, &tokens)?;
        }

        self.auth_lease
            .observe_credential_freshness(
                &lease_key,
                epoch_secs(Utc::now()),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(lifecycle_err)?;
        let restore_snapshot = self
            .auth_lease
            .capture_auth_lifecycle_restore_snapshot(&lease_key);
        let snapshot = restore_snapshot.snapshot().clone();
        if durable_marker::marker_relation_for_tokens_and_snapshot(&tokens, &snapshot, key)
            != durable_marker::AuthLeaseDurableMarkerRelation::Matches
        {
            return Err(McpOAuthError::ReauthRequired {
                server_name: target.server_name().to_string(),
            });
        }
        let disposition = self
            .auth_lease
            .resolve_credential_use_admission(&lease_key, CredentialUseIntent::UseCredential)
            .map_err(lifecycle_err)?;
        match disposition {
            CredentialUseDisposition::Authorized
            | CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing => Ok(Some(AdmittedMcpCredential {
                tokens,
                metadata,
                restore_snapshot,
                disposition,
            })),
            CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => Err(McpOAuthError::ReauthRequired {
                server_name: target.server_name().to_string(),
            }),
        }
    }

    /// Coordinator closure. It deliberately reloads and re-admits after the
    /// coordinator lock is held so a waiter observes a winner's durable token
    /// instead of issuing a second refresh or overwriting that result.
    async fn refresh_stored_credential_under_coordinator(
        &self,
        target: &McpServerIdentity,
        key: &TokenKey,
    ) -> Result<PersistedTokens, RefreshError> {
        let lease_key = target
            .lease_key()
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let admitted = self
            .load_admitted_stored_credential(target, key)
            .await
            .map_err(refresh_error_from_mcp)?
            .ok_or_else(|| {
                RefreshError::ReauthRequired(
                    "stored MCP OAuth credential disappeared before refresh".to_string(),
                )
            })?;
        if admitted.disposition == CredentialUseDisposition::Authorized {
            return Ok(admitted.tokens);
        }
        if admitted.disposition == CredentialUseDisposition::AlreadyRefreshing {
            return Err(RefreshError::Refresh(
                "AuthMachine reports an MCP OAuth refresh already in flight".to_string(),
            ));
        }

        let begin_disposition = self
            .auth_lease
            .resolve_credential_use_admission(&lease_key, CredentialUseIntent::BeginRefresh)
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        match begin_disposition {
            CredentialUseDisposition::RefreshRequired => self
                .auth_lease
                .begin_refresh(&lease_key)
                .map_err(|error| RefreshError::Refresh(error.to_string()))?,
            CredentialUseDisposition::AlreadyRefreshing => {
                return Err(RefreshError::Refresh(
                    "AuthMachine reports an MCP OAuth refresh already in flight".to_string(),
                ));
            }
            CredentialUseDisposition::ReauthRequired => {
                return Err(RefreshError::ReauthRequired(
                    "MCP OAuth credential requires reauthentication".to_string(),
                ));
            }
            CredentialUseDisposition::Authorized
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => {
                return Err(RefreshError::Refresh(format!(
                    "AuthMachine rejected MCP OAuth BeginRefresh admission: {begin_disposition:?}"
                )));
            }
        }
        let refreshing_snapshot = self.auth_lease.snapshot(&lease_key);

        let Some(refresh_token) = admitted.tokens.refresh_token.clone() else {
            let observation = meerkat_core::RefreshFailureObservation::local_credential_unusable();
            let disposition = self
                .close_refresh_failure(key, &lease_key, &observation)
                .await?;
            return Err(RefreshError::Classified {
                message: "stored MCP OAuth credential has no refresh token".to_string(),
                observation,
                disposition,
            });
        };
        let metadata = &admitted.metadata;
        let endpoints = OAuthEndpoints {
            client_id: metadata.client.client_id.clone(),
            authorize_url: metadata.discovery.authorization_endpoint.clone(),
            token_url: metadata.discovery.token_endpoint.clone(),
            device_code_url: None,
            redirect_uri: metadata.client.redirect_uri.clone(),
            scopes: metadata.discovery.scopes.clone(),
            extra_authorize_params: Vec::new(),
            token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
            include_state_in_token_exchange: false,
            extra_token_params: vec![("resource".to_string(), metadata.discovery.resource.clone())],
            refresh_scopes: metadata.discovery.scopes.clone(),
            extra_headers: Vec::new(),
        };
        let refreshed = match exchange_refresh_token(
            &self.http,
            &endpoints,
            &refresh_token,
            metadata.client.client_secret.as_deref(),
        )
        .await
        {
            Ok(refreshed) => refreshed,
            Err(error) => {
                let observation = oauth_refresh_observation(&error);
                let disposition = self
                    .close_refresh_failure(key, &lease_key, &observation)
                    .await?;
                return Err(RefreshError::Classified {
                    message: error.to_string(),
                    observation,
                    disposition,
                });
            }
        };
        let refreshed_at = Utc::now();
        let mut persisted = match persisted_tokens_from_result(
            &refreshed,
            &metadata.discovery,
            &metadata.client,
            target,
            refreshed_at,
        ) {
            Ok(persisted) => persisted,
            Err(error) => {
                let observation = meerkat_core::RefreshFailureObservation::transient();
                self.auth_lease
                    .refresh_failed(&lease_key, observation)
                    .map_err(|transition_error| {
                        RefreshError::Refresh(format!(
                            "{error}; AuthMachine refresh_failed rejected closure: {transition_error}"
                        ))
                    })?;
                return Err(RefreshError::Refresh(error.to_string()));
            }
        };
        if persisted.refresh_token.is_none() {
            persisted.refresh_token = Some(refresh_token);
        }

        let current_tokens = match self.token_store().load(key).await {
            Ok(tokens) => tokens,
            Err(error) => {
                let observation = meerkat_core::RefreshFailureObservation::transient();
                self.auth_lease
                    .refresh_failed(&lease_key, observation)
                    .map_err(|transition_error| {
                        RefreshError::Refresh(format!(
                            "{error}; AuthMachine refresh_failed rejected closure: {transition_error}"
                        ))
                    })?;
                return Err(RefreshError::Refresh(error.to_string()));
            }
        };
        let current_snapshot = self.auth_lease.snapshot(&lease_key);
        if current_tokens.as_ref() != Some(&admitted.tokens)
            || current_snapshot != refreshing_snapshot
        {
            let observation = meerkat_core::RefreshFailureObservation::transient();
            self.auth_lease
                .refresh_failed(&lease_key, observation)
                .map_err(|error| RefreshError::Refresh(error.to_string()))?;
            return Err(RefreshError::Refresh(
                "MCP OAuth token or AuthMachine lifecycle changed during refresh; stale result discarded"
                    .to_string(),
            ));
        }

        let transition = match self.auth_lease.complete_refresh(
            &lease_key,
            meerkat_core::persisted_token_expires_at_epoch_secs(&persisted),
            epoch_secs(refreshed_at),
        ) {
            Ok(transition) => transition,
            Err(error) => {
                let observation = meerkat_core::RefreshFailureObservation::transient();
                self.auth_lease
                    .refresh_failed(&lease_key, observation)
                    .map_err(|transition_error| {
                        RefreshError::Refresh(format!(
                            "{error}; AuthMachine refresh_failed rejected closure: {transition_error}"
                        ))
                    })?;
                return Err(RefreshError::Refresh(error.to_string()));
            }
        };
        let published = match meerkat_core::mark_tokens_lifecycle_published_for_transition(
            key,
            &persisted,
            &transition,
        ) {
            Ok(published) => published,
            Err(error) => {
                return Err(self
                    .rollback_refresh_publication(
                        key,
                        &lease_key,
                        &admitted.tokens,
                        &admitted.restore_snapshot,
                        format!("failed to mark refreshed MCP OAuth token: {error}"),
                    )
                    .await);
            }
        };
        if let Err(error) = self.token_store().save(key, &published).await {
            return Err(self
                .rollback_refresh_publication(
                    key,
                    &lease_key,
                    &admitted.tokens,
                    &admitted.restore_snapshot,
                    format!("failed to save refreshed MCP OAuth token: {error}"),
                )
                .await);
        }
        Ok(published)
    }

    /// Close every begun refresh through AuthMachine. Permanently unusable
    /// credentials are also removed from the durable store, so a new process
    /// cannot resurrect and retry bytes that the token endpoint rejected.
    async fn close_refresh_failure(
        &self,
        key: &TokenKey,
        lease_key: &LeaseKey,
        observation: &meerkat_core::RefreshFailureObservation,
    ) -> Result<RefreshFailureDisposition, RefreshError> {
        let disposition = self
            .auth_lease
            .resolve_refresh_failure_disposition(lease_key, observation.clone())
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        if disposition == RefreshFailureDisposition::ReauthRequired {
            // Durable terminality commits first. If the process dies after this
            // clear, a new authority observes no credential and cannot restore
            // the permanently rejected marker. The coordinator-owned task then
            // closes the in-memory Refreshing phase without a cancellation gap.
            if let Err(error) = self.token_store().clear(key).await {
                let closure = self
                    .auth_lease
                    .refresh_failed(lease_key, observation.clone())
                    .map_err(|transition_error| transition_error.to_string());
                let closure_suffix = closure
                    .err()
                    .map(|error| format!("; AuthMachine refresh closure also failed: {error}"))
                    .unwrap_or_default();
                return Err(RefreshError::DurableTerminalCommit {
                    message: format!(
                        "permanently rejected MCP OAuth credential removal failed: {error}{closure_suffix}"
                    ),
                    observation: observation.clone(),
                    disposition,
                });
            }
            if let Err(error) = self
                .auth_lease
                .refresh_failed(lease_key, observation.clone())
            {
                let reconciliation = self
                    .auth_lease
                    .release_credential_lifecycle(lease_key)
                    .err()
                    .map(|release_error| {
                        format!("; credential lifecycle release also failed: {release_error}")
                    })
                    .unwrap_or_default();
                return Err(RefreshError::Refresh(format!(
                    "durable credential was removed but AuthMachine refresh closure failed: {error}{reconciliation}"
                )));
            }
            return Ok(disposition);
        }
        self.auth_lease
            .refresh_failed(lease_key, observation.clone())
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        Ok(disposition)
    }

    async fn rollback_refresh_publication(
        &self,
        key: &TokenKey,
        lease_key: &LeaseKey,
        previous_tokens: &PersistedTokens,
        previous_snapshot: &AuthLeaseRestoreSnapshot,
        reason: String,
    ) -> RefreshError {
        let mut rollback_errors = Vec::new();
        if let Err(error) = self.auth_lease.release_credential_lifecycle(lease_key) {
            rollback_errors.push(format!("AuthMachine release failed: {error}"));
        }
        let mut restored_tokens = previous_tokens.clone();
        match meerkat_core::restore_token_lifecycle_snapshot(&self.auth_lease, previous_snapshot) {
            Ok(Some(transition)) => {
                match meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    key,
                    previous_tokens,
                    &transition,
                ) {
                    Ok(marked) => restored_tokens = marked,
                    Err(error) => rollback_errors.push(format!("marker restore failed: {error}")),
                }
            }
            Ok(None) => {}
            Err(error) => rollback_errors.push(format!("AuthMachine restore failed: {error}")),
        }
        if let Err(error) = self.token_store().save(key, &restored_tokens).await {
            rollback_errors.push(format!("TokenStore restore failed: {error}"));
        }
        let suffix = if rollback_errors.is_empty() {
            String::new()
        } else {
            format!("; {}", rollback_errors.join("; "))
        };
        RefreshError::Refresh(format!("{reason}{suffix}"))
    }

    async fn discover(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
        _redirect_uri: &str,
    ) -> Result<StoredMcpOAuthDiscovery, McpOAuthError> {
        require_https_or_loopback(target, target.server_url(), "MCP protected resource")?;
        let resource_metadata_url = match www_authenticate.and_then(resource_metadata_from_header) {
            Some(value) => absolutize_url(target.server_url(), &value).map_err(|error| {
                McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name().to_string(),
                    reason: error,
                }
            })?,
            None => {
                self.discover_resource_metadata_url_by_well_known(target)
                    .await?
            }
        };
        let resource: ProtectedResourceMetadata = self
            .http
            .get(resource_metadata_url.clone())
            .send()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .json()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: format!("decode protected resource metadata: {error}"),
            })?;
        if resource.resource != target.server_url() {
            return Err(McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: format!(
                    "protected resource metadata resource '{}' does not match MCP server '{}'",
                    resource.resource,
                    target.server_url()
                ),
            });
        }
        let auth_server = resource
            .authorization_servers
            .first()
            .cloned()
            .ok_or_else(|| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: "protected resource metadata has no authorization_servers".to_string(),
            })?;
        require_https_or_loopback(target, &auth_server, "authorization server issuer")?;
        let authorization_metadata_url = authorization_server_metadata_url(&auth_server)?;
        let auth: AuthorizationServerMetadata = self
            .http
            .get(&authorization_metadata_url)
            .send()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .json()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: format!("decode authorization server metadata: {error}"),
            })?;
        if auth.issuer != auth_server {
            return Err(McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: format!(
                    "authorization server metadata issuer '{}' does not match discovered issuer '{}'",
                    auth.issuer, auth_server
                ),
            });
        }
        let registration_endpoint =
            auth.registration_endpoint
                .clone()
                .ok_or_else(|| McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name().to_string(),
                    reason: "authorization server metadata has no registration_endpoint"
                        .to_string(),
                })?;
        let authorization_endpoint =
            absolutize_url(&authorization_metadata_url, &auth.authorization_endpoint).map_err(
                |reason| McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name().to_string(),
                    reason,
                },
            )?;
        let token_endpoint = absolutize_url(&authorization_metadata_url, &auth.token_endpoint)
            .map_err(|reason| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason,
            })?;
        let registration_endpoint =
            absolutize_url(&authorization_metadata_url, &registration_endpoint).map_err(
                |reason| McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name().to_string(),
                    reason,
                },
            )?;
        require_https_or_loopback(target, &authorization_endpoint, "authorization endpoint")?;
        require_https_or_loopback(target, &token_endpoint, "token endpoint")?;
        require_https_or_loopback(target, &registration_endpoint, "registration endpoint")?;
        Ok(StoredMcpOAuthDiscovery {
            resource: resource.resource,
            resource_metadata_url,
            authorization_server: auth_server,
            authorization_metadata_url,
            authorization_endpoint,
            token_endpoint,
            registration_endpoint,
            scopes: Vec::new(),
        })
    }

    async fn discover_resource_metadata_url_by_well_known(
        &self,
        target: &McpServerIdentity,
    ) -> Result<String, McpOAuthError> {
        for candidate in protected_resource_well_known_candidates(target.server_url())? {
            let response = self.http.get(&candidate).send().await;
            if let Ok(response) = response
                && response.status().is_success()
            {
                return Ok(candidate);
            }
        }
        Err(McpOAuthError::DiscoveryFailed {
            server_name: target.server_name().to_string(),
            reason: "no oauth-protected-resource metadata found".to_string(),
        })
    }

    async fn register_client(
        &self,
        target: &McpServerIdentity,
        discovery: &StoredMcpOAuthDiscovery,
        redirect_uri: &str,
    ) -> Result<StoredMcpOAuthClient, McpOAuthError> {
        let body = serde_json::json!({
            "client_name": CLIENT_NAME,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let wire: DynamicClientRegistrationResponse = self
            .http
            .post(&discovery.registration_endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .json()
            .await
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name().to_string(),
                reason: format!("decode: {error}"),
            })?;
        let token_endpoint_auth_method =
            wire.token_endpoint_auth_method
                .ok_or_else(|| McpOAuthError::RegistrationFailed {
                    server_name: target.server_name().to_string(),
                    reason:
                        "token_endpoint_auth_method missing; public PKCE clients require explicit 'none'"
                            .to_string(),
                })?;
        if token_endpoint_auth_method != "none" {
            return Err(McpOAuthError::RegistrationFailed {
                server_name: target.server_name().to_string(),
                reason: format!(
                    "unsupported token_endpoint_auth_method '{token_endpoint_auth_method}'"
                ),
            });
        }
        Ok(StoredMcpOAuthClient {
            client_id: wire.client_id,
            client_secret: None,
            token_endpoint_auth_method,
            redirect_uri: redirect_uri.to_string(),
        })
    }
}

fn stored_metadata_for_target(
    target: &McpServerIdentity,
    tokens: &PersistedTokens,
) -> Result<StoredMcpOAuthMetadata, McpOAuthError> {
    let metadata: StoredMcpOAuthMetadata = serde_json::from_value(tokens.metadata.clone())
        .map_err(|_| McpOAuthError::MissingStoredMetadata {
            server_name: target.server_name().to_string(),
        })?;
    if metadata.server_name != target.server_name() || metadata.server_url != target.server_url() {
        return Err(McpOAuthError::ReauthRequired {
            server_name: target.server_name().to_string(),
        });
    }
    Ok(metadata)
}

fn persisted_tokens_from_result(
    result: &OAuthTokenResult,
    discovery: &StoredMcpOAuthDiscovery,
    client: &StoredMcpOAuthClient,
    target: &McpServerIdentity,
    now: DateTime<Utc>,
) -> Result<PersistedTokens, McpOAuthError> {
    let expires_at =
        result
            .expires_at_from(now)
            .map_err(|error| McpOAuthError::TokenExchangeFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?;
    let scopes = result
        .scope
        .as_deref()
        .map(|scope| scope.split_whitespace().map(str::to_string).collect())
        .unwrap_or_else(|| discovery.scopes.clone());
    let metadata = StoredMcpOAuthMetadata {
        server_name: target.server_name().to_string(),
        server_url: target.server_url().to_string(),
        discovery: discovery.clone(),
        client: client.clone(),
    };
    Ok(PersistedTokens {
        auth_mode: PersistedAuthMode::McpOauth,
        primary_secret: Some(result.access_token.clone()),
        refresh_token: result.refresh_token.clone(),
        id_token: result.id_token.clone(),
        expires_at,
        last_refresh: Some(now),
        scopes,
        account_id: Some(target.server_name().to_string()),
        metadata: serde_json::to_value(metadata).map_err(|error| {
            McpOAuthError::TokenExchangeFailed {
                server_name: target.server_name().to_string(),
                reason: format!("failed to serialize token metadata: {error}"),
            }
        })?,
    })
}

fn map_oauth_exchange_error(target: &McpServerIdentity, error: OAuthError) -> McpOAuthError {
    McpOAuthError::TokenExchangeFailed {
        server_name: target.server_name().to_string(),
        reason: error.to_string(),
    }
}

fn map_coordinated_login_error(
    target: &McpServerIdentity,
    error: CredentialMutationError,
) -> McpOAuthError {
    match error {
        CredentialMutationError::TokenStore(reason) => McpOAuthError::TokenStore(reason),
        CredentialMutationError::AuthLifecycle(reason)
        | CredentialMutationError::Operation(reason)
        | CredentialMutationError::LockFailed(reason) => McpOAuthError::AuthLifecycle {
            server_name: target.server_name().to_string(),
            reason,
        },
        CredentialMutationError::Cancelled => McpOAuthError::AuthLifecycle {
            server_name: target.server_name().to_string(),
            reason: "credential mutation coordinator cancelled the login commit".to_string(),
        },
    }
}

fn lifecycle_snapshot_is_absent(snapshot: &AuthLeaseSnapshot) -> bool {
    snapshot.phase.is_none()
        && !snapshot.credential_present
        && snapshot.generation == 0
        && snapshot.credential_published_at_millis.is_none()
}

fn rollback_error_suffix(errors: &[String]) -> String {
    if errors.is_empty() {
        "; previous credential and lifecycle restored".to_string()
    } else {
        format!("; rollback faults: {}", errors.join("; "))
    }
}

fn refresh_error_from_mcp(error: McpOAuthError) -> RefreshError {
    match error {
        McpOAuthError::ReauthRequired { .. }
        | McpOAuthError::MissingStoredToken { .. }
        | McpOAuthError::MissingStoredMetadata { .. } => {
            RefreshError::ReauthRequired(error.to_string())
        }
        other => RefreshError::Refresh(other.to_string()),
    }
}

fn map_coordinated_refresh_error(target: &McpServerIdentity, error: RefreshError) -> McpOAuthError {
    if let RefreshError::DurableTerminalCommit { message, .. } = &error {
        return McpOAuthError::TokenStore(message.clone());
    }
    if matches!(&error, RefreshError::ReauthRequired(_))
        || error.refresh_failure_disposition() == Some(RefreshFailureDisposition::ReauthRequired)
    {
        McpOAuthError::ReauthRequired {
            server_name: target.server_name().to_string(),
        }
    } else {
        McpOAuthError::RefreshFailed {
            server_name: target.server_name().to_string(),
            reason: error.to_string(),
        }
    }
}

/// Whole seconds since the Unix epoch, clamped to a non-negative `u64` for the
/// `AuthMachine` lease lifecycle inputs.
fn epoch_secs(time: DateTime<Utc>) -> u64 {
    time.timestamp().max(0) as u64
}

fn resource_metadata_from_header(header: &str) -> Option<String> {
    auth_param(header, "resource_metadata")
}

fn auth_param(header: &str, key: &str) -> Option<String> {
    for segment in header.split(',') {
        let segment = segment.trim();
        let Some((name, raw_value)) = segment.split_once('=') else {
            continue;
        };
        let name = name.split_whitespace().last().unwrap_or(name).trim();
        if name != key {
            continue;
        }
        return unquote_auth_value(raw_value.trim());
    }
    None
}

fn unquote_auth_value(raw: &str) -> Option<String> {
    if let Some(body) = raw.strip_prefix('"') {
        let mut out = String::new();
        let mut escaped = false;
        for ch in body.chars() {
            if escaped {
                out.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return Some(out);
            } else {
                out.push(ch);
            }
        }
        None
    } else {
        Some(raw.trim_end_matches(';').to_string())
    }
}

fn absolutize_url(base: &str, value: &str) -> Result<String, String> {
    let base = reqwest::Url::parse(base).map_err(|error| error.to_string())?;
    base.join(value)
        .map(|url| url.to_string())
        .map_err(|error| error.to_string())
}

fn protected_resource_well_known_candidates(
    server_url: &str,
) -> Result<Vec<String>, McpOAuthError> {
    let url = reqwest::Url::parse(server_url).map_err(|error| McpOAuthError::DiscoveryFailed {
        server_name: "unknown".to_string(),
        reason: error.to_string(),
    })?;
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_matches('/');
    let mut candidates = Vec::new();
    if !path.is_empty() {
        candidates.push(format!(
            "{origin}/.well-known/oauth-protected-resource/{path}"
        ));
        candidates.push(format!(
            "{origin}/{path}/.well-known/oauth-protected-resource"
        ));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    Ok(candidates)
}

fn authorization_server_metadata_url(server: &str) -> Result<String, McpOAuthError> {
    let url = reqwest::Url::parse(server).map_err(|error| McpOAuthError::DiscoveryFailed {
        server_name: "unknown".to_string(),
        reason: error.to_string(),
    })?;
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_matches('/');
    if path.is_empty() {
        Ok(format!("{origin}/.well-known/oauth-authorization-server"))
    } else {
        Ok(format!(
            "{origin}/.well-known/oauth-authorization-server/{path}"
        ))
    }
}

fn require_https_or_loopback(
    target: &McpServerIdentity,
    value: &str,
    label: &str,
) -> Result<(), McpOAuthError> {
    let url = reqwest::Url::parse(value).map_err(|error| McpOAuthError::DiscoveryFailed {
        server_name: target.server_name().to_string(),
        reason: format!("invalid {label}: {error}"),
    })?;
    if url.scheme() == "https" || is_loopback_url(&url) {
        return Ok(());
    }
    Err(McpOAuthError::DiscoveryFailed {
        server_name: target.server_name().to_string(),
        reason: format!("{label} must use https"),
    })
}

fn is_loopback_url(url: &reqwest::Url) -> bool {
    matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    )
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DynamicClientRegistrationResponse {
    client_id: String,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMcpOAuthDiscovery {
    resource: String,
    resource_metadata_url: String,
    authorization_server: String,
    authorization_metadata_url: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMcpOAuthClient {
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    token_endpoint_auth_method: String,
    redirect_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMcpOAuthMetadata {
    server_name: String,
    server_url: String,
    discovery: StoredMcpOAuthDiscovery,
    client: StoredMcpOAuthClient,
}

struct AdmittedMcpCredential {
    tokens: PersistedTokens,
    metadata: StoredMcpOAuthMetadata,
    restore_snapshot: AuthLeaseRestoreSnapshot,
    disposition: CredentialUseDisposition,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{EphemeralTokenStore, InMemoryCoordinator};
    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Redirect};
    use axum::routing::{get, post};
    use axum::{Form, Json, Router};
    use parking_lot::Mutex;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    /// A certified `AuthMachine` lease handle for tests. `meerkat-runtime` is a
    /// dev-dependency, so tests can mint the same generated lease the CLI
    /// injects in production (the lib cannot, to avoid a dep cycle).
    fn test_auth_lease() -> GeneratedAuthLeaseHandle {
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            handle,
        )
        .expect("test auth lease must be certified by generated AuthMachine authority")
    }

    #[test]
    fn coordinated_refresh_mapping_uses_machine_disposition_not_raw_observation() {
        let target = McpServerIdentity::from_server_config("glean", "https://mcp.example.test/mcp");
        let observation = meerkat_core::RefreshFailureObservation::local_credential_unusable();

        let unclassified = map_coordinated_refresh_error(
            &target,
            RefreshError::Observed {
                message: "unclassified boundary observation".to_string(),
                observation: observation.clone(),
            },
        );
        assert!(matches!(unclassified, McpOAuthError::RefreshFailed { .. }));

        let classified = map_coordinated_refresh_error(
            &target,
            RefreshError::Classified {
                message: "AuthMachine classified terminal failure".to_string(),
                observation,
                disposition: RefreshFailureDisposition::ReauthRequired,
            },
        );
        assert!(matches!(classified, McpOAuthError::ReauthRequired { .. }));
    }

    #[derive(Debug, Clone, Copy, Default)]
    enum AuthorizeOutcome {
        #[default]
        Success,
        StateMismatch,
        Denied,
        NoCallback,
    }

    #[derive(Default)]
    struct TestState {
        opened_url: Mutex<Option<String>>,
        redirect_uri: Mutex<Option<String>>,
        registration_requests: Mutex<Vec<Value>>,
        token_requests: Mutex<Vec<Value>>,
        include_registration_endpoint: Mutex<bool>,
        omit_token_endpoint_auth_method: Mutex<bool>,
        resource_override: Mutex<Option<String>>,
        issuer_override: Mutex<Option<String>>,
        authorize_outcome: Mutex<AuthorizeOutcome>,
        token_fails: Mutex<bool>,
        token_transiently_fails: Mutex<bool>,
        pause_refresh: AtomicBool,
        refresh_started: Notify,
        refresh_release: Notify,
    }

    struct RecordingBrowser {
        state: Arc<TestState>,
        http: Client,
    }

    #[async_trait]
    impl BrowserOpener for RecordingBrowser {
        async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
            *self.state.opened_url.lock() = Some(url.to_string());
            let _response = self.http.get(url).send().await.unwrap();
            Ok(())
        }
    }

    struct NoCallbackBrowser {
        state: Arc<TestState>,
    }

    struct FailNextSaveStore {
        inner: Arc<EphemeralTokenStore>,
        fail_next_save: AtomicBool,
    }

    struct FailNextSaveDynStore {
        inner: Arc<dyn TokenStore>,
        fail_next_save: AtomicBool,
    }

    struct FailClearStore {
        inner: Arc<EphemeralTokenStore>,
    }

    #[async_trait]
    impl TokenStore for FailClearStore {
        async fn load(
            &self,
            key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, crate::auth_store::TokenStoreError> {
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), crate::auth_store::TokenStoreError> {
            self.inner.save(key, tokens).await
        }

        async fn clear(&self, _key: &TokenKey) -> Result<(), crate::auth_store::TokenStoreError> {
            Err(crate::auth_store::TokenStoreError::Io(
                "injected clear failure".to_string(),
            ))
        }

        async fn list(&self) -> Result<Vec<TokenKey>, crate::auth_store::TokenStoreError> {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "fail-clear"
        }
    }

    #[async_trait]
    impl TokenStore for FailNextSaveDynStore {
        async fn load(
            &self,
            key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, crate::auth_store::TokenStoreError> {
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), crate::auth_store::TokenStoreError> {
            if self.fail_next_save.swap(false, Ordering::SeqCst) {
                return Err(crate::auth_store::TokenStoreError::Io(
                    "injected save failure".to_string(),
                ));
            }
            self.inner.save(key, tokens).await
        }

        async fn clear(&self, key: &TokenKey) -> Result<(), crate::auth_store::TokenStoreError> {
            self.inner.clear(key).await
        }

        async fn list(&self) -> Result<Vec<TokenKey>, crate::auth_store::TokenStoreError> {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "fail-next-save-dyn"
        }
    }

    #[async_trait]
    impl TokenStore for FailNextSaveStore {
        async fn load(
            &self,
            key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, crate::auth_store::TokenStoreError> {
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), crate::auth_store::TokenStoreError> {
            if self.fail_next_save.swap(false, Ordering::SeqCst) {
                return Err(crate::auth_store::TokenStoreError::Io(
                    "injected save failure".to_string(),
                ));
            }
            self.inner.save(key, tokens).await
        }

        async fn clear(&self, key: &TokenKey) -> Result<(), crate::auth_store::TokenStoreError> {
            self.inner.clear(key).await
        }

        async fn list(&self) -> Result<Vec<TokenKey>, crate::auth_store::TokenStoreError> {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "fail-next-save"
        }
    }

    #[async_trait]
    impl BrowserOpener for NoCallbackBrowser {
        async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
            *self.state.opened_url.lock() = Some(url.to_string());
            Ok(())
        }
    }

    async fn spawn_oauth_fixture() -> (String, Arc<TestState>) {
        let state = Arc::new(TestState {
            include_registration_endpoint: Mutex::new(true),
            ..TestState::default()
        });
        let app = Router::new()
            .route("/mcp", post(mcp_endpoint))
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(protected_resource),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(authorization_metadata),
            )
            .route("/register", post(register_client))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(Arc::clone(&state));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), state)
    }

    async fn mcp_endpoint(headers: HeaderMap) -> impl IntoResponse {
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some("Bearer access-token")
        {
            return (
                StatusCode::OK,
                Json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}})),
            )
                .into_response();
        }
        (
            StatusCode::UNAUTHORIZED,
            [(
                "www-authenticate",
                r#"Bearer error="invalid_request", resource_metadata="/.well-known/oauth-protected-resource/mcp""#,
            )],
            "",
        )
            .into_response()
    }

    async fn protected_resource(
        State(state): State<Arc<TestState>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let host = headers
            .get("host")
            .and_then(|value| value.to_str().ok())
            .unwrap();
        let resource = state
            .resource_override
            .lock()
            .clone()
            .unwrap_or_else(|| format!("http://{host}/mcp"));
        Json(serde_json::json!({
            "resource": resource,
            "authorization_servers": [format!("http://{host}")],
            "scopes_supported": ["mcp.read"]
        }))
    }

    async fn authorization_metadata(
        State(state): State<Arc<TestState>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let host = headers
            .get("host")
            .and_then(|value| value.to_str().ok())
            .unwrap();
        let issuer = state
            .issuer_override
            .lock()
            .clone()
            .unwrap_or_else(|| format!("http://{host}"));
        let mut body = serde_json::json!({
            "issuer": issuer,
            "authorization_endpoint": "/authorize",
            "token_endpoint": "/token",
        });
        if *state.include_registration_endpoint.lock() {
            body["registration_endpoint"] = serde_json::json!("/register");
        }
        Json(body)
    }

    async fn register_client(
        State(state): State<Arc<TestState>>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        let redirect_uri = body["redirect_uris"][0].as_str().unwrap().to_string();
        state.registration_requests.lock().push(body);
        *state.redirect_uri.lock() = Some(redirect_uri);
        Json(serde_json::json!({
            "client_id": "client-123",
            "client_secret": "ignored-secret-for-public-client",
            "token_endpoint_auth_method": if *state.omit_token_endpoint_auth_method.lock() {
                Value::Null
            } else {
                Value::String("none".to_string())
            }
        }))
    }

    async fn authorize(
        State(state): State<Arc<TestState>>,
        Query(params): Query<HashMap<String, String>>,
    ) -> impl IntoResponse {
        let redirect_uri = state.redirect_uri.lock().clone().unwrap();
        let state_param = params.get("state").unwrap();
        match *state.authorize_outcome.lock() {
            AuthorizeOutcome::Success => Redirect::temporary(&format!(
                "{redirect_uri}?code=fixture-code&state={state_param}"
            ))
            .into_response(),
            AuthorizeOutcome::StateMismatch => {
                Redirect::temporary(&format!("{redirect_uri}?code=fixture-code&state=wrong"))
                    .into_response()
            }
            AuthorizeOutcome::Denied => Redirect::temporary(&format!(
                "{redirect_uri}?error=access_denied&state={state_param}"
            ))
            .into_response(),
            AuthorizeOutcome::NoCallback => {
                (StatusCode::OK, "authorization left pending").into_response()
            }
        }
    }

    async fn token(
        State(state): State<Arc<TestState>>,
        Form(body): Form<HashMap<String, String>>,
    ) -> impl IntoResponse {
        state
            .token_requests
            .lock()
            .push(serde_json::to_value(&body).unwrap());
        if body.get("grant_type").map(String::as_str) == Some("refresh_token")
            && state.pause_refresh.load(Ordering::SeqCst)
        {
            state.refresh_started.notify_one();
            state.refresh_release.notified().await;
        }
        if *state.token_fails.lock() {
            // RFC 6749 §5.2 error response: a JSON body carrying the typed
            // `error` code. The refresh-failure classifier parses this `error`
            // field to decide reauth-vs-retry.
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid_grant" })),
            )
                .into_response();
        }
        if *state.token_transiently_fails.lock() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "temporarily_unavailable" })),
            )
                .into_response();
        }
        Json(serde_json::json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "scope": "mcp.read"
        }))
        .into_response()
    }

    #[test]
    fn server_identity_key_is_typed_and_stable() {
        let a =
            McpServerIdentity::from_server_config("glean", "https://king-be.glean.com/mcp/default")
                .token_key()
                .unwrap();
        let b =
            McpServerIdentity::from_server_config("glean", "https://king-be.glean.com/mcp/default")
                .token_key()
                .unwrap();
        let c = McpServerIdentity::from_server_config("glean", "https://other.example/mcp/default")
            .token_key()
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.realm.as_str(), MCP_TOKEN_REALM);
    }

    #[test]
    fn server_identity_is_typed_and_owns_key_derivation() {
        // Row #349 gate (1): realm-key derivation flows through the typed
        // `McpServerIdentity`, and the token key and the AuthMachine lease key
        // share one identity derivation (structurally identical binding).
        let identity =
            McpServerIdentity::from_server_config("glean", "https://king-be.glean.com/mcp/default");
        let token_key = identity.token_key().unwrap();
        let lease_key = identity.lease_key().unwrap();
        assert_eq!(token_key.realm.as_str(), MCP_TOKEN_REALM);
        assert_eq!(lease_key.realm.as_str(), MCP_TOKEN_REALM);
        assert_eq!(
            token_key.binding.as_str(),
            lease_key.binding.as_str(),
            "token key and lease key must share one typed identity binding"
        );
        // Distinct server URL yields a distinct identity binding.
        let other =
            McpServerIdentity::from_server_config("glean", "https://other.example/mcp/default");
        assert_ne!(
            other.token_key().unwrap().binding.as_str(),
            token_key.binding.as_str()
        );
    }

    #[tokio::test]
    async fn interactive_login_token_write_is_lease_published() {
        // Row #349 gate (2): the post-login durable token write carries the
        // AuthMachine lease lifecycle marker — the credential write is owned by
        // a committed lease transition, not a bare store.save.
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        authority
            .interactive_login(&target, None)
            .await
            .expect("login succeeds");

        let stored = store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert!(
            meerkat_core::tokens_lifecycle_published(&stored),
            "post-login token write must be wrapped in an AuthMachine lease transition"
        );
        // The pre-existing MCP metadata survives alongside the marker.
        assert_eq!(stored.metadata["client"]["client_id"], "client-123");
    }

    #[tokio::test]
    async fn refreshed_token_write_is_lease_published() {
        // The refresh-success persist mirrors the login path: refreshed
        // tokens carry the AuthMachine lease lifecycle marker stamped at the
        // refreshed credential's own expiry — never a bare store.save that
        // drops the lifecycle-published marker.
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        authority
            .interactive_login(&target, None)
            .await
            .expect("initial login succeeds");
        let key = target.token_key().unwrap();
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;

        let token = authority
            .stored_bearer_token(&target)
            .await
            .expect("expired token refreshes")
            .expect("refresh yields a bearer token");
        assert_eq!(token, "access-token");

        let refreshed = store.load(&key).await.unwrap().unwrap();
        assert!(
            meerkat_core::tokens_lifecycle_published(&refreshed),
            "post-refresh token write must be wrapped in an AuthMachine lease transition"
        );
        // The marker is stamped against the refreshed credential's expiry.
        let publication = meerkat_core::tokens_lifecycle_publication(&refreshed)
            .expect("refreshed tokens carry a lifecycle publication");
        assert_eq!(
            publication.expires_at,
            meerkat_core::persisted_token_expires_at_epoch_secs(&refreshed),
            "lifecycle marker expiry must match the refreshed token expiry"
        );
        // MCP metadata survives the refresh publish.
        assert_eq!(refreshed.metadata["client"]["client_id"], "client-123");
    }

    #[tokio::test]
    async fn unmarked_non_expiring_stored_token_is_dead_data() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        authority.interactive_login(&target, None).await.unwrap();
        let key = target.token_key().unwrap();
        let mut stored = store.load(&key).await.unwrap().unwrap();
        let metadata = stored_metadata_for_target(&target, &stored).unwrap();
        stored.expires_at = None;
        stored.metadata = serde_json::to_value(metadata).unwrap();
        store.save(&key, &stored).await.unwrap();
        auth_lease
            .release_lease(&target.lease_key().unwrap())
            .unwrap();

        let error = authority
            .stored_bearer_token(&target)
            .await
            .expect_err("unmarked persisted bytes must never be admitted");
        assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
        assert_eq!(
            state.token_requests.lock().len(),
            1,
            "dead stored bytes must not reach the refresh endpoint"
        );
    }

    #[tokio::test]
    async fn marked_non_expiring_stored_token_restores_and_is_admitted() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        authority.interactive_login(&target, None).await.unwrap();
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = None;
        })
        .await;
        let lease_key = target.lease_key().unwrap();
        let stored = store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert!(durable_marker::marker_payload_valid_for_tokens(
            &stored,
            &target.token_key().unwrap()
        ));
        let restored_auth_lease = test_auth_lease();
        let restored_authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            restored_auth_lease.clone(),
        );

        let token = restored_authority
            .stored_bearer_token(&target)
            .await
            .unwrap()
            .expect("marked token is present");
        assert_eq!(token, "access-token");
        let snapshot = restored_auth_lease.snapshot(&lease_key);
        assert_eq!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid)
        );
        assert!(snapshot.credential_present);
        assert_eq!(state.token_requests.lock().len(), 1);
    }

    #[tokio::test]
    async fn concurrent_expired_reads_share_one_refresh_transaction() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        authority.interactive_login(&target, None).await.unwrap();
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;

        let mut reads = Vec::new();
        for _ in 0..8 {
            let authority = authority.clone();
            let target = target.clone();
            reads.push(tokio::spawn(async move {
                authority.stored_bearer_token(&target).await
            }));
        }
        for read in reads {
            assert_eq!(
                read.await.unwrap().unwrap().as_deref(),
                Some("access-token")
            );
        }
        let requests = state.token_requests.lock();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["grant_type"] == "refresh_token")
                .count(),
            1,
            "same-key readers must share one OAuth refresh"
        );
    }

    #[cfg(feature = "file-lock")]
    #[tokio::test]
    async fn interactive_login_commit_waits_for_cross_process_refresh_transaction() {
        use crate::auth_store::{FileLockCoordinator, FileTokenStore};

        let (base, state) = spawn_oauth_fixture().await;
        let temp = tempfile::tempdir().unwrap();
        let lock_dir = temp.path().join("locks");
        let store: Arc<dyn TokenStore> =
            Arc::new(FileTokenStore::new(temp.path().join("credentials")));
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        let seed = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(
                Arc::clone(&store),
                Arc::new(FileLockCoordinator::new(lock_dir.clone())),
            ),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            test_auth_lease(),
        );
        seed.interactive_login(&target, None).await.unwrap();
        republish_stored_tokens(&seed, store.as_ref(), &target, |tokens| {
            tokens.primary_secret = Some("stale-access-token".to_string());
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;

        state.pause_refresh.store(true, Ordering::SeqCst);
        let refresh = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(
                Arc::clone(&store),
                Arc::new(FileLockCoordinator::new(lock_dir.clone())),
            ),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            test_auth_lease(),
        );
        let refresh_target = target.clone();
        let refresh_task =
            tokio::spawn(async move { refresh.stored_bearer_token(&refresh_target).await });
        state.refresh_started.notified().await;

        let login_lease = test_auth_lease();
        let login = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(
                Arc::clone(&store),
                Arc::new(FileLockCoordinator::new(lock_dir)),
            ),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            login_lease.clone(),
        );
        let login_target = target.clone();
        let mut login_task =
            tokio::spawn(async move { login.interactive_login(&login_target, None).await });
        wait_for_token_grant_count(&state, "authorization_code", 2).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut login_task)
                .await
                .is_err(),
            "login commit must wait behind the refresh mutation lock"
        );

        state.refresh_release.notify_one();
        assert_eq!(
            refresh_task.await.unwrap().unwrap().as_deref(),
            Some("access-token")
        );
        assert_eq!(login_task.await.unwrap().unwrap(), "access-token");

        let key = target.token_key().unwrap();
        let committed = store.load(&key).await.unwrap().unwrap();
        assert_eq!(committed.primary_secret.as_deref(), Some("access-token"));
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &committed,
                &login_lease.snapshot(&target.lease_key().unwrap()),
                &key,
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches
        );
    }

    #[cfg(feature = "file-lock")]
    #[tokio::test]
    async fn failed_login_after_paused_refresh_restores_refresh_winner() {
        use crate::auth_store::{FileLockCoordinator, FileTokenStore};

        let (base, state) = spawn_oauth_fixture().await;
        let temp = tempfile::tempdir().unwrap();
        let lock_dir = temp.path().join("locks");
        let durable_store: Arc<dyn TokenStore> =
            Arc::new(FileTokenStore::new(temp.path().join("credentials")));
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        let seed = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(
                Arc::clone(&durable_store),
                Arc::new(FileLockCoordinator::new(lock_dir.clone())),
            ),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            test_auth_lease(),
        );
        seed.interactive_login(&target, None).await.unwrap();
        republish_stored_tokens(&seed, durable_store.as_ref(), &target, |tokens| {
            tokens.primary_secret = Some("stale-access-token".to_string());
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;

        state.pause_refresh.store(true, Ordering::SeqCst);
        let refresh = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(
                Arc::clone(&durable_store),
                Arc::new(FileLockCoordinator::new(lock_dir.clone())),
            ),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            test_auth_lease(),
        );
        let refresh_target = target.clone();
        let refresh_task =
            tokio::spawn(async move { refresh.stored_bearer_token(&refresh_target).await });
        state.refresh_started.notified().await;

        let failing_store = Arc::new(FailNextSaveDynStore {
            inner: Arc::clone(&durable_store),
            fail_next_save: AtomicBool::new(true),
        });
        let login_lease = test_auth_lease();
        let login = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(
                failing_store,
                Arc::new(FileLockCoordinator::new(lock_dir)),
            ),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            login_lease.clone(),
        );
        let login_target = target.clone();
        let mut login_task =
            tokio::spawn(async move { login.interactive_login(&login_target, None).await });
        wait_for_token_grant_count(&state, "authorization_code", 2).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut login_task)
                .await
                .is_err(),
            "failed login transaction must still wait behind refresh"
        );

        state.refresh_release.notify_one();
        assert_eq!(
            refresh_task.await.unwrap().unwrap().as_deref(),
            Some("access-token")
        );
        assert!(matches!(
            login_task.await.unwrap(),
            Err(McpOAuthError::TokenStore(_))
        ));

        let key = target.token_key().unwrap();
        let restored = durable_store.load(&key).await.unwrap().unwrap();
        assert_eq!(
            restored.primary_secret.as_deref(),
            Some("access-token"),
            "login rollback must not restore the stale pre-refresh credential"
        );
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(
                &restored,
                &login_lease.snapshot(&target.lease_key().unwrap()),
                &key,
            ),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches
        );
    }

    #[tokio::test]
    async fn refresh_save_failure_restores_previous_token_and_machine_snapshot() {
        let (base, state) = spawn_oauth_fixture().await;
        let inner = Arc::new(EphemeralTokenStore::new());
        let store = Arc::new(FailNextSaveStore {
            inner: Arc::clone(&inner),
            fail_next_save: AtomicBool::new(false),
        });
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        authority.interactive_login(&target, None).await.unwrap();
        let previous = republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        store.fail_next_save.store(true, Ordering::SeqCst);

        let error = authority
            .stored_bearer_token(&target)
            .await
            .expect_err("injected refresh commit failure must surface");
        assert!(matches!(error, McpOAuthError::RefreshFailed { .. }));
        let key = target.token_key().unwrap();
        let restored = inner.load(&key).await.unwrap().unwrap();
        assert_eq!(restored.primary_secret, previous.primary_secret);
        assert_eq!(restored.expires_at, previous.expires_at);
        let snapshot = auth_lease.snapshot(&target.lease_key().unwrap());
        assert_ne!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Refreshing),
            "failed persistence must not strand AuthMachine in Refreshing"
        );
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(&restored, &snapshot, &key),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches,
            "compensation must restore token and machine as one lifecycle publication"
        );
    }

    #[tokio::test]
    async fn interactive_relogin_save_failure_restores_previous_token_and_machine_snapshot() {
        let (base, state) = spawn_oauth_fixture().await;
        let inner = Arc::new(EphemeralTokenStore::new());
        let store = Arc::new(FailNextSaveStore {
            inner: Arc::clone(&inner),
            fail_next_save: AtomicBool::new(false),
        });
        let seed_authority = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        seed_authority
            .interactive_login(&target, None)
            .await
            .unwrap();
        let previous =
            republish_stored_tokens(&seed_authority, store.as_ref(), &target, |tokens| {
                tokens.primary_secret = Some("previous-access-token".to_string());
                tokens.refresh_token = Some("previous-refresh-token".to_string());
            })
            .await;
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            auth_lease.clone(),
        );
        store.fail_next_save.store(true, Ordering::SeqCst);

        let error = authority
            .interactive_login(&target, None)
            .await
            .expect_err("injected relogin commit failure must surface");
        assert!(matches!(&error, McpOAuthError::TokenStore(_)));

        let key = target.token_key().unwrap();
        let restored = inner.load(&key).await.unwrap().unwrap();
        assert_eq!(restored.primary_secret, previous.primary_secret);
        assert_eq!(restored.refresh_token, previous.refresh_token);
        assert_eq!(restored.expires_at, previous.expires_at);
        assert_eq!(restored.metadata["client"], previous.metadata["client"]);
        let snapshot = auth_lease.snapshot(&target.lease_key().unwrap());
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(&restored, &snapshot, &key),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches,
            "failed relogin must restore one admitted token/lifecycle publication"
        );
        assert_eq!(
            authority
                .stored_bearer_token(&target)
                .await
                .unwrap()
                .as_deref(),
            Some("previous-access-token"),
            "the previously usable credential must remain usable after rollback"
        );
    }

    #[test]
    fn parses_resource_metadata_from_www_authenticate() {
        let header = r#"Bearer error="invalid_request", resource_metadata="/.well-known/oauth-protected-resource/mcp""#;
        assert_eq!(
            resource_metadata_from_header(header).as_deref(),
            Some("/.well-known/oauth-protected-resource/mcp")
        );
    }

    fn recording_browser(state: Arc<TestState>) -> Arc<dyn BrowserOpener> {
        Arc::new(RecordingBrowser {
            state,
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .unwrap(),
        })
    }

    async fn wait_for_token_grant_count(state: &TestState, grant_type: &str, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let observed = state
                    .token_requests
                    .lock()
                    .iter()
                    .filter(|request| request["grant_type"] == grant_type)
                    .count();
                if observed >= expected {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("expected OAuth token request was not observed");
    }

    async fn republish_stored_tokens(
        authority: &McpOAuthAuthority,
        store: &dyn TokenStore,
        target: &McpServerIdentity,
        mutate: impl FnOnce(&mut PersistedTokens),
    ) -> PersistedTokens {
        let key = target.token_key().unwrap();
        let mut tokens = store.load(&key).await.unwrap().unwrap();
        mutate(&mut tokens);
        let published = authority
            .publish_login_tokens_via_lease(target, &key, &tokens)
            .unwrap();
        store.save(&key, &published).await.unwrap();
        published
    }

    async fn assert_login_fails_closed(
        state: Arc<TestState>,
        store: Arc<EphemeralTokenStore>,
        target: &McpServerIdentity,
        result: Result<String, McpOAuthError>,
        expected_reason: &str,
    ) {
        let err = result.expect_err("login should fail");
        assert!(
            err.to_string().contains(expected_reason),
            "expected {expected_reason:?} in error, got {err}"
        );
        assert!(
            store
                .load(&target.token_key().unwrap())
                .await
                .unwrap()
                .is_none(),
            "failed login must not persist MCP OAuth tokens"
        );
        assert!(
            state.token_requests.lock().len() <= 1,
            "failed login should not retry token exchange unexpectedly"
        );
    }

    #[tokio::test]
    async fn interactive_login_discovers_registers_exchanges_and_stores_token() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        let token = authority
            .interactive_login(
                &target,
                Some(r#"Bearer resource_metadata="/.well-known/oauth-protected-resource/mcp""#),
            )
            .await
            .expect("login succeeds");
        assert_eq!(token, "access-token");
        let stored = store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.auth_mode, PersistedAuthMode::McpOauth);
        assert_eq!(stored.primary_secret.as_deref(), Some("access-token"));
        assert!(stored.metadata["client"]["client_id"] == "client-123");
        assert!(
            stored.metadata["client"].get("client_secret").is_none(),
            "MCP DCR requests a public PKCE client and must not persist a DCR secret"
        );
        assert_eq!(
            stored.metadata["client"]["token_endpoint_auth_method"],
            "none"
        );
        assert_eq!(
            stored.metadata["discovery"]["resource"],
            format!("{base}/mcp")
        );

        let opened_url = state.opened_url.lock().clone().unwrap();
        assert!(
            opened_url.contains("resource="),
            "authorize URL should carry an OAuth resource indicator"
        );
        assert!(
            !opened_url.contains("scope="),
            "MCP OAuth v1 should not request every advertised supported scope"
        );
        let registration = state.registration_requests.lock();
        assert_eq!(
            registration[0]["token_endpoint_auth_method"], "none",
            "DCR should explicitly request a public token endpoint auth method"
        );
        let token_requests = state.token_requests.lock();
        assert_eq!(
            token_requests[0]["resource"],
            format!("{base}/mcp"),
            "authorization-code exchange should carry the MCP resource indicator"
        );
        assert!(
            token_requests[0].get("client_secret").is_none(),
            "public-client token exchange must not send a DCR secret"
        );
        assert!(
            token_requests[0].get("scope").is_none(),
            "authorization-code exchange should not request every advertised supported scope"
        );
    }

    #[tokio::test]
    async fn interactive_login_uses_well_known_fallback_without_challenge_header() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let token = authority
            .interactive_login(&target, None)
            .await
            .expect("well-known discovery fallback should login");

        assert_eq!(token, "access-token");
        let stored = store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.metadata["discovery"]["resource_metadata_url"],
            format!("{base}/.well-known/oauth-protected-resource/mcp")
        );
    }

    #[tokio::test]
    async fn stored_token_refresh_uses_resource_and_invalid_grant_requires_reauth() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        authority
            .interactive_login(&target, None)
            .await
            .expect("initial login succeeds");
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        *state.token_fails.lock() = true;

        let error = authority
            .stored_bearer_token(&target)
            .await
            .expect_err("invalid refresh grant should require reauth");

        assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
        assert_eq!(
            auth_lease.snapshot(&target.lease_key().unwrap()).phase,
            Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired),
            "invalid_grant must close Refreshing through AuthRefreshFailed"
        );
        assert!(
            store
                .load(&target.token_key().unwrap())
                .await
                .unwrap()
                .is_none(),
            "permanently rejected credentials must remain dead across processes"
        );
        let requests_before_restart = {
            let token_requests = state.token_requests.lock();
            assert_eq!(
                token_requests.last().unwrap()["grant_type"],
                "refresh_token"
            );
            assert_eq!(
                token_requests.last().unwrap()["resource"],
                format!("{base}/mcp"),
                "refresh exchange should carry the MCP resource indicator"
            );
            token_requests.len()
        };
        let restarted = McpOAuthAuthority::with_test_http(
            store.clone(),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            test_auth_lease(),
        );
        assert!(
            restarted
                .stored_bearer_token(&target)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            state.token_requests.lock().len(),
            requests_before_restart,
            "a restarted authority must not retry a durably cleared permanent credential"
        );
    }

    #[tokio::test]
    async fn permanent_refresh_clear_failure_is_typed_and_closes_machine() {
        let (base, state) = spawn_oauth_fixture().await;
        let inner = Arc::new(EphemeralTokenStore::new());
        let store = Arc::new(FailClearStore {
            inner: Arc::clone(&inner),
        });
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        authority.interactive_login(&target, None).await.unwrap();
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        *state.token_fails.lock() = true;

        let error = authority
            .stored_bearer_token(&target)
            .await
            .expect_err("durable clear failure must surface");
        assert!(matches!(&error, McpOAuthError::TokenStore(_)));
        assert!(error.to_string().contains("injected clear failure"));
        assert_eq!(
            auth_lease.snapshot(&target.lease_key().unwrap()).phase,
            Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired),
            "even a failed durable clear must close the begun refresh"
        );
        assert!(
            inner
                .load(&target.token_key().unwrap())
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn durable_clear_crash_point_reconciles_stranded_refreshing_lifecycle() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            recording_browser(state),
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        authority.interactive_login(&target, None).await.unwrap();
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        let lease_key = target.lease_key().unwrap();
        auth_lease
            .observe_credential_freshness(
                &lease_key,
                epoch_secs(Utc::now()),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .unwrap();
        auth_lease.begin_refresh(&lease_key).unwrap();
        store.clear(&target.token_key().unwrap()).await.unwrap();
        assert_eq!(
            auth_lease.snapshot(&lease_key).phase,
            Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
        );

        assert!(
            authority
                .stored_bearer_token(&target)
                .await
                .unwrap()
                .is_none()
        );
        let reconciled = auth_lease.snapshot(&lease_key);
        assert!(
            reconciled.phase.is_none(),
            "released lifecycle projects no active phase"
        );
        assert!(!reconciled.credential_present);
    }

    #[tokio::test]
    async fn transient_refresh_failure_closes_machine_back_to_expiring() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let auth_lease = test_auth_lease();
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            recording_browser(Arc::clone(&state)),
            Client::new(),
            auth_lease.clone(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        authority.interactive_login(&target, None).await.unwrap();
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        *state.token_transiently_fails.lock() = true;

        let error = authority
            .stored_bearer_token(&target)
            .await
            .expect_err("transient refresh failure must surface");
        assert!(matches!(error, McpOAuthError::RefreshFailed { .. }));
        assert_eq!(
            auth_lease.snapshot(&target.lease_key().unwrap()).phase,
            Some(meerkat_core::handles::AuthLeasePhase::Expiring),
            "transient boundary evidence must close Refreshing through AuthRefreshFailed"
        );
    }

    #[tokio::test]
    async fn stored_token_metadata_must_match_requested_target() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let original = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));
        let other = McpServerIdentity::from_server_config("glean", format!("{base}/other-mcp"));

        authority
            .interactive_login(&original, None)
            .await
            .expect("initial login succeeds");
        let stored = store
            .load(&original.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        store
            .save(&other.token_key().unwrap(), &stored)
            .await
            .unwrap();

        let error = authority
            .stored_bearer_token(&other)
            .await
            .expect_err("mismatched stored metadata should not be trusted");

        assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
    }

    #[tokio::test]
    async fn interactive_login_missing_dcr_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.include_registration_endpoint.lock() = false;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "registration_endpoint",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when discovery cannot find DCR"
        );
    }

    #[tokio::test]
    async fn interactive_login_missing_dcr_auth_method_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.omit_token_endpoint_auth_method.lock() = true;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "token_endpoint_auth_method missing",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when DCR does not explicitly confirm public-client auth"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_mismatched_protected_resource_metadata() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.resource_override.lock() = Some(format!("{base}/other-mcp"));
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match MCP server",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when protected resource metadata is mismatched"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_relative_protected_resource_metadata() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.resource_override.lock() = Some("/mcp".to_string());
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match MCP server",
        )
        .await;
    }

    #[tokio::test]
    async fn interactive_login_rejects_remote_http_protected_resource() {
        let (_base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", "http://mcp.example.test/mcp");

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "MCP protected resource must use https",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open for remote HTTP protected resources"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_mismatched_authorization_server_issuer() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.issuer_override.lock() = Some("https://issuer.example.invalid".to_string());
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match discovered issuer",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when authorization-server metadata is mismatched"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_issuer_without_exact_codepoint_match() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.issuer_override.lock() = Some(format!("{base}/"));
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match discovered issuer",
        )
        .await;
    }

    #[test]
    fn oauth_metadata_urls_must_be_https_unless_loopback() {
        let target = McpServerIdentity::from_server_config("glean", "https://mcp.example.test/mcp");

        require_https_or_loopback(
            &target,
            "https://issuer.example.test",
            "authorization server issuer",
        )
        .expect("https issuer is allowed");
        require_https_or_loopback(
            &target,
            "http://127.0.0.1:1234/register",
            "registration endpoint",
        )
        .expect("loopback http is allowed for local tests/dev");
        let error = require_https_or_loopback(
            &target,
            "http://issuer.example.test",
            "authorization server issuer",
        )
        .expect_err("remote http issuer should fail closed");
        assert!(error.to_string().contains("must use https"));
    }

    #[tokio::test]
    async fn interactive_login_token_exchange_failure_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.token_fails.lock() = true;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "invalid_grant")
            .await;
    }

    #[tokio::test]
    async fn interactive_login_state_mismatch_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.authorize_outcome.lock() = AuthorizeOutcome::StateMismatch;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "state mismatch")
            .await;
    }

    #[tokio::test]
    async fn interactive_login_denied_auth_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.authorize_outcome.lock() = AuthorizeOutcome::Denied;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority = McpOAuthAuthority::with_test_http(
            store.clone(),
            browser,
            Client::new(),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "user denied").await;
    }

    #[tokio::test]
    async fn interactive_login_timeout_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.authorize_outcome.lock() = AuthorizeOutcome::NoCallback;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser: Arc<dyn BrowserOpener> = Arc::new(NoCallbackBrowser {
            state: Arc::clone(&state),
        });
        let authority = McpOAuthAuthority::with_http_and_login_timeout(
            store.clone(),
            browser,
            Client::new(),
            Duration::from_millis(10),
            test_auth_lease(),
        );
        let target = McpServerIdentity::from_server_config("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "timeout").await;
        assert!(
            state.opened_url.lock().is_some(),
            "timeout test should still reach the browser-open boundary"
        );
    }
}

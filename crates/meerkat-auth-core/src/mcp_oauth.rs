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
    CredentialMutationError, PersistedAuthMode, PersistedTokens, ProviderAuthPersistence,
    RefreshCoordinator, RefreshError, TokenKey, TokenStore,
};
use crate::connector_oauth::{
    ConnectorAccountObservation, ConnectorOAuthDescriptor, ConnectorOAuthRefusal,
};
use crate::oauth_flow::{OAuthBrowserFlowIdentity, OAuthFlowAuthority, OAuthFlowError};
use crate::{BrowserOAuthFlowCommit, save_oauth_tokens_and_consume_browser_flow};
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
pub const MCP_INTERACTIVE_LOGIN_TIMEOUT: Duration =
    crate::connector_oauth::CONNECTOR_BROWSER_LOGIN_WINDOW;

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

/// Validated discovery and registered-client facts supplied to a trusted host
/// strategy. The host supplies the expected account and requested scopes.
/// This is not an action projection and must never be sent to an agent model.
pub struct McpOAuthCeremonyContext<'a> {
    pub issuer: &'a str,
    pub client: &'a str,
    pub resource: &'a str,
    pub redirect_uri: &'a str,
}

/// Provider-specific account verification supplied by the trusted embedding
/// host. It must authenticate provider evidence, not decode an unverified JWT
/// or infer identity from an MCP label. No strategy means interactive refusal.
#[async_trait]
pub trait McpOAuthAccountStrategy: Send + Sync {
    fn descriptor(
        &self,
        target: &McpServerIdentity,
        context: &McpOAuthCeremonyContext<'_>,
    ) -> Result<ConnectorOAuthDescriptor, ConnectorOAuthRefusal>;
    async fn observe_account(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal>;
}

/// RAII projection onto the existing owner, not an independent lifecycle.
struct AdmittedBrowserAttempt {
    authority: Arc<dyn OAuthFlowAuthority>,
    target: meerkat_core::AuthCredentialIdentity,
    identity: OAuthBrowserFlowIdentity,
    state: String,
    redirect_uri: String,
}
impl Drop for AdmittedBrowserAttempt {
    fn drop(&mut self) {
        // A consumed/revoked/expired attempt is already terminal. An abandoned
        // live attempt is retired by the same canonical persisted flow owner.
        let _ = self.authority.expire(
            &self.state,
            &self.target,
            self.identity.clone(),
            &self.redirect_uri,
        );
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthError {
    #[error(transparent)]
    Verification(#[from] ConnectorOAuthRefusal),
    #[error("MCP OAuth flow owner refused the ceremony")]
    Flow(#[source] OAuthFlowError),
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
    interactive: Option<(
        Arc<dyn OAuthFlowAuthority>,
        Arc<dyn McpOAuthAccountStrategy>,
    )>,
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
            interactive: None,
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
            interactive: None,
        }
    }

    pub fn with_interactive_strategy(
        mut self,
        authority: Arc<dyn OAuthFlowAuthority>,
        strategy: Arc<dyn McpOAuthAccountStrategy>,
    ) -> Result<Self, McpOAuthError> {
        if !authority.terminal_flow_state_is_authmachine_owned() {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable.into());
        }
        self.auth_lease = authority
            .generated_credential_lifecycle()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        self.interactive = Some((authority, strategy));
        Ok(self)
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
        let (authority, strategy) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let binding = bind_loopback_callback(CALLBACK_PATH)
            .await
            .map_err(|_| McpOAuthError::Browser("callback bind failed".into()))?;
        let redirect_uri = binding.redirect_url.clone();
        // Keep binding ownership on every pre-admission failure so even a
        // rejected discovery/strategy joins its actual callback server.
        let prepared = async {
            let mut discovery = self
                .discover(target, www_authenticate, &redirect_uri)
                .await?;
            let client = self
                .register_client(target, &discovery, &redirect_uri)
                .await?;
            let descriptor = strategy.descriptor(
                target,
                &McpOAuthCeremonyContext {
                    issuer: &discovery.authorization_server,
                    client: &client.client_id,
                    resource: &discovery.resource,
                    redirect_uri: &redirect_uri,
                },
            )?;
            let facts = descriptor.parameters();
            if facts.issuer != discovery.authorization_server
                || facts.client != client.client_id
                || facts.resource != discovery.resource
                || facts.redirect_uri != redirect_uri
            {
                return Err(McpOAuthError::Verification(
                    ConnectorOAuthRefusal::DescriptorMismatch,
                ));
            }
            discovery.scopes = facts.scopes.iter().cloned().collect();
            let pkce = PkcePair::generate_s256();
            let credential_identity = target.auth_binding_ref()?.into();
            let identity: OAuthBrowserFlowIdentity = descriptor.clone().into();
            let state = authority
                .start(
                    credential_identity,
                    identity.clone(),
                    redirect_uri.clone(),
                    pkce.verifier.secret().to_owned(),
                )
                .map_err(McpOAuthError::Flow)?;
            Ok((discovery, client, descriptor, pkce, state, identity))
        }
        .await;
        let (discovery, client, descriptor, pkce, state, identity) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                binding
                    .cancel()
                    .await
                    .map_err(|_| McpOAuthError::Browser("callback retirement failed".into()))?;
                return Err(error);
            }
        };
        let attempt = AdmittedBrowserAttempt {
            authority: authority.clone(),
            target: target.auth_binding_ref()?.into(),
            identity,
            state,
            redirect_uri: redirect_uri.clone(),
        };
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
        let callback = binding.expect_state(attempt.state.clone());
        if self
            .browser
            .open(&endpoints.authorize_url_with_pkce(&pkce.challenge, &attempt.state))
            .await
            .is_err()
        {
            callback
                .cancel()
                .await
                .map_err(|_| McpOAuthError::Browser("callback retirement failed".into()))?;
            return Err(McpOAuthError::Browser(
                "external browser launch failed".into(),
            ));
        }
        let admitted = match authority.verify(
            &attempt.state,
            &attempt.target,
            attempt.identity.clone(),
            &redirect_uri,
        ) {
            Ok(record) => record,
            Err(error) => {
                callback
                    .cancel()
                    .await
                    .map_err(|_| McpOAuthError::Browser("callback retirement failed".into()))?;
                return Err(McpOAuthError::Flow(error));
            }
        };
        let remaining = self
            .login_timeout
            .saturating_sub(admitted.created_at.elapsed());
        let outcome = callback
            .wait(remaining)
            .await
            .map_err(|error| map_oauth_exchange_error(target, error))?;
        let record = authority
            .verify(
                &outcome.state,
                &attempt.target,
                attempt.identity.clone(),
                &redirect_uri,
            )
            .map_err(McpOAuthError::Flow)?;
        let token = exchange_authorization_code_with_state(
            &self.http,
            &endpoints,
            &outcome.code,
            &record.pkce_verifier,
            client.client_secret.as_deref(),
            Some(&outcome.state),
        )
        .await
        .map_err(|_| McpOAuthError::TokenExchangeFailed {
            server_name: target.server_name().to_owned(),
            reason: "authorization-code exchange failed".into(),
        })?;
        let evidence = descriptor
            .verify_account(strategy.observe_account(&descriptor, &token).await?, &token)?;
        let mut persisted =
            persisted_tokens_from_result(&token, &discovery, &client, target, Utc::now())?;
        persisted.account_id = Some(evidence.account().to_owned());
        persisted.scopes = evidence.granted_scopes().iter().cloned().collect();
        let committed = save_oauth_tokens_and_consume_browser_flow(
            self.provider_auth_persistence.clone(),
            self.auth_lease.clone(),
            attempt.target.clone(),
            persisted,
            BrowserOAuthFlowCommit {
                authority: authority.clone(),
                state: attempt.state.clone(),
                completion: evidence.into(),
                redirect_uri,
            },
        )
        .await
        .map_err(|error| map_coordinated_login_error(target, error))?;
        committed
            .primary_secret
            .ok_or_else(|| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_owned(),
                reason: "committed credential has no bearer token".into(),
            })
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
        // Refresh preserves the verified credential subject; a server label is never an account.
        persisted.account_id = admitted.tokens.account_id.clone();
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
        if !auth
            .code_challenge_methods_supported
            .iter()
            .any(|method| method == "S256")
        {
            return Err(McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_owned(),
                reason: "authorization server does not advertise PKCE S256".into(),
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
        account_id: None,
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
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
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

/// `Debug` redacts the client secret.
#[derive(Clone, Serialize, Deserialize)]
struct StoredMcpOAuthClient {
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    token_endpoint_auth_method: String,
    redirect_uri: String,
}

impl std::fmt::Debug for StoredMcpOAuthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredMcpOAuthClient")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "token_endpoint_auth_method",
                &self.token_endpoint_auth_method,
            )
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
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

    #[test]
    fn stored_mcp_oauth_client_debug_redacts_client_secret() {
        const SECRET: &str = "sk-live-secret-value";
        let client = StoredMcpOAuthClient {
            client_id: "client-visible".into(),
            client_secret: Some(SECRET.into()),
            token_endpoint_auth_method: "client_secret_post".into(),
            redirect_uri: "http://127.0.0.1:1/callback".into(),
        };
        let rendered = format!("{client:?} {client:#?}");
        assert!(!rendered.contains(SECRET), "secret leaked: {rendered}");
        assert!(rendered.contains("client-visible"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
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
        assert_eq!(a.realm().as_str(), MCP_TOKEN_REALM);
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
        assert_eq!(token_key.realm().as_str(), MCP_TOKEN_REALM);
        assert_eq!(lease_key.realm().as_str(), MCP_TOKEN_REALM);
        assert_eq!(
            token_key.binding().unwrap().as_str(),
            lease_key.binding().unwrap().as_str(),
            "token key and lease key must share one typed identity binding"
        );
        // Distinct server URL yields a distinct identity binding.
        let other =
            McpServerIdentity::from_server_config("glean", "https://other.example/mcp/default");
        assert_ne!(
            other.token_key().unwrap().binding().unwrap().as_str(),
            token_key.binding().unwrap().as_str()
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
}

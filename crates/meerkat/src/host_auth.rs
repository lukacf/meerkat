//! Runtime-independent interactive-auth service for native embedding hosts.
//!
//! The host owns loopback HTTP, browser launch, and UI. This service owns
//! target/owner resolution, PKCE and one-time state, token exchange,
//! coordinated persistence, AuthMachine lifecycle publication, status, and
//! logout.
//!
//! The same split covers OAuth-protected MCP servers
//! ([`HostAuthService::mcp_login_start`] / [`HostAuthService::mcp_login_complete`]).
//! Agents never drive this flow: an MCP server that needs a human reports
//! the typed `AuthorizationRequired` host status, and the host decides when
//! to ask its user.
//!
//! # Host obligation: an unobservable browser context
//!
//! The authorize URL and `state` returned by a login start, and the `code`
//! and `state` delivered to the loopback callback, are bearer material for
//! one attempt: whoever holds them can complete it. The host must:
//!
//! - open the authorize URL only in a browser context that no agent-drivable
//!   tool can observe or control (not a browser, shell, screenshot, MCP or
//!   computer-use tool available to any agent of this host);
//! - bind the loopback callback itself and deliver `state`/`code` only to the
//!   matching login complete call;
//! - never place the authorize URL, `state`, `code` or callback data in a
//!   tool result, transcript, agent event, elicitation result or log.
//!
//! Login start, callback and completion types redact these values in `Debug`;
//! completion projections are secret-free.

use chrono::{DateTime, Utc};
use meerkat_core::connection::{WriteOwnerError, resolve_write_owner};
use meerkat_core::handles::{AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, LeaseKey};
use meerkat_core::{
    AuthBindingRef, AuthStatusPhase, BindingId, Config, OAuthProviderIdentity, ProfileId, Provider,
    RealmId, ResolvedConnectionTarget,
};
use meerkat_providers::auth_oauth::{
    DevicePollOutcome, OAuthError, PkcePair, exchange_authorization_code_with_state,
    poll_device_code, request_device_code,
};
use meerkat_providers::auth_store::{
    CredentialMutationError, PersistedTokens, ProviderAuthPersistence, TokenStoreError,
    credential_source_uses_persisted_store, persisted_auth_mode_is_oauth_login,
};
use meerkat_providers::mcp_oauth::{
    McpOAuthAccountStrategy, McpOAuthAuthority, McpOAuthCallback, McpOAuthError,
    McpOAuthLoginComplete, McpOAuthLoginStart, McpOAuthLoopbackBegin, McpServerIdentity,
    OidcUserInfoAccountStrategy,
};
use meerkat_providers::oauth_flow::{
    OAuthFlowError, OAuthTargetValidationError, oauth_provider_resolution,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Exact provider binding a host wants to inspect or mutate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthTarget {
    pub provider: OAuthProviderIdentity,
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<ProfileId>,
}

/// Secret-free status projection for native UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthStatus {
    pub auth_binding: AuthBindingRef,
    pub provider: Provider,
    pub profile_id: String,
    pub phase: AuthStatusPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// Browser navigation data returned by [`HostAuthService::login_start`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthLoginStart {
    pub auth_binding: AuthBindingRef,
    pub authorize_url: String,
    pub state: String,
    pub redirect_uri: String,
    pub provider: OAuthProviderIdentity,
}

/// Secret-free successful login projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthLoginComplete {
    pub auth_binding: AuthBindingRef,
    pub provider: Provider,
    pub profile_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub has_refresh_token: bool,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAuthDeviceStart {
    pub auth_binding: AuthBindingRef,
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
    pub provider: OAuthProviderIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum HostAuthDevicePoll {
    Pending,
    SlowDown,
    AccessDenied,
    Expired,
    Ready(HostAuthLoginComplete),
}

/// Secret-free authorization phase of one MCP server target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostMcpAuthPhase {
    /// A durable credential for the selected account is present and either
    /// unexpired or refreshable.
    Authorized,
    /// The stored credential has expired and cannot be refreshed.
    ReauthRequired,
    /// No usable credential exists: the target is awaiting human
    /// authorization through the host's browser channel.
    AuthorizationRequired,
}

/// Secret-free MCP authorization status for host UI. This is a host-channel
/// projection, never an agent event payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostMcpAuthStatus {
    pub target: McpServerIdentity,
    pub phase: HostMcpAuthPhase,
    pub expires_at: Option<DateTime<Utc>>,
    pub account_id: Option<String>,
}

impl HostMcpAuthStatus {
    /// Wire projection for `auth/status/get`.
    pub fn to_wire(&self) -> meerkat_contracts::WireMcpAuthStatus {
        meerkat_contracts::WireMcpAuthStatus {
            mcp: mcp_auth_target_to_wire(&self.target),
            phase: match self.phase {
                HostMcpAuthPhase::Authorized => meerkat_contracts::WireMcpAuthPhase::Authorized,
                HostMcpAuthPhase::ReauthRequired => {
                    meerkat_contracts::WireMcpAuthPhase::ReauthRequired
                }
                HostMcpAuthPhase::AuthorizationRequired => {
                    meerkat_contracts::WireMcpAuthPhase::AuthorizationRequired
                }
            },
            expires_at: self.expires_at.map(|at| at.to_rfc3339()),
            account_id: self.account_id.clone(),
        }
    }
}

/// Wire projection of an MCP login start disposition.
pub fn mcp_login_disposition_to_wire(
    disposition: meerkat_providers::mcp_oauth::McpOAuthLoginDisposition,
) -> meerkat_contracts::WireMcpLoginDisposition {
    match disposition {
        meerkat_providers::mcp_oauth::McpOAuthLoginDisposition::Started => {
            meerkat_contracts::WireMcpLoginDisposition::Started
        }
        meerkat_providers::mcp_oauth::McpOAuthLoginDisposition::Joined => {
            meerkat_contracts::WireMcpLoginDisposition::Joined
        }
    }
}

/// Parse a wire MCP target (`auth/login/*`, `auth/status/get`) into the
/// native identity, validating the selected account at the boundary.
pub fn mcp_auth_target_from_wire(
    target: &meerkat_contracts::WireMcpAuthTarget,
) -> Result<McpServerIdentity, McpOAuthError> {
    let identity = McpServerIdentity::from_server_config(&target.server_name, &target.server_url);
    match target.oauth_account.as_deref() {
        Some(account) => identity.with_expected_account(account),
        None => Ok(identity),
    }
}

/// Wire projection of a native MCP target.
pub fn mcp_auth_target_to_wire(target: &McpServerIdentity) -> meerkat_contracts::WireMcpAuthTarget {
    meerkat_contracts::WireMcpAuthTarget {
        server_name: target.server_name().to_owned(),
        server_url: target.server_url().to_owned(),
        oauth_account: target.expected_account().map(str::to_owned),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostAuthError {
    #[error(transparent)]
    Target(#[from] meerkat_core::ConnectionTargetError),
    #[error(transparent)]
    WriteOwner(#[from] WriteOwnerError),
    #[error(transparent)]
    OAuthTarget(#[from] OAuthTargetValidationError),
    #[error(transparent)]
    OAuthFlow(#[from] OAuthFlowError),
    #[error("OAuth token exchange failed")]
    OAuthExchange(#[from] OAuthError),
    #[error(transparent)]
    CredentialMutation(#[from] CredentialMutationError),
    #[error(transparent)]
    TokenStore(#[from] TokenStoreError),
    #[error(transparent)]
    Factory(#[from] meerkat_client::FactoryError),
    #[error("provider auth persistence is not configured for this runtime")]
    PersistenceUnavailable,
    #[error("AuthMachine lifecycle update failed")]
    Lifecycle(#[source] meerkat_core::handles::DslTransitionError),
    #[error("credential status rehydration failed")]
    StatusRehydrate(#[source] meerkat_core::auth::AuthStatusRehydrateError),
    #[error("provider '{0}' requires the device-code login flow")]
    BrowserFlowUnsupported(OAuthProviderIdentity),
    #[error("provider '{0}' does not support the device-code login flow")]
    DeviceFlowUnsupported(OAuthProviderIdentity),
    #[error(transparent)]
    McpOAuth(#[from] McpOAuthError),
}

/// Injectable native-host authentication facade.
#[derive(Clone)]
pub struct HostAuthService {
    persistence: ProviderAuthPersistence,
    authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
    http: reqwest::Client,
    mcp_account_strategy: Arc<dyn McpOAuthAccountStrategy>,
}

impl HostAuthService {
    pub fn new(
        persistence: ProviderAuthPersistence,
        authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
    ) -> Self {
        Self {
            persistence,
            authority,
            http: reqwest::Client::new(),
            mcp_account_strategy: Arc::new(OidcUserInfoAccountStrategy::new()),
        }
    }

    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Replace the MCP account-evidence strategy. The default is
    /// [`OidcUserInfoAccountStrategy`]; hosts whose MCP servers prove the
    /// account another way supply their own.
    pub fn with_mcp_account_strategy(mut self, strategy: Arc<dyn McpOAuthAccountStrategy>) -> Self {
        self.mcp_account_strategy = strategy;
        self
    }

    /// The native MCP OAuth authority bound to this service's persistence,
    /// AuthMachine lease and flow owner. Use it as the factory's
    /// `McpAuthResolver` so agent connections share the host's credentials.
    pub fn mcp_oauth_authority(&self) -> Result<McpOAuthAuthority, HostAuthError> {
        Ok(McpOAuthAuthority::with_http(
            self.persistence.clone(),
            self.http.clone(),
            self.authority.generated_auth_lease_handle(),
        )
        .with_interactive_strategy(
            self.authority.oauth_flow_authority(),
            Arc::clone(&self.mcp_account_strategy),
        )?)
    }

    /// Admit one host-driven MCP OAuth attempt. The returned projection is
    /// host-only (see the module docs): open its authorize URL in an
    /// unobservable browser context and deliver the loopback callback to
    /// [`Self::mcp_login_complete`]. If an attempt is already pending for the
    /// target, its projection is returned with `disposition = Joined`; no
    /// second attempt is admitted.
    pub async fn mcp_login_start(
        &self,
        target: &McpServerIdentity,
        redirect_uri: &str,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoginStart, HostAuthError> {
        Ok(self
            .mcp_oauth_authority()?
            .login_start(target, redirect_uri, www_authenticate)
            .await?)
    }

    /// Host loopback login: bind the callback listener and admit the attempt
    /// (or report the one already pending for the target). The returned
    /// pending login launches the browser off the async runtime (advisory
    /// only), completes from its own callback, and has a typed cancel.
    pub async fn mcp_begin_loopback_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoopbackBegin, HostAuthError> {
        Ok(self
            .mcp_oauth_authority()?
            .begin_loopback_login(target, www_authenticate)
            .await?)
    }

    /// Complete an admitted MCP OAuth attempt from the host's loopback
    /// callback. Returns a secret-free projection.
    pub async fn mcp_login_complete(
        &self,
        target: &McpServerIdentity,
        callback: McpOAuthCallback,
    ) -> Result<McpOAuthLoginComplete, HostAuthError> {
        Ok(self
            .mcp_oauth_authority()?
            .login_complete(target, callback)
            .await?)
    }

    /// Retire an abandoned MCP OAuth attempt (timeout, cancellation or a
    /// closed browser).
    pub fn mcp_login_cancel(
        &self,
        target: &McpServerIdentity,
        start: &McpOAuthLoginStart,
    ) -> Result<(), HostAuthError> {
        Ok(self.mcp_oauth_authority()?.login_cancel(target, start)?)
    }

    /// Secret-free authorization status of one MCP target, projected from
    /// its durable credential. It performs no refresh and no network I/O.
    pub async fn mcp_status(
        &self,
        target: &McpServerIdentity,
    ) -> Result<HostMcpAuthStatus, HostAuthError> {
        let stored = self
            .persistence
            .token_store()
            .load(&target.token_key()?)
            .await?
            .filter(|tokens| {
                tokens.auth_mode == meerkat_providers::auth_store::PersistedAuthMode::McpOauth
                    && tokens.primary_secret.is_some()
                    && target
                        .expected_account()
                        .is_none_or(|account| tokens.account_id.as_deref() == Some(account))
            });
        let Some(tokens) = stored else {
            return Ok(HostMcpAuthStatus {
                target: target.clone(),
                phase: HostMcpAuthPhase::AuthorizationRequired,
                expires_at: None,
                account_id: None,
            });
        };
        let expired = tokens.expires_at.is_some_and(|at| at <= Utc::now());
        let phase = if expired && tokens.refresh_token.is_none() {
            HostMcpAuthPhase::ReauthRequired
        } else {
            HostMcpAuthPhase::Authorized
        };
        Ok(HostMcpAuthStatus {
            target: target.clone(),
            phase,
            expires_at: tokens.expires_at,
            account_id: tokens.account_id,
        })
    }

    /// Construct the service from the same persistence capability an
    /// [`crate::AgentFactory`] uses for provider resolution.
    pub fn from_factory(
        factory: &crate::AgentFactory,
        authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
    ) -> Result<Self, HostAuthError> {
        let persistence = factory
            .resolution_provider_auth_persistence()
            .map_err(HostAuthError::Factory)?
            .ok_or(HostAuthError::PersistenceUnavailable)?;
        Ok(Self::new(persistence, authority))
    }

    pub async fn status(
        &self,
        config: &Config,
        target: &HostAuthTarget,
    ) -> Result<HostAuthStatus, HostAuthError> {
        let resolved = resolve_target(config, target)?;
        validate_resolved_oauth_target(&resolved, target.provider)?;
        let auth_binding = resolved.auth_binding;
        let lease_key = LeaseKey::from_credential_identity(&resolved.credential_identity);
        let now = Utc::now();
        let auth_lease = self.authority.generated_auth_lease_handle();
        auth_lease
            .observe_credential_freshness(
                &lease_key,
                now.timestamp().max(0) as u64,
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(HostAuthError::Lifecycle)?;
        let mut snapshot = auth_lease.snapshot(&lease_key);
        let expected_mode =
            meerkat_providers::NormalizedAuthMethod::from_auth_profile(&resolved.auth_profile)
                .and_then(meerkat_providers::NormalizedAuthMethod::persisted_auth_mode);
        let source_uses_store =
            credential_source_uses_persisted_store(&resolved.auth_profile.source);
        let oauth_mode = expected_mode
            .map(persisted_auth_mode_is_oauth_login)
            .unwrap_or(false);
        let store = self.persistence.token_store();
        let mut stored = None;
        if source_uses_store {
            let phase = AuthStatusPhase::from_lease_snapshot(now, &snapshot);
            if phase.is_no_live_lease() {
                if let Some(expected_mode) = expected_mode {
                    stored = meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
                        store.as_ref(),
                        &auth_lease,
                        &resolved.credential_identity,
                        expected_mode,
                        now,
                    )
                    .await
                    .map_err(HostAuthError::StatusRehydrate)?;
                    snapshot = auth_lease.snapshot(&lease_key);
                }
            } else {
                stored = store
                    .load(
                        &meerkat_providers::auth_store::TokenKey::from_credential_identity(
                            &resolved.credential_identity,
                        ),
                    )
                    .await?;
            }
        }
        if stored
            .as_ref()
            .is_some_and(|tokens| Some(tokens.auth_mode) != expected_mode)
        {
            stored = None;
        }
        let marker_snapshot;
        let projection_snapshot = if oauth_mode {
            marker_snapshot = stored.as_ref().and_then(|tokens| {
                meerkat_core::oauth_status_projection_snapshot_from_newer_marker(&snapshot, tokens)
            });
            marker_snapshot.as_ref().unwrap_or(&snapshot)
        } else {
            &snapshot
        };
        let projection =
            meerkat_core::project_published_auth_status(now, stored.as_ref(), projection_snapshot);
        Ok(HostAuthStatus {
            auth_binding,
            provider: resolved.backend.provider,
            profile_id: resolved.auth_profile.id,
            phase: projection.phase,
            expires_at: projection.expires_at,
            account_id: projection
                .tokens
                .and_then(|tokens| tokens.account_id.clone()),
        })
    }

    pub async fn login_start(
        &self,
        config: &Config,
        target: &HostAuthTarget,
        redirect_uri: impl Into<String>,
    ) -> Result<HostAuthLoginStart, HostAuthError> {
        let redirect_uri = redirect_uri.into();
        if !target.provider.supports_browser_flow() {
            return Err(HostAuthError::BrowserFlowUnsupported(target.provider));
        }
        let resolved = resolve_writable_oauth_target(config, target)?;
        let pkce = PkcePair::generate_s256();
        let lease_key = LeaseKey::from_credential_identity(&resolved.credential_identity);
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let state = self.authority.oauth_flow_authority().start(
            resolved.credential_identity.clone(),
            meerkat_providers::oauth_flow::OAuthBrowserFlowIdentity::from(target.provider),
            redirect_uri.clone(),
            pkce.verifier.secret().clone(),
        )?;
        let authorize_url = oauth_provider_resolution(target.provider, redirect_uri.clone())
            .endpoints
            .authorize_url_with_pkce(&pkce.challenge, &state);
        Ok(HostAuthLoginStart {
            auth_binding: resolved.auth_binding,
            authorize_url,
            state,
            redirect_uri,
            provider: target.provider,
        })
    }

    pub async fn login_complete(
        &self,
        config: &Config,
        target: &HostAuthTarget,
        redirect_uri: impl Into<String>,
        state: impl Into<String>,
        code: impl AsRef<str>,
    ) -> Result<HostAuthLoginComplete, HostAuthError> {
        let redirect_uri = redirect_uri.into();
        let state = state.into();
        if !target.provider.supports_browser_flow() {
            return Err(HostAuthError::BrowserFlowUnsupported(target.provider));
        }
        let resolved = resolve_writable_oauth_target(config, target)?;
        let oauth_flow_authority = self.authority.oauth_flow_authority();
        let flow = oauth_flow_authority.verify(
            &state,
            &resolved.credential_identity,
            meerkat_providers::oauth_flow::OAuthBrowserFlowIdentity::from(target.provider),
            &redirect_uri,
        )?;
        let oauth = oauth_provider_resolution(target.provider, redirect_uri.clone());
        let exchanged = exchange_authorization_code_with_state(
            &self.http,
            &oauth.endpoints,
            code.as_ref(),
            &flow.pkce_verifier,
            oauth.client_secret,
            Some(&state),
        )
        .await?;
        let now = Utc::now();
        let expires_at = exchanged.expires_at_from(now)?;
        let tokens = PersistedTokens {
            auth_mode: target.provider.auth_mode(),
            primary_secret: Some(exchanged.access_token),
            refresh_token: exchanged.refresh_token,
            id_token: exchanged.id_token,
            expires_at,
            last_refresh: Some(now),
            scopes: exchanged
                .scope
                .as_deref()
                .map(|scope| scope.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            account_id: None,
            metadata: serde_json::Value::Null,
        };
        let committed =
            meerkat_providers::browser_login::save_oauth_tokens_and_consume_browser_flow(
                self.persistence.clone(),
                self.authority.generated_auth_lease_handle(),
                resolved.credential_identity.clone(),
                tokens,
                meerkat_providers::browser_login::BrowserOAuthFlowCommit {
                    authority: oauth_flow_authority,
                    state,
                    completion: (target.provider).into(),
                    redirect_uri,
                },
            )
            .await?;
        Ok(HostAuthLoginComplete {
            auth_binding: resolved.auth_binding,
            provider: resolved.backend.provider,
            profile_id: resolved.auth_profile.id,
            expires_at: committed.expires_at,
            has_refresh_token: committed.refresh_token.is_some(),
            scopes: committed.scopes,
        })
    }

    pub async fn device_start(
        &self,
        config: &Config,
        target: &HostAuthTarget,
    ) -> Result<HostAuthDeviceStart, HostAuthError> {
        let resolved = resolve_writable_oauth_target(config, target)?;
        let oauth = oauth_provider_resolution(target.provider, "");
        if oauth.endpoints.device_code_url.is_none() {
            return Err(HostAuthError::DeviceFlowUnsupported(target.provider));
        }
        let device = request_device_code(&self.http, &oauth.endpoints).await?;
        let lease_key = LeaseKey::from_credential_identity(&resolved.credential_identity);
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        self.authority.oauth_flow_authority().admit_device_code(
            resolved.credential_identity,
            target.provider,
            device.device_code.clone(),
            std::time::Duration::from_secs(device.expires_in),
        )?;
        Ok(HostAuthDeviceStart {
            auth_binding: resolved.auth_binding,
            device_code: device.device_code,
            user_code: device.user_code,
            verification_uri: device.verification_uri,
            verification_uri_complete: device.verification_uri_complete,
            expires_in: device.expires_in,
            interval: device.interval,
            provider: target.provider,
        })
    }

    pub async fn device_poll(
        &self,
        config: &Config,
        target: &HostAuthTarget,
        device_code: &str,
    ) -> Result<HostAuthDevicePoll, HostAuthError> {
        let resolved = resolve_writable_oauth_target(config, target)?;
        let oauth = oauth_provider_resolution(target.provider, "");
        if oauth.endpoints.device_code_url.is_none() {
            return Err(HostAuthError::DeviceFlowUnsupported(target.provider));
        }
        let poll_lease = self
            .authority
            .oauth_flow_authority()
            .begin_device_code_poll(device_code, &resolved.credential_identity, target.provider)?;
        let outcome = poll_device_code(
            &self.http,
            &oauth.endpoints,
            device_code,
            oauth.client_secret,
        )
        .await?;
        match outcome {
            DevicePollOutcome::Pending => {
                poll_lease.finish()?;
                Ok(HostAuthDevicePoll::Pending)
            }
            DevicePollOutcome::SlowDown => {
                poll_lease.finish()?;
                Ok(HostAuthDevicePoll::SlowDown)
            }
            DevicePollOutcome::AccessDenied => {
                poll_lease.consume()?;
                Ok(HostAuthDevicePoll::AccessDenied)
            }
            DevicePollOutcome::Expired => {
                poll_lease.consume()?;
                Ok(HostAuthDevicePoll::Expired)
            }
            DevicePollOutcome::Ready(exchanged) => {
                let now = Utc::now();
                let expires_at = exchanged.expires_at_from(now)?;
                let tokens = PersistedTokens {
                    auth_mode: target.provider.auth_mode(),
                    primary_secret: Some(exchanged.access_token),
                    refresh_token: exchanged.refresh_token,
                    id_token: exchanged.id_token,
                    expires_at,
                    last_refresh: Some(now),
                    scopes: exchanged
                        .scope
                        .as_deref()
                        .map(|scope| scope.split_whitespace().map(String::from).collect())
                        .unwrap_or_default(),
                    account_id: None,
                    metadata: serde_json::Value::Null,
                };
                let committed =
                    meerkat_providers::browser_login::save_oauth_tokens_and_consume_device_flow(
                        self.persistence.clone(),
                        self.authority.generated_auth_lease_handle(),
                        resolved.credential_identity,
                        tokens,
                        poll_lease,
                    )
                    .await?;
                Ok(HostAuthDevicePoll::Ready(HostAuthLoginComplete {
                    auth_binding: resolved.auth_binding,
                    provider: resolved.backend.provider,
                    profile_id: resolved.auth_profile.id,
                    expires_at: committed.expires_at,
                    has_refresh_token: committed.refresh_token.is_some(),
                    scopes: committed.scopes,
                }))
            }
        }
    }

    pub async fn logout(
        &self,
        config: &Config,
        target: &HostAuthTarget,
    ) -> Result<AuthBindingRef, HostAuthError> {
        let resolved = resolve_writable_oauth_target(config, target)?;
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
            self.persistence.clone(),
            self.authority.generated_auth_lease_handle(),
            resolved.credential_identity,
        )
        .await?;
        Ok(resolved.auth_binding)
    }
}

fn resolve_target(
    config: &Config,
    target: &HostAuthTarget,
) -> Result<ResolvedConnectionTarget, HostAuthError> {
    if let Some(provider) = target.provider.provider() {
        return Ok(meerkat_core::resolve_realm_binding_target_for_provider(
            config,
            provider,
            Some(&target.realm_id),
            Some(&target.binding_id),
            target.profile_id.as_ref(),
            None,
            false,
        )?);
    }
    Ok(meerkat_core::resolve_explicit_auth_binding_target(
        config,
        &AuthBindingRef {
            realm: target.realm_id.clone(),
            binding: target.binding_id.clone(),
            profile: target.profile_id.clone(),
            origin: meerkat_core::BindingOrigin::Configured,
        },
    )?)
}

fn resolve_writable_target(
    config: &Config,
    target: &HostAuthTarget,
) -> Result<ResolvedConnectionTarget, HostAuthError> {
    let resolved = resolve_target(config, target)?;
    resolve_write_owner(config, &target.realm_id, &target.binding_id)?;
    Ok(resolved)
}

fn resolve_writable_oauth_target(
    config: &Config,
    target: &HostAuthTarget,
) -> Result<ResolvedConnectionTarget, HostAuthError> {
    let resolved = resolve_writable_target(config, target)?;
    validate_resolved_oauth_target(&resolved, target.provider)?;
    Ok(resolved)
}

fn validate_resolved_oauth_target(
    resolved: &ResolvedConnectionTarget,
    provider: OAuthProviderIdentity,
) -> Result<(), HostAuthError> {
    meerkat_providers::oauth_flow::validate_oauth_login_connection_target(resolved, provider)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{
        AuthProfileConfig, BackendProfileConfig, CredentialSourceSpec, ProviderBindingConfig,
        RealmConfigSection,
    };
    use std::sync::Arc;

    fn config_with_inherited_openai() -> Config {
        let mut config = Config::default();
        let mut global = RealmConfigSection::default();
        global.backend.insert(
            "openai".to_string(),
            BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "chatgpt_backend".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        );
        global.auth.insert(
            "openai".to_string(),
            AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "managed_chatgpt_oauth".to_string(),
                source: CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        global.binding.insert(
            "openai".to_string(),
            ProviderBindingConfig {
                backend_profile: "openai".to_string(),
                auth_profile: "openai".to_string(),
                credential_account: None,
                default_model: Some("gpt-5.4".to_string()),
                policy: Default::default(),
                provider_default: true,
            },
        );
        config.realm.insert("global".to_string(), global);
        config.realm.insert(
            "project".to_string(),
            RealmConfigSection {
                parent: Some(RealmId::global()),
                ..Default::default()
            },
        );
        config
    }

    #[test]
    fn inherited_login_target_returns_typed_owner_error() {
        let config = config_with_inherited_openai();
        let target = HostAuthTarget {
            provider: OAuthProviderIdentity::OpenAiChatGpt,
            realm_id: RealmId::parse("project").unwrap(),
            binding_id: BindingId::parse("openai").unwrap(),
            profile_id: None,
        };
        let error = resolve_writable_target(&config, &target).unwrap_err();
        assert!(matches!(
            error,
            HostAuthError::WriteOwner(WriteOwnerError::Inherited {
                ref owner,
                ..
            }) if owner == "global"
        ));
    }

    #[test]
    fn read_target_is_owner_stamped() {
        let config = config_with_inherited_openai();
        let target = HostAuthTarget {
            provider: OAuthProviderIdentity::OpenAiChatGpt,
            realm_id: RealmId::parse("project").unwrap(),
            binding_id: BindingId::parse("openai").unwrap(),
            profile_id: None,
        };
        let resolved = resolve_target(&config, &target).unwrap();
        assert_eq!(resolved.auth_binding.realm.as_str(), "global");
    }

    #[test]
    fn oauth_logout_target_rejects_non_oauth_binding() {
        let mut config = config_with_inherited_openai();
        let global = config.realm.get_mut("global").unwrap();
        global.auth.get_mut("openai").unwrap().auth_method = "api_key".to_string();
        global.backend.get_mut("openai").unwrap().backend_kind = "openai_api".to_string();
        let target = HostAuthTarget {
            provider: OAuthProviderIdentity::OpenAiChatGpt,
            realm_id: RealmId::global(),
            binding_id: BindingId::parse("openai").unwrap(),
            profile_id: None,
        };

        assert!(matches!(
            resolve_writable_oauth_target(&config, &target),
            Err(HostAuthError::OAuthTarget(_))
        ));
    }

    #[tokio::test]
    async fn absent_status_is_secret_free_and_owner_stamped() {
        let config = config_with_inherited_openai();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let persistence = ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let service = HostAuthService::new(persistence, runtime.provider_auth_runtime_authority());
        let status = service
            .status(
                &config,
                &HostAuthTarget {
                    provider: OAuthProviderIdentity::OpenAiChatGpt,
                    realm_id: RealmId::parse("project").unwrap(),
                    binding_id: BindingId::parse("openai").unwrap(),
                    profile_id: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(status.auth_binding.realm, RealmId::global());
        assert!(status.phase.is_no_live_lease());
        let json = serde_json::to_value(status).unwrap();
        assert!(json.get("primary_secret").is_none());
        assert!(json.get("refresh_token").is_none());
        assert!(json.get("id_token").is_none());
    }
}

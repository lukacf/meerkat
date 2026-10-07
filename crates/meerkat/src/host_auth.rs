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
//! # Host obligation: keep the attempt out of agent channels
//!
//! The authorize URL and `state` returned by a login start, and the `code`
//! and `state` delivered to the loopback callback, are bearer material for
//! one attempt: whoever holds them can complete it. The host must:
//!
//! - bind the loopback callback itself and deliver `state`/`code` only to the
//!   matching login complete call;
//! - never place the authorize URL, `state`, `code` or callback data in a
//!   tool result, transcript, agent event, elicitation result or log.
//!
//! The host opens the authorize URL in the user's own browser (the system
//! browser by default). An ordinary host is supported: no agent-unreachable
//! host is required. A host whose agents have a browser or computer-use tool
//! should not open the URL in a browser such a tool drives.
//!
//! Login start, callback and completion types redact these values in `Debug`;
//! completion projections are secret-free, and the attempt status carries
//! only a non-secret reference.
//!
//! Wire callers (RPC, REST) are host-privileged by contract: a start that
//! joins an attempt already pending for the same configured server returns
//! that attempt's authorize URL and state to the caller.

use chrono::{DateTime, Utc};
use meerkat_core::connection::{CredentialAccountId, CredentialAccountRef};
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
use meerkat_providers::connector_login::{
    ConnectorAccountStrategy, ConnectorAuthPhase, ConnectorAuthStatus, ConnectorLoginComplete,
    ConnectorLoginError, ConnectorLoginStart, ConnectorOAuthAuthority, ConnectorOAuthCallback,
    ConnectorOAuthTarget, ConnectorStrategies, ConnectorVerifiedAccount,
};
use meerkat_providers::connector_oauth::{AccountSelection, ScopeEvidence};
use meerkat_providers::mcp_oauth::{
    McpAccountSelection, McpOAuthAccountStrategy, McpOAuthAuthority, McpOAuthCallback,
    McpOAuthError, McpOAuthLoginComplete, McpOAuthLoginStart, McpOAuthLoopbackBegin,
    McpOAuthPendingAttempt, McpServerIdentity, OidcUserInfoAccountStrategy,
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
    /// The verified subject; always `None` for an unverified grant.
    pub account_id: Option<String>,
    /// The target's account verification mode; `None` for a legacy
    /// (unselected) target.
    pub account_verification: Option<meerkat_providers::connector_oauth::AccountVerification>,
    /// The login attempt pending for the target, if any: a non-secret
    /// reference and its expiry, never its authorize URL or state.
    pub attempt: Option<McpOAuthPendingAttempt>,
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
            account_verification: mcp_account_verification_to_wire(self.account_verification),
            expires_at: self.expires_at.map(|at| at.to_rfc3339()),
            account_id: self.account_id.clone(),
            attempt: self
                .attempt
                .as_ref()
                .map(|attempt| meerkat_contracts::WireMcpAuthAttempt {
                    attempt_ref: attempt.attempt_ref.as_str().to_owned(),
                    phase: meerkat_contracts::WireMcpAuthAttemptPhase::Pending,
                    expires_at: attempt.expires_at.to_rfc3339(),
                }),
        }
    }
}

/// Wire projection of an MCP target's account verification mode (`None`:
/// a legacy target).
pub fn mcp_account_verification_to_wire(
    verification: Option<meerkat_providers::connector_oauth::AccountVerification>,
) -> meerkat_contracts::WireMcpAccountVerification {
    use meerkat_providers::connector_oauth::AccountVerification;
    match verification {
        Some(AccountVerification::Verified) => {
            meerkat_contracts::WireMcpAccountVerification::Verified
        }
        Some(AccountVerification::Unverified) => {
            meerkat_contracts::WireMcpAccountVerification::Unverified
        }
        None => meerkat_contracts::WireMcpAccountVerification::Legacy,
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

/// The default MCP credential source for a runtime-backed host: the native
/// MCP OAuth authority bound to `persistence` and the runtime's AuthMachine
/// lease and flow owner. Agents never open a browser through it; a missing
/// credential is the typed `AuthorizationRequired` host status. `None` when
/// the host has no provider-auth persistence or the owner is not
/// AuthMachine-backed.
#[cfg(feature = "mcp")]
pub fn default_mcp_auth_resolver(
    persistence: Option<ProviderAuthPersistence>,
    authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
) -> Option<Arc<dyn meerkat_mcp::McpAuthResolver>> {
    let service = HostAuthService::new(persistence?, authority);
    match service.mcp_oauth_authority() {
        Ok(authority) => Some(Arc::new(authority)),
        Err(error) => {
            tracing::warn!(
                error = %error,
                "MCP OAuth default resolver unavailable; OAuth-protected MCP servers connect without credentials"
            );
            None
        }
    }
}

/// Why a host-requested MCP target was refused. Login and status only ever
/// address configured servers: a client-supplied name or URL is never an
/// authority for discovery, client registration or credential storage.
#[derive(Debug, thiserror::Error)]
pub enum HostMcpTargetRefusal {
    #[error("MCP server '{server_name}' is not configured")]
    UnknownServer { server_name: String },
    #[error("MCP server '{server_name}' is configured with a different URL")]
    UrlMismatch { server_name: String },
    #[error("MCP server '{server_name}' is configured for a different OAuth account")]
    AccountMismatch { server_name: String },
    #[error(
        "MCP server '{server_name}' does not use OAuth login (streamable HTTP without a static Authorization header)"
    )]
    NotOAuthCapable { server_name: String },
    #[error("MCP configuration could not be read")]
    ConfigUnavailable(#[source] meerkat_core::mcp_config::McpConfigError),
}

impl HostMcpTargetRefusal {
    /// Whether this refuses the caller's request rather than reporting an
    /// unreadable configuration.
    pub fn is_refusal(&self) -> bool {
        !matches!(self, Self::ConfigUnavailable(_))
    }
}

/// Resolve a wire MCP target against the configured MCP servers at the
/// host's convention roots (project wins over user, as for sessions and the
/// CLI). The server name must be configured, `server_url` must equal its
/// configured URL, the server must use OAuth login, and a requested
/// `oauth_account` must equal the configured one. The identity is built from
/// the configuration, never from the request.
pub async fn resolve_configured_mcp_target(
    target: &meerkat_contracts::WireMcpAuthTarget,
    context_root: Option<&std::path::Path>,
    user_config_root: Option<&std::path::Path>,
) -> Result<McpServerIdentity, HostAuthError> {
    use meerkat_core::mcp_config::{McpConfig, McpTransportConfig, McpTransportKind};
    let server_name = || target.server_name.clone();
    let config = McpConfig::load_from_roots(context_root, user_config_root)
        .await
        .map_err(HostMcpTargetRefusal::ConfigUnavailable)?;
    let server = config
        .servers
        .into_iter()
        .find(|server| server.name == target.server_name)
        .ok_or_else(|| HostMcpTargetRefusal::UnknownServer {
            server_name: server_name(),
        })?;
    let McpTransportConfig::Http(http) = &server.transport else {
        return Err(HostMcpTargetRefusal::NotOAuthCapable {
            server_name: server_name(),
        }
        .into());
    };
    if http.url != target.server_url {
        return Err(HostMcpTargetRefusal::UrlMismatch {
            server_name: server_name(),
        }
        .into());
    }
    if !matches!(server.transport_kind(), McpTransportKind::StreamableHttp)
        || http
            .headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("authorization"))
    {
        return Err(HostMcpTargetRefusal::NotOAuthCapable {
            server_name: server_name(),
        }
        .into());
    }
    // The caller only names the configured selection; a different value is
    // refused and never selects or downgrades the mode.
    let configured_selection = http
        .oauth_account_selection
        .map(mcp_account_selection_to_wire);
    if target
        .oauth_account
        .as_deref()
        .is_some_and(|requested| http.oauth_account.as_deref() != Some(requested))
        || target
            .oauth_account_selection
            .is_some_and(|requested| configured_selection != Some(requested))
    {
        return Err(HostMcpTargetRefusal::AccountMismatch {
            server_name: server_name(),
        }
        .into());
    }
    Ok(McpServerIdentity::from_config(&server)?)
}

fn mcp_account_selection_to_wire(
    selection: meerkat_core::mcp_config::McpOAuthAccountSelection,
) -> meerkat_contracts::WireMcpAccountSelection {
    use meerkat_core::mcp_config::McpOAuthAccountSelection;
    match selection {
        McpOAuthAccountSelection::Discover => meerkat_contracts::WireMcpAccountSelection::Discover,
        McpOAuthAccountSelection::Unverified => {
            meerkat_contracts::WireMcpAccountSelection::Unverified
        }
    }
}

/// Wire projection of a native MCP target.
pub fn mcp_auth_target_to_wire(target: &McpServerIdentity) -> meerkat_contracts::WireMcpAuthTarget {
    meerkat_contracts::WireMcpAuthTarget {
        server_name: target.server_name().to_owned(),
        server_url: target.server_url().to_owned(),
        oauth_account: target.expected_account().map(str::to_owned),
        oauth_account_selection: match target.selection() {
            McpAccountSelection::Discover => {
                Some(meerkat_contracts::WireMcpAccountSelection::Discover)
            }
            McpAccountSelection::Unverified => {
                Some(meerkat_contracts::WireMcpAccountSelection::Unverified)
            }
            McpAccountSelection::Legacy | McpAccountSelection::Known(_) => None,
        },
    }
}

/// The connector slot named on the wire.
pub fn connector_slot_from_wire(
    slot: &meerkat_contracts::WireConnectorSlot,
) -> Result<CredentialAccountRef, HostAuthError> {
    Ok(CredentialAccountRef {
        realm: RealmId::parse(&slot.realm_id)
            .map_err(|error| HostAuthError::ConnectorTarget(error.to_string()))?,
        account: CredentialAccountId::parse(&slot.slot_id)
            .map_err(|error| HostAuthError::ConnectorTarget(error.to_string()))?,
    })
}

pub fn connector_slot_to_wire(slot: &CredentialAccountRef) -> meerkat_contracts::WireConnectorSlot {
    meerkat_contracts::WireConnectorSlot {
        realm_id: slot.realm.to_string(),
        slot_id: slot.account.to_string(),
    }
}

/// The connector login target named on the wire.
pub fn connector_target_from_wire(
    target: &meerkat_contracts::WireConnectorAuthTarget,
) -> Result<ConnectorOAuthTarget, HostAuthError> {
    use meerkat_contracts::WireConnectorAccountSelection;
    Ok(ConnectorOAuthTarget {
        slot: connector_slot_from_wire(&target.slot)?,
        issuer: target.issuer.clone(),
        client: target.client.clone(),
        resource: target.resource.clone(),
        scopes: target.scopes.iter().cloned().collect(),
        strategy_id: target.strategy_id.clone(),
        account: match &target.account_selection {
            WireConnectorAccountSelection::Known { account } => {
                AccountSelection::Known(account.clone())
            }
            WireConnectorAccountSelection::Discover => AccountSelection::Discover,
        },
    })
}

fn verified_account_to_wire(
    account: &ConnectorVerifiedAccount,
) -> meerkat_contracts::WireConnectorVerifiedAccount {
    meerkat_contracts::WireConnectorVerifiedAccount {
        issuer: account.issuer.clone(),
        strategy_id: account.strategy_id.clone(),
        subject: account.subject.clone(),
    }
}

fn scope_evidence_to_wire(evidence: ScopeEvidence) -> meerkat_contracts::WireScopeEvidence {
    match evidence {
        ScopeEvidence::TokenEndpointResponse => {
            meerkat_contracts::WireScopeEvidence::TokenEndpointResponse
        }
        ScopeEvidence::RetainedOnRefresh { from } => {
            meerkat_contracts::WireScopeEvidence::RetainedOnRefresh {
                granted_at: DateTime::<Utc>::from_timestamp(from.granted_at_epoch_secs, 0)
                    .map(|at| at.to_rfc3339())
                    .unwrap_or_default(),
            }
        }
    }
}

/// `auth/login/complete` result for a connector login.
pub fn connector_ready_to_wire(done: &ConnectorLoginComplete) -> meerkat_contracts::WireLoginReady {
    meerkat_contracts::WireLoginReady {
        state: None,
        target: meerkat_contracts::WireLoginReadyTarget::Connector(
            meerkat_contracts::WireConnectorLoginReady {
                connector: connector_slot_to_wire(&done.slot),
                verified_account: verified_account_to_wire(&done.verified_account),
                scope_evidence: scope_evidence_to_wire(done.scope_evidence),
            },
        ),
        expires_at: done.expires_at.map(|at| at.to_rfc3339()),
        has_refresh_token: done.has_refresh_token,
        scopes: done.scopes.clone(),
    }
}

/// `auth/status/get` result for a connector slot.
pub fn connector_status_to_wire(
    status: &ConnectorAuthStatus,
) -> meerkat_contracts::WireConnectorAuthStatus {
    meerkat_contracts::WireConnectorAuthStatus {
        connector: connector_slot_to_wire(&status.slot),
        phase: match status.phase {
            ConnectorAuthPhase::Authorized => meerkat_contracts::WireMcpAuthPhase::Authorized,
            ConnectorAuthPhase::ReauthRequired => {
                meerkat_contracts::WireMcpAuthPhase::ReauthRequired
            }
            ConnectorAuthPhase::AuthorizationRequired => {
                meerkat_contracts::WireMcpAuthPhase::AuthorizationRequired
            }
        },
        verified_account: status
            .verified_account
            .as_ref()
            .map(verified_account_to_wire),
        scopes: status.scopes.clone(),
        scope_evidence: status.scope_evidence.map(scope_evidence_to_wire),
        expires_at: status.expires_at.map(|at| at.to_rfc3339()),
        has_refresh_token: status.has_refresh_token,
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
    #[error(transparent)]
    McpTarget(#[from] HostMcpTargetRefusal),
    #[error(transparent)]
    Connector(#[from] ConnectorLoginError),
    #[error("invalid connector target: {0}")]
    ConnectorTarget(String),
}

/// Injectable native-host authentication facade.
#[derive(Clone)]
pub struct HostAuthService {
    persistence: ProviderAuthPersistence,
    authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
    http: reqwest::Client,
    mcp_account_strategy: Arc<dyn McpOAuthAccountStrategy>,
    connector_strategies: ConnectorStrategies,
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
            connector_strategies: ConnectorStrategies::with_defaults(),
        }
    }

    /// Install a connector account strategy (keyed by its strategy id) next
    /// to the default OIDC UserInfo strategy.
    pub fn with_connector_strategy(mut self, strategy: Arc<dyn ConnectorAccountStrategy>) -> Self {
        self.connector_strategies = self.connector_strategies.with(strategy);
        self
    }

    /// The native connector OAuth owner bound to this service's persistence,
    /// AuthMachine lease and flow owner. Native hosts use it for bearer
    /// tokens (with refresh); its HTTP client follows no redirects.
    pub fn connector_oauth_authority(&self) -> Result<ConnectorOAuthAuthority, HostAuthError> {
        Ok(ConnectorOAuthAuthority::new(
            self.persistence.clone(),
            self.authority.oauth_flow_authority(),
            self.connector_strategies.clone(),
        )?)
    }

    /// Admit one host-driven connector OAuth attempt into `target.slot`. The
    /// returned projection is host-only (see the module docs).
    pub async fn connector_login_start(
        &self,
        target: &ConnectorOAuthTarget,
        redirect_uri: &str,
    ) -> Result<ConnectorLoginStart, HostAuthError> {
        Ok(self
            .connector_oauth_authority()?
            .login_start(target, redirect_uri)
            .await?)
    }

    /// Complete an admitted connector attempt from the host's loopback
    /// callback. The admitted descriptor must equal `target`'s facts.
    pub async fn connector_login_complete(
        &self,
        target: &ConnectorOAuthTarget,
        callback: ConnectorOAuthCallback,
    ) -> Result<ConnectorLoginComplete, HostAuthError> {
        Ok(self
            .connector_oauth_authority()?
            .login_complete_for_target(target, callback)
            .await?)
    }

    /// Retire the connector attempt admitted under `state` for `slot`.
    pub fn connector_login_cancel(
        &self,
        slot: &CredentialAccountRef,
        state: &str,
    ) -> Result<(), HostAuthError> {
        Ok(self
            .connector_oauth_authority()?
            .login_cancel(slot, state)?)
    }

    /// Disconnect a connector slot: remove its credential and release its
    /// lifecycle. A slot holding another owner's credential is refused.
    pub async fn connector_logout(&self, slot: &CredentialAccountRef) -> Result<(), HostAuthError> {
        Ok(self.connector_oauth_authority()?.logout(slot).await?)
    }

    /// Secret-free status of a connector slot. No refresh, no network I/O.
    pub async fn connector_status(
        &self,
        slot: &CredentialAccountRef,
    ) -> Result<ConnectorAuthStatus, HostAuthError> {
        Ok(self.connector_oauth_authority()?.status(slot).await?)
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
    ///
    /// It uses its own HTTP client, which follows no redirects, rather than
    /// [`Self::with_http_client`]'s.
    pub fn mcp_oauth_authority(&self) -> Result<McpOAuthAuthority, HostAuthError> {
        Ok(McpOAuthAuthority::new(
            self.persistence.clone(),
            self.authority.generated_auth_lease_handle(),
        )
        .with_interactive_strategy(
            self.authority.oauth_flow_authority(),
            Arc::clone(&self.mcp_account_strategy),
        )?)
    }

    /// Admit one host-driven MCP OAuth attempt. The returned projection is
    /// host-only (see the module docs): open its authorize URL in the user's
    /// browser and deliver the loopback callback to
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

    /// Typed cancel by `state` for hosts without a start projection (wire
    /// callers): retire the attempt admitted under `state` for `target`.
    pub fn mcp_login_cancel_by_state(
        &self,
        target: &McpServerIdentity,
        state: &str,
    ) -> Result<(), HostAuthError> {
        Ok(self.mcp_oauth_authority()?.cancel_attempt(target, state)?)
    }

    /// Typed cancel by the non-secret `attempt_ref` that [`Self::mcp_status`]
    /// reports, for hosts that no longer hold the attempt's state (for
    /// example after a host restart): retire that attempt for `target`.
    pub fn mcp_login_cancel_by_attempt_ref(
        &self,
        target: &McpServerIdentity,
        attempt_ref: &str,
    ) -> Result<(), HostAuthError> {
        Ok(self
            .mcp_oauth_authority()?
            .cancel_attempt_by_ref(target, attempt_ref)?)
    }

    /// Disconnect one MCP target: remove its stored credential and release
    /// the credential lifecycle. A pending attempt is unaffected, and nothing
    /// is revoked at the provider.
    pub async fn mcp_logout(&self, target: &McpServerIdentity) -> Result<(), HostAuthError> {
        Ok(self.mcp_oauth_authority()?.logout(target).await?)
    }

    /// Secret-free authorization status of one MCP target, projected from
    /// its durable credential and its pending attempt. It performs no
    /// refresh and no network I/O.
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
                    && meerkat_providers::mcp_oauth::stored_credential_matches_selection(
                        target, tokens,
                    )
            });
        let attempt = match self.mcp_oauth_authority() {
            Ok(authority) => authority.pending_attempt(target)?,
            // Without an AuthMachine-owned flow owner no attempt can have
            // been admitted, so none is pending.
            Err(HostAuthError::McpOAuth(McpOAuthError::Verification(
                meerkat_providers::connector_oauth::ConnectorOAuthRefusal::VerificationUnavailable,
            ))) => None,
            Err(error) => return Err(error),
        };
        let Some(tokens) = stored else {
            return Ok(HostMcpAuthStatus {
                target: target.clone(),
                phase: HostMcpAuthPhase::AuthorizationRequired,
                expires_at: None,
                account_id: None,
                account_verification: target.account_verification(),
                attempt,
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
            account_verification: target.account_verification(),
            attempt,
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

    fn write_project_mcp(root: &std::path::Path, toml: &str) {
        let dir = root.join(".rkat");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mcp.toml"), toml).unwrap();
    }

    fn wire_target(
        name: &str,
        url: &str,
        account: Option<&str>,
    ) -> meerkat_contracts::WireMcpAuthTarget {
        meerkat_contracts::WireMcpAuthTarget {
            server_name: name.into(),
            server_url: url.into(),
            oauth_account: account.map(Into::into),
            oauth_account_selection: None,
        }
    }

    const CONFIGURED_MCP: &str = r#"
[[servers]]
name = "glean"
url = "https://glean.example/mcp"
oauth_account = "subject-7"

[[servers]]
name = "static"
url = "https://static.example/mcp"
headers = { Authorization = "Bearer fixed" }

[[servers]]
name = "local"
command = "true"
"#;

    const SELECTED_MCP: &str = r#"
[[servers]]
name = "legacy"
url = "https://legacy.example/mcp"

[[servers]]
name = "discover"
url = "https://discover.example/mcp"
oauth_account_selection = "discover"

[[servers]]
name = "unverified"
url = "https://unverified.example/mcp"
oauth_account_selection = "unverified"
"#;

    #[tokio::test]
    async fn only_host_configuration_selects_discover_or_unverified() {
        use meerkat_contracts::WireMcpAccountSelection as Wire;
        let root = tempfile::tempdir().unwrap();
        write_project_mcp(root.path(), SELECTED_MCP);
        let resolve = |name: &str, url: &str, selection: Option<Wire>| {
            let mut target = wire_target(name, url, None);
            target.oauth_account_selection = selection;
            let root = root.path().to_path_buf();
            async move { resolve_configured_mcp_target(&target, Some(&root), None).await }
        };
        // The configured mode resolves whether or not the caller names it.
        for named in [None, Some(Wire::Unverified)] {
            let target = resolve("unverified", "https://unverified.example/mcp", named)
                .await
                .unwrap();
            assert_eq!(target.selection(), &McpAccountSelection::Unverified);
            assert_eq!(
                mcp_auth_target_to_wire(&target).oauth_account_selection,
                Some(Wire::Unverified)
            );
        }
        let discover = resolve("discover", "https://discover.example/mcp", None)
            .await
            .unwrap();
        assert_eq!(discover.selection(), &McpAccountSelection::Discover);
        // A request can neither opt a legacy target in nor change a mode.
        for (name, url, requested) in [
            ("legacy", "https://legacy.example/mcp", Wire::Unverified),
            ("legacy", "https://legacy.example/mcp", Wire::Discover),
            ("discover", "https://discover.example/mcp", Wire::Unverified),
            (
                "unverified",
                "https://unverified.example/mcp",
                Wire::Discover,
            ),
        ] {
            assert!(
                matches!(
                    resolve(name, url, Some(requested)).await,
                    Err(HostAuthError::McpTarget(
                        HostMcpTargetRefusal::AccountMismatch { .. }
                    ))
                ),
                "{name} accepted a requested {requested:?}"
            );
        }
        let legacy = resolve("legacy", "https://legacy.example/mcp", None)
            .await
            .unwrap();
        assert_eq!(legacy.selection(), &McpAccountSelection::Legacy);
    }

    #[test]
    fn mcp_status_reports_account_verification_and_never_an_unverified_account() {
        use meerkat_contracts::WireMcpAccountVerification as Wire;
        use meerkat_core::mcp_config::McpOAuthAccountSelection as Selection;
        let base = McpServerIdentity::from_server_config("s", "https://s.example/mcp");
        let configured = |selection| {
            let mut config = meerkat_core::McpServerConfig::streamable_http(
                "s",
                "https://s.example/mcp",
                std::collections::HashMap::new(),
            );
            if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut config.transport
            {
                http.oauth_account_selection = Some(selection);
            }
            McpServerIdentity::from_config(&config).unwrap()
        };
        for (target, expected) in [
            (base.clone(), Wire::Legacy),
            (
                base.with_expected_account("subject-7").unwrap(),
                Wire::Verified,
            ),
            (configured(Selection::Discover), Wire::Verified),
            (configured(Selection::Unverified), Wire::Unverified),
        ] {
            let status = HostMcpAuthStatus {
                account_verification: target.account_verification(),
                target,
                phase: HostMcpAuthPhase::Authorized,
                expires_at: None,
                account_id: None,
                attempt: None,
            };
            let wire = serde_json::to_value(status.to_wire()).unwrap();
            assert_eq!(wire["account_verification"], serde_json::json!(expected));
            assert!(wire.get("account_id").is_none());
        }
    }

    #[tokio::test]
    async fn configured_mcp_target_resolves_from_config_not_request() {
        let root = tempfile::tempdir().unwrap();
        write_project_mcp(root.path(), CONFIGURED_MCP);
        for account in [None, Some("subject-7")] {
            let target = resolve_configured_mcp_target(
                &wire_target("glean", "https://glean.example/mcp", account),
                Some(root.path()),
                None,
            )
            .await
            .unwrap();
            assert_eq!(target.server_url(), "https://glean.example/mcp");
            assert_eq!(target.expected_account(), Some("subject-7"));
        }
    }

    #[tokio::test]
    async fn unconfigured_or_mismatched_mcp_targets_are_refused() {
        let root = tempfile::tempdir().unwrap();
        write_project_mcp(root.path(), CONFIGURED_MCP);
        let refusal = |target| {
            let root = root.path().to_path_buf();
            async move {
                match resolve_configured_mcp_target(&target, Some(&root), None).await {
                    Err(HostAuthError::McpTarget(refusal)) => refusal,
                    other => panic!("expected a typed MCP target refusal, got {other:?}"),
                }
            }
        };
        assert!(matches!(
            refusal(wire_target("unknown", "https://glean.example/mcp", None)).await,
            HostMcpTargetRefusal::UnknownServer { .. }
        ));
        assert!(matches!(
            refusal(wire_target("glean", "https://attacker.example/mcp", None)).await,
            HostMcpTargetRefusal::UrlMismatch { .. }
        ));
        assert!(matches!(
            refusal(wire_target(
                "glean",
                "https://glean.example/mcp",
                Some("other")
            ))
            .await,
            HostMcpTargetRefusal::AccountMismatch { .. }
        ));
        assert!(matches!(
            refusal(wire_target("static", "https://static.example/mcp", None)).await,
            HostMcpTargetRefusal::NotOAuthCapable { .. }
        ));
        assert!(matches!(
            refusal(wire_target("local", "true", None)).await,
            HostMcpTargetRefusal::NotOAuthCapable { .. }
        ));
        let empty = tempfile::tempdir().unwrap();
        assert!(matches!(
            resolve_configured_mcp_target(
                &wire_target("glean", "https://glean.example/mcp", None),
                Some(empty.path()),
                None,
            )
            .await,
            Err(HostAuthError::McpTarget(
                HostMcpTargetRefusal::UnknownServer { .. }
            ))
        ));
    }
}

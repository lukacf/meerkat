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
    ConnectorAccountStrategy, ConnectorAuthPhase, ConnectorAuthStatus, ConnectorClientAuthProfile,
    ConnectorLoginComplete, ConnectorLoginError, ConnectorLoginStart, ConnectorOAuthAuthority,
    ConnectorOAuthCallback, ConnectorOAuthTarget, ConnectorStrategies, ConnectorVerifiedAccount,
};
use meerkat_providers::connector_oauth::{AccountSelection, ScopeEvidence};
use meerkat_providers::mcp_oauth::{
    McpAccountSelection, McpOAuthAccountStrategy, McpOAuthAuthority, McpOAuthCallback,
    McpOAuthError, McpOAuthLoginComplete, McpOAuthLoginStart, McpOAuthLoopbackBegin,
    McpOAuthNetworkBounds, McpOAuthPendingAttempt, McpOAuthRegisteredCallback,
    McpRegisteredRedirectBase, McpServerIdentity, OidcUserInfoAccountStrategy,
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
    /// Set when the credential store did not answer this non-blocking read;
    /// `phase` then reflects the live lease alone and never claims absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_observation: Option<meerkat_core::CredentialObservation>,
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
///
/// Its credentials are the realm-less shared slot; a host serving a realm
/// uses [`default_mcp_auth_resolver_in_realm`].
#[cfg(feature = "mcp")]
pub fn default_mcp_auth_resolver(
    persistence: Option<ProviderAuthPersistence>,
    authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
) -> Option<Arc<dyn meerkat_mcp::McpAuthResolver>> {
    mcp_auth_resolver_for(HostAuthService::new(persistence?, authority))
}

/// [`default_mcp_auth_resolver`] scoped to the host runtime's `realm`: every
/// MCP credential it resolves, refreshes or requests is that realm's own.
#[cfg(feature = "mcp")]
pub fn default_mcp_auth_resolver_in_realm(
    persistence: Option<ProviderAuthPersistence>,
    authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
    realm: RealmId,
) -> Option<Arc<dyn meerkat_mcp::McpAuthResolver>> {
    mcp_auth_resolver_for(HostAuthService::new(persistence?, authority).with_mcp_realm(realm))
}

#[cfg(feature = "mcp")]
fn mcp_auth_resolver_for(
    service: HostAuthService,
) -> Option<Arc<dyn meerkat_mcp::McpAuthResolver>> {
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
    /// The realm config and an `mcp.toml` scope define the server's name
    /// differently, so it has no single configured definition.
    #[error(transparent)]
    DefinitionConflict(meerkat_core::mcp_config::McpServerDefinitionConflict),
    /// A realm server holds an environment reference (`${`); realm servers
    /// are literal, so the realm's MCP set does not resolve.
    #[error(transparent)]
    RealmServerEnvReference(meerkat_core::mcp_config::McpRealmServerEnvReference),
    /// A configured server's access policy is invalid, so the realm's MCP
    /// set does not resolve.
    #[error(transparent)]
    InvalidAccessPolicy(meerkat_core::McpServerAccessError),
    /// The server is defined only in an `mcp.toml` a confined process can
    /// write, so it is not a server the host installed: nothing is
    /// discovered, registered or stored for it
    /// ([`resolve_configured_mcp_target_in_realm_trusting`]).
    #[error("MCP server '{server_name}' is not configured: {refusal}")]
    Refused {
        server_name: String,
        refusal: meerkat_core::confinement::ConfinementRefusal,
    },
}

impl HostMcpTargetRefusal {
    /// Whether this refuses the caller's request rather than reporting an
    /// unreadable or contradictory configuration.
    pub fn is_refusal(&self) -> bool {
        !matches!(
            self,
            Self::ConfigUnavailable(_)
                | Self::DefinitionConflict(_)
                | Self::RealmServerEnvReference(_)
                | Self::InvalidAccessPolicy(_)
        )
    }
}

/// Resolve a wire MCP target against the configured MCP servers at the
/// host's convention roots (project wins over user, as for sessions and the
/// CLI). The server name must be configured, `server_url` must equal its
/// configured URL, the server must use OAuth login, and a requested
/// `oauth_account` must equal the configured one. The identity is built from
/// the configuration, never from the request.
///
/// Every `mcp.toml` is trusted here, so a definition a confined process
/// planted resolves: a host that confines its agents' launches resolves
/// through [`resolve_configured_mcp_target_in_realm_trusting`].
#[deprecated(
    since = "0.9.2",
    note = "trusts every mcp.toml, including one a confined process can write; use `resolve_configured_mcp_target_in_realm_trusting` with the host's `HostConfinement::untrusted_source`"
)]
pub async fn resolve_configured_mcp_target(
    target: &meerkat_contracts::WireMcpAuthTarget,
    context_root: Option<&std::path::Path>,
    user_config_root: Option<&std::path::Path>,
) -> Result<McpServerIdentity, HostAuthError> {
    #[allow(deprecated)]
    resolve_configured_mcp_target_in_realm(target, &[], context_root, user_config_root).await
}

/// [`resolve_configured_mcp_target`] for a host serving a realm: the
/// configured servers are the realm's MCP set
/// ([`McpConfig::effective_servers_from_roots`](meerkat_core::McpConfig::effective_servers_from_roots)),
/// so a server installed in the realm's config resolves like one in an
/// `mcp.toml` file. `realm_servers` is the realm's composed
/// `Config::tools.mcp_servers`.
///
/// Every `mcp.toml` is trusted here, as in
/// [`resolve_configured_mcp_target`].
#[deprecated(
    since = "0.9.2",
    note = "trusts every mcp.toml, including one a confined process can write; use `resolve_configured_mcp_target_in_realm_trusting` with the host's `HostConfinement::untrusted_source`"
)]
pub async fn resolve_configured_mcp_target_in_realm(
    target: &meerkat_contracts::WireMcpAuthTarget,
    realm_servers: &[meerkat_core::McpServerConfig],
    context_root: Option<&std::path::Path>,
    user_config_root: Option<&std::path::Path>,
) -> Result<McpServerIdentity, HostAuthError> {
    resolve_configured_mcp_target_in_realm_trusting(
        target,
        realm_servers,
        context_root,
        user_config_root,
        &|_| false,
    )
    .await
}

/// [`resolve_configured_mcp_target_in_realm`] for a host whose confined
/// processes may write an `mcp.toml` file: `untrusted` names each file they
/// can write ([`crate::HostConfinement::untrusted_source`]), as for the
/// host's sessions
/// ([`McpConfig::effective_servers_from_roots_trusting`](meerkat_core::McpConfig::effective_servers_from_roots_trusting)).
/// A server only such a file defines is not one the host installed: it is
/// refused ([`HostMcpTargetRefusal::Refused`]) before anything is
/// discovered, registered or stored for it, so a definition a confined
/// process planted cannot point a login at a server of its choosing. A
/// definition a trusted source states identically resolves from that
/// source.
pub async fn resolve_configured_mcp_target_in_realm_trusting(
    target: &meerkat_contracts::WireMcpAuthTarget,
    realm_servers: &[meerkat_core::McpServerConfig],
    context_root: Option<&std::path::Path>,
    user_config_root: Option<&std::path::Path>,
    untrusted: &(dyn Fn(&std::path::Path) -> bool + Send + Sync),
) -> Result<McpServerIdentity, HostAuthError> {
    use meerkat_core::mcp_config::{
        EffectiveMcpServersError, McpConfig, McpTransportConfig, McpTransportKind,
    };
    let server_name = || target.server_name.clone();
    let servers = McpConfig::effective_servers_from_roots_trusting(
        realm_servers,
        context_root,
        user_config_root,
        untrusted,
    )
    .await
    .map_err(|error| match error {
        EffectiveMcpServersError::Config(error) => HostMcpTargetRefusal::ConfigUnavailable(error),
        EffectiveMcpServersError::Conflict(conflict) => {
            HostMcpTargetRefusal::DefinitionConflict(conflict)
        }
        EffectiveMcpServersError::RealmEnvReference(reference) => {
            HostMcpTargetRefusal::RealmServerEnvReference(reference)
        }
        EffectiveMcpServersError::Access(error) => HostMcpTargetRefusal::InvalidAccessPolicy(error),
    })?;
    if servers
        .refused
        .iter()
        .any(|refused| refused.server.name == target.server_name)
    {
        return Err(HostMcpTargetRefusal::Refused {
            server_name: server_name(),
            refusal: meerkat_core::confinement::ConfinementRefusal::UntrustedConfigurationSource,
        }
        .into());
    }
    let server = servers
        .admitted
        .into_iter()
        .map(|configured| configured.server)
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
        client_auth_profile: target.client_auth_profile.clone(),
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

/// Fixed public text for an infrastructure failure. The internal detail
/// (store paths, persistence errors) stays in protected diagnostics.
const INFRASTRUCTURE_PUBLIC_MESSAGE: &str = "auth infrastructure failure";

/// Fixed public text for a refused credential access, one per public reason.
/// The store's backend and platform detail stays in protected diagnostics.
const CREDENTIAL_ACCESS_DENIED_PUBLIC_MESSAGE: &str = "access to the stored credential was denied";
const CREDENTIAL_INTERACTION_REQUIRED_PUBLIC_MESSAGE: &str =
    "access to the stored credential needs user interaction";
const CREDENTIAL_ACCESS_PENDING_PUBLIC_MESSAGE: &str =
    "access to the stored credential is still pending";
const CREDENTIAL_STORE_UNAVAILABLE_PUBLIC_MESSAGE: &str = "the credential store is unavailable";

impl HostAuthError {
    /// The typed public reason of this error: the one mapping every auth
    /// surface projects. Exhaustive on purpose, so a new native error must
    /// choose its reason.
    pub fn reason(&self) -> meerkat_contracts::WireAuthErrorReason {
        use meerkat_contracts::WireAuthErrorReason as R;
        match self {
            Self::Target(error) => connection_target_reason(error),
            Self::WriteOwner(error) => match error {
                WriteOwnerError::Inherited { .. } => R::BindingInherited,
                WriteOwnerError::Unknown { .. } => R::BindingNotFound,
                WriteOwnerError::Chain(_) => R::InvalidTarget,
            },
            Self::OAuthTarget(_) | Self::ConnectorTarget(_) => R::InvalidTarget,
            Self::OAuthFlow(error) => oauth_flow_reason(error),
            Self::OAuthExchange(_) => R::UpstreamFailure,
            Self::CredentialMutation(error) => credential_mutation_reason(error),
            Self::TokenStore(error) => error
                .credential_access()
                .map_or(R::Infrastructure, |(refusal, _)| {
                    credential_access_reason(refusal)
                }),
            Self::Factory(_)
            | Self::PersistenceUnavailable
            | Self::Lifecycle(_)
            | Self::StatusRehydrate(_) => R::Infrastructure,
            Self::BrowserFlowUnsupported(_) | Self::DeviceFlowUnsupported(_) => R::FlowUnsupported,
            Self::McpOAuth(error) => mcp_oauth_reason(error),
            Self::McpTarget(refusal) => match refusal {
                HostMcpTargetRefusal::UnknownServer { .. }
                | HostMcpTargetRefusal::Refused { .. } => R::McpServerNotConfigured,
                HostMcpTargetRefusal::UrlMismatch { .. }
                | HostMcpTargetRefusal::NotOAuthCapable { .. } => R::McpServerMismatch,
                HostMcpTargetRefusal::AccountMismatch { .. } => R::AccountMismatch,
                HostMcpTargetRefusal::ConfigUnavailable(_)
                | HostMcpTargetRefusal::DefinitionConflict(_)
                | HostMcpTargetRefusal::RealmServerEnvReference(_)
                | HostMcpTargetRefusal::InvalidAccessPolicy(_) => R::ConfigurationInvalid,
            },
            Self::Connector(error) => connector_reason(error),
        }
    }

    /// The stable sub-kind every auth surface projects beside
    /// [`Self::reason`], for the refusals a host must tell apart within one
    /// reason.
    pub fn detail(&self) -> Option<meerkat_contracts::WireAuthErrorDetail> {
        use meerkat_contracts::WireAuthErrorDetail as D;
        match self {
            Self::McpOAuth(McpOAuthError::RemoteCustodyRequired { .. })
            | Self::Connector(ConnectorLoginError::RemoteCustodyRequired) => {
                Some(D::RemoteCustodyRequired)
            }
            // Whether the authorization code had been sent when the stage
            // ran out of time.
            Self::McpOAuth(McpOAuthError::NetworkTimedOut { stage, .. }) => {
                Some(if stage.code_sent() {
                    D::UpstreamTimeoutAfterCode
                } else {
                    D::UpstreamTimeout
                })
            }
            _ => None,
        }
    }

    /// The error text a public surface may show. Infrastructure failures and
    /// refused credential accesses use fixed text per reason; their detail
    /// goes only to protected diagnostics.
    pub fn public_message(&self) -> String {
        match fixed_public_message(self.reason()) {
            Some(text) => text.to_owned(),
            None => self.to_string(),
        }
    }

    /// Whether the public text withholds this error's detail. A surface logs
    /// the full error to protected diagnostics when it does.
    pub fn withholds_detail(&self) -> bool {
        fixed_public_message(self.reason()).is_some()
    }
}

/// The fixed public text of a reason whose detail is withheld, or `None`
/// when the error's own text may be shown.
fn fixed_public_message(reason: meerkat_contracts::WireAuthErrorReason) -> Option<&'static str> {
    use meerkat_contracts::WireAuthErrorReason as R;
    match reason {
        R::Infrastructure => Some(INFRASTRUCTURE_PUBLIC_MESSAGE),
        R::CredentialAccessDenied => Some(CREDENTIAL_ACCESS_DENIED_PUBLIC_MESSAGE),
        R::CredentialInteractionRequired => Some(CREDENTIAL_INTERACTION_REQUIRED_PUBLIC_MESSAGE),
        R::CredentialAccessPending => Some(CREDENTIAL_ACCESS_PENDING_PUBLIC_MESSAGE),
        R::CredentialStoreUnavailable => Some(CREDENTIAL_STORE_UNAVAILABLE_PUBLIC_MESSAGE),
        _ => None,
    }
}

fn connection_target_reason(
    error: &meerkat_core::ConnectionTargetError,
) -> meerkat_contracts::WireAuthErrorReason {
    use meerkat_contracts::WireAuthErrorReason as R;
    use meerkat_core::ConnectionTargetError as E;
    match error {
        E::UnknownRealm(_) => R::RealmNotFound,
        E::MissingDefaultBinding { .. } => R::BindingNotFound,
        E::BindingInvalid { .. } => R::BindingInvalid,
        E::RealmConfigInvalid { .. } => R::ConfigurationInvalid,
        E::MissingRealm
        | E::InvalidRealmId { .. }
        | E::InvalidBindingId { .. }
        | E::ProviderMismatch { .. }
        | E::AmbiguousCredentialAccountBindings { .. }
        | E::RealmChain(_) => R::InvalidTarget,
    }
}

fn oauth_flow_reason(error: &OAuthFlowError) -> meerkat_contracts::WireAuthErrorReason {
    use meerkat_contracts::WireAuthErrorReason as R;
    match error {
        OAuthFlowError::Missing => R::AttemptMissing,
        OAuthFlowError::BrowserIdentityMismatch
        | OAuthFlowError::ProviderMismatch { .. }
        | OAuthFlowError::RedirectUriMismatch
        | OAuthFlowError::TargetMismatch { .. } => R::AttemptMismatch,
        OAuthFlowError::Connector(refusal) => connector_refusal_reason(*refusal),
        OAuthFlowError::DevicePollInProgress => R::DevicePollInProgress,
        OAuthFlowError::DeviceCodeAlreadyAdmitted => R::DeviceCodeAlreadyAdmitted,
        OAuthFlowError::DeviceExpiryOutOfRange => R::DeviceExpiryInvalid,
        OAuthFlowError::RegistryProjectionMissing { .. }
        | OAuthFlowError::StateGenerationFailed
        | OAuthFlowError::LifecycleRejected { .. }
        | OAuthFlowError::PersistenceFailed { .. } => R::Infrastructure,
    }
}

fn connector_refusal_reason(
    refusal: meerkat_providers::connector_oauth::ConnectorOAuthRefusal,
) -> meerkat_contracts::WireAuthErrorReason {
    use meerkat_contracts::WireAuthErrorReason as R;
    use meerkat_providers::connector_oauth::ConnectorOAuthRefusal as C;
    match refusal {
        C::InvalidDescriptor => R::InvalidTarget,
        C::DescriptorMismatch => R::AttemptMismatch,
        C::AccountMismatch => R::AccountMismatch,
        C::MissingScopes => R::MissingScopes,
        C::CredentialMismatch => R::CredentialMismatch,
        C::VerificationUnavailable => R::VerificationUnavailable,
        // The host's own egress profile does not admit the endpoint.
        C::EgressRefused(_) => R::InvalidTarget,
    }
}

fn slot_refusal_reason(
    refusal: meerkat_providers::auth_store::CredentialSlotRefusal,
) -> meerkat_contracts::WireAuthErrorReason {
    use meerkat_contracts::WireAuthErrorReason as R;
    use meerkat_providers::auth_store::CredentialSlotRefusal as S;
    match refusal {
        S::Occupied => R::SlotOccupied,
        S::AccountMismatch => R::SlotAccountMismatch,
        S::ContextMismatch => R::SlotContextMismatch,
        S::ModeMismatch => R::SlotModeMismatch,
        S::UnverifiedConnectorPublication => R::UnverifiedConnectorPublication,
    }
}

/// The public reason of a refused credential access. An unobserved or
/// unsupported access is reported as the store being unavailable.
fn credential_access_reason(
    refusal: meerkat_providers::auth_store::TokenStoreRefusal,
) -> meerkat_contracts::WireAuthErrorReason {
    use meerkat_contracts::WireAuthErrorReason as R;
    use meerkat_providers::auth_store::TokenStoreRefusal as Refusal;
    match refusal {
        Refusal::Denied => R::CredentialAccessDenied,
        Refusal::InteractionRequired => R::CredentialInteractionRequired,
        Refusal::Pending => R::CredentialAccessPending,
        Refusal::Unavailable | Refusal::NotObserved | Refusal::Unsupported => {
            R::CredentialStoreUnavailable
        }
    }
}

fn credential_mutation_reason(
    error: &CredentialMutationError,
) -> meerkat_contracts::WireAuthErrorReason {
    match error {
        CredentialMutationError::SlotRefused(refusal) => slot_refusal_reason(*refusal),
        CredentialMutationError::CredentialAccess { refusal, .. } => {
            credential_access_reason(*refusal)
        }
        CredentialMutationError::StalePreparation
        | CredentialMutationError::Operation(_)
        | CredentialMutationError::TokenStore(_)
        | CredentialMutationError::AuthLifecycle(_)
        | CredentialMutationError::Cancelled
        | CredentialMutationError::LockFailed(_)
        | CredentialMutationError::RollbackFailed(_) => {
            meerkat_contracts::WireAuthErrorReason::Infrastructure
        }
    }
}

fn mcp_oauth_reason(error: &McpOAuthError) -> meerkat_contracts::WireAuthErrorReason {
    use meerkat_contracts::WireAuthErrorReason as R;
    match error {
        McpOAuthError::InvalidAccountSelection
        | McpOAuthError::UnsupportedAccountSelection
        | McpOAuthError::RealmMismatch { .. }
        | McpOAuthError::TokenKey { .. } => R::InvalidTarget,
        McpOAuthError::AccountSelectionRequired => R::AccountSelectionRequired,
        McpOAuthError::Verification(refusal) => connector_refusal_reason(*refusal),
        McpOAuthError::Flow(error) => oauth_flow_reason(error),
        // A confirmed retirement of the attempt the wait served.
        McpOAuthError::AttemptRetired { .. } => R::AttemptMissing,
        McpOAuthError::AdmissionReadFailed { read, .. } => mcp_oauth_reason(read),
        McpOAuthError::MissingStoredToken { .. }
        | McpOAuthError::HumanAuthorizationRequired { .. } => R::AuthorizationRequired,
        McpOAuthError::Callback { .. } => R::CallbackUnavailable,
        // A registered attempt's completion without the host's custody; the
        // error data's `detail` tells it apart from a malformed target.
        McpOAuthError::RemoteCustodyRequired { .. } => R::InvalidTarget,
        McpOAuthError::DiscoveryFailed { .. }
        | McpOAuthError::RegistrationFailed { .. }
        | McpOAuthError::TokenExchangeFailed { .. }
        | McpOAuthError::RefreshFailed { .. } => R::UpstreamFailure,
        // The host's own egress profile does not admit the destination.
        McpOAuthError::EgressRefused { .. } => R::InvalidTarget,
        McpOAuthError::ReauthRequired { .. } => R::ReauthRequired,
        // The slot holds a credential this login may not replace.
        McpOAuthError::DisconnectRequired { .. } => R::SlotOccupied,
        McpOAuthError::CredentialSlot { refusal, .. } => slot_refusal_reason(*refusal),
        McpOAuthError::CredentialAccess { refusal, .. } => credential_access_reason(*refusal),
        McpOAuthError::StalePreparation
        | McpOAuthError::TokenStore(_)
        | McpOAuthError::MissingStoredMetadata { .. }
        | McpOAuthError::AuthLifecycle { .. }
        | McpOAuthError::EgressUnavailable { .. }
        | McpOAuthError::HttpClientUnavailable(_) => R::Infrastructure,
        // A stage of the provider's servers got no answer within its bound;
        // the error data's `detail` says whether the code had been sent.
        McpOAuthError::NetworkTimedOut { .. } => R::UpstreamFailure,
        // Neither a success nor a failure: the commit may still land.
        McpOAuthError::CommitUnsettled { .. } => R::CommitUnsettled,
    }
}

fn connector_reason(error: &ConnectorLoginError) -> meerkat_contracts::WireAuthErrorReason {
    use meerkat_contracts::WireAuthErrorReason as R;
    match error {
        ConnectorLoginError::UnknownStrategy => R::UnknownStrategy,
        ConnectorLoginError::InvalidRedirect | ConnectorLoginError::RemoteCustodyRequired => {
            R::InvalidTarget
        }
        ConnectorLoginError::Verification(refusal) => connector_refusal_reason(*refusal),
        ConnectorLoginError::Flow(error) => oauth_flow_reason(error),
        ConnectorLoginError::Slot(refusal) => slot_refusal_reason(*refusal),
        ConnectorLoginError::DiscoveryFailed(_)
        | ConnectorLoginError::TokenExchangeFailed
        | ConnectorLoginError::RefreshFailed(_) => R::UpstreamFailure,
        ConnectorLoginError::ReauthRequired => R::ReauthRequired,
        ConnectorLoginError::CredentialAccess { refusal, .. } => credential_access_reason(*refusal),
        ConnectorLoginError::StalePreparation
        | ConnectorLoginError::TokenStore(_)
        | ConnectorLoginError::AuthLifecycle(_)
        | ConnectorLoginError::HttpClientUnavailable(_) => R::Infrastructure,
        ConnectorLoginError::ClientConfig(_)
        | ConnectorLoginError::ClientSecretUnavailable { .. } => R::ConfigurationInvalid,
    }
}

/// Injectable native-host authentication facade.
#[derive(Clone)]
pub struct HostAuthService {
    persistence: ProviderAuthPersistence,
    authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
    /// Token and device-code requests follow no redirects. A build failure
    /// is kept and every exchange fails with it.
    http: Result<reqwest::Client, meerkat_providers::auth_oauth::CredentialHttpClientUnavailable>,
    mcp_account_strategy: Arc<dyn McpOAuthAccountStrategy>,
    connector_strategies: ConnectorStrategies,
    /// The runtime realm whose MCP credentials this service owns.
    mcp_realm: Option<RealmId>,
    /// The host's registered `https` redirect base for MCP logins (host
    /// configuration), when the host serves a hosted callback.
    mcp_registered_redirect_base: Option<McpRegisteredRedirectBase>,
    /// How long the MCP OAuth owner waits on the network.
    mcp_network_bounds: McpOAuthNetworkBounds,
    /// Host-installed connector client-auth profiles (secrets from host
    /// configuration).
    connector_client_auth: Vec<ConnectorClientAuthProfile>,
    /// Provider protocols a client-auth profile may name.
    connector_protocols: meerkat_providers::connector_protocol::ConnectorProtocols,
    /// Where the connector owner reads the host's current client-auth
    /// profiles at each use (normally the realm's configuration).
    connector_client_source:
        Option<Arc<dyn meerkat_providers::connector_login::ConnectorClientAuthSource>>,
    /// The one connector OAuth owner, built on first use and shared by every
    /// clone of this service. A connector builder starts a new one.
    connector_authority: Arc<std::sync::OnceLock<ConnectorOAuthAuthority>>,
}

impl HostAuthService {
    pub fn new(
        persistence: ProviderAuthPersistence,
        authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
    ) -> Self {
        Self {
            persistence,
            authority,
            http: meerkat_providers::auth_oauth::credential_http_client(),
            mcp_account_strategy: Arc::new(OidcUserInfoAccountStrategy::new()),
            connector_strategies: ConnectorStrategies::with_defaults(),
            mcp_realm: None,
            mcp_registered_redirect_base: None,
            mcp_network_bounds: McpOAuthNetworkBounds::default(),
            connector_client_auth: Vec::new(),
            connector_protocols:
                meerkat_providers::connector_protocol::ConnectorProtocols::with_defaults(),
            connector_client_source: None,
            connector_authority: Arc::default(),
        }
    }

    /// Scope MCP credentials to the host runtime's `realm`: its MCP
    /// authority binds every target to that realm, so each realm has its own
    /// MCP logins, refresh lineage and logout. Native hosts serving a realm
    /// set this; the credential stored before realm scoping is left unused.
    pub fn with_mcp_realm(mut self, realm: RealmId) -> Self {
        self.mcp_realm = Some(realm);
        self
    }

    /// Serve MCP logins through the host's own `https` callback under
    /// `base` as well as through loopback redirects. Host configuration
    /// only: each target's exact redirect is derived from it
    /// ([`Self::mcp_registered_redirect_uri`]).
    pub fn with_mcp_registered_redirect_base(mut self, base: McpRegisteredRedirectBase) -> Self {
        self.mcp_registered_redirect_base = Some(base);
        self
    }

    /// The host's registered redirect base, if it serves one.
    pub fn mcp_registered_redirect_base(&self) -> Option<&McpRegisteredRedirectBase> {
        self.mcp_registered_redirect_base.as_ref()
    }

    /// Bound the MCP OAuth owner's network waits by `bounds` (see
    /// [`McpOAuthNetworkBounds`]; the default applies otherwise). They hold
    /// for the MCP authority's own client and its account strategy.
    pub fn with_mcp_network_bounds(mut self, bounds: McpOAuthNetworkBounds) -> Self {
        self.mcp_network_bounds = bounds;
        self
    }

    /// The exact registered `https` redirect of `target` in this service's
    /// realm, when the host serves a registered base: the URI the
    /// authorization server redirects to, which the host routes to its
    /// callback endpoint. Local and secret-free.
    pub fn mcp_registered_redirect_uri(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<String>, HostAuthError> {
        let Some(base) = &self.mcp_registered_redirect_base else {
            return Ok(None);
        };
        Ok(Some(
            self.mcp_oauth_authority()?
                .registered_redirect_uri(target, base)?,
        ))
    }

    /// Replace the provider protocols client-auth profiles may name. The
    /// default set is the built-in Slack and Salesforce protocols.
    pub fn with_connector_protocols(
        mut self,
        protocols: meerkat_providers::connector_protocol::ConnectorProtocols,
    ) -> Self {
        self.connector_protocols = protocols;
        self.connector_authority = Arc::default();
        self
    }

    /// Install the host's connector client-auth profiles (see
    /// [`ConnectorClientAuthProfile`]). A connector login names one by id.
    pub fn with_connector_client_auth_profiles(
        mut self,
        profiles: impl IntoIterator<Item = ConnectorClientAuthProfile>,
    ) -> Self {
        self.connector_client_auth = profiles.into_iter().collect();
        self.connector_authority = Arc::default();
        self
    }

    /// Read the connector client-auth profiles from `source` at each connector
    /// login and refresh, next to any installed directly.
    pub fn with_connector_client_source(
        mut self,
        source: Arc<dyn meerkat_providers::connector_login::ConnectorClientAuthSource>,
    ) -> Self {
        self.connector_client_source = Some(source);
        self.connector_authority = Arc::default();
        self
    }

    /// Use the connector clients `realm` declares in its configuration (its
    /// own document folded over its parent chain), read at each use.
    pub fn with_realm_connector_clients(
        self,
        source: Arc<dyn meerkat_core::RealmConfigSource>,
        realm: RealmId,
    ) -> Self {
        self.with_connector_client_source(Arc::new(
            crate::connector_clients::RealmConnectorClients::from_realm_source(source, realm),
        ))
    }

    /// Install a connector account strategy (keyed by its strategy id) next
    /// to the default OIDC UserInfo strategy.
    pub fn with_connector_strategy(mut self, strategy: Arc<dyn ConnectorAccountStrategy>) -> Self {
        self.connector_strategies = self.connector_strategies.with(strategy);
        self.connector_authority = Arc::default();
        self
    }

    /// Whether this service installs a connector client-auth profile whose
    /// canonical meaning is exactly `binding` (`<id>@<sha256>`,
    /// [`ConnectorClientAuthProfile::binding`]).
    pub fn has_connector_client_auth_binding(&self, binding: &str) -> bool {
        self.connector_client_auth
            .iter()
            .any(|profile| profile.binding() == binding)
    }

    /// The native connector OAuth owner bound to this service's persistence,
    /// AuthMachine lease and flow owner. Native hosts use it for bearer
    /// tokens (with refresh); its HTTP client follows no redirects.
    ///
    /// It is built once and retained: every connector method of this
    /// service, and of every clone of it, runs on this same owner, and the
    /// returned value is a handle to it, so attempts admitted through one
    /// clone complete or cancel through another.
    pub fn connector_oauth_authority(&self) -> Result<ConnectorOAuthAuthority, HostAuthError> {
        if let Some(retained) = self.connector_authority.get() {
            return Ok(retained.clone());
        }
        let built = ConnectorOAuthAuthority::new(
            self.persistence.clone(),
            self.authority.oauth_flow_authority(),
            self.connector_strategies.clone(),
        )?
        .with_client_auth_profiles(self.connector_client_auth.iter().cloned())?
        .with_connector_protocols(self.connector_protocols.clone());
        let built = match &self.connector_client_source {
            Some(source) => built.with_client_auth_source(Arc::clone(source)),
            None => built,
        };
        Ok(self.connector_authority.get_or_init(|| built).clone())
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

    /// Complete an admitted connector attempt from the host's callback (a
    /// loopback one, or a registered `https` one with the retained
    /// `host_attempt_handle`). The admitted descriptor must equal `target`'s
    /// facts, and the callback's `redirect_uri` must equal the admitted
    /// attempt's; neither is taken on trust.
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

    /// Retire the connector attempt admitted under `state` for `slot`. `Ok`
    /// means no credential will ever be published from that attempt; a
    /// completion still running for it fails before its commit. A refusal
    /// with the missing-attempt flow error means the attempt is already
    /// terminal (committed, retired or never admitted); the completion's own
    /// result, or [`connector_status`](Self::connector_status), reports which.
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

    /// Passively check a connector slot's stored credential against
    /// `expect`. It loads the persisted tokens (and may rehydrate the slot's
    /// auth state from them) but extracts and uses no token, performs no
    /// refresh and enters no call.
    pub async fn connector_check_expectation(
        &self,
        expect: &meerkat_providers::connector_login::ConnectorCallExpectation,
    ) -> Result<(), HostAuthError> {
        Ok(self
            .connector_oauth_authority()?
            .check_expectation(expect)
            .await?)
    }

    /// The redirect-free token-exchange client, or its typed build failure.
    fn http(&self) -> Result<&reqwest::Client, HostAuthError> {
        self.http
            .as_ref()
            .map_err(|error| HostAuthError::OAuthExchange(OAuthError::from(*error)))
    }

    /// Use `http` for token and device-code requests. It must not follow
    /// redirects (reqwest `redirect::Policy::none()`).
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Ok(http);
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
    /// [`Self::with_http_client`]'s, and waits on the network within this
    /// service's [`Self::with_mcp_network_bounds`].
    pub fn mcp_oauth_authority(&self) -> Result<McpOAuthAuthority, HostAuthError> {
        let authority = McpOAuthAuthority::new(
            self.persistence.clone(),
            self.authority.generated_auth_lease_handle(),
        )
        .with_network_bounds(self.mcp_network_bounds)
        .with_interactive_strategy(
            self.authority.oauth_flow_authority(),
            Arc::clone(&self.mcp_account_strategy),
        )?;
        Ok(match &self.mcp_realm {
            Some(realm) => authority.in_realm(realm.clone()),
            None => authority,
        })
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

    /// Admit one host-driven MCP OAuth attempt on the target's registered
    /// `https` redirect under this service's base: the host's own callback
    /// endpoint, for a host whose users' browsers cannot reach its loopback
    /// address. A started attempt returns a host-only `host_attempt_handle`,
    /// a secret the host keeps server-side with its ceremony and passes to
    /// [`Self::mcp_login_complete_registered`]; a joined projection carries
    /// none. Refused when the service has no registered base. See
    /// [`McpOAuthAuthority::login_start_registered`].
    pub async fn mcp_login_start_registered(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoginStart, HostAuthError> {
        let Some(base) = &self.mcp_registered_redirect_base else {
            return Err(McpOAuthError::Verification(
                meerkat_providers::connector_oauth::ConnectorOAuthRefusal::InvalidDescriptor,
            )
            .into());
        };
        Ok(self
            .mcp_oauth_authority()?
            .login_start_registered(target, base, www_authenticate)
            .await?)
    }

    /// Complete an attempt admitted on a registered redirect from the host's
    /// `https` callback, with the host's retained `host_attempt_handle`. A
    /// callback without that custody is refused and leaves the attempt to
    /// its owner. Returns a secret-free projection. See
    /// [`McpOAuthAuthority::login_complete_registered`].
    pub async fn mcp_login_complete_registered(
        &self,
        target: &McpServerIdentity,
        callback: McpOAuthRegisteredCallback,
    ) -> Result<McpOAuthLoginComplete, HostAuthError> {
        Ok(self
            .mcp_oauth_authority()?
            .login_complete_registered(target, callback)
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

    /// Disconnect one MCP target: retire every attempt pending for it, then
    /// remove its stored credential and release the credential lifecycle.
    /// Nothing is revoked at the provider.
    pub async fn mcp_logout(&self, target: &McpServerIdentity) -> Result<(), HostAuthError> {
        Ok(self.mcp_oauth_authority()?.logout(target).await?)
    }

    /// `target` scoped to the realm whose MCP credentials this service
    /// serves: the exact identity its login, logout and live connections use
    /// for that credential slot (the configured target is realm-less).
    pub fn mcp_target_in_service_realm(
        &self,
        target: &McpServerIdentity,
    ) -> Result<McpServerIdentity, HostAuthError> {
        Ok(target.scoped_to(self.mcp_realm.as_ref())?)
    }

    /// Secret-free authorization status of one MCP target, projected from
    /// its durable credential and its pending attempt. It performs no
    /// refresh and no network I/O.
    pub async fn mcp_status(
        &self,
        target: &McpServerIdentity,
    ) -> Result<HostMcpAuthStatus, HostAuthError> {
        let target = &target.scoped_to(self.mcp_realm.as_ref())?;
        let stored = self
            .persistence
            .token_store()
            .load_for(
                &target.token_key()?,
                meerkat_providers::auth_store::TokenStoreAccess::Status,
            )
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
        let mut credential_observation = None;
        if source_uses_store {
            // Status never waits behind credential work and never starts an
            // operating-system credential read.
            let phase = AuthStatusPhase::from_lease_snapshot(now, &snapshot);
            let read = if phase.is_no_live_lease() {
                match expected_mode {
                    Some(expected_mode) => {
                        let read = meerkat_core::observe_marked_tokens_for_status_for_identity(
                            store.as_ref(),
                            &auth_lease,
                            &resolved.credential_identity,
                            expected_mode,
                            now,
                        )
                        .await
                        .map_err(HostAuthError::StatusRehydrate)?;
                        snapshot = auth_lease.snapshot(&lease_key);
                        Some(read)
                    }
                    None => None,
                }
            } else {
                Some(meerkat_core::CredentialStatusRead::from_observation(
                    store
                        .observe_status(
                            &meerkat_providers::auth_store::TokenKey::from_credential_identity(
                                &resolved.credential_identity,
                            ),
                        )
                        .await?,
                ))
            };
            if let Some(read) = read {
                credential_observation = read.observation();
                stored = read.into_tokens();
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
            credential_observation,
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
            self.http()?,
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
        let device = request_device_code(self.http()?, &oauth.endpoints).await?;
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
            self.http()?,
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
#[allow(deprecated)]
mod tests {
    use super::*;
    use meerkat_core::{
        AuthProfileConfig, BackendProfileConfig, CredentialSourceSpec, ProviderBindingConfig,
        RealmConfigSection,
    };
    use std::sync::Arc;

    #[test]
    fn credential_access_public_messages_preserve_reasons_without_internal_details() {
        use meerkat_contracts::WireAuthErrorReason as Reason;
        use meerkat_providers::auth_store::{TokenStoreAccess, TokenStoreRefusal};

        const DETAIL: &str = "private-credential-detail-canary-150a";
        const BACKEND: &str = "private-credential-backend-canary-150a";
        let cases = [
            (
                HostAuthError::Connector(ConnectorLoginError::CredentialAccess {
                    refusal: TokenStoreRefusal::Denied,
                    detail: DETAIL.into(),
                }),
                Reason::CredentialAccessDenied,
            ),
            (
                HostAuthError::McpOAuth(McpOAuthError::CredentialAccess {
                    refusal: TokenStoreRefusal::InteractionRequired,
                    detail: DETAIL.into(),
                }),
                Reason::CredentialInteractionRequired,
            ),
            (
                HostAuthError::CredentialMutation(CredentialMutationError::CredentialAccess {
                    refusal: TokenStoreRefusal::Pending,
                    detail: DETAIL.into(),
                }),
                Reason::CredentialAccessPending,
            ),
            (
                HostAuthError::TokenStore(TokenStoreError::refused(
                    BACKEND,
                    TokenStoreAccess::Interactive,
                    TokenStoreRefusal::Unavailable,
                    DETAIL,
                )),
                Reason::CredentialStoreUnavailable,
            ),
            (
                HostAuthError::TokenStore(TokenStoreError::BackendFailed {
                    backend: BACKEND,
                    detail: DETAIL.into(),
                }),
                Reason::CredentialStoreUnavailable,
            ),
        ];
        for (error, expected_reason) in cases {
            assert_eq!(error.reason(), expected_reason);
            assert!(error.to_string().contains(DETAIL));
            if matches!(&error, HostAuthError::TokenStore(_)) {
                assert!(error.to_string().contains(BACKEND));
            }
            let public = error.public_message();
            assert!(!public.is_empty());
            assert!(!public.contains(DETAIL), "{expected_reason:?}: {public}");
            assert!(!public.contains(BACKEND), "{expected_reason:?}: {public}");
        }
    }

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

    /// The native MCP OAuth owner states a connection's account from its
    /// local selection alone: a Known selection is its subject at the
    /// server's origin (never the URL's path, query or userinfo); a
    /// discovered, unverified or legacy selection states none.
    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn the_mcp_oauth_owner_states_only_a_known_selection_as_the_account() {
        use meerkat_core::OperationAccount;
        use meerkat_core::mcp_config::{McpOAuthAccountSelection, McpTransportConfig};
        use meerkat_mcp::McpAuthResolver as _;
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let persistence = ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let authority =
            HostAuthService::new(persistence, runtime.provider_auth_runtime_authority())
                .mcp_oauth_authority()
                .expect("the MCP OAuth owner");
        let target = |account: Option<&str>, selection: Option<McpOAuthAccountSelection>| {
            let mut server = meerkat_core::McpServerConfig::streamable_http(
                "mail",
                "https://user:pw@mcp.example:8443/api?token=x",
                std::collections::HashMap::new(),
            );
            if let McpTransportConfig::Http(http) = &mut server.transport {
                http.oauth_account = account.map(str::to_owned);
                http.oauth_account_selection = selection;
            }
            meerkat_providers::mcp_oauth::McpServerIdentity::from_config(&server)
                .expect("the server's OAuth target")
        };
        assert_eq!(
            authority.selected_account(&target(Some("subject-7"), None)),
            OperationAccount::selected("https://mcp.example:8443", "subject-7")
        );
        for unstated in [
            target(None, None),
            target(None, Some(McpOAuthAccountSelection::Discover)),
            target(None, Some(McpOAuthAccountSelection::Unverified)),
        ] {
            assert_eq!(
                authority.selected_account(&unstated),
                OperationAccount::Unavailable
            );
        }
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

    /// Login resolves a server installed only in the realm's own config, is
    /// refused for it in a realm without it, and refuses a name the realm and
    /// an `mcp.toml` define differently.
    #[tokio::test]
    async fn realm_installed_mcp_target_resolves_only_in_its_realm() {
        let realm_server = meerkat_core::McpServerConfig::streamable_http(
            "realm-docs",
            "https://docs.example/mcp",
            std::collections::HashMap::new(),
        );
        let target = wire_target("realm-docs", "https://docs.example/mcp", None);
        let roots = tempfile::tempdir().unwrap();

        let resolved = resolve_configured_mcp_target_in_realm(
            &target,
            std::slice::from_ref(&realm_server),
            Some(roots.path()),
            None,
        )
        .await
        .unwrap();
        assert_eq!(resolved.server_url(), "https://docs.example/mcp");
        assert!(matches!(
            resolve_configured_mcp_target_in_realm(&target, &[], Some(roots.path()), None).await,
            Err(HostAuthError::McpTarget(
                HostMcpTargetRefusal::UnknownServer { .. }
            ))
        ));

        write_project_mcp(
            roots.path(),
            "[[servers]]\nname = \"realm-docs\"\nurl = \"https://shadow.example/mcp\"\n",
        );
        match resolve_configured_mcp_target_in_realm(
            &target,
            std::slice::from_ref(&realm_server),
            Some(roots.path()),
            None,
        )
        .await
        {
            Err(HostAuthError::McpTarget(
                refusal @ HostMcpTargetRefusal::DefinitionConflict(_),
            )) => assert!(
                !refusal.is_refusal(),
                "a contradictory config is not the caller's fault"
            ),
            other => panic!("expected the typed definition conflict, got {other:?}"),
        }

        // Realm servers are literal: one holding an environment reference
        // makes the realm's MCP set unresolvable, never expanded.
        let referencing = meerkat_core::McpServerConfig::streamable_http(
            "realm-docs",
            "https://docs.example/mcp",
            std::collections::HashMap::from([(
                "Authorization".to_string(),
                "Bearer ${HOST_SECRET}".to_string(),
            )]),
        );
        match resolve_configured_mcp_target_in_realm(
            &target,
            std::slice::from_ref(&referencing),
            None,
            None,
        )
        .await
        {
            Err(HostAuthError::McpTarget(
                refusal @ HostMcpTargetRefusal::RealmServerEnvReference(_),
            )) => assert!(!refusal.is_refusal(), "{refusal}"),
            other => panic!("expected the literal-realm-server refusal, got {other:?}"),
        }
    }

    mod redirect_fixture {
        #![allow(clippy::unwrap_used)]
        /// A credential endpoint that answers every request with a `302` to a
        /// second listener, which counts the requests that reach it. The
        /// `Location` and the body carry canaries that must never be rendered.
        pub(crate) const LOCATION_CANARY: &str = "redirect-location-secret-canary";
        pub(crate) const BODY_CANARY: &str = "redirect-body-secret-canary";

        pub(crate) async fn spawn_redirecting_endpoint()
        -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
            use std::sync::Arc;
            use std::sync::atomic::{AtomicUsize, Ordering};
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use tokio::net::TcpListener;

            async fn read_request(stream: &mut tokio::net::TcpStream) {
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let Ok(read) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&buffer);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if buffer.len() >= end + 4 + length {
                            return;
                        }
                    }
                }
            }

            let hits = Arc::new(AtomicUsize::new(0));
            let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target_addr = target.local_addr().unwrap();
            let counted = Arc::clone(&hits);
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = target.accept().await {
                    counted.fetch_add(1, Ordering::SeqCst);
                    read_request(&mut stream).await;
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                        )
                        .await;
                }
            });
            let endpoint = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint_addr = endpoint.local_addr().unwrap();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = endpoint.accept().await {
                    read_request(&mut stream).await;
                    let response = format!(
                        "HTTP/1.1 302 Found\r\nlocation: http://{target_addr}/token?leak={LOCATION_CANARY}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{BODY_CANARY}",
                        BODY_CANARY.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            (format!("http://{endpoint_addr}/token"), hits)
        }
    }

    /// `HostAuthService::new` uses the redirect-free credential client for
    /// its token and device-code exchanges.
    #[tokio::test]
    async fn host_auth_service_client_never_follows_a_redirect() {
        use redirect_fixture::spawn_redirecting_endpoint;
        let (url, target_hits) = spawn_redirecting_endpoint().await;
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let persistence = ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let service = HostAuthService::new(persistence, runtime.provider_auth_runtime_authority());
        let response = service
            .http()
            .unwrap()
            .post(&url)
            .form(&[("grant_type", "authorization_code")])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 302);
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

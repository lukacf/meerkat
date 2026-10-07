//! Runtime-discovered OAuth for HTTP MCP servers.
//!
//! The MCP config owns only server intent. This module owns OAuth discovery,
//! dynamic client registration, PKCE loopback flow, token persistence, and
//! refresh for MCP resources.

use std::collections::BTreeSet;
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
    LoopbackClosed, LoopbackHandle, OAuthEndpoints, OAuthTokenRequestFormat, OAuthTokenResult,
    PkceChallenge, PkcePair, bind_loopback_callback, exchange_authorization_code_with_state,
    exchange_refresh_token, oauth_refresh_observation,
};
use crate::auth_store::{
    CredentialMutationError, PersistedAuthMode, PersistedTokens, ProviderAuthPersistence,
    RefreshCoordinator, RefreshError, TokenKey, TokenStore,
};
use crate::connector_oauth::{
    AccountSelection, AccountVerification, ConnectorAccountObservation, ConnectorOAuthDescriptor,
    ConnectorOAuthParameters, ConnectorOAuthRefusal, McpCredentialBinding, OAuthBrowserActionRef,
};
use crate::oauth_flow::{
    OAuthBrowserFlowCompletion, OAuthBrowserFlowIdentity, OAuthFlowAuthority, OAuthFlowError,
};
use crate::{BrowserOAuthFlowCommit, save_oauth_tokens_and_consume_browser_flow};
use meerkat_core::auth::RefreshFailureDisposition;
use meerkat_core::connection::{AuthBindingRef, BindingId, BindingOrigin, RealmId};
use meerkat_core::generated::auth_lease_durable_lifecycle_marker as durable_marker;
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeaseRestoreSnapshot, AuthLeaseSnapshot,
    CredentialUseDisposition, CredentialUseIntent, GeneratedAuthLeaseHandle, LeaseKey,
};

const MCP_TOKEN_REALM: &str = "mcp-oauth";
/// Loopback path conventionally used by host listeners for MCP callbacks.
pub const MCP_OAUTH_CALLBACK_PATH: &str = "/mcp/oauth/callback";
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
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct McpServerIdentity {
    server_name: String,
    server_url: String,
    selection: McpAccountSelection,
}

/// How an MCP target's OAuth credential is bound to a provider account.
/// Only host configuration selects it; each mode has its own credential
/// slot, so a mode change never reuses or migrates another mode's
/// credential.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum McpAccountSelection {
    /// No selection (no `oauth_account` or `oauth_account_selection`):
    /// stored-credential use with its pre-selection semantics. Interactive
    /// login refuses it.
    Legacy,
    /// The provider subject the credential must prove.
    Known(String),
    /// Bind the subject the account strategy verifies at the first login.
    Discover,
    /// Explicit opt-in to a resource-bound credential with no account
    /// evidence. It never establishes, verifies or represents an account.
    Unverified,
}

/// The Known subject is not rendered.
impl std::fmt::Debug for McpAccountSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known(_) => f.write_str("Known(..)"),
            other => f.write_str(other.mode_name()),
        }
    }
}

impl McpAccountSelection {
    fn mode_name(&self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Known(_) => "known",
            Self::Discover => "discover",
            Self::Unverified => "unverified",
        }
    }
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
            selection: McpAccountSelection::Legacy,
        }
    }

    /// Select the provider's stable subject/account ID, never a display name.
    /// Selected identities use a distinct vault and lifecycle key and never
    /// fall back to credentials stored under the legacy unselected identity.
    pub fn with_expected_account(
        mut self,
        expected_account: impl Into<String>,
    ) -> Result<Self, McpOAuthError> {
        let account = expected_account.into();
        if account.trim().is_empty()
            || account.len() > 4096
            || account.chars().any(char::is_control)
        {
            return Err(McpOAuthError::InvalidAccountSelection);
        }
        self.selection = McpAccountSelection::Known(account);
        Ok(self)
    }

    /// Select account discovery or the explicitly unverified resource-bound
    /// mode. Only [`Self::from_config`] calls this, so the mode always comes
    /// from host configuration; agent input, start parameters, a failed
    /// verification or a legacy credential never select it.
    pub(crate) fn with_account_selection(
        mut self,
        selection: meerkat_core::mcp_config::McpOAuthAccountSelection,
    ) -> Self {
        use meerkat_core::mcp_config::McpOAuthAccountSelection as Configured;
        self.selection = match selection {
            Configured::Discover => McpAccountSelection::Discover,
            Configured::Unverified => McpAccountSelection::Unverified,
        };
        self
    }

    /// The Known subject, when the target names one.
    pub fn expected_account(&self) -> Option<&str> {
        match &self.selection {
            McpAccountSelection::Known(account) => Some(account),
            McpAccountSelection::Legacy
            | McpAccountSelection::Discover
            | McpAccountSelection::Unverified => None,
        }
    }

    pub fn selection(&self) -> &McpAccountSelection {
        &self.selection
    }

    /// Whether the target has any account selection (Known, Discover or
    /// Unverified). Every selected target uses its OAuth resolver only.
    pub fn is_selected(&self) -> bool {
        self.selection != McpAccountSelection::Legacy
    }

    /// The account verification the target's credential carries: `None`
    /// for a legacy (unselected) target.
    pub fn account_verification(&self) -> Option<AccountVerification> {
        match self.selection {
            McpAccountSelection::Legacy => None,
            McpAccountSelection::Known(_) | McpAccountSelection::Discover => {
                Some(AccountVerification::Verified)
            }
            McpAccountSelection::Unverified => Some(AccountVerification::Unverified),
        }
    }

    /// Use the same persisted selection for native connections and login.
    pub fn from_config(config: &meerkat_core::McpServerConfig) -> Result<Self, McpOAuthError> {
        use meerkat_core::mcp_config::{McpHttpTransport, McpTransportConfig};
        let McpTransportConfig::Http(http) = &config.transport else {
            return Err(McpOAuthError::UnsupportedAccountSelection);
        };
        let target = Self::from_server_config(config.name.clone(), http.url.clone());
        let target = match (&http.oauth_account, http.oauth_account_selection) {
            (None, None) => return Ok(target),
            (Some(_), Some(_)) => return Err(McpOAuthError::InvalidAccountSelection),
            (Some(account), None) => target.with_expected_account(account.clone())?,
            (None, Some(selection)) => target.with_account_selection(selection),
        };
        // Every selection uses its OAuth resolver alone: a static
        // Authorization header or another transport would bypass it.
        if http.transport.unwrap_or_default() != McpHttpTransport::StreamableHttp
            || http
                .headers
                .keys()
                .any(|name| name.eq_ignore_ascii_case("authorization"))
        {
            return Err(McpOAuthError::UnsupportedAccountSelection);
        }
        Ok(target)
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
        // Each selection mode has its own key tag, so the slots of Known,
        // Discover, Unverified and legacy targets never coincide.
        match &self.selection {
            McpAccountSelection::Legacy => {}
            McpAccountSelection::Known(account) => {
                hasher.update(b"\0mcp-account-v1\0");
                hasher.update(account.as_bytes());
            }
            McpAccountSelection::Discover => hasher.update(b"\0mcp-account-discover-v1"),
            McpAccountSelection::Unverified => hasher.update(b"\0mcp-account-unverified-v1"),
        }
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

impl std::fmt::Debug for McpServerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServerIdentity")
            .field("server_name", &self.server_name)
            .field("server_url", &self.server_url)
            .field("account_selection", &self.selection.mode_name())
            .finish()
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

/// Validated discovery and registered-client facts supplied to a trusted host
/// strategy. The host supplies the expected account and requested scopes.
/// This is not an action projection and must never be sent to an agent model.
pub struct McpOAuthCeremonyContext<'a> {
    pub issuer: &'a str,
    pub client: &'a str,
    pub resource: &'a str,
    pub redirect_uri: &'a str,
    /// The account this attempt must prove: the target's Known subject, the
    /// bound subject when an occupied Discover slot reconnects, or
    /// `Discover` for an empty Discover slot. Never `Unverified`.
    pub account: &'a AccountSelection,
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

    /// Check that this strategy can verify an account at `issuer`. It runs
    /// when an attempt starts, after discovery and before client
    /// registration or any human consent, so an issuer that cannot supply
    /// the strategy's evidence is refused before the human signs in.
    /// Default: nothing to check.
    async fn preflight(&self, _issuer: &str) -> Result<(), ConnectorOAuthRefusal> {
        Ok(())
    }
}

/// Stable strategy identifier bound into OIDC UserInfo descriptors.
pub const OIDC_USERINFO_STRATEGY_ID: &str = "oidc-userinfo-v1";

/// Strategy identifier the native owner binds into explicitly unverified
/// resource-bound descriptors. No account strategy runs for them.
pub const UNVERIFIED_RESOURCE_STRATEGY_ID: &str = "unverified-resource-v1";

/// Production account strategy: the issuer's OpenID Connect UserInfo
/// response, fetched with the exchanged access token, is the authenticated
/// provider evidence.
///
/// The descriptor requests `openid` (plus any configured scopes) and binds
/// the target's expected account, which is the OIDC subject (`sub`). The
/// observation discovers `userinfo_endpoint` from the issuer's
/// `/.well-known/openid-configuration` (whose `issuer` must equal the admitted
/// issuer), calls it over https (or loopback) with the access token, and
/// reports `sub`. Granted scopes come from the token response, or the
/// requested scopes when the response omits `scope` (RFC 6749 section 5.1).
/// No JWT is decoded. Servers without OIDC UserInfo refuse with
/// `VerificationUnavailable`; hosts with provider-specific evidence supply
/// their own strategy.
#[derive(Clone)]
pub struct OidcUserInfoAccountStrategy {
    /// A build failure is kept: the strategy then cannot verify anything,
    /// and never falls back to a client that follows redirects.
    http: Result<Client, CredentialHttpClientUnavailable>,
    required_scopes: std::collections::BTreeSet<String>,
}

impl Default for OidcUserInfoAccountStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl OidcUserInfoAccountStrategy {
    /// The bearer token is only ever sent to the validated issuer's
    /// `userinfo_endpoint`, so this client follows no redirects.
    pub fn new() -> Self {
        Self {
            http: no_redirect_client(),
            required_scopes: std::collections::BTreeSet::new(),
        }
    }

    pub fn with_http(http: Client) -> Self {
        Self {
            http: Ok(http),
            required_scopes: std::collections::BTreeSet::new(),
        }
    }

    fn http(&self) -> Result<&Client, ConnectorOAuthRefusal> {
        self.http
            .as_ref()
            .map_err(|_| ConnectorOAuthRefusal::VerificationUnavailable)
    }

    /// Additional scopes the MCP resource requires, requested alongside
    /// `openid` and required in the granted set.
    pub fn with_required_scopes(mut self, scopes: impl IntoIterator<Item = String>) -> Self {
        self.required_scopes.extend(scopes);
        self
    }
}

#[derive(Deserialize)]
struct OpenIdConfiguration {
    issuer: String,
    #[serde(default)]
    userinfo_endpoint: Option<String>,
}

#[derive(Deserialize)]
struct OidcUserInfo {
    sub: String,
}

#[async_trait]
impl McpOAuthAccountStrategy for OidcUserInfoAccountStrategy {
    fn descriptor(
        &self,
        target: &McpServerIdentity,
        context: &McpOAuthCeremonyContext<'_>,
    ) -> Result<ConnectorOAuthDescriptor, ConnectorOAuthRefusal> {
        let _ = target;
        if context.account.is_unverified() {
            return Err(ConnectorOAuthRefusal::InvalidDescriptor);
        }
        let mut scopes = self.required_scopes.clone();
        scopes.insert("openid".to_owned());
        crate::connector_oauth::ConnectorOAuthParameters {
            issuer: context.issuer.to_owned(),
            client: context.client.to_owned(),
            resource: context.resource.to_owned(),
            redirect_uri: context.redirect_uri.to_owned(),
            scopes,
            expected_account: context.account.clone(),
            strategy_id: OIDC_USERINFO_STRATEGY_ID.to_owned(),
        }
        .try_into()
    }

    async fn observe_account(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        self.observe_userinfo(descriptor, tokens).await
    }

    /// The issuer must publish an OpenID Connect UserInfo endpoint this
    /// strategy can use; the bearer is never sent anywhere else.
    async fn preflight(&self, issuer: &str) -> Result<(), ConnectorOAuthRefusal> {
        self.userinfo_endpoint(issuer).await.map(|_| ())
    }
}

impl OidcUserInfoAccountStrategy {
    /// The issuer's UserInfo endpoint: discovered from its OpenID Connect
    /// configuration, whose `issuer` must equal the admitted issuer, and
    /// served over https (or loopback).
    async fn userinfo_endpoint(&self, issuer: &str) -> Result<reqwest::Url, ConnectorOAuthRefusal> {
        let unavailable = |_| ConnectorOAuthRefusal::VerificationUnavailable;
        let configuration_url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let configuration: OpenIdConfiguration = self
            .http()?
            .get(&configuration_url)
            .send()
            .await
            .map_err(unavailable)?
            .error_for_status()
            .map_err(unavailable)?
            .json()
            .await
            .map_err(unavailable)?;
        if configuration.issuer != issuer {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable);
        }
        let userinfo_endpoint = configuration
            .userinfo_endpoint
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let userinfo_url = reqwest::Url::parse(&userinfo_endpoint)
            .map_err(|_| ConnectorOAuthRefusal::VerificationUnavailable)?;
        if userinfo_url.scheme() != "https" && !is_loopback_url(&userinfo_url) {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable);
        }
        Ok(userinfo_url)
    }

    /// The issuer's UserInfo `sub` for `tokens`' access token, with the
    /// granted scopes of the token response (the requested scopes when the
    /// response omits `scope`).
    pub async fn observe_userinfo(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        let unavailable = |_| ConnectorOAuthRefusal::VerificationUnavailable;
        let facts = descriptor.parameters();
        let userinfo_url = self.userinfo_endpoint(&facts.issuer).await?;
        let userinfo: OidcUserInfo = self
            .http()?
            .get(userinfo_url)
            .bearer_auth(&tokens.access_token)
            .send()
            .await
            .map_err(unavailable)?
            .error_for_status()
            .map_err(unavailable)?
            .json()
            .await
            .map_err(unavailable)?;
        if userinfo.sub.is_empty() {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable);
        }
        let granted_scopes = match tokens.scope.as_deref() {
            Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
            None => facts.scopes.clone(),
        };
        Ok(ConnectorAccountObservation {
            account: userinfo.sub,
            granted_scopes,
        })
    }
}

/// Result of [`McpOAuthAuthority::begin_loopback_login`].
pub enum McpOAuthLoopbackBegin {
    /// This host admitted the attempt and owns its callback binding.
    Started(McpOAuthPendingLogin),
    /// An attempt was already pending for the target, owned by another
    /// listener. No second attempt was admitted.
    Joined(McpOAuthLoginStart),
}

/// Outcome of an advisory browser launch. It never retries, cancels or
/// completes the attempt: the attempt ends only by completion, an explicit
/// [`McpOAuthPendingLogin::cancel`], or expiry of the admitted attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOAuthBrowserLaunch {
    Launched,
    /// The opener reported failure (it may still have navigated). The
    /// failure detail is not kept, so no URL can leak through it.
    Failed,
}

/// Open `url` in the system browser without logging it.
///
/// The URL is passed as one process argument (no shell) to the platform
/// opener (`open`, `xdg-open`, or `rundll32 url.dll,FileProtocolHandler`),
/// whose output is discarded. Nothing here logs the command line, so the
/// authorize URL and state never reach `log` or `tracing` records.
pub fn open_system_browser(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(url);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "browser opener exited unsuccessfully",
        ))
    }
}

/// One admitted host loopback login: the attempt and its callback binding.
///
/// It keeps the exact attempt's cleanup custody until an owner result is
/// observed: a completion that returned success consumed it; otherwise
/// dropping this login, including the future of an unfinished
/// [`complete`](Self::complete), retires the attempt through its flow owner
/// (best effort, result unobserved) and signals the callback listener to
/// terminate (no joined receipt). [`close`](Self::close) performs both
/// cleanups and reports each outcome.
pub struct McpOAuthPendingLogin {
    authority: McpOAuthAuthority,
    start: McpOAuthLoginStart,
    /// The live listener, until the callback is taken or the login closes.
    callback: Option<LoopbackHandle>,
    /// The listener's cleanup outcome once the callback was taken from it.
    listener: Option<McpOAuthListenerCleanup>,
    completion: PendingCompletion,
    /// What the completion's own retire guard observed, if it ran.
    guard_retirement: RetireObserver,
}

/// Whether, and how, the code reached the canonical completion.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingCompletion {
    /// No code was handed to the completion.
    NotEntered,
    /// Handed off; the completion's result was not observed (its future was
    /// dropped). An already-owned credential commit may still settle.
    Unobserved,
    /// The completion returned a failure.
    Failed,
    /// The completion returned success: the owner consumed the attempt.
    Consumed,
}

/// The attempt cleanup that [`McpOAuthPendingLogin::close`] observed.
#[derive(Debug)]
pub enum McpOAuthAttemptCleanup {
    /// The attempt was retired through its flow owner (by this close, or by
    /// the completion's own retire guard, whose result was recorded).
    Retired,
    /// A completion returned success: the owner consumed the attempt.
    Consumed,
    /// The flow owner holds no such attempt. `Missing` does not say whether
    /// it was consumed, retired or expired. When `completion_unobserved`,
    /// the code was handed to a completion whose result was never observed,
    /// so a credential may or may not have been published.
    Absent { completion_unobserved: bool },
    /// The flow owner did not retire the attempt.
    RetirementFailed(McpOAuthError),
}

/// The callback listener cleanup that [`McpOAuthPendingLogin::close`]
/// observed.
#[derive(Debug)]
pub enum McpOAuthListenerCleanup {
    /// The listener is closed and its accepted-connection drain was joined.
    Joined(LoopbackClosed),
    /// The listener's retirement failed; no receipt exists.
    Failed(crate::auth_oauth::OAuthError),
    /// A consuming wait retired the listener; it returned no receipt.
    NotReported,
}

/// Both cleanup outcomes of [`McpOAuthPendingLogin::close`], reported
/// independently. Neither carries a code or state.
#[must_use]
#[derive(Debug)]
pub struct McpOAuthPendingClose {
    pub attempt: McpOAuthAttemptCleanup,
    pub listener: McpOAuthListenerCleanup,
}

impl McpOAuthPendingLogin {
    /// Host-only browser navigation for this attempt (see
    /// [`McpOAuthLoginStart`]).
    pub fn start(&self) -> &McpOAuthLoginStart {
        &self.start
    }

    /// Launch the browser with `open` on the blocking pool, never on the
    /// async runtime. Advisory only.
    pub async fn launch_browser<F>(&self, open: F) -> McpOAuthBrowserLaunch
    where
        F: FnOnce(String) -> std::io::Result<()> + Send + 'static,
    {
        let url = self.start.authorize_url.clone();
        match tokio::task::spawn_blocking(move || open(url)).await {
            Ok(Ok(())) => McpOAuthBrowserLaunch::Launched,
            Ok(Err(_)) | Err(_) => McpOAuthBrowserLaunch::Failed,
        }
    }

    /// [`launch_browser`](Self::launch_browser) with
    /// [`open_system_browser`].
    pub async fn launch_system_browser(&self) -> McpOAuthBrowserLaunch {
        self.launch_browser(|url| open_system_browser(&url)).await
    }

    /// Wait for the loopback callback, bounded by both `timeout` and the
    /// admitted attempt's own expiry, then complete the login. Every failure
    /// retires the attempt; so does dropping this future before it finishes.
    ///
    /// The wait keeps the listener's legacy tie: like `tokio::time::timeout`,
    /// it polls the callback and drain once before the window is consulted.
    pub async fn complete(self, timeout: Duration) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        let mut pending = self;
        let server_name = pending.start.target.server_name().to_owned();
        let Some(callback) = pending.callback.take() else {
            return Err(McpOAuthError::HumanAuthorizationRequired { server_name });
        };
        pending.listener = Some(McpOAuthListenerCleanup::NotReported);
        let window = timeout.min(pending.start.remaining());
        // A failed wait leaves the attempt admitted: dropping `pending`
        // retires it.
        let outcome = callback
            .wait(window)
            .await
            .map_err(|error| callback_wait_error(server_name, error))?;
        pending.run_completion(outcome).await
    }

    /// Borrowed completion. The callback wait is bounded by `deadline` and
    /// the admitted attempt's expiry, whichever is earlier: a deadline already
    /// passed wins over a queued callback. The wait is safe to abandon: the
    /// listener and the attempt stay with this login for an explicit
    /// [`close`](Self::close).
    ///
    /// The deadline bounds only the callback wait. It neither extends the
    /// attempt's validity nor bounds the completion after delivery (issuer
    /// discovery, token exchange, account observation, credential commit),
    /// which has no timeout of its own. Dropping this future while the token
    /// exchange is outstanding prevents local publication by this completion;
    /// once the credential commit was handed to its owner, that owner may
    /// still finish after the drop. The authorization code is never replayed.
    pub async fn complete_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        let server_name = self.start.target.server_name().to_owned();
        let Some(callback) = self.callback.as_mut() else {
            return Err(McpOAuthError::HumanAuthorizationRequired { server_name });
        };
        let expires = tokio::time::Instant::from_std(self.start.expires_at);
        let outcome = callback
            .wait_until(deadline.min(expires))
            .await
            .map_err(|error| callback_wait_error(server_name, error))?;
        if let Some(callback) = self.callback.take() {
            // The delivered wait already joined the drain, so this close
            // returns its recorded result without suspending.
            self.listener = Some(match callback.close().await {
                Ok(closed) => McpOAuthListenerCleanup::Joined(closed),
                Err(error) => McpOAuthListenerCleanup::Failed(error),
            });
        }
        self.run_completion(outcome).await
    }

    /// Hand the delivered callback to the canonical completion, recording
    /// what is observed of it.
    async fn run_completion(
        &mut self,
        outcome: crate::auth_oauth::LoopbackOutcome,
    ) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        let start = self.start.clone();
        self.completion = PendingCompletion::Unobserved;
        let completed = self
            .authority
            .login_complete_observed(
                &start.target,
                McpOAuthCallback {
                    redirect_uri: start.redirect_uri.clone(),
                    state: outcome.state,
                    code: outcome.code,
                },
                Some(self.guard_retirement.clone()),
            )
            .await;
        self.completion = if completed.is_ok() {
            PendingCompletion::Consumed
        } else {
            PendingCompletion::Failed
        };
        completed
    }

    /// Retire the admitted attempt through its flow owner, then close the
    /// callback listener and await its drain. Both cleanups are always
    /// attempted and each outcome is reported. The retirement runs on the
    /// first poll, before the first suspension; dropping the future during
    /// the drain loses only the listener's receipt (the drain is signalled).
    /// Dropping it before its first poll leaves this login's `Drop` to
    /// retire the attempt, without any receipt.
    pub async fn close(self) -> McpOAuthPendingClose {
        let mut pending = self;
        let completion = std::mem::replace(&mut pending.completion, PendingCompletion::Consumed);
        let attempt = match completion {
            PendingCompletion::Consumed => McpOAuthAttemptCleanup::Consumed,
            completion => {
                let guard = pending.guard_retirement.lock().take();
                match guard {
                    Some(Ok(())) => McpOAuthAttemptCleanup::Retired,
                    // The guard failed or never ran: this close retires.
                    Some(Err(_)) | None => attempt_cleanup(
                        pending
                            .authority
                            .login_cancel(&pending.start.target, &pending.start),
                        completion == PendingCompletion::Unobserved,
                    ),
                }
            }
        };
        let listener = match pending.callback.take() {
            Some(callback) => match callback.close().await {
                Ok(closed) => McpOAuthListenerCleanup::Joined(closed),
                Err(error) => McpOAuthListenerCleanup::Failed(error),
            },
            None => pending
                .listener
                .take()
                .unwrap_or(McpOAuthListenerCleanup::NotReported),
        };
        McpOAuthPendingClose { attempt, listener }
    }

    /// [`close`](Self::close) projected onto the original return type: a
    /// failed attempt retirement is returned first (an absent attempt is
    /// `Flow(Missing)`, as before), then a failed listener retirement as
    /// `Callback`. Both cleanups are still attempted.
    pub async fn cancel(self) -> Result<(), McpOAuthError> {
        let server_name = self.start.target.server_name().to_owned();
        let closed = self.close().await;
        match closed.attempt {
            McpOAuthAttemptCleanup::RetirementFailed(error) => return Err(error),
            McpOAuthAttemptCleanup::Absent { .. } => {
                return Err(McpOAuthError::Flow(OAuthFlowError::Missing));
            }
            McpOAuthAttemptCleanup::Retired | McpOAuthAttemptCleanup::Consumed => {}
        }
        match closed.listener {
            McpOAuthListenerCleanup::Failed(_) => Err(McpOAuthError::Callback {
                server_name,
                reason: "callback listener retirement failed".into(),
            }),
            McpOAuthListenerCleanup::Joined(_) | McpOAuthListenerCleanup::NotReported => Ok(()),
        }
    }
}

/// The retire guard's own result, recorded for the pending login that
/// handed the code over.
type RetireObserver = Arc<parking_lot::Mutex<Option<Result<(), OAuthFlowError>>>>;

fn callback_wait_error(server_name: String, error: crate::auth_oauth::OAuthError) -> McpOAuthError {
    McpOAuthError::Callback {
        server_name,
        reason: match error {
            crate::auth_oauth::OAuthError::Timeout => {
                "timeout waiting for the authorization callback".into()
            }
            other => other.to_string(),
        },
    }
}

/// A close's own retirement result. `Missing` is absence, not proof of
/// consumption or retirement.
fn attempt_cleanup(
    retired: Result<(), McpOAuthError>,
    completion_unobserved: bool,
) -> McpOAuthAttemptCleanup {
    match retired {
        Ok(()) => McpOAuthAttemptCleanup::Retired,
        Err(McpOAuthError::Flow(OAuthFlowError::Missing)) => McpOAuthAttemptCleanup::Absent {
            completion_unobserved,
        },
        Err(error) => McpOAuthAttemptCleanup::RetirementFailed(error),
    }
}

impl Drop for McpOAuthPendingLogin {
    fn drop(&mut self) {
        if self.completion != PendingCompletion::Consumed {
            // Best effort, result unobserved: the exact attempt is retired
            // unless a completion consumed it. Retiring one the completion's
            // guard already retired is a harmless `Missing`.
            let _ = self.authority.login_cancel(&self.start.target, &self.start);
        }
        // Dropping the handle signals the listener to terminate, without a
        // joined receipt.
        drop(self.callback.take());
    }
}

/// Host-only browser navigation for one admitted MCP OAuth attempt.
///
/// This is a host-channel projection. It must never be placed in a tool
/// result, transcript, agent event, elicitation result or ordinary log: the
/// authorize URL and state let whoever follows them complete the attempt.
/// The host opens `authorize_url` in the user's own browser, not in one an
/// agent tool drives. `Debug` redacts the secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct McpOAuthLoginStart {
    pub target: McpServerIdentity,
    pub authorize_url: String,
    pub state: String,
    pub redirect_uri: String,
    /// Whether this call admitted the attempt or joined one already pending
    /// for the target.
    pub disposition: McpOAuthLoginDisposition,
    /// Admitted attempt identity, constructible only by `login_start`, used
    /// to retire an abandoned attempt through its flow owner.
    identity: OAuthBrowserFlowIdentity,
    /// When the admitted attempt expires.
    expires_at: std::time::Instant,
}

impl McpOAuthLoginStart {
    /// Time left before the admitted attempt expires.
    pub fn remaining(&self) -> Duration {
        self.expires_at
            .saturating_duration_since(std::time::Instant::now())
    }
}

/// How [`McpOAuthAuthority::login_start`] produced its projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOAuthLoginDisposition {
    /// A new attempt was admitted.
    Started,
    /// An attempt was already pending for the target; this is its
    /// projection. No second attempt exists.
    Joined,
}

impl std::fmt::Debug for McpOAuthLoginStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthLoginStart")
            .field("target", &self.target)
            .field("authorize_url", &"<redacted>")
            .field("state", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field("disposition", &self.disposition)
            .finish_non_exhaustive()
    }
}

/// The host's loopback callback for [`McpOAuthAuthority::login_complete`]:
/// the `redirect_uri` returned by `login_start`, and the callback's `state`
/// and `code`. Everything else is taken from the admitted attempt.
#[derive(Clone, PartialEq, Eq)]
pub struct McpOAuthCallback {
    pub redirect_uri: String,
    pub state: String,
    pub code: String,
}

impl std::fmt::Debug for McpOAuthCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthCallback")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"<redacted>")
            .field("code", &"<redacted>")
            .finish()
    }
}

/// Retires an admitted attempt through its flow owner unless disarmed: every
/// non-success exit from `login_complete` passes through its `Drop`.
struct AttemptRetireGuard {
    authority: Arc<dyn OAuthFlowAuthority>,
    state: String,
    target: meerkat_core::AuthCredentialIdentity,
    identity: OAuthBrowserFlowIdentity,
    redirect_uri: String,
    armed: bool,
    /// Where a pending login that handed the code over reads this guard's
    /// own retirement result.
    observed: Option<RetireObserver>,
}

impl Drop for AttemptRetireGuard {
    fn drop(&mut self) {
        if self.armed {
            let retired = self.authority.expire(
                &self.state,
                &self.target,
                self.identity.clone(),
                &self.redirect_uri,
            );
            if let Some(observed) = &self.observed {
                *observed.lock() = Some(retired);
            }
        }
    }
}

/// Authorization-server endpoints validated for one issuer.
struct AuthorizationServerEndpoints {
    authorization_metadata_url: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    /// The server advertises `offline_access` in its `scopes_supported`.
    offline_access: bool,
}

/// An attempt pending for an MCP target, as a host may observe it: a
/// non-secret reference and its expiry. It carries no authorize URL, state,
/// code or verifier, so it is safe for host status projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthPendingAttempt {
    /// Non-secret reference to the attempt: a digest of its state. Cancel
    /// takes it in place of the state.
    pub attempt_ref: OAuthBrowserActionRef,
    /// When the admitted attempt expires.
    pub expires_at: DateTime<Utc>,
}

/// Secret-free result of a completed MCP OAuth login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthLoginComplete {
    pub target: McpServerIdentity,
    /// The verified subject; always `None` for an unverified grant.
    pub account_id: Option<String>,
    pub account_verification: AccountVerification,
    pub expires_at: Option<DateTime<Utc>>,
    pub has_refresh_token: bool,
    pub scopes: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthError {
    #[error("MCP OAuth expected account selection is invalid")]
    InvalidAccountSelection,
    #[error("MCP OAuth interactive login requires an explicit expected account")]
    AccountSelectionRequired,
    #[error(
        "MCP OAuth account selection requires its OAuth resolver and streamable HTTP without static Authorization"
    )]
    UnsupportedAccountSelection,
    #[error(transparent)]
    Verification(#[from] ConnectorOAuthRefusal),
    #[error("MCP OAuth flow owner refused the ceremony")]
    Flow(#[source] OAuthFlowError),
    #[error("MCP OAuth refresh preparation is no longer current")]
    StalePreparation,
    #[error("MCP OAuth token not found for '{server_name}'. Run: rkat mcp login {server_name}")]
    MissingStoredToken { server_name: String },
    /// The MCP server needs a human to complete its browser authorization.
    /// The host owns that ceremony through `login_start`/`login_complete`;
    /// this refusal never carries an authorize URL, state or code.
    #[error("MCP server '{server_name}' is awaiting human authorization")]
    HumanAuthorizationRequired { server_name: String },
    /// The host's loopback callback could not be bound, failed, or timed out.
    #[error("MCP OAuth callback failed for '{server_name}': {reason}")]
    Callback { server_name: String, reason: String },
    #[error("MCP OAuth discovery failed for '{server_name}': {reason}")]
    DiscoveryFailed { server_name: String, reason: String },
    #[error("MCP OAuth dynamic client registration failed for '{server_name}': {reason}")]
    RegistrationFailed { server_name: String, reason: String },
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
    /// The target's credential slot is occupied by a credential this login
    /// may not replace (an unverified grant, another context or a
    /// credential without an account binding). Disconnect first.
    #[error(
        "MCP server '{server_name}' already has a credential this login cannot replace; disconnect it first"
    )]
    DisconnectRequired { server_name: String },
    /// The credential slot refused the commit inside its exclusive
    /// mutation (for example a racing occupant of a Discover slot).
    /// The redirect-free credential HTTP client could not be built, so no
    /// OAuth request is sent.
    #[error(transparent)]
    HttpClientUnavailable(CredentialHttpClientUnavailable),
    #[error("MCP OAuth credential slot for '{server_name}' refused the login: {refusal}")]
    CredentialSlot {
        server_name: String,
        refusal: crate::auth_store::CredentialSlotRefusal,
    },
}

impl McpOAuthError {
    /// Whether this is a refusal of the caller's request (selection,
    /// verification, flow-owner or credential-state refusals) rather than an
    /// infrastructure or upstream failure. Surfaces map it to their
    /// invalid-request class.
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::InvalidAccountSelection
            | Self::AccountSelectionRequired
            | Self::UnsupportedAccountSelection
            | Self::Verification(_)
            | Self::Flow(_)
            | Self::MissingStoredToken { .. }
            | Self::HumanAuthorizationRequired { .. }
            | Self::ReauthRequired { .. }
            | Self::DisconnectRequired { .. }
            | Self::CredentialSlot { .. }
            | Self::TokenKey { .. } => true,
            Self::StalePreparation
            | Self::Callback { .. }
            | Self::DiscoveryFailed { .. }
            | Self::RegistrationFailed { .. }
            | Self::TokenExchangeFailed { .. }
            | Self::RefreshFailed { .. }
            | Self::TokenStore(_)
            | Self::MissingStoredMetadata { .. }
            | Self::HttpClientUnavailable(_)
            | Self::AuthLifecycle { .. } => false,
        }
    }
}

#[derive(Clone)]
pub struct McpOAuthAuthority {
    /// Follows no redirects. A build failure is kept, and every network
    /// step fails with it; nothing falls back to a permissive client.
    http: Result<Client, CredentialHttpClientUnavailable>,
    /// Token vault plus same-key refresh serialization authority.
    provider_auth_persistence: ProviderAuthPersistence,
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
    /// The default HTTP client follows no redirects: metadata, client
    /// registration and token requests go only to validated endpoints.
    pub fn new(
        provider_auth_persistence: ProviderAuthPersistence,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http: no_redirect_client(),
            provider_auth_persistence,
            auth_lease,
            interactive: None,
        }
    }

    /// `http` must not follow redirects (reqwest `redirect::Policy::none()`);
    /// a redirect answer to discovery or registration is refused.
    pub fn with_http(
        provider_auth_persistence: ProviderAuthPersistence,
        http: Client,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http: Ok(http),
            provider_auth_persistence,
            auth_lease,
            interactive: None,
        }
    }

    /// The redirect-free HTTP client, or the typed build failure.
    fn http(&self) -> Result<&Client, McpOAuthError> {
        self.http
            .as_ref()
            .map_err(|error| McpOAuthError::HttpClientUnavailable(*error))
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

    /// This authority without its interactive strategy: stored-credential use
    /// for unselected (no `oauth_account`) targets, with the semantics they
    /// had before any interactive strategy was installed. The interactive
    /// authority itself keeps refusing unselected targets.
    pub fn stored_only(&self) -> Self {
        Self {
            interactive: None,
            ..self.clone()
        }
    }

    /// Check local prerequisites before a surface performs discovery preflight.
    /// This does not authorize network effects or verify provider account data.
    pub fn validate_interactive_selection(
        &self,
        target: &McpServerIdentity,
    ) -> Result<(), McpOAuthError> {
        if !target.is_selected() {
            return Err(McpOAuthError::AccountSelectionRequired);
        }
        if self.interactive.is_none() {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable.into());
        }
        Ok(())
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
        if self.interactive.is_some() && !target.is_selected() {
            return Err(McpOAuthError::AccountSelectionRequired);
        }
        let key = target.token_key()?;
        let lease_key = target.lease_key()?;
        let admitted = {
            let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
            self.load_admitted_stored_credential(target, &key, &_guard)
                .await?
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
                // A coordinator may return another waiter's result. Recheck
                // the selected subject on that exact result before use.
                verify_stored_account(target, &refreshed)?;
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

    /// Admit one host-driven browser attempt for `target`.
    ///
    /// The host owns the loopback listener and the browser: it binds the
    /// callback (for example with [`Self::begin_loopback_login`]), passes the
    /// loopback redirect URI here, opens the returned authorize URL in the
    /// user's own browser (not one an agent tool drives), and feeds the
    /// callback to [`Self::login_complete`]. This method performs
    /// discovery, the account strategy's preflight (before any client
    /// registration or browser), dynamic client registration and the
    /// strategy's descriptor, extended with the resource's scopes, then
    /// admits PKCE/state through the AuthMachine-owned flow authority. It
    /// opens no listener and launches no browser.
    ///
    /// Start-or-join is serialized per target: while an attempt is pending
    /// for the target, its projection is returned with
    /// [`McpOAuthLoginDisposition::Joined`] and no second attempt is admitted.
    pub async fn login_start(
        &self,
        target: &McpServerIdentity,
        redirect_uri: &str,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoginStart, McpOAuthError> {
        self.validate_interactive_selection(target)?;
        let (authority, strategy) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        require_loopback_redirect(redirect_uri)?;
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        let _admission = acquire_admission_lock(target.binding_slug()).await;
        if let Some((state, record)) = authority
            .pending_connector_browser_attempt(&credential_identity)
            .map_err(McpOAuthError::Flow)?
        {
            // `None`: the pending attempt no longer matches the server, or
            // its preflight is not retained; it was retired, so admit a
            // fresh one.
            if let Some(joined) = self.join_pending_attempt(target, &state, record).await? {
                return Ok(joined);
            }
        }
        // The slot's current credential decides what this attempt may
        // publish. The commit re-checks it inside the slot's exclusive
        // mutation; this read only refuses early, before any network I/O.
        let occupant = self.slot_occupant(target).await?;
        let selection = attempt_selection(target, occupant.as_ref())?;
        let (mut discovery, requested) = self
            .discover(target, www_authenticate, redirect_uri)
            .await?;
        // Before client registration and before the human is asked to
        // consent: an issuer the strategy cannot verify is refused here.
        // An unverified resource grant runs no account strategy.
        if !selection.is_unverified() {
            strategy.preflight(&discovery.authorization_server).await?;
        }
        let client = match &occupant {
            // Reconnect with the admitted client: a fresh registration would
            // change the credential's context. A loopback redirect may use
            // any port (RFC 8252 section 7.3).
            Some(occupant) => {
                if occupant.metadata.discovery.authorization_server
                    != discovery.authorization_server
                    || occupant.metadata.discovery.resource != discovery.resource
                {
                    return Err(McpOAuthError::DisconnectRequired {
                        server_name: target.server_name().to_owned(),
                    });
                }
                StoredMcpOAuthClient {
                    redirect_uri: redirect_uri.to_owned(),
                    ..occupant.metadata.client.clone()
                }
            }
            None => {
                self.register_client(target, &discovery, redirect_uri)
                    .await?
            }
        };
        let context = McpOAuthCeremonyContext {
            issuer: &discovery.authorization_server,
            client: &client.client_id,
            resource: &discovery.resource,
            redirect_uri,
            account: &selection,
        };
        let descriptor = if selection.is_unverified() {
            unverified_descriptor(&context, &requested.resource_scopes)?
        } else {
            require_resource_scopes(
                strategy.descriptor(target, &context)?,
                &requested.resource_scopes,
            )?
        };
        let facts = descriptor.parameters();
        if facts.expected_account != selection {
            return Err(ConnectorOAuthRefusal::AccountMismatch.into());
        }
        if facts.issuer != discovery.authorization_server
            || facts.client != client.client_id
            || facts.resource != discovery.resource
            || facts.redirect_uri != redirect_uri
        {
            return Err(McpOAuthError::Verification(
                ConnectorOAuthRefusal::DescriptorMismatch,
            ));
        }
        if let Some(occupant) = &occupant
            && occupant.binding.stable_context != descriptor.stable_context()
        {
            return Err(McpOAuthError::DisconnectRequired {
                server_name: target.server_name().to_owned(),
            });
        }
        discovery.scopes = facts.scopes.iter().cloned().collect();
        let pkce = PkcePair::generate_s256();
        let identity: OAuthBrowserFlowIdentity = descriptor.clone().into();
        let state = authority
            .start(
                credential_identity,
                identity.clone(),
                redirect_uri.to_owned(),
                pkce.verifier.secret().to_owned(),
            )
            .map_err(McpOAuthError::Flow)?;
        // This attempt passed its strategy preflight (or needs none); a
        // later join reuses this receipt instead of repeating discovery.
        retain_preflight_receipt(&state);
        let expires_at = std::time::Instant::now() + MCP_INTERACTIVE_LOGIN_TIMEOUT;
        let authorize_url = authorize_endpoints(&discovery, &client, requested.offline_access)
            .authorize_url_with_pkce(&pkce.challenge, &state);
        Ok(McpOAuthLoginStart {
            target: target.clone(),
            authorize_url,
            state,
            redirect_uri: redirect_uri.to_owned(),
            disposition: McpOAuthLoginDisposition::Started,
            identity,
            expires_at,
        })
    }

    /// The `McpOauth` credential currently in `target`'s slot, if any, with
    /// its stored metadata and account binding. An occupant this target may
    /// never reconnect over (no binding, or another verification mode) is
    /// refused here: it needs an explicit disconnect.
    async fn slot_occupant(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<SlotOccupant>, McpOAuthError> {
        let Some(tokens) = self
            .token_store()
            .load(&target.token_key()?)
            .await
            .map_err(|error| McpOAuthError::TokenStore(error.to_string()))?
        else {
            return Ok(None);
        };
        let disconnect = || McpOAuthError::DisconnectRequired {
            server_name: target.server_name().to_owned(),
        };
        if tokens.auth_mode != PersistedAuthMode::McpOauth {
            return Err(disconnect());
        }
        let binding = McpCredentialBinding::from_tokens(&tokens).ok_or_else(disconnect)?;
        let metadata = stored_metadata_for_target(target, &tokens).map_err(|_| disconnect())?;
        if Some(binding.verification) != target.account_verification() {
            return Err(disconnect());
        }
        Ok(Some(SlotOccupant {
            account_id: tokens.account_id,
            binding,
            metadata,
        }))
    }

    /// Re-project the attempt already pending for `target` from its admitted
    /// descriptor. Only the recorded issuer's authorization-server metadata
    /// is fetched (no client registration, no account preflight). An
    /// attempt that no longer matches the target, or whose preflight this
    /// process does not retain, is retired and `None` returned.
    async fn join_pending_attempt(
        &self,
        target: &McpServerIdentity,
        state: &str,
        record: crate::oauth_flow::OAuthFlowRecord,
    ) -> Result<Option<McpOAuthLoginStart>, McpOAuthError> {
        if !pending_attempt_matches(target, &record) || !preflight_receipt_retained(state) {
            // A failed retirement is propagated: no fresh attempt is admitted
            // beside one the flow owner still holds.
            if let Some((authority, _)) = self.interactive.as_ref() {
                authority
                    .expire(
                        state,
                        &record.target,
                        record.provider.clone(),
                        &record.redirect_uri,
                    )
                    .map_err(McpOAuthError::Flow)?;
            }
            return Ok(None);
        }
        let OAuthBrowserFlowIdentity::Connector { connector } = &record.provider else {
            return Ok(None);
        };
        let facts = connector.parameters();
        let endpoints = self
            .discover_authorization_server(target, &facts.issuer)
            .await?;
        let offline_access = endpoints.offline_access;
        let discovery = completion_discovery(target, facts, endpoints)?;
        let client = StoredMcpOAuthClient {
            client_id: facts.client.clone(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_owned(),
            redirect_uri: record.redirect_uri.clone(),
        };
        let challenge = PkceChallenge::s256_for_verifier(&record.pkce_verifier);
        // The admitted descriptor carries the resource scopes; the joined
        // projection requests exactly what the original start requested.
        let authorize_url = authorize_endpoints(&discovery, &client, offline_access)
            .authorize_url_with_pkce(&challenge, state);
        Ok(Some(McpOAuthLoginStart {
            target: target.clone(),
            authorize_url,
            state: state.to_owned(),
            redirect_uri: record.redirect_uri,
            disposition: McpOAuthLoginDisposition::Joined,
            identity: record.provider,
            expires_at: record.created_at + MCP_INTERACTIVE_LOGIN_TIMEOUT,
        }))
    }

    /// Host loopback login: bind the callback listener, then admit (or join)
    /// the attempt. A joined attempt is bound to another host's listener, so
    /// this binding is retired and the joined projection returned instead.
    pub async fn begin_loopback_login(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoopbackBegin, McpOAuthError> {
        let binding = bind_loopback_callback(MCP_OAUTH_CALLBACK_PATH)
            .await
            .map_err(|_| McpOAuthError::Callback {
                server_name: target.server_name().to_owned(),
                reason: "loopback callback listener could not be bound".into(),
            })?;
        let start = match self
            .login_start(target, &binding.redirect_url, www_authenticate)
            .await
        {
            Ok(start) => start,
            Err(error) => {
                let _ = binding.cancel().await;
                return Err(error);
            }
        };
        if start.disposition == McpOAuthLoginDisposition::Joined {
            let _ = binding.cancel().await;
            return Ok(McpOAuthLoopbackBegin::Joined(start));
        }
        let callback = binding.expect_state(start.state.clone());
        Ok(McpOAuthLoopbackBegin::Started(McpOAuthPendingLogin {
            authority: self.clone(),
            start,
            callback: Some(callback),
            listener: None,
            completion: PendingCompletion::NotEntered,
            guard_retirement: RetireObserver::default(),
        }))
    }

    /// Complete one admitted attempt from the host's loopback callback.
    ///
    /// Before any network I/O, the flow owner must name a live attempt
    /// admitted under `state` for this exact target, and verify it against
    /// the callback's redirect URI. Issuer, client, resource and scopes then
    /// come only from that admitted descriptor; the only network fetch before
    /// the token exchange is the recorded issuer's authorization-server
    /// metadata (whose issuer must still match). The exchange uses the PKCE
    /// verifier retained by the flow owner. Account verification,
    /// persistence and AuthMachine publication are the existing canonical
    /// commit. Every non-success exit retires the attempt; a consumed attempt
    /// cannot be replayed.
    pub async fn login_complete(
        &self,
        target: &McpServerIdentity,
        callback: McpOAuthCallback,
    ) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        self.login_complete_observed(target, callback, None).await
    }

    /// [`login_complete`](Self::login_complete) that records its retire
    /// guard's own result for the pending login that handed the code over.
    async fn login_complete_observed(
        &self,
        target: &McpServerIdentity,
        callback: McpOAuthCallback,
        observed: Option<RetireObserver>,
    ) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        self.validate_interactive_selection(target)?;
        let (authority, strategy) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let McpOAuthCallback {
            redirect_uri,
            state,
            code,
        } = callback;
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        let admitted = authority
            .admitted_connector_browser_attempt(&state, &credential_identity)
            .map_err(McpOAuthError::Flow)?
            .ok_or(McpOAuthError::Flow(OAuthFlowError::Missing))?;
        let mut retire = AttemptRetireGuard {
            authority: Arc::clone(authority),
            state: state.clone(),
            target: credential_identity.clone(),
            identity: admitted.provider.clone(),
            redirect_uri: admitted.redirect_uri.clone(),
            armed: true,
            observed,
        };
        let record = authority
            .verify(
                &state,
                &credential_identity,
                admitted.provider.clone(),
                &redirect_uri,
            )
            .map_err(McpOAuthError::Flow)?;
        if !pending_attempt_matches(target, &record) {
            return Err(ConnectorOAuthRefusal::AccountMismatch.into());
        }
        let OAuthBrowserFlowIdentity::Connector { connector } = &record.provider else {
            return Err(McpOAuthError::Flow(OAuthFlowError::BrowserIdentityMismatch));
        };
        let facts = connector.parameters();
        let unverified = facts.expected_account.is_unverified();
        let context = McpOAuthCeremonyContext {
            issuer: &facts.issuer,
            client: &facts.client,
            resource: &facts.resource,
            redirect_uri: &facts.redirect_uri,
            account: &facts.expected_account,
        };
        let expected_descriptor = if unverified {
            unverified_descriptor(&context, &BTreeSet::new())?
        } else {
            strategy.descriptor(target, &context)?
        };
        if !admitted_extends_strategy_descriptor(connector, &expected_descriptor) {
            return Err(McpOAuthError::Verification(
                ConnectorOAuthRefusal::DescriptorMismatch,
            ));
        }
        // The admitted descriptor is the authority: the strategy's facts plus
        // the resource scopes selected when the attempt started.
        let descriptor: &ConnectorOAuthDescriptor = connector;
        let endpoints = self
            .discover_authorization_server(target, &facts.issuer)
            .await?;
        let discovery = completion_discovery(target, facts, endpoints)?;
        let client = StoredMcpOAuthClient {
            client_id: facts.client.clone(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_owned(),
            redirect_uri: record.redirect_uri.clone(),
        };
        let token = exchange_authorization_code_with_state(
            self.http()?,
            &mcp_oauth_endpoints(&discovery, &client),
            &code,
            &record.pkce_verifier,
            None,
            Some(&state),
        )
        .await
        .map_err(|_| McpOAuthError::TokenExchangeFailed {
            server_name: target.server_name().to_owned(),
            reason: "authorization-code exchange failed".into(),
        })?;
        let binding = McpCredentialBinding::for_descriptor(descriptor);
        let mut persisted = persisted_tokens_from_result(
            &token,
            &discovery,
            &client,
            target,
            &binding,
            Utc::now(),
        )?;
        let completion: OAuthBrowserFlowCompletion = if unverified {
            // No account is observed, inferred or reported for this grant.
            let grant = descriptor.accept_unverified_grant(&token)?;
            persisted.account_id = None;
            persisted.scopes = grant.granted_scopes().iter().cloned().collect();
            grant.into()
        } else {
            let evidence = descriptor
                .verify_account(strategy.observe_account(descriptor, &token).await?, &token)?;
            persisted.account_id = Some(evidence.account().to_owned());
            persisted.scopes = evidence.granted_scopes().iter().cloned().collect();
            evidence.into()
        };
        let committed = save_oauth_tokens_and_consume_browser_flow(
            self.provider_auth_persistence.clone(),
            self.auth_lease.clone(),
            credential_identity,
            persisted,
            BrowserOAuthFlowCommit {
                authority: authority.clone(),
                state,
                completion,
                redirect_uri: record.redirect_uri.clone(),
            },
        )
        .await
        .map_err(|error| map_coordinated_login_error(target, error))?;
        // The commit consumed the attempt.
        retire.armed = false;
        if committed.primary_secret.is_none() {
            return Err(McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_owned(),
                reason: "committed credential has no bearer token".into(),
            });
        }
        Ok(McpOAuthLoginComplete {
            target: target.clone(),
            account_id: committed.account_id.clone(),
            account_verification: binding.verification,
            expires_at: committed.expires_at,
            has_refresh_token: committed.refresh_token.is_some(),
            scopes: committed.scopes,
        })
    }

    /// Retire an abandoned attempt (host timeout, cancellation or closed
    /// browser) through its flow owner. Cancellation consumes no
    /// authorization response and publishes no credential.
    pub fn login_cancel(
        &self,
        target: &McpServerIdentity,
        start: &McpOAuthLoginStart,
    ) -> Result<(), McpOAuthError> {
        let (authority, _) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        authority
            .expire(
                &start.state,
                &target.auth_binding_ref()?.into(),
                start.identity.clone(),
                &start.redirect_uri,
            )
            .map_err(McpOAuthError::Flow)
    }

    /// Typed cancel by `state` for hosts that keep no start projection (for
    /// example wire callers): retire the attempt admitted under `state` for
    /// this exact target. Local only; an unknown state is refused.
    pub fn cancel_attempt(
        &self,
        target: &McpServerIdentity,
        state: &str,
    ) -> Result<(), McpOAuthError> {
        let (authority, _) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        let record = authority
            .admitted_connector_browser_attempt(state, &credential_identity)
            .map_err(McpOAuthError::Flow)?
            .ok_or(McpOAuthError::Flow(OAuthFlowError::Missing))?;
        authority
            .expire(
                state,
                &credential_identity,
                record.provider,
                &record.redirect_uri,
            )
            .map_err(McpOAuthError::Flow)
    }

    /// The attempt pending for `target`, if any, without its secrets.
    ///
    /// Read-only and local: it admits, extends, consumes and retires
    /// nothing, and performs no network I/O. A host uses it to find an
    /// attempt it no longer holds, for example after its own restart, when
    /// the flow owner restored an attempt whose callback listener is gone.
    /// Only an attempt of the target's selection for this exact resource is
    /// reported; an unselected target has none.
    pub fn pending_attempt(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<McpOAuthPendingAttempt>, McpOAuthError> {
        if !target.is_selected() {
            return Ok(None);
        }
        let (authority, _) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        let Some((state, record)) = authority
            .pending_connector_browser_attempt(&credential_identity)
            .map_err(McpOAuthError::Flow)?
        else {
            return Ok(None);
        };
        if !pending_attempt_matches(target, &record) {
            return Ok(None);
        }
        let remaining = MCP_INTERACTIVE_LOGIN_TIMEOUT.saturating_sub(record.created_at.elapsed());
        let remaining =
            chrono::TimeDelta::from_std(remaining).unwrap_or_else(|_| chrono::TimeDelta::zero());
        let now = Utc::now();
        Ok(Some(McpOAuthPendingAttempt {
            attempt_ref: OAuthBrowserActionRef::project(&state),
            expires_at: now.checked_add_signed(remaining).unwrap_or(now),
        }))
    }

    /// Typed cancel by the non-secret reference a [`Self::pending_attempt`]
    /// read reported, for a host that no longer holds the attempt's state:
    /// retire the attempt pending for `target` under that reference. Local
    /// only; an unknown or stale reference is refused.
    pub fn cancel_attempt_by_ref(
        &self,
        target: &McpServerIdentity,
        attempt_ref: &str,
    ) -> Result<(), McpOAuthError> {
        let (authority, _) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        let (state, record) = authority
            .pending_connector_browser_attempt(&credential_identity)
            .map_err(McpOAuthError::Flow)?
            .ok_or(McpOAuthError::Flow(OAuthFlowError::Missing))?;
        if OAuthBrowserActionRef::project(&state).as_str() != attempt_ref
            || !pending_attempt_matches(target, &record)
        {
            return Err(McpOAuthError::Flow(OAuthFlowError::Missing));
        }
        authority
            .expire(
                &state,
                &credential_identity,
                record.provider,
                &record.redirect_uri,
            )
            .map_err(McpOAuthError::Flow)
    }

    /// Disconnect `target`: retire every browser attempt pending for its
    /// slot, then remove its stored credential and release the credential
    /// lifecycle through the same coordinated mutation that guards refresh
    /// and login commits. Both happen even when no credential is stored, so
    /// a callback admitted before the disconnect cannot repopulate the slot:
    /// its commit finds the attempt retired. Nothing is revoked at the
    /// provider.
    pub async fn logout(&self, target: &McpServerIdentity) -> Result<(), McpOAuthError> {
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        if let Some((authority, _)) = self.interactive.as_ref() {
            // The flow owner holds at most one attempt per slot; each pass
            // retires the one it reports, so this ends when none is left.
            while let Some((state, record)) = authority
                .pending_connector_browser_attempt(&credential_identity)
                .map_err(McpOAuthError::Flow)?
            {
                authority
                    .expire(
                        &state,
                        &credential_identity,
                        record.provider,
                        &record.redirect_uri,
                    )
                    .map_err(McpOAuthError::Flow)?;
            }
        }
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
            self.provider_auth_persistence.clone(),
            self.auth_lease.clone(),
            credential_identity,
        )
        .await
        .map_err(|error| map_coordinated_login_error(target, error))
    }

    /// Load one durable MCP credential through its marker, AuthMachine
    /// projection, freshness observation, and generated use-admission gate.
    /// The caller holds the per-binding lifecycle guard for this whole read.
    async fn load_admitted_stored_credential(
        &self,
        target: &McpServerIdentity,
        key: &TokenKey,
        guard: &meerkat_core::AuthLoginLifecycleGuard,
    ) -> Result<Option<AdmittedMcpCredential>, McpOAuthError> {
        if guard.lease_key() != &target.lease_key()? {
            return Err(McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: "credential lifecycle guard belongs to another lease".into(),
            });
        }
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
        verify_stored_account(target, &tokens)?;
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
        let marker_relation =
            durable_marker::marker_relation_for_tokens_and_snapshot(&tokens, &snapshot, key);
        // Legacy interrupted-refresh bytes may be normalized only against
        // their still-authorized owner. Check before any durable restoration
        // can replace a newer verdict or create an absent owner.
        if meerkat_core::tokens_lifecycle_publication(&tokens).and_then(|marker| marker.phase)
            == Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
            && (marker_relation != durable_marker::AuthLeaseDurableMarkerRelation::Matches
                || !crate::resolver::legacy_refresh_owner_allows_preparation(
                    &self.auth_lease,
                    &lease_key,
                )
                .map_err(lifecycle_err)?)
        {
            return Err(McpOAuthError::StalePreparation);
        }
        let restore_from_durable_marker = lifecycle_snapshot_is_absent(&snapshot)
            || matches!(
                marker_relation,
                durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
            );
        if restore_from_durable_marker {
            tokens = meerkat_core::rehydrate_marked_tokens_for_status_with_guard(
                self.token_store().as_ref(),
                &self.auth_lease,
                &auth_binding,
                PersistedAuthMode::McpOauth,
                Utc::now(),
                guard,
            )
            .await
            .map_err(|error| McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .ok_or_else(|| McpOAuthError::ReauthRequired {
                server_name: target.server_name().to_string(),
            })?;
            verify_stored_account(target, &tokens)?;
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
        // Before any lifecycle transition: without a redirect-free client no
        // refresh begins.
        let http = self
            .http()
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        let lease_key = target
            .lease_key()
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let mut admitted = self
            .load_admitted_stored_credential(target, key, &_guard)
            .await
            .map_err(refresh_error_from_mcp)?
            .ok_or_else(|| {
                RefreshError::ReauthRequired(
                    "stored MCP OAuth credential disappeared before refresh".to_string(),
                )
            })?;
        if crate::resolver::normalize_interrupted_refresh_under_coordinator(
            self.token_store().as_ref(),
            &self.auth_lease,
            key,
            &admitted.tokens,
            &_guard,
        )
        .await?
        .is_some()
        {
            admitted = self
                .load_admitted_stored_credential(target, key, &_guard)
                .await
                .map_err(refresh_error_from_mcp)?
                .ok_or(RefreshError::StalePreparation)?;
        }
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
        drop(_guard);
        let exchange = exchange_refresh_token(
            http,
            &endpoints,
            &refresh_token,
            metadata.client.client_secret.as_deref(),
        )
        .await;
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let current_tokens = self
            .token_store()
            .load(key)
            .await
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        if current_tokens.as_ref() != Some(&admitted.tokens)
            || self.auth_lease.snapshot(&lease_key) != refreshing_snapshot
        {
            return Err(RefreshError::StalePreparation);
        }
        let refreshed = match exchange {
            Ok(refreshed) => refreshed,
            Err(error) => {
                // Host-side diagnostics only. The response body may echo the
                // refresh grant, so it is never logged or rendered; its size
                // is enough to tell an empty refusal from a verbose one.
                if let crate::auth_oauth::OAuthError::TokenEndpoint { status, body } = &error {
                    tracing::debug!(
                        server_name = target.server_name(),
                        status,
                        body_bytes = body.len(),
                        "MCP OAuth token endpoint refused the refresh"
                    );
                }
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
        // Subject: a verified credential's refreshed token must still be the
        // bound account. An unverified grant stays unverified: nothing is
        // observed for it.
        if let Some(bound) = admitted
            .tokens
            .account_id
            .as_deref()
            .filter(|_| target.account_verification() == Some(AccountVerification::Verified))
        {
            match self
                .observe_refreshed_subject(target, metadata, bound, &refreshed)
                .await
            {
                Ok(observed) if observed == bound => {}
                Ok(_) => {
                    let observation = meerkat_core::RefreshFailureObservation::transient();
                    return Err(
                        match self.auth_lease.refresh_failed(&lease_key, observation) {
                            Ok(_) => RefreshError::CredentialIdentityMismatch,
                            Err(error) => RefreshError::Refresh(format!(
                                "refreshed MCP OAuth subject differs from the bound account; AuthMachine refresh_failed rejected closure: {error}"
                            )),
                        },
                    );
                }
                Err(reason) => {
                    let observation = meerkat_core::RefreshFailureObservation::transient();
                    return Err(
                        match self.auth_lease.refresh_failed(&lease_key, observation) {
                            Ok(_) => RefreshError::Refresh(reason.to_owned()),
                            Err(error) => RefreshError::Refresh(format!(
                                "{reason}; AuthMachine refresh_failed rejected closure: {error}"
                            )),
                        },
                    );
                }
            }
        }
        let refreshed_at = Utc::now();
        let mut persisted = match persisted_tokens_from_result_with_binding(
            &refreshed,
            &metadata.discovery,
            &metadata.client,
            target,
            metadata.account_binding.as_ref(),
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
        // Refresh keeps the bound subject (re-observed above when verified,
        // absent for an unverified grant); a server label is never an account.
        persisted.account_id = admitted.tokens.account_id.clone();
        if persisted.refresh_token.is_none() {
            persisted.refresh_token = Some(refresh_token);
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

    /// The subject the account strategy observes for a refreshed token of a
    /// verified credential bound to `bound`. `Err` means the subject could
    /// not be observed (no strategy, another strategy or context, or the
    /// provider evidence was unavailable), which is distinct from a
    /// different subject.
    async fn observe_refreshed_subject(
        &self,
        target: &McpServerIdentity,
        metadata: &StoredMcpOAuthMetadata,
        bound: &str,
        refreshed: &OAuthTokenResult,
    ) -> Result<String, &'static str> {
        let Some((_, strategy)) = self.interactive.as_ref() else {
            return Err("MCP OAuth account strategy is not installed");
        };
        let account = AccountSelection::Known(bound.to_owned());
        let descriptor = strategy
            .descriptor(
                target,
                &McpOAuthCeremonyContext {
                    issuer: &metadata.discovery.authorization_server,
                    client: &metadata.client.client_id,
                    resource: &metadata.discovery.resource,
                    redirect_uri: &metadata.client.redirect_uri,
                    account: &account,
                },
            )
            .map_err(|_| "MCP OAuth account strategy refused the stored context")?;
        if let Some(binding) = &metadata.account_binding
            && (binding.stable_context != descriptor.stable_context()
                || binding.strategy_id != descriptor.parameters().strategy_id)
        {
            return Err("installed MCP OAuth account strategy differs from the bound one");
        }
        strategy
            .observe_account(&descriptor, refreshed)
            .await
            .map(|observation| observation.account)
            .map_err(|_| "refreshed MCP OAuth subject could not be observed")
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
    ) -> Result<(StoredMcpOAuthDiscovery, RequestedScopes), McpOAuthError> {
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
            .http()?
            .get(resource_metadata_url.clone())
            .send()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .error_for_status_refusing_redirects()
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error,
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
        let endpoints = self
            .discover_authorization_server(target, &auth_server)
            .await?;
        let requested = RequestedScopes {
            resource_scopes: select_resource_scopes(www_authenticate, &resource.scopes_supported),
            offline_access: endpoints.offline_access,
        };
        Ok((
            StoredMcpOAuthDiscovery {
                resource: resource.resource,
                resource_metadata_url,
                authorization_server: auth_server,
                authorization_metadata_url: endpoints.authorization_metadata_url,
                authorization_endpoint: endpoints.authorization_endpoint,
                token_endpoint: endpoints.token_endpoint,
                registration_endpoint: endpoints.registration_endpoint,
                scopes: Vec::new(),
            },
            requested,
        ))
    }

    /// Fetch and validate the authorization-server metadata of `auth_server`:
    /// RFC 8414, then OpenID Connect discovery when the RFC 8414 document is
    /// absent (see [`authorization_server_metadata_urls`]). The first document
    /// found must have the exact issuer, PKCE S256, and https (or loopback)
    /// endpoints. Redirects are refused.
    async fn discover_authorization_server(
        &self,
        target: &McpServerIdentity,
        auth_server: &str,
    ) -> Result<AuthorizationServerEndpoints, McpOAuthError> {
        let auth_server = auth_server.to_owned();
        require_https_or_loopback(target, &auth_server, "authorization server issuer")?;
        let mut found = None;
        for candidate in authorization_server_metadata_urls(&auth_server)? {
            let response = self.http()?.get(&candidate).send().await.map_err(|error| {
                McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name().to_string(),
                    reason: error.to_string(),
                }
            })?;
            // Only an absent document moves on to the next well-known URI;
            // any other refusal fails closed.
            if matches!(
                response.status(),
                reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
            ) {
                continue;
            }
            found = Some((candidate, response));
            break;
        }
        let Some((authorization_metadata_url, response)) = found else {
            return Err(McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: "no authorization server metadata (RFC 8414 or OpenID Connect discovery)"
                    .to_string(),
            });
        };
        let auth: AuthorizationServerMetadata = response
            .error_for_status_refusing_redirects()
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error,
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
        let offline_access = auth
            .scopes_supported
            .iter()
            .any(|scope| scope == OFFLINE_ACCESS_SCOPE);
        Ok(AuthorizationServerEndpoints {
            authorization_metadata_url,
            authorization_endpoint,
            token_endpoint,
            registration_endpoint,
            offline_access,
        })
    }

    async fn discover_resource_metadata_url_by_well_known(
        &self,
        target: &McpServerIdentity,
    ) -> Result<String, McpOAuthError> {
        for candidate in protected_resource_well_known_candidates(target.server_url())? {
            let response = self.http()?.get(&candidate).send().await;
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
        // MCP authorization requires the application type. Every redirect
        // admitted here is a loopback redirect (RFC 8252), i.e. a native
        // client; OIDC servers default an omitted type to `web`.
        let body = serde_json::json!({
            "client_name": CLIENT_NAME,
            "application_type": "native",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let wire: DynamicClientRegistrationResponse = self
            .http()?
            .post(&discovery.registration_endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            })?
            .error_for_status_refusing_redirects()
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name().to_string(),
                reason: error,
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

/// Whether a stored credential belongs to `target`'s selection: the Known
/// subject; a verified Discover subject; or an unverified grant with no
/// account. A legacy target keeps its pre-selection semantics. A recorded
/// binding must carry the target's verification mode.
fn verify_stored_account(
    target: &McpServerIdentity,
    tokens: &PersistedTokens,
) -> Result<(), McpOAuthError> {
    if stored_credential_matches_selection(target, tokens) {
        Ok(())
    } else {
        Err(ConnectorOAuthRefusal::AccountMismatch.into())
    }
}

/// See [`verify_stored_account`]. Hosts use it to project status from a
/// stored credential without admitting it.
pub fn stored_credential_matches_selection(
    target: &McpServerIdentity,
    tokens: &PersistedTokens,
) -> bool {
    let binding = McpCredentialBinding::from_tokens(tokens);
    let verification = binding.as_ref().map(|binding| binding.verification);
    match target.selection() {
        McpAccountSelection::Legacy => true,
        // A Known credential committed before account bindings were
        // recorded has none; its subject still has to match.
        McpAccountSelection::Known(account) => {
            tokens.account_id.as_deref() == Some(account.as_str())
                && verification.is_none_or(|mode| mode == AccountVerification::Verified)
        }
        McpAccountSelection::Discover => {
            tokens
                .account_id
                .as_deref()
                .is_some_and(|account| !account.is_empty())
                && verification == Some(AccountVerification::Verified)
        }
        McpAccountSelection::Unverified => {
            tokens.account_id.is_none() && verification == Some(AccountVerification::Unverified)
        }
    }
}

/// A credential occupying the slot a login would publish into.
struct SlotOccupant {
    account_id: Option<String>,
    binding: McpCredentialBinding,
    metadata: StoredMcpOAuthMetadata,
}

/// The account selection one attempt is admitted with. An empty Discover
/// slot discovers; an occupied one reconnects only its bound subject,
/// admitted as Known while keeping the Discover slot. An occupied
/// unverified slot never reconnects: replacing it needs a disconnect.
fn attempt_selection(
    target: &McpServerIdentity,
    occupant: Option<&SlotOccupant>,
) -> Result<AccountSelection, McpOAuthError> {
    let disconnect = || McpOAuthError::DisconnectRequired {
        server_name: target.server_name().to_owned(),
    };
    match (target.selection(), occupant) {
        (McpAccountSelection::Legacy, _) => Err(McpOAuthError::AccountSelectionRequired),
        (McpAccountSelection::Known(account), None) => Ok(AccountSelection::Known(account.clone())),
        (McpAccountSelection::Known(account), Some(occupant)) => {
            if occupant.account_id.as_deref() == Some(account.as_str()) {
                Ok(AccountSelection::Known(account.clone()))
            } else {
                Err(disconnect())
            }
        }
        (McpAccountSelection::Discover, None) => Ok(AccountSelection::Discover),
        (McpAccountSelection::Discover, Some(occupant)) => occupant
            .account_id
            .clone()
            .filter(|account| !account.is_empty())
            .map(AccountSelection::Known)
            .ok_or_else(disconnect),
        (McpAccountSelection::Unverified, None) => Ok(AccountSelection::Unverified),
        (McpAccountSelection::Unverified, Some(_)) => Err(disconnect()),
    }
}

/// The natively built descriptor of an explicitly unverified attempt: the
/// admitted issuer, client, resource and redirect, the resource's scopes,
/// no account, and the unverified strategy identifier.
fn unverified_descriptor(
    context: &McpOAuthCeremonyContext<'_>,
    resource_scopes: &BTreeSet<String>,
) -> Result<ConnectorOAuthDescriptor, McpOAuthError> {
    if !context.account.is_unverified() {
        return Err(ConnectorOAuthRefusal::InvalidDescriptor.into());
    }
    Ok(ConnectorOAuthParameters {
        issuer: context.issuer.to_owned(),
        client: context.client.to_owned(),
        resource: context.resource.to_owned(),
        scopes: resource_scopes.clone(),
        redirect_uri: context.redirect_uri.to_owned(),
        expected_account: AccountSelection::Unverified,
        strategy_id: UNVERIFIED_RESOURCE_STRATEGY_ID.to_owned(),
    }
    .try_into()?)
}

/// Attempts this process admitted after a successful strategy preflight
/// (or for a mode that needs none), keyed by the non-secret reference of
/// their state. A join reuses this receipt; an attempt without one (for
/// example restored after a restart) is retired and admitted afresh, so no
/// attempt reaches the browser without its preflight.
fn preflight_receipts()
-> &'static parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static RECEIPTS: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    > = std::sync::OnceLock::new();
    RECEIPTS.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
}

fn retain_preflight_receipt(state: &str) {
    let now = std::time::Instant::now();
    let mut receipts = preflight_receipts().lock();
    // A receipt is useless once its attempt has expired.
    receipts.retain(|_, admitted| now.duration_since(*admitted) < MCP_INTERACTIVE_LOGIN_TIMEOUT);
    receipts.insert(
        OAuthBrowserActionRef::project(state).as_str().to_owned(),
        now,
    );
}

fn preflight_receipt_retained(state: &str) -> bool {
    preflight_receipts()
        .lock()
        .contains_key(OAuthBrowserActionRef::project(state).as_str())
}

/// Serializes start-or-join per MCP target within this process, so a target
/// has at most one admitted attempt. Only admission takes this lock: the
/// credential lifecycle (refresh, use, commit) keeps its own guard.
async fn acquire_admission_lock(binding_slug: String) -> tokio::sync::OwnedMutexGuard<()> {
    static ADMISSION: std::sync::OnceLock<
        parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let lock = {
        let mut locks = ADMISSION
            .get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
            .lock();
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(locks.entry(binding_slug).or_default())
    };
    lock.lock_owned().await
}

/// The redirect-free HTTP client for credential endpoints could not be
/// built. Nothing falls back to a client that follows redirects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the redirect-free credential HTTP client could not be built")]
pub struct CredentialHttpClientUnavailable;

/// HTTP client for MCP and connector OAuth endpoints: follows no redirects.
/// A build failure is returned, never replaced by a default client.
pub(crate) fn no_redirect_client() -> Result<Client, CredentialHttpClientUnavailable> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| CredentialHttpClientUnavailable)
}

pub(crate) trait RefuseRedirect {
    /// `error_for_status` that also refuses any 3xx answer explicitly.
    fn error_for_status_refusing_redirects(self) -> Result<reqwest::Response, String>;
}

impl RefuseRedirect for reqwest::Response {
    fn error_for_status_refusing_redirects(self) -> Result<reqwest::Response, String> {
        if self.status().is_redirection() {
            return Err(format!(
                "unexpected redirect ({}); redirects are refused",
                self.status()
            ));
        }
        self.error_for_status().map_err(|error| error.to_string())
    }
}

/// RFC 8252 section 7.3: the redirect must be an http loopback address.
fn require_loopback_redirect(redirect_uri: &str) -> Result<(), McpOAuthError> {
    let url = reqwest::Url::parse(redirect_uri)
        .map_err(|_| McpOAuthError::Verification(ConnectorOAuthRefusal::InvalidDescriptor))?;
    if url.scheme() != "http" || !is_loopback_url(&url) {
        return Err(McpOAuthError::Verification(
            ConnectorOAuthRefusal::InvalidDescriptor,
        ));
    }
    Ok(())
}

/// Discovery for an admitted attempt, built only from its descriptor and the
/// recorded issuer's validated endpoints.
fn completion_discovery(
    target: &McpServerIdentity,
    facts: &crate::connector_oauth::ConnectorOAuthParameters,
    endpoints: AuthorizationServerEndpoints,
) -> Result<StoredMcpOAuthDiscovery, McpOAuthError> {
    let resource_metadata_url = protected_resource_well_known_candidates(target.server_url())?
        .into_iter()
        .next()
        .unwrap_or_default();
    Ok(StoredMcpOAuthDiscovery {
        resource: facts.resource.clone(),
        resource_metadata_url,
        authorization_server: facts.issuer.clone(),
        authorization_metadata_url: endpoints.authorization_metadata_url,
        authorization_endpoint: endpoints.authorization_endpoint,
        token_endpoint: endpoints.token_endpoint,
        registration_endpoint: endpoints.registration_endpoint,
        scopes: facts.scopes.iter().cloned().collect(),
    })
}

fn mcp_oauth_endpoints(
    discovery: &StoredMcpOAuthDiscovery,
    client: &StoredMcpOAuthClient,
) -> OAuthEndpoints {
    OAuthEndpoints {
        client_id: client.client_id.clone(),
        authorize_url: discovery.authorization_endpoint.clone(),
        token_url: discovery.token_endpoint.clone(),
        device_code_url: None,
        redirect_uri: client.redirect_uri.clone(),
        scopes: discovery.scopes.clone(),
        extra_authorize_params: vec![("resource".to_string(), discovery.resource.clone())],
        token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
        include_state_in_token_exchange: false,
        extra_token_params: vec![("resource".to_string(), discovery.resource.clone())],
        refresh_scopes: discovery.scopes.clone(),
        extra_headers: Vec::new(),
    }
}

/// What one attempt asks the authorization server for, beyond the account
/// strategy's own evidence scopes (MCP authorization, "Scope Selection
/// Strategy").
struct RequestedScopes {
    /// The resource's scopes, required in the grant.
    resource_scopes: BTreeSet<String>,
    /// The authorization server advertises `offline_access`: requested so it
    /// can issue a refresh token, never required.
    offline_access: bool,
}

/// The `offline_access` scope (OpenID Connect Core 1.0, section 11).
const OFFLINE_ACCESS_SCOPE: &str = "offline_access";

/// The resource's scopes for one attempt: the `scope` of the 401 challenge
/// when the caller observed one, else the resource metadata's
/// `scopes_supported`. `offline_access` is never a resource scope.
fn select_resource_scopes(
    www_authenticate: Option<&str>,
    scopes_supported: &[String],
) -> BTreeSet<String> {
    let challenged: BTreeSet<String> = www_authenticate
        .and_then(|header| auth_param(header, "scope"))
        .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    let selected = if challenged.is_empty() {
        scopes_supported.iter().cloned().collect()
    } else {
        challenged
    };
    selected
        .into_iter()
        .filter(|scope| scope != OFFLINE_ACCESS_SCOPE)
        .collect()
}

/// The strategy's descriptor with the resource's scopes added to its
/// required scopes. Every other fact is unchanged.
fn require_resource_scopes(
    descriptor: ConnectorOAuthDescriptor,
    resource_scopes: &BTreeSet<String>,
) -> Result<ConnectorOAuthDescriptor, McpOAuthError> {
    if resource_scopes.is_subset(&descriptor.parameters().scopes) {
        return Ok(descriptor);
    }
    let mut parameters = crate::connector_oauth::ConnectorOAuthParameters::from(descriptor);
    parameters.scopes.extend(resource_scopes.iter().cloned());
    Ok(parameters.try_into()?)
}

/// Whether `admitted` is `strategy` plus resource scopes: the descriptor
/// [`require_resource_scopes`] admitted for this strategy. Completion cannot
/// re-derive the resource scopes (it fetches no resource metadata), so the
/// admitted descriptor carries them and may only be stricter.
fn admitted_extends_strategy_descriptor(
    admitted: &ConnectorOAuthDescriptor,
    strategy: &ConnectorOAuthDescriptor,
) -> bool {
    let (admitted, strategy) = (admitted.parameters(), strategy.parameters());
    admitted.issuer == strategy.issuer
        && admitted.client == strategy.client
        && admitted.resource == strategy.resource
        && admitted.redirect_uri == strategy.redirect_uri
        && admitted.expected_account == strategy.expected_account
        && admitted.strategy_id == strategy.strategy_id
        && strategy.scopes.is_subset(&admitted.scopes)
}

/// Authorize-request endpoints: the attempt's required scopes, plus
/// `offline_access` when the authorization server advertises it.
fn authorize_endpoints(
    discovery: &StoredMcpOAuthDiscovery,
    client: &StoredMcpOAuthClient,
    offline_access: bool,
) -> OAuthEndpoints {
    let mut endpoints = mcp_oauth_endpoints(discovery, client);
    if offline_access
        && !endpoints
            .scopes
            .iter()
            .any(|scope| scope == OFFLINE_ACCESS_SCOPE)
    {
        endpoints.scopes.push(OFFLINE_ACCESS_SCOPE.to_owned());
    }
    endpoints
}

/// Whether a pending connector attempt belongs to `target`: its descriptor
/// names the target's resource and an account selection of the target's
/// mode.
fn pending_attempt_matches(
    target: &McpServerIdentity,
    record: &crate::oauth_flow::OAuthFlowRecord,
) -> bool {
    let OAuthBrowserFlowIdentity::Connector { connector } = &record.provider else {
        return false;
    };
    let facts = connector.parameters();
    if facts.resource != target.server_url() {
        return false;
    }
    let unverified = facts.expected_account.is_unverified()
        && facts.strategy_id == UNVERIFIED_RESOURCE_STRATEGY_ID;
    match target.selection() {
        McpAccountSelection::Legacy => false,
        McpAccountSelection::Known(account) => {
            facts.expected_account.known() == Some(account.as_str())
        }
        // An empty Discover slot admits Discover; an occupied one reconnects
        // its bound subject as Known. The slot key already scopes the
        // attempt to this target's Discover slot.
        McpAccountSelection::Discover => {
            !facts.expected_account.is_unverified()
                && facts.strategy_id != UNVERIFIED_RESOURCE_STRATEGY_ID
        }
        McpAccountSelection::Unverified => unverified,
    }
}

fn persisted_tokens_from_result(
    result: &OAuthTokenResult,
    discovery: &StoredMcpOAuthDiscovery,
    client: &StoredMcpOAuthClient,
    target: &McpServerIdentity,
    binding: &McpCredentialBinding,
    now: DateTime<Utc>,
) -> Result<PersistedTokens, McpOAuthError> {
    persisted_tokens_from_result_with_binding(result, discovery, client, target, Some(binding), now)
}

/// Refresh keeps the credential's recorded binding (or its absence, for a
/// credential that predates account bindings) unchanged.
fn persisted_tokens_from_result_with_binding(
    result: &OAuthTokenResult,
    discovery: &StoredMcpOAuthDiscovery,
    client: &StoredMcpOAuthClient,
    target: &McpServerIdentity,
    binding: Option<&McpCredentialBinding>,
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
        account_binding: binding.cloned(),
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

fn map_coordinated_login_error(
    target: &McpServerIdentity,
    error: CredentialMutationError,
) -> McpOAuthError {
    match error {
        CredentialMutationError::StalePreparation => McpOAuthError::StalePreparation,
        CredentialMutationError::TokenStore(reason) => McpOAuthError::TokenStore(reason),
        CredentialMutationError::AuthLifecycle(reason)
        | CredentialMutationError::Operation(reason)
        | CredentialMutationError::LockFailed(reason) => McpOAuthError::AuthLifecycle {
            server_name: target.server_name().to_string(),
            reason,
        },
        CredentialMutationError::SlotRefused(refusal) => McpOAuthError::CredentialSlot {
            server_name: target.server_name().to_string(),
            refusal,
        },
        CredentialMutationError::Cancelled => McpOAuthError::AuthLifecycle {
            server_name: target.server_name().to_string(),
            reason: "credential mutation coordinator cancelled the login commit".to_string(),
        },
    }
}

pub(crate) fn lifecycle_snapshot_is_absent(snapshot: &AuthLeaseSnapshot) -> bool {
    snapshot.phase.is_none()
        && !snapshot.credential_present
        && snapshot.generation == 0
        && snapshot.credential_published_at_millis.is_none()
}

fn refresh_error_from_mcp(error: McpOAuthError) -> RefreshError {
    match error {
        McpOAuthError::Verification(ConnectorOAuthRefusal::AccountMismatch) => {
            RefreshError::CredentialIdentityMismatch
        }
        McpOAuthError::StalePreparation => RefreshError::StalePreparation,
        McpOAuthError::ReauthRequired { .. }
        | McpOAuthError::MissingStoredToken { .. }
        | McpOAuthError::MissingStoredMetadata { .. } => {
            RefreshError::ReauthRequired(error.to_string())
        }
        other => RefreshError::Refresh(other.to_string()),
    }
}

fn map_coordinated_refresh_error(target: &McpServerIdentity, error: RefreshError) -> McpOAuthError {
    if matches!(error, RefreshError::CredentialIdentityMismatch) {
        return ConnectorOAuthRefusal::AccountMismatch.into();
    }
    if matches!(&error, RefreshError::StalePreparation) {
        return McpOAuthError::StalePreparation;
    }
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
pub(crate) fn epoch_secs(time: DateTime<Utc>) -> u64 {
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

pub(crate) fn absolutize_url(base: &str, value: &str) -> Result<String, String> {
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

/// Well-known authorization-server metadata URIs of `server`, in the order
/// MCP authorization (2026-07-28) requires: RFC 8414 with path insertion,
/// OpenID Connect discovery with path insertion, then with path appending;
/// for an issuer without a path, RFC 8414 then OpenID Connect discovery.
fn authorization_server_metadata_urls(server: &str) -> Result<Vec<String>, McpOAuthError> {
    let url = reqwest::Url::parse(server).map_err(|error| McpOAuthError::DiscoveryFailed {
        server_name: "unknown".to_string(),
        reason: error.to_string(),
    })?;
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_matches('/');
    if path.is_empty() {
        Ok(vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ])
    } else {
        Ok(vec![
            format!("{origin}/.well-known/oauth-authorization-server/{path}"),
            format!("{origin}/.well-known/openid-configuration/{path}"),
            format!("{origin}/{path}/.well-known/openid-configuration"),
        ])
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

pub(crate) fn is_loopback_url(url: &reqwest::Url) -> bool {
    matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    )
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
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
    #[serde(default)]
    scopes_supported: Vec<String>,
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
    /// Read by [`McpCredentialBinding::from_tokens`] under this exact name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_binding: Option<McpCredentialBinding>,
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

    #[tokio::test]
    async fn a_failed_client_build_is_typed_and_never_a_permissive_default() {
        let strategy = OidcUserInfoAccountStrategy {
            http: Err(CredentialHttpClientUnavailable),
            required_scopes: BTreeSet::new(),
        };
        // No request is sent with some other client: the strategy refuses.
        assert_eq!(
            strategy.preflight("http://127.0.0.1:1").await,
            Err(ConnectorOAuthRefusal::VerificationUnavailable)
        );
        let error = McpOAuthError::HttpClientUnavailable(CredentialHttpClientUnavailable);
        assert!(!error.is_refusal(), "a host fault, not a refused request");
        assert!(no_redirect_client().is_ok());
    }

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
    fn authorization_server_metadata_tries_rfc8414_then_openid_connect_discovery() {
        assert_eq!(
            authorization_server_metadata_urls("https://auth.example.test").unwrap(),
            vec![
                "https://auth.example.test/.well-known/oauth-authorization-server",
                "https://auth.example.test/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            authorization_server_metadata_urls("https://auth.example.test/tenant1/").unwrap(),
            vec![
                "https://auth.example.test/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.test/.well-known/openid-configuration/tenant1",
                "https://auth.example.test/tenant1/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn resource_scopes_prefer_the_challenge_and_never_require_offline_access() {
        let supported = vec![
            "mcp.read".to_owned(),
            "offline_access".to_owned(),
            "mcp.admin".to_owned(),
        ];
        let challenge = r#"Bearer error="insufficient_scope", scope="mcp.write offline_access", resource_metadata="https://mcp.example.test/.well-known/oauth-protected-resource""#;
        assert_eq!(
            select_resource_scopes(Some(challenge), &supported),
            BTreeSet::from(["mcp.write".to_owned()])
        );
        assert_eq!(
            select_resource_scopes(Some(r#"Bearer resource_metadata="/prm""#), &supported),
            BTreeSet::from(["mcp.admin".to_owned(), "mcp.read".to_owned()])
        );
        assert_eq!(select_resource_scopes(None, &[]), BTreeSet::new());
    }

    #[test]
    fn parses_resource_metadata_from_www_authenticate() {
        let header = r#"Bearer error="invalid_request", resource_metadata="/.well-known/oauth-protected-resource/mcp""#;
        assert_eq!(
            resource_metadata_from_header(header).as_deref(),
            Some("/.well-known/oauth-protected-resource/mcp")
        );
    }

    // Host-driven login tests live in tests/mcp_oauth_owner.rs so the
    // runtime flow owner and test use the same auth-core trait identity. These
    // private refresh tests seed a managed credential through the shared commit
    // owner, then exercise the real MCP refresh HTTP and generated lease.
    use crate::{EphemeralTokenStore, InMemoryCoordinator};
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Form, Json, Router};
    use parking_lot::Mutex;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;

    fn test_auth_lease() -> GeneratedAuthLeaseHandle {
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            handle,
        )
        .expect("test auth lease must be certified by generated AuthMachine authority")
    }

    #[derive(Default)]
    struct TestState {
        token_requests: Mutex<Vec<Value>>,
        token_fails: Mutex<bool>,
        pause_refresh: AtomicBool,
        refresh_started: Notify,
        refresh_release: Notify,
    }

    fn ce_refresh_authority(
        store: Arc<EphemeralTokenStore>,
        auth: GeneratedAuthLeaseHandle,
    ) -> McpOAuthAuthority {
        McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store, Arc::new(InMemoryCoordinator::new())),
            Client::new(),
            auth,
        )
    }

    async fn spawn_oauth_fixture() -> (String, Arc<TestState>) {
        let state = Arc::new(TestState::default());
        let app = Router::new()
            .route("/token", post(ce_refresh_token))
            .with_state(Arc::clone(&state));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), state)
    }

    async fn ce_refresh_token(
        State(state): State<Arc<TestState>>,
        Form(body): Form<HashMap<String, String>>,
    ) -> impl IntoResponse {
        state
            .token_requests
            .lock()
            .push(serde_json::to_value(&body).unwrap());
        assert_eq!(
            body.get("grant_type").map(String::as_str),
            Some("refresh_token")
        );
        if state.pause_refresh.load(Ordering::SeqCst) {
            state.refresh_started.notify_one();
            state.refresh_release.notified().await;
        }
        if *state.token_fails.lock() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid_grant" })),
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

    async fn ce_seed_stored_credential(
        authority: &McpOAuthAuthority,
        target: &McpServerIdentity,
        base: &str,
    ) {
        let discovery = StoredMcpOAuthDiscovery {
            resource: target.server_url().to_string(),
            resource_metadata_url: format!("{base}/.well-known/oauth-protected-resource/mcp"),
            authorization_server: base.to_string(),
            authorization_metadata_url: format!("{base}/.well-known/oauth-authorization-server"),
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            registration_endpoint: format!("{base}/register"),
            scopes: vec!["mcp.read".to_string()],
        };
        let client = StoredMcpOAuthClient {
            client_id: "client-123".to_string(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_string(),
            redirect_uri: format!("{base}/mcp/oauth/callback"),
        };
        let result = OAuthTokenResult {
            access_token: "access-token".to_string(),
            refresh_token: Some("refresh-token".to_string()),
            id_token: None,
            expires_in_secs: Some(3600),
            scope: Some("mcp.read".to_string()),
        };
        let tokens = persisted_tokens_from_result_with_binding(
            &result,
            &discovery,
            &client,
            target,
            None,
            Utc::now(),
        )
        .unwrap();
        let stored = crate::save_tokens_and_publish_lifecycle(
            authority.provider_auth_persistence.clone(),
            authority.auth_lease.clone(),
            target.auth_binding_ref().unwrap().into(),
            tokens,
        )
        .await
        .unwrap();
        let key = target.token_key().unwrap();
        let snapshot = authority.auth_lease.snapshot(&target.lease_key().unwrap());
        assert!(snapshot.credential_present);
        assert_eq!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid)
        );
        assert_eq!(
            authority.token_store().load(&key).await.unwrap(),
            Some(stored.clone())
        );
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(&stored, &snapshot, &key),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches
        );
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
        crate::save_tokens_and_publish_lifecycle(
            authority.provider_auth_persistence.clone(),
            authority.auth_lease.clone(),
            target.auth_binding_ref().unwrap().into(),
            tokens,
        )
        .await
        .unwrap()
    }

    struct CeRefreshRelease(Arc<TestState>);
    impl Drop for CeRefreshRelease {
        fn drop(&mut self) {
            self.0.pause_refresh.store(false, Ordering::SeqCst);
            self.0.refresh_release.notify_one();
        }
    }

    #[tokio::test]
    async fn ce_mcp_http_releases_only_lifecycle_guard() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let auth = test_auth_lease();
        let authority = ce_refresh_authority(store.clone(), auth.clone());
        let target = McpServerIdentity::from_server_config("ce-mcp", format!("{base}/mcp"));
        ce_seed_stored_credential(&authority, &target, &base).await;
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        let key = target.token_key().unwrap();
        let lease = target.lease_key().unwrap();
        let original = store.load(&key).await.unwrap();
        state.pause_refresh.store(true, Ordering::SeqCst);
        let release = CeRefreshRelease(state.clone());
        let child = authority.clone();
        let child_target = target.clone();
        let mut task = tokio::spawn(async move { child.stored_bearer_token(&child_target).await });
        let entered =
            tokio::time::timeout(Duration::from_secs(10), state.refresh_started.notified()).await;
        let guard = meerkat_core::try_acquire_auth_login_lifecycle_guard(&lease);
        let available_during_http = guard.is_some();
        drop(guard);
        let during = store.load(&key).await.unwrap();
        drop(release);
        let finished = tokio::time::timeout(Duration::from_secs(10), &mut task).await;
        if finished.is_err() {
            task.abort();
            let _ = task.await;
        }
        assert!(
            entered.is_ok(),
            "must observe the actual MCP refresh endpoint"
        );
        assert_eq!(
            finished.unwrap().unwrap().unwrap().as_deref(),
            Some("access-token")
        );
        assert_eq!(during, original, "HTTP wait does not publish new bytes");
        assert_eq!(
            state
                .token_requests
                .lock()
                .iter()
                .filter(|r| r["grant_type"] == "refresh_token")
                .count(),
            1
        );
        assert!(
            available_during_http,
            "actual MCP HTTP must not retain the lifecycle guard"
        );
    }

    #[tokio::test]
    async fn ce_mcp_stale_success_and_failure_preserve_replacement() {
        let mut observations = Vec::new();
        for invalid_grant in [false, true] {
            let (base, state) = spawn_oauth_fixture().await;
            let store = Arc::new(EphemeralTokenStore::new());
            let auth = test_auth_lease();
            let authority = ce_refresh_authority(store.clone(), auth.clone());
            let target =
                McpServerIdentity::from_server_config("ce-mcp-stale", format!("{base}/mcp"));
            ce_seed_stored_credential(&authority, &target, &base).await;
            republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
                tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
            })
            .await;
            let key = target.token_key().unwrap();
            let lease = target.lease_key().unwrap();
            state.pause_refresh.store(true, Ordering::SeqCst);
            let release = CeRefreshRelease(state.clone());
            let child = authority.clone();
            let child_target = target.clone();
            let mut task =
                tokio::spawn(async move { child.stored_bearer_token(&child_target).await });
            let entered =
                tokio::time::timeout(Duration::from_secs(10), state.refresh_started.notified())
                    .await;
            // Explicit off-protocol fault injection through the real generated
            // owner, to exercise stale checks without fabricating snapshots.
            let mut replacement = store.load(&key).await.unwrap().unwrap();
            replacement.primary_secret = Some("ce-mcp-new-access".into());
            replacement.refresh_token = Some("ce-mcp-new-refresh".into());
            replacement.expires_at = Some(Utc::now() + chrono::Duration::hours(1));
            let transition = auth
                .acquire_lease(
                    &lease,
                    meerkat_core::persisted_token_expires_at_epoch_secs(&replacement),
                )
                .unwrap();
            let replacement = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key,
                &replacement,
                &transition,
            )
            .unwrap();
            store.save(&key, &replacement).await.unwrap();
            auth.begin_refresh(&lease).unwrap();
            let expected = auth.snapshot(&lease);
            *state.token_fails.lock() = invalid_grant;
            drop(release);
            let finished = tokio::time::timeout(Duration::from_secs(10), &mut task).await;
            if finished.is_err() {
                task.abort();
                let _ = task.await;
            }
            assert!(entered.is_ok(), "actual paused endpoint is mandatory");
            let refused = matches!(
                finished.unwrap().unwrap(),
                Err(McpOAuthError::StalePreparation)
            );
            let count = state
                .token_requests
                .lock()
                .iter()
                .filter(|r| r["grant_type"] == "refresh_token")
                .count();
            observations.push((
                refused,
                expected,
                auth.snapshot(&lease),
                replacement,
                store.load(&key).await.unwrap(),
                count,
            ));
        }
        for (refused, expected, actual, replacement, stored, count) in observations {
            assert!(refused, "stale result must not yield a bearer credential");
            assert_eq!(count, 1);
            assert_eq!(
                actual, expected,
                "stale HTTP may not fail the replacement's refresh owner"
            );
            assert_eq!(
                stored,
                Some(replacement),
                "invalid_grant from the old credential cannot clear its replacement"
            );
        }
    }
    #[test]
    fn ce_mcp_stale_error_roundtrip_remains_typed() {
        let target = McpServerIdentity::from_server_config("stale-test", "http://localhost/mcp");
        let public = map_coordinated_refresh_error(&target, RefreshError::StalePreparation);
        assert!(matches!(public, McpOAuthError::StalePreparation));
        assert!(
            !public.is_refusal(),
            "stale internal preparation is not an invalid caller request"
        );
        assert!(matches!(
            refresh_error_from_mcp(public),
            RefreshError::StalePreparation
        ));
    }

    // Make historical marker bytes through the real generated owner. The
    // bounded clock wait only establishes an actual later publication; no
    // serialized marker or snapshot field is edited by this fixture.
    async fn ce_mcp_publish_later_marker(
        authority: &McpOAuthAuthority,
        target: &McpServerIdentity,
        refreshing: bool,
    ) -> PersistedTokens {
        let key = target.token_key().unwrap();
        let lease = target.lease_key().unwrap();
        let old_time = authority
            .auth_lease
            .snapshot(&lease)
            .credential_published_at_millis
            .unwrap();
        let mut tokens = authority.token_store().load(&key).await.unwrap().unwrap();
        tokens.expires_at = Some(if refreshing {
            Utc::now() - chrono::Duration::seconds(1)
        } else {
            Utc::now() + chrono::Duration::hours(1)
        });
        let later = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let transition = authority
                    .auth_lease
                    .acquire_lease(
                        &lease,
                        meerkat_core::persisted_token_expires_at_epoch_secs(&tokens),
                    )
                    .unwrap();
                if transition.credential_published_at_millis().unwrap() > old_time {
                    break transition;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("actual generated later publication");
        let transition = if refreshing {
            authority.auth_lease.begin_refresh(&lease).unwrap();
            let captured = authority
                .auth_lease
                .capture_auth_lifecycle_restore_snapshot(&lease);
            authority
                .auth_lease
                .release_credential_lifecycle(&lease)
                .unwrap();
            meerkat_core::restore_token_lifecycle_snapshot(&authority.auth_lease, &captured)
                .unwrap()
                .unwrap()
        } else {
            later
        };
        let published = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &key,
            &tokens,
            &transition,
        )
        .unwrap();
        authority
            .token_store()
            .save(&key, &published)
            .await
            .unwrap();
        published
    }

    #[tokio::test]
    async fn ce_mcp_newer_legacy_marker_cannot_replace_reauth_or_absent_owner() {
        for absent in [false, true] {
            for through_coordinator in [false, true] {
                let (base, state) = spawn_oauth_fixture().await;
                let store = Arc::new(EphemeralTokenStore::new());
                let mut auth = test_auth_lease();
                let target =
                    McpServerIdentity::from_server_config("ce-legacy-owner", format!("{base}/mcp"));
                let lease = target.lease_key().unwrap();
                let mut authority = ce_refresh_authority(store.clone(), auth.clone());
                ce_seed_stored_credential(&authority, &target, &base).await;
                let previous = auth.capture_auth_lifecycle_restore_snapshot(&lease);
                let key = target.token_key().unwrap();
                let legacy = ce_mcp_publish_later_marker(&authority, &target, true).await;
                if absent {
                    // A cold process uses its actual fresh generated owner;
                    // restoring an empty capture retains the old high-water mark.
                    auth = test_auth_lease();
                    authority = ce_refresh_authority(store.clone(), auth.clone());
                } else {
                    auth.restore_auth_lifecycle_snapshot(&previous).unwrap();
                    auth.mark_reauth_required(&lease).unwrap();
                }
                let expected = auth.snapshot(&lease);
                if absent {
                    assert!(lifecycle_snapshot_is_absent(&expected));
                } else {
                    assert_eq!(
                        expected.phase,
                        Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
                    );
                    assert_eq!(
                        durable_marker::marker_relation_for_tokens_and_snapshot(
                            &legacy, &expected, &key
                        ),
                        durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
                    );
                }
                assert_eq!(
                    meerkat_core::tokens_lifecycle_publication(&legacy)
                        .unwrap()
                        .phase,
                    Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
                );
                let rejected = if through_coordinator {
                    let child = authority.clone();
                    let child_target = target.clone();
                    let child_key = key.clone();
                    let result = tokio::time::timeout(
                        Duration::from_secs(10),
                        authority.refresh_coordinator().with_refresh(
                            key.clone(),
                            Box::new(move || {
                                Box::pin(async move {
                                    child
                                        .refresh_stored_credential_under_coordinator(
                                            &child_target,
                                            &child_key,
                                        )
                                        .await
                                })
                            }),
                        ),
                    )
                    .await
                    .expect("coordinated legacy check must finish");
                    matches!(result, Err(RefreshError::StalePreparation))
                } else {
                    let result = tokio::time::timeout(
                        Duration::from_secs(10),
                        authority.stored_bearer_token(&target),
                    )
                    .await
                    .expect("public legacy load must finish");
                    matches!(result, Err(McpOAuthError::StalePreparation))
                };
                assert!(rejected, "legacy mismatch must preserve the actual owner");
                assert_eq!(auth.snapshot(&lease), expected);
                assert_eq!(store.load(&key).await.unwrap(), Some(legacy));
                assert_eq!(
                    state
                        .token_requests
                        .lock()
                        .iter()
                        .filter(|request| request["grant_type"] == "refresh_token")
                        .count(),
                    0
                );
            }
        }
    }

    #[tokio::test]
    async fn ce_mcp_matching_legacy_marker_normalizes_and_refreshes() {
        for closed_in_memory in [false, true] {
            let (base, state) = spawn_oauth_fixture().await;
            let store = Arc::new(EphemeralTokenStore::new());
            let auth = test_auth_lease();
            let authority = ce_refresh_authority(store.clone(), auth.clone());
            let target =
                McpServerIdentity::from_server_config("ce-legacy-match", format!("{base}/mcp"));
            ce_seed_stored_credential(&authority, &target, &base).await;
            let legacy = ce_mcp_publish_later_marker(&authority, &target, true).await;
            let key = target.token_key().unwrap();
            let lease = target.lease_key().unwrap();
            if closed_in_memory {
                auth.refresh_failed(&lease, meerkat_core::RefreshFailureObservation::transient())
                    .unwrap();
                auth.observe_credential_freshness(
                    &lease,
                    epoch_secs(Utc::now()),
                    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
                )
                .unwrap();
                assert_eq!(
                    auth.snapshot(&lease).phase,
                    Some(meerkat_core::handles::AuthLeasePhase::Expired)
                );
            }
            assert_eq!(
                durable_marker::marker_relation_for_tokens_and_snapshot(
                    &legacy,
                    &auth.snapshot(&lease),
                    &key
                ),
                durable_marker::AuthLeaseDurableMarkerRelation::Matches
            );
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(10),
                    authority.stored_bearer_token(&target)
                )
                .await
                .expect("matching legacy refresh must finish")
                .unwrap()
                .as_deref(),
                Some("access-token")
            );
            assert_eq!(
                state
                    .token_requests
                    .lock()
                    .iter()
                    .filter(|request| request["grant_type"] == "refresh_token")
                    .count(),
                1
            );
            let stored = store.load(&key).await.unwrap().unwrap();
            assert_eq!(
                auth.snapshot(&lease).phase,
                Some(meerkat_core::handles::AuthLeasePhase::Valid)
            );
            assert_eq!(
                meerkat_core::tokens_lifecycle_publication(&stored)
                    .unwrap()
                    .phase,
                Some(meerkat_core::handles::AuthLeasePhase::Valid)
            );
            assert_eq!(
                durable_marker::marker_relation_for_tokens_and_snapshot(
                    &stored,
                    &auth.snapshot(&lease),
                    &key
                ),
                durable_marker::AuthLeaseDurableMarkerRelation::Matches
            );
        }
    }

    #[tokio::test]
    async fn ce_mcp_nonlegacy_newer_and_cold_restore_remain_supported() {
        for absent in [false, true] {
            let (base, state) = spawn_oauth_fixture().await;
            let store = Arc::new(EphemeralTokenStore::new());
            let mut auth = test_auth_lease();
            let target =
                McpServerIdentity::from_server_config("ce-ordinary-restore", format!("{base}/mcp"));
            let lease = target.lease_key().unwrap();
            let mut authority = ce_refresh_authority(store.clone(), auth.clone());
            ce_seed_stored_credential(&authority, &target, &base).await;
            let previous = auth.capture_auth_lifecycle_restore_snapshot(&lease);
            let newer = ce_mcp_publish_later_marker(&authority, &target, false).await;
            let key = target.token_key().unwrap();
            if absent {
                auth = test_auth_lease();
                authority = ce_refresh_authority(store.clone(), auth.clone());
                assert!(lifecycle_snapshot_is_absent(&auth.snapshot(&lease)));
            } else {
                auth.restore_auth_lifecycle_snapshot(&previous).unwrap();
                auth.mark_reauth_required(&lease).unwrap();
                assert_eq!(
                    durable_marker::marker_relation_for_tokens_and_snapshot(
                        &newer,
                        &auth.snapshot(&lease),
                        &key
                    ),
                    durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
                );
            }
            assert_eq!(
                tokio::time::timeout(
                    Duration::from_secs(10),
                    authority.stored_bearer_token(&target)
                )
                .await
                .expect("ordinary durable restore must finish")
                .unwrap()
                .as_deref(),
                Some("access-token")
            );
            assert_eq!(store.load(&key).await.unwrap(), Some(newer.clone()));
            assert_eq!(
                durable_marker::marker_relation_for_tokens_and_snapshot(
                    &newer,
                    &auth.snapshot(&lease),
                    &key
                ),
                durable_marker::AuthLeaseDurableMarkerRelation::Matches
            );
            assert_eq!(
                state
                    .token_requests
                    .lock()
                    .iter()
                    .filter(|request| request["grant_type"] == "refresh_token")
                    .count(),
                0
            );
        }
    }

    #[test]
    fn a_close_reports_its_own_retirement_result_without_inference() {
        assert!(matches!(
            attempt_cleanup(Ok(()), false),
            McpOAuthAttemptCleanup::Retired
        ));
        // Missing is absence, never proof of consumption or retirement.
        assert!(matches!(
            attempt_cleanup(Err(McpOAuthError::Flow(OAuthFlowError::Missing)), true),
            McpOAuthAttemptCleanup::Absent {
                completion_unobserved: true
            }
        ));
        assert!(matches!(
            attempt_cleanup(
                Err(McpOAuthError::Flow(OAuthFlowError::BrowserIdentityMismatch)),
                false
            ),
            McpOAuthAttemptCleanup::RetirementFailed(McpOAuthError::Flow(
                OAuthFlowError::BrowserIdentityMismatch
            ))
        ));
    }
}

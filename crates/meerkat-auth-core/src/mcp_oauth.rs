//! Runtime-discovered OAuth for HTTP MCP servers.
//!
//! The MCP config owns only server intent. This module owns OAuth discovery,
//! dynamic client registration, the PKCE browser flow (through a loopback
//! redirect, or a host-registered `https` callback completed from the host's
//! custody), token persistence, and refresh for MCP resources.

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
    RefreshCoordinator, RefreshError, TokenKey, TokenStore, TokenStoreAccess,
};
use crate::connector_oauth::{
    AccountSelection, AccountVerification, ConnectorAccountObservation, ConnectorOAuthDescriptor,
    ConnectorOAuthParameters, ConnectorOAuthRefusal, McpCredentialBinding, OAuthBrowserActionRef,
};
use crate::http_egress::{
    HttpEgressError, HttpEgressProfile, HttpEgressRefusal, HttpEgressUnavailable,
};
/// What a failed login start left of the attempt it tried to admit;
/// [`McpOAuthStartFailure`] carries it.
pub use crate::oauth_flow::OAuthStartAdmission;
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

/// Default bound of one network stage of an MCP OAuth start, completion or
/// refresh ([`McpOAuthNetworkStage`]): every request of the stage, from its
/// first send to its last body byte. Shorter than
/// [`crate::auth_oauth::CREDENTIAL_REQUEST_TIMEOUT`], so a stall ends as this
/// owner's typed stage outcome.
pub const MCP_OAUTH_STAGE_BOUND: Duration = Duration::from_secs(20);

/// Default bound of a start's network stages together (resource and
/// authorization-server discovery, account preflight, client registration).
pub const MCP_OAUTH_START_BOUND: Duration = Duration::from_secs(60);

/// Default bound of a completion's network stages together (issuer metadata,
/// token exchange, account evidence). It never extends past the attempt's own
/// validity: a result after that would be refused anyway.
pub const MCP_OAUTH_COMPLETION_BOUND: Duration = Duration::from_secs(60);

/// Default time a completion observes its credential commit before the flow
/// owner decides which side of the attempt's consume the commit is on. Longer
/// than a refresh holds the credential (two stages), so a login queued behind
/// a stalled refresh lands once that refresh has failed.
pub const MCP_OAUTH_COMMIT_OBSERVATION_BOUND: Duration = Duration::from_secs(60);

/// One network stage of an MCP OAuth start or completion, as named by
/// [`McpOAuthError::NetworkTimedOut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum McpOAuthNetworkStage {
    /// The protected resource's metadata: its well-known probes and the
    /// metadata document. A start stage.
    ResourceMetadata,
    /// The authorization server's metadata (RFC 8414 or OpenID Connect
    /// discovery). A start stage, and the first completion stage: the code
    /// has not been sent.
    AuthorizationServerMetadata,
    /// The account strategy's preflight of the issuer. A start stage.
    AccountPreflight,
    /// Dynamic client registration. A start stage.
    ClientRegistration,
    /// The authorization-code exchange: the code was sent and no answer
    /// arrived, so it may be spent at the provider. A completion stage.
    TokenExchange,
    /// The account strategy's evidence for the exchanged token: the code was
    /// redeemed and the tokens were discarded unpublished. A completion stage.
    AccountEvidence,
}

impl McpOAuthNetworkStage {
    /// Whether the authorization code had been sent when this stage ended.
    /// Either way nothing was published and the code is never replayed.
    pub fn code_sent(self) -> bool {
        matches!(self, Self::TokenExchange | Self::AccountEvidence)
    }
}

impl std::fmt::Display for McpOAuthNetworkStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ResourceMetadata => "protected resource metadata discovery",
            Self::AuthorizationServerMetadata => "authorization server metadata discovery",
            Self::AccountPreflight => "account strategy preflight",
            Self::ClientRegistration => "dynamic client registration",
            Self::TokenExchange => "authorization-code exchange",
            Self::AccountEvidence => "account evidence",
        })
    }
}

/// The deadline `bound` from now. A bound too large to add (a host passing
/// `Duration::MAX` to mean "no bound") saturates to a deadline about 30 years
/// out instead of panicking, as `tokio::time::timeout` does.
fn deadline_after(bound: Duration) -> tokio::time::Instant {
    const FAR_FUTURE: Duration = Duration::from_secs(86_400 * 365 * 30);
    let now = tokio::time::Instant::now();
    now.checked_add(bound).unwrap_or_else(|| now + FAR_FUTURE)
}

/// How long the MCP OAuth owner waits on the network.
///
/// Each network stage ([`McpOAuthNetworkStage`], and a refresh's token
/// request and subject re-observation) is bounded by `stage`; one bound covers
/// all of a stage's sequential requests (the up to three well-known probes and
/// the resource metadata document; the two or three authorization-server
/// candidates), so a slow provider spends one bound across them. A start's
/// stages together are bounded by `start`, and a completion's by `completion`,
/// never past the attempt's own validity. A completion observes its credential
/// commit for `commit_observation`, then the flow owner decides: an attempt it
/// can still retire publishes nothing, one it cannot is
/// [`McpOAuthError::CommitUnsettled`]. The defaults are
/// [`MCP_OAUTH_STAGE_BOUND`], [`MCP_OAUTH_START_BOUND`],
/// [`MCP_OAUTH_COMPLETION_BOUND`] and [`MCP_OAUTH_COMMIT_OBSERVATION_BOUND`].
/// Any bound is accepted: one too large to add to the current time, such as
/// `Duration::MAX`, saturates and never panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpOAuthNetworkBounds {
    stage: Duration,
    start: Duration,
    completion: Duration,
    commit_observation: Duration,
}

impl Default for McpOAuthNetworkBounds {
    fn default() -> Self {
        Self {
            stage: MCP_OAUTH_STAGE_BOUND,
            start: MCP_OAUTH_START_BOUND,
            completion: MCP_OAUTH_COMPLETION_BOUND,
            commit_observation: MCP_OAUTH_COMMIT_OBSERVATION_BOUND,
        }
    }
}

impl McpOAuthNetworkBounds {
    /// The bound of one network stage.
    #[must_use]
    pub fn with_stage(mut self, bound: Duration) -> Self {
        self.stage = bound;
        self
    }

    /// The bound of a start's network stages together.
    #[must_use]
    pub fn with_start(mut self, bound: Duration) -> Self {
        self.start = bound;
        self
    }

    /// The bound of a completion's network stages together.
    #[must_use]
    pub fn with_completion(mut self, bound: Duration) -> Self {
        self.completion = bound;
        self
    }

    /// How long a completion observes its credential commit.
    #[must_use]
    pub fn with_commit_observation(mut self, bound: Duration) -> Self {
        self.commit_observation = bound;
        self
    }

    /// The bound of one network stage.
    pub fn stage(&self) -> Duration {
        self.stage
    }

    /// The bound of a start's network stages together.
    pub fn start(&self) -> Duration {
        self.start
    }

    /// The bound of a completion's network stages together.
    pub fn completion(&self) -> Duration {
        self.completion
    }

    /// How long a completion observes its credential commit.
    pub fn commit_observation(&self) -> Duration {
        self.commit_observation
    }
}

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
/// to `binding_slug` so the token realm key and the
/// `AuthMachine` lease key are guaranteed structurally identical for the same
/// server.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct McpServerIdentity {
    server_name: String,
    server_url: String,
    selection: McpAccountSelection,
    /// The Meerkat realm whose credential this is. `None` only for an identity
    /// built by the realm-less constructors, which name the shared pre-realm
    /// slot; native hosts bind every target to their runtime realm.
    realm: Option<RealmId>,
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
            realm: None,
        }
    }

    /// The same target in `realm`: its own credential slot, lifecycle lease
    /// and login attempts, never shared with another realm or with the
    /// realm-less slot.
    pub fn in_realm(mut self, realm: RealmId) -> Self {
        self.realm = Some(realm);
        self
    }

    /// [`Self::from_config`] in `realm`.
    pub fn from_config_in_realm(
        realm: RealmId,
        config: &meerkat_core::McpServerConfig,
    ) -> Result<Self, McpOAuthError> {
        Ok(Self::from_config(config)?.in_realm(realm))
    }

    /// The realm of the credential, when the target names one.
    pub fn realm(&self) -> Option<&RealmId> {
        self.realm.as_ref()
    }

    /// The target as a host serving `host_realm` addresses it: bound to that
    /// realm when it names none, refused when it names another
    /// ([`McpOAuthError::RealmMismatch`]), unchanged for a realm-less host.
    pub fn scoped_to(&self, host_realm: Option<&RealmId>) -> Result<Self, McpOAuthError> {
        match (host_realm, &self.realm) {
            (None, _) => Ok(self.clone()),
            (Some(host), None) => Ok(self.clone().in_realm(host.clone())),
            (Some(host), Some(own)) if host == own => Ok(self.clone()),
            (Some(_), Some(_)) => Err(McpOAuthError::RealmMismatch {
                server_name: self.server_name.clone(),
            }),
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
        // A realm-scoped target has its own slot per realm. The realm-less
        // slot keeps its pre-realm digest and is never read for a realm.
        if let Some(realm) = &self.realm {
            hasher.update(b"\0mcp-realm-v1\0");
            hasher.update(realm.as_str().as_bytes());
        }
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
            .field("realm", &self.realm.as_ref().map(RealmId::as_str))
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

    /// The host egress profile this strategy's own HTTP traffic enforces, if
    /// any. A credential owner under a required profile is bound to it only
    /// when its strategy enforces that same profile. Default: none.
    fn http_egress(&self) -> Option<HttpEgressProfile> {
        None
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
    /// The host egress profile every request of this strategy is admitted
    /// by; a required profile also supplies the client.
    egress: HttpEgressProfile,
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
            egress: HttpEgressProfile::trusted_host(),
        }
    }

    pub fn with_http(http: Client) -> Self {
        Self {
            http: Ok(http),
            required_scopes: std::collections::BTreeSet::new(),
            egress: HttpEgressProfile::trusted_host(),
        }
    }

    /// Enforce the host's egress `profile` for every request of this
    /// strategy. A required profile admits each URL and supplies the
    /// redirect-free client; a refusal is a local verification refusal.
    #[must_use]
    pub fn with_http_egress(mut self, profile: &HttpEgressProfile) -> Self {
        self.egress = profile.clone();
        self
    }

    /// The client for one request to `url`, after the egress profile admits
    /// it.
    fn http_for(&self, url: &str) -> Result<&Client, ConnectorOAuthRefusal> {
        self.egress
            .admit(url)
            .map_err(verification_egress_refusal)?;
        match self.egress.credential_client() {
            Some(client) => client.map_err(|_| ConnectorOAuthRefusal::VerificationUnavailable),
            None => self
                .http
                .as_ref()
                .map_err(|_| ConnectorOAuthRefusal::VerificationUnavailable),
        }
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
            client_auth_profile: None,
            protocol: None,
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

    fn http_egress(&self) -> Option<HttpEgressProfile> {
        self.egress.is_required().then(|| self.egress.clone())
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
            .http_for(&configuration_url)?
            .get(&configuration_url)
            .send()
            .await
            .map_err(verification_send_error)?
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
            .http_for(userinfo_url.as_str())?
            .get(userinfo_url)
            .bearer_auth(&tokens.access_token)
            .send()
            .await
            .map_err(verification_send_error)?
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

/// A verification request the host egress profile did not admit: a typed
/// local refusal, never evidence about the account.
fn verification_egress_refusal(error: HttpEgressError) -> ConnectorOAuthRefusal {
    match error {
        HttpEgressError::Refused(refusal) => ConnectorOAuthRefusal::EgressRefused(refusal),
        HttpEgressError::Unavailable(_) => ConnectorOAuthRefusal::VerificationUnavailable,
    }
}

/// A failed verification request: the egress profile's own resolver
/// refusal stays typed; anything else is unavailable evidence.
fn verification_send_error(error: reqwest::Error) -> ConnectorOAuthRefusal {
    crate::http_egress::egress_refusal_in(&error).map_or(
        ConnectorOAuthRefusal::VerificationUnavailable,
        ConnectorOAuthRefusal::EgressRefused,
    )
}

/// A failed OAuth request of the credential owner during `stage`: the egress
/// profile's own resolver refusal is the typed egress outcome, a request that
/// passed its client's bound is the stage's typed timeout, and anything else
/// is `otherwise`.
fn oauth_send_error(
    target: &McpServerIdentity,
    stage: McpOAuthNetworkStage,
    error: reqwest::Error,
    otherwise: impl FnOnce(reqwest::Error) -> McpOAuthError,
) -> McpOAuthError {
    match crate::http_egress::egress_refusal_in(&error) {
        Some(refusal) => McpOAuthError::from_egress(target, HttpEgressError::Refused(refusal)),
        None if error.is_timeout() => McpOAuthError::NetworkTimedOut {
            server_name: target.server_name().to_owned(),
            stage,
        },
        None => otherwise(error),
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

/// Open `url` in the system browser without logging it, naming no host
/// judgement: the opener runs with the host's environment as it is, as for a
/// host that confines no launch kind.
///
/// Deprecated: a host that confines its agents' launches must not open a
/// browser this way, since a confined process could have planted the opener
/// on the host's `PATH`. Open with [`open_system_browser_guarded`] and the
/// host's launch guard (`HostConfinement::launch_guard`, `None` for a host
/// that confines nothing), or with [`open_system_browser_trusting`].
///
/// ```compile_fail
/// #![deny(deprecated)]
/// let _ = meerkat_auth_core::mcp_oauth::open_system_browser("https://example.invalid");
/// ```
#[deprecated(
    since = "0.9.3",
    note = "names no host judgement, so a host that confines its launches could run an opener a confined process planted on PATH; use `open_system_browser_guarded` with the host's launch guard (`HostConfinement::launch_guard`), or `open_system_browser_trusting`"
)]
pub fn open_system_browser(url: &str) -> std::io::Result<()> {
    open_system_browser_guarded(url, None)
}

/// Open `url` in the system browser without logging it, under the host's
/// launch guard.
///
/// The URL is passed as one process argument (no shell) to the platform
/// opener (`/usr/bin/open`, `xdg-open`, or the system `rundll32.exe
/// url.dll,FileProtocolHandler`), whose output is discarded
/// ([`meerkat_core::HostLaunchExemption::browser_opener`]). Nothing here
/// logs the command line, so the authorize URL and state never reach `log`
/// or `tracing` records. `launch_guard` is the host's judgement of the paths
/// its confined processes could change (`HostConfinement::launch_guard`):
/// with one, the opener and every program it starts by name come from a
/// `PATH` without relative directories or the directories it names
/// ([`meerkat_core::confinement::trusted_search_path`]), so none is one a
/// confined process planted; `None`, for a host that confines no launch kind,
/// runs the opener with the host's environment as it is.
pub fn open_system_browser_guarded(
    url: &str,
    launch_guard: Option<&meerkat_core::confinement::UntrustedPathFn>,
) -> std::io::Result<()> {
    // A host launch exemption: the opener must reach the user's desktop
    // session, which a confined process cannot.
    run_browser_opener(meerkat_core::HostLaunchExemption::browser_opener(
        url,
        launch_guard,
    ))
}

/// [`open_system_browser_guarded`] for a host that confines its agents'
/// launches, with its judgement `untrusted`.
pub fn open_system_browser_trusting(
    url: &str,
    untrusted: &meerkat_core::confinement::UntrustedPathFn,
) -> std::io::Result<()> {
    open_system_browser_guarded(url, Some(untrusted))
}

/// Run a browser opener without logging its URL, discarding its output.
fn run_browser_opener(mut command: std::process::Command) -> std::io::Result<()> {
    use std::process::Stdio;
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
    /// This login's wake-up registration: a confirmed retirement of its
    /// attempt (cancel by state or reference, logout, a cancel handle) ends
    /// the wait.
    wake: AttemptWakeRegistration,
    /// A wait observed the flow owner's confirmed retirement of the attempt.
    confirmed_retired: bool,
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
#[non_exhaustive]
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
#[non_exhaustive]
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

/// Retires one pending loopback login's attempt from another task. Holds no
/// code; the state stays redacted in `Debug`.
#[derive(Clone)]
pub struct McpOAuthCancelHandle {
    authority: McpOAuthAuthority,
    start: McpOAuthLoginStart,
}

impl McpOAuthCancelHandle {
    /// Retire the attempt through its flow owner. A confirmed retirement
    /// wakes the login's wait; an absent attempt (completed, retired or
    /// expired) is `Flow(Missing)` and wakes nothing.
    pub fn cancel(&self) -> Result<(), McpOAuthError> {
        self.authority.login_cancel(&self.start.target, &self.start)
    }
}

impl std::fmt::Debug for McpOAuthCancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthCancelHandle")
            .field("server_name", &self.start.target.server_name())
            .finish_non_exhaustive()
    }
}

impl McpOAuthPendingLogin {
    /// Host-only browser navigation for this attempt (see
    /// [`McpOAuthLoginStart`]).
    pub fn start(&self) -> &McpOAuthLoginStart {
        &self.start
    }

    /// A handle another task can use to retire this login's attempt. Its
    /// retirement goes through the flow owner like any cancel, and the
    /// confirmed retirement wakes this login's wait.
    pub fn cancel_handle(&self) -> McpOAuthCancelHandle {
        McpOAuthCancelHandle {
            authority: self.authority.clone(),
            start: self.start.clone(),
        }
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

    /// [`launch_browser`](Self::launch_browser) with the opener of a host
    /// that names no judgement ([`open_system_browser`]).
    ///
    /// Deprecated: pass [`open_system_browser_guarded`] with the host's
    /// launch guard to `launch_browser` instead, as a host that confines its
    /// agents' launches must.
    ///
    /// ```compile_fail
    /// #![deny(deprecated)]
    /// async fn open(pending: &meerkat_auth_core::McpOAuthPendingLogin) {
    ///     let _ = pending.launch_system_browser().await;
    /// }
    /// ```
    #[deprecated(
        since = "0.9.3",
        note = "names no host judgement; pass `open_system_browser_guarded` with the host's launch guard (`HostConfinement::launch_guard`) to `launch_browser`"
    )]
    pub async fn launch_system_browser(&self) -> McpOAuthBrowserLaunch {
        self.launch_browser(|url| open_system_browser_guarded(&url, None))
            .await
    }

    /// Wait for the loopback callback, bounded by both `timeout` and the
    /// admitted attempt's own expiry, then complete the login. Every failure
    /// retires the attempt; so does dropping this future before it finishes.
    ///
    /// The wait keeps the listener's legacy tie: like `tokio::time::timeout`,
    /// it polls the callback and drain once before the window is consulted.
    ///
    /// A confirmed retirement of the attempt (cancel by state or reference,
    /// logout, a [`cancel_handle`](Self::cancel_handle)) ends the wait, or the
    /// completion it entered, with [`McpOAuthError::AttemptRetired`], carrying
    /// this login's [`close`](Self::close) receipt: the listener is joined
    /// first. The completion after delivery is bounded by the authority's
    /// [`McpOAuthNetworkBounds`].
    pub async fn complete(self, timeout: Duration) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        let mut pending = self;
        let server_name = pending.start.target.server_name().to_owned();
        let window = timeout.min(pending.start.remaining());
        let deadline = tokio::time::Instant::now() + window;
        let step = {
            let McpOAuthPendingLogin { callback, wake, .. } = &mut pending;
            let Some(callback) = callback.as_mut() else {
                return Err(McpOAuthError::HumanAuthorizationRequired { server_name });
            };
            tokio::select! {
                biased;
                () = wake.retired() => None,
                outcome = callback.wait_poll_first(deadline) => Some(outcome),
            }
        };
        let outcome = match step {
            None => {
                pending.confirmed_retired = true;
                let cleanup = pending.close().await;
                return Err(McpOAuthError::AttemptRetired {
                    server_name,
                    cleanup: Some(Box::new(cleanup)),
                });
            }
            // A failed wait leaves the attempt admitted: dropping `pending`
            // retires it.
            Some(outcome) => outcome.map_err(|error| callback_wait_error(server_name, error))?,
        };
        if let Some(callback) = pending.callback.take() {
            // The delivered wait already joined the drain.
            pending.listener = Some(match callback.close().await {
                Ok(closed) => McpOAuthListenerCleanup::Joined(closed),
                Err(error) => McpOAuthListenerCleanup::Failed(error),
            });
        }
        match pending.run_completion(outcome).await {
            // The entered completion observed a confirmed retirement: the
            // consuming wait carries its receipt, as for a retired wait.
            Err(McpOAuthError::AttemptRetired {
                server_name,
                cleanup: None,
            }) => {
                let cleanup = pending.close().await;
                Err(McpOAuthError::AttemptRetired {
                    server_name,
                    cleanup: Some(Box::new(cleanup)),
                })
            }
            completed => completed,
        }
    }

    /// Borrowed completion. The callback wait is bounded by `deadline` and
    /// the admitted attempt's expiry, whichever is earlier: a deadline already
    /// passed wins over a queued callback. The wait is safe to abandon: the
    /// listener and the attempt stay with this login for an explicit
    /// [`close`](Self::close).
    ///
    /// The deadline bounds only the callback wait and never extends the
    /// attempt's validity. The completion after delivery (issuer discovery,
    /// token exchange, account observation, credential commit) is bounded by
    /// the authority's [`McpOAuthNetworkBounds`], never past the attempt's
    /// validity, and a confirmed retirement of the attempt ends it at once
    /// with [`McpOAuthError::AttemptRetired`]. A commit not observed within
    /// its bound ends as [`McpOAuthError::CommitUnsettled`] when the flow owner
    /// can no longer retire the attempt; its owner finishes on its own.
    /// Dropping this future while the token exchange is outstanding prevents
    /// local publication by this completion; once the credential commit was
    /// handed to its owner, that owner may still finish after the drop. The
    /// authorization code is never replayed.
    pub async fn complete_until(
        &mut self,
        deadline: tokio::time::Instant,
    ) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        let server_name = self.start.target.server_name().to_owned();
        let expires = tokio::time::Instant::from_std(self.start.expires_at);
        let step = {
            let McpOAuthPendingLogin { callback, wake, .. } = &mut *self;
            let Some(callback) = callback.as_mut() else {
                return Err(McpOAuthError::HumanAuthorizationRequired { server_name });
            };
            tokio::select! {
                biased;
                () = wake.retired() => None,
                outcome = callback.wait_until(deadline.min(expires)) => Some(outcome),
            }
        };
        let Some(outcome) = step else {
            // The flow owner confirmed the retirement. The listener stays with
            // this login for its joined `close`.
            self.confirmed_retired = true;
            return Err(McpOAuthError::AttemptRetired {
                server_name,
                cleanup: None,
            });
        };
        let outcome = outcome.map_err(|error| callback_wait_error(server_name, error))?;
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
                CompletionCustody::Loopback,
                Some(self.guard_retirement.clone()),
            )
            .await;
        self.completion = match &completed {
            Ok(_) => PendingCompletion::Consumed,
            // The commit's owner may still publish: its result is unobserved.
            Err(McpOAuthError::CommitUnsettled { .. }) => PendingCompletion::Unobserved,
            Err(McpOAuthError::AttemptRetired { .. }) => {
                // The flow owner confirmed the retirement the completion saw.
                self.confirmed_retired = true;
                PendingCompletion::Failed
            }
            Err(_) => PendingCompletion::Failed,
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
            // The flow owner confirmed a retirement of this attempt, whether
            // or not a wait observed it.
            _ if pending.confirmed_retired || *pending.wake.retired.borrow() => {
                McpOAuthAttemptCleanup::Retired
            }
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
        if self.completion != PendingCompletion::Consumed
            && !self.confirmed_retired
            && !*self.wake.retired.borrow()
        {
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
    /// For an attempt this call admitted on the target's registered `https`
    /// redirect ([`McpOAuthAuthority::login_start_registered`]): the
    /// host-only proof that this host started it. The host keeps it
    /// server-side with its ceremony and passes it back in
    /// [`McpOAuthRegisteredCallback::host_attempt_handle`]; it never goes to
    /// the browser, a model or a log. `None` for loopback attempts and for a
    /// joined projection, which carries no custody.
    pub host_attempt_handle: Option<String>,
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

    /// The attempt's non-secret reference, as [`McpOAuthPendingAttempt`]
    /// reports it: status reads and cancel by reference take it. It is a
    /// digest of the state and authorizes nothing.
    pub fn attempt_ref(&self) -> OAuthBrowserActionRef {
        OAuthBrowserActionRef::project(&self.state)
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

/// A login start that failed: the error the start returns, and what this
/// call left of the attempt it tried to admit
/// ([`McpOAuthAuthority::login_start_with_admission`]).
///
/// The admission speaks of this call only. [`OAuthStartAdmission::NotAdmitted`]
/// says this call admitted nothing; another attempt for the target can still
/// be pending, such as one another start admitted or the one this start
/// would have joined. A pending read ([`McpOAuthAuthority::pending_attempt`])
/// or a logout answers for those.
/// [`OAuthStartAdmission::RetirementUnconfirmed`] names the attempt for
/// [`McpOAuthAuthority::cancel_attempt_by_ref`].
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct McpOAuthStartFailure {
    pub error: McpOAuthError,
    pub admission: OAuthStartAdmission,
}

impl McpOAuthStartFailure {
    fn not_admitted(error: McpOAuthError) -> Self {
        Self {
            error,
            admission: OAuthStartAdmission::NotAdmitted,
        }
    }
}

/// What a start prepared before the flow owner's start: the projection of
/// an attempt it joined, or an attempt to admit.
enum PreparedAttempt<'a> {
    Joined(McpOAuthLoginStart),
    Ready(ReadyAttempt<'a>),
}

/// A start whose every step before the flow owner's start passed. It holds
/// the target's start-or-join lock until the start returns.
struct ReadyAttempt<'a> {
    authority: &'a dyn OAuthFlowAuthority,
    _admission: tokio::sync::OwnedMutexGuard<()>,
    target: McpServerIdentity,
    binding: AuthBindingRef,
    credential_identity: meerkat_core::AuthCredentialIdentity,
    discovery: StoredMcpOAuthDiscovery,
    client: StoredMcpOAuthClient,
    offline_access: bool,
    identity: OAuthBrowserFlowIdentity,
}

impl std::fmt::Debug for McpOAuthLoginStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthLoginStart")
            .field("target", &self.target)
            .field("authorize_url", &"<redacted>")
            .field("state", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field("disposition", &self.disposition)
            .field(
                "host_attempt_handle",
                &self.host_attempt_handle.as_ref().map(|_| "<redacted>"),
            )
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

/// The base of the `https` redirect URIs a host registered for MCP OAuth
/// logins: its own callback endpoint, for a host whose users' browsers cannot
/// reach its loopback address. It is host configuration only (an embedding
/// host's own setting, `rkat-rpc --mcp-oauth-redirect-base`), never a
/// request, an agent tool, an MCP server definition or a mob profile.
///
/// Each target's redirect is derived from it: the base followed by one path
/// segment, the target's credential binding slug
/// ([`McpOAuthAuthority::registered_redirect_uri`]). So every server, realm
/// and account selection has its own exact redirect, and a callback compares
/// byte for byte: no normalization, prefix or pattern matching.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct McpRegisteredRedirectBase(String);

impl McpRegisteredRedirectBase {
    /// Admit `base`: an absolute `https` URL with a host and no userinfo,
    /// query or fragment, already in its canonical form. Anything else is
    /// refused ([`ConnectorOAuthRefusal::InvalidDescriptor`]).
    pub fn parse(base: &str) -> Result<Self, McpOAuthError> {
        if valid_registered_redirect(base) {
            Ok(Self(base.to_owned()))
        } else {
            Err(McpOAuthError::Verification(
                ConnectorOAuthRefusal::InvalidDescriptor,
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The redirect for one target's binding slug: one more path segment.
    fn redirect_for(&self, binding_slug: &str) -> String {
        if self.0.ends_with('/') {
            format!("{}{binding_slug}", self.0)
        } else {
            format!("{}/{binding_slug}", self.0)
        }
    }
}

impl std::fmt::Debug for McpRegisteredRedirectBase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("McpRegisteredRedirectBase")
            .field(&self.0)
            .finish()
    }
}

/// The host's callback for an attempt admitted on a registered `https`
/// redirect ([`McpOAuthAuthority::login_complete_registered`]).
///
/// `redirect_uri` is the URI of the host route that received the callback,
/// from the host's own route configuration (never from the request's `Host`
/// or forwarded headers): it must equal the admitted attempt's redirect.
/// `state`, `code` and `iss` (RFC 9207, when the authorization server sent
/// it) come from the callback query. `host_attempt_handle` is the handle the
/// start returned, from the host's retained ceremony: it proves the host
/// started this attempt and never stands in for the state, the PKCE
/// verifier or the redirect check. Everything else comes from the admitted
/// attempt. `Debug` redacts the secrets.
#[derive(Clone, PartialEq, Eq)]
pub struct McpOAuthRegisteredCallback {
    pub redirect_uri: String,
    pub state: String,
    pub code: String,
    pub iss: Option<String>,
    pub host_attempt_handle: String,
}

impl std::fmt::Debug for McpOAuthRegisteredCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthRegisteredCallback")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"<redacted>")
            .field("code", &"<redacted>")
            .field("iss", &self.iss)
            .field("host_attempt_handle", &"<redacted>")
            .finish()
    }
}

/// Where an attempt's authorization response is delivered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RedirectMode {
    /// An http loopback redirect (RFC 8252): a native client.
    Loopback,
    /// The target's registered `https` redirect: the host's own callback,
    /// completed only from the host's retained custody.
    Registered,
}

/// What a completion proves about the attempt it completes.
#[derive(Clone, Copy)]
enum CompletionCustody<'a> {
    /// A loopback completion. A registered attempt never completes this way.
    Loopback,
    /// A registered completion: the host's retained handle, and the
    /// authorization response's issuer when the callback carried one.
    Registered {
        host_attempt_handle: &'a str,
        iss: Option<&'a str>,
    },
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
    /// The completion's wake-up: a confirmed retirement of the attempt (by
    /// state or reference, a cancel handle, a logout) ends it at once.
    wake: AttemptWakeRegistration,
}

/// The detail of a commit this completion stopped observing before it
/// consumed the attempt: nothing was published.
const COMMIT_NOT_ADMITTED: &str = "the credential commit was not admitted in time: earlier credential work for this key is still running";

impl AttemptRetireGuard {
    /// `work`, unless the flow owner confirms this attempt's retirement
    /// first: then [`McpOAuthError::AttemptRetired`] at once, and `work`, an
    /// in-flight request included, is dropped. Safe at any point before the
    /// commit consumes the attempt: the flow owner never consumes a retired
    /// attempt, so nothing can be published from it.
    async fn unless_retired<T>(
        &mut self,
        target: &McpServerIdentity,
        work: impl std::future::Future<Output = Result<T, McpOAuthError>>,
    ) -> Result<T, McpOAuthError> {
        tokio::select! {
            biased;
            () = self.wake.retired() => {
                // Already retired by the flow owner: nothing is left to retire.
                self.armed = false;
                Err(McpOAuthError::AttemptRetired {
                    server_name: target.server_name().to_owned(),
                    cleanup: None,
                })
            }
            result = work => result,
        }
    }

    /// The end of a commit this completion stopped observing (its bound
    /// passed, or its owner was lost before it reported). The flow owner
    /// linearizes the commit's consume against this retirement: an attempt it
    /// still retires was never consumed and now cannot be, so nothing is
    /// published; one it cannot retire was consumed, or may have been, so the
    /// outcome is [`McpOAuthError::CommitUnsettled`] until the commit settles.
    fn settle_unobserved_commit(&mut self, target: &McpServerIdentity) -> McpOAuthError {
        self.armed = false;
        let retired = retire_attempt(
            self.authority.as_ref(),
            &self.state,
            &self.target,
            self.identity.clone(),
            &self.redirect_uri,
        );
        match retired {
            Ok(()) => {
                if let Some(observed) = &self.observed {
                    *observed.lock() = Some(Ok(()));
                }
                McpOAuthError::CredentialAccess {
                    refusal: meerkat_core::auth::TokenStoreRefusal::Pending,
                    detail: COMMIT_NOT_ADMITTED.into(),
                }
            }
            Err(_) => McpOAuthError::CommitUnsettled {
                server_name: target.server_name().to_owned(),
            },
        }
    }
}

impl Drop for AttemptRetireGuard {
    fn drop(&mut self) {
        if self.armed {
            let retired = retire_attempt(
                self.authority.as_ref(),
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
    /// The server says its authorization responses carry `iss` (RFC 9207).
    iss_parameter_supported: bool,
}

/// What the use path would do with an MCP target's stored credential, as a
/// status read reports it ([`McpOAuthAuthority::stored_credential_status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpStoredCredentialPhase {
    /// No usable credential is stored: a login is needed.
    AuthorizationRequired,
    /// The use path accepts the stored credential, or refreshes it first.
    Authorized,
    /// The use path refuses the stored credential until a new login.
    ReauthRequired,
}

/// One status read of an MCP target's stored credential: the phase the use
/// path decides, with the secret-free fields of the exact credential that
/// read saw.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct McpStoredCredentialStatus {
    pub phase: McpStoredCredentialPhase,
    pub expires_at: Option<DateTime<Utc>>,
    pub account_id: Option<String>,
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
    /// The target names another realm than the authority serves. Realms
    /// never share or reach each other's MCP credentials.
    #[error("MCP server '{server_name}' belongs to another realm than this host serves")]
    RealmMismatch { server_name: String },
    #[error(
        "MCP OAuth interactive login requires an account selection: oauth_account (a provider subject ID) or oauth_account_selection (discover or unverified)"
    )]
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
    /// An attempt admitted on a registered `https` redirect was completed
    /// without the host's retained attempt handle, with another one, or
    /// through the loopback completion; or a registered completion named a
    /// loopback attempt. Refused before anything else: the attempt is left
    /// to its owner, so such a callback cannot cancel it.
    #[error("MCP OAuth callback for '{server_name}' requires the host's retained attempt")]
    RemoteCustodyRequired { server_name: String },
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
    /// The credential store refused access (denied, interaction required,
    /// pending or unavailable): never a refresh failure or absence.
    #[error("MCP OAuth credential access refused ({refusal}): {detail}")]
    CredentialAccess {
        refusal: meerkat_core::auth::TokenStoreRefusal,
        detail: String,
    },
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
        "MCP server '{server_name}' already has a credential this login cannot replace; disconnect it first: log it out, then log in again"
    )]
    DisconnectRequired { server_name: String },
    /// The credential slot refused the commit inside its exclusive
    /// mutation (for example a racing occupant of a Discover slot).
    /// The redirect-free credential HTTP client could not be built, so no
    /// OAuth request is sent.
    #[error(transparent)]
    HttpClientUnavailable(CredentialHttpClientUnavailable),
    /// The flow owner confirmed the retirement of the attempt a loopback wait
    /// was serving (cancel by state or reference, logout, a cancel handle).
    /// A consuming wait carries its joined close receipt; a borrowed wait
    /// leaves the listener with the login for its own close.
    #[error("MCP OAuth login for '{server_name}' was cancelled")]
    AttemptRetired {
        server_name: String,
        cleanup: Option<Box<McpOAuthPendingClose>>,
    },
    /// A loopback login admitted its attempt, but reading it back from the
    /// flow owner failed. The login's own close then ran: `cleanup` reports
    /// the attempt retirement and the listener join beside the read fault.
    #[error("MCP OAuth login for '{server_name}' could not confirm its admitted attempt: {read}")]
    AdmissionReadFailed {
        server_name: String,
        read: Box<McpOAuthError>,
        cleanup: Box<McpOAuthPendingClose>,
    },
    #[error("MCP OAuth credential slot for '{server_name}' refused the login: {refusal}")]
    CredentialSlot {
        server_name: String,
        refusal: crate::auth_store::CredentialSlotRefusal,
    },
    /// The host's required egress profile refused an OAuth endpoint (or the
    /// MCP target itself) before anything was sent to it.
    #[error("MCP OAuth for '{server_name}' was refused by the host egress profile: {refusal}")]
    EgressRefused {
        server_name: String,
        refusal: HttpEgressRefusal,
    },
    /// The host's required egress profile cannot be enforced for this
    /// credential owner (an invalid policy, an unbuildable client, or an
    /// account strategy not bound to the same profile). Nothing was sent.
    #[error("MCP OAuth for '{server_name}' cannot enforce the host egress profile: {reason}")]
    EgressUnavailable {
        server_name: String,
        reason: HttpEgressUnavailable,
    },
    /// A network stage of a start or completion got no answer within its
    /// bound ([`McpOAuthNetworkBounds`]). Nothing was published: a start
    /// admitted no attempt, and a completion's attempt was retired. Retry with
    /// a new login; the authorization code is never replayed.
    /// [`McpOAuthNetworkStage::code_sent`] tells whether the code had been
    /// sent (and may be spent at the provider).
    #[error("MCP OAuth {stage} for '{server_name}' got no answer within its bound")]
    NetworkTimedOut {
        server_name: String,
        stage: McpOAuthNetworkStage,
    },
    /// The credential commit consumed the attempt, or may have, and its
    /// outcome is unknown: it did not report within its observation bound,
    /// its owner was lost, or it failed after the consume and could not
    /// restore the previous credential and lifecycle. A commit still running
    /// keeps running to its end; once the in-flight credential mutations
    /// settle ([`crate::auth_store::settle_in_flight_credential_mutations`],
    /// re-exported by the `meerkat` facade), a status read
    /// ([`McpOAuthAuthority::stored_credential_status`]) reports what a use of
    /// the stored credential does. Never a success and never a clean failure.
    #[error(
        "MCP OAuth login for '{server_name}': whether the credential landed is unknown until the credential commit settles"
    )]
    CommitUnsettled { server_name: String },
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
            | Self::RealmMismatch { .. }
            | Self::Verification(_)
            | Self::MissingStoredToken { .. }
            | Self::HumanAuthorizationRequired { .. }
            | Self::RemoteCustodyRequired { .. }
            | Self::ReauthRequired { .. }
            | Self::DisconnectRequired { .. }
            | Self::CredentialSlot { .. }
            | Self::AttemptRetired { .. }
            | Self::EgressRefused { .. }
            | Self::TokenKey { .. } => true,
            // Flow-owner persistence and lifecycle failures are not refusals.
            Self::Flow(error) => error.is_refusal(),
            Self::AdmissionReadFailed { read, .. } => read.is_refusal(),
            Self::StalePreparation
            | Self::Callback { .. }
            | Self::DiscoveryFailed { .. }
            | Self::RegistrationFailed { .. }
            | Self::TokenExchangeFailed { .. }
            | Self::RefreshFailed { .. }
            | Self::TokenStore(_)
            | Self::CredentialAccess { .. }
            | Self::MissingStoredMetadata { .. }
            | Self::HttpClientUnavailable(_)
            | Self::EgressUnavailable { .. }
            | Self::AuthLifecycle { .. }
            | Self::NetworkTimedOut { .. }
            | Self::CommitUnsettled { .. } => false,
        }
    }

    /// The typed MCP OAuth outcome of a required egress profile that did not
    /// admit an effect for `target`.
    pub fn from_egress(target: &McpServerIdentity, error: HttpEgressError) -> Self {
        let server_name = target.server_name().to_owned();
        match error {
            HttpEgressError::Refused(refusal) => Self::EgressRefused {
                server_name,
                refusal,
            },
            HttpEgressError::Unavailable(reason) => Self::EgressUnavailable {
                server_name,
                reason,
            },
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
    /// The host runtime realm every target is bound to, when the host serves
    /// one. Each entry point scopes its target to it first, so keys, leases
    /// and attempts never fall back to the realm-less shared slot.
    realm: Option<RealmId>,
    /// The host egress profile every OAuth request of this authority is
    /// admitted by; a required profile also supplies its client.
    egress: HttpEgressProfile,
    /// How long this authority waits on the network.
    bounds: McpOAuthNetworkBounds,
}

impl McpOAuthAuthority {
    /// Serve `realm`: every target is bound to it, and a target of another
    /// realm is refused ([`McpOAuthError::RealmMismatch`]).
    pub fn in_realm(mut self, realm: RealmId) -> Self {
        self.realm = Some(realm);
        self
    }

    /// The realm this authority serves, if any.
    pub fn realm(&self) -> Option<&RealmId> {
        self.realm.as_ref()
    }

    /// `target` bound to this authority's realm.
    fn scoped(&self, target: &McpServerIdentity) -> Result<McpServerIdentity, McpOAuthError> {
        target.scoped_to(self.realm.as_ref())
    }

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
            realm: None,
            egress: HttpEgressProfile::trusted_host(),
            bounds: McpOAuthNetworkBounds::default(),
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
            realm: None,
            egress: HttpEgressProfile::trusted_host(),
            bounds: McpOAuthNetworkBounds::default(),
        }
    }

    /// Enforce the host's egress `profile` for every OAuth request of this
    /// authority: discovery, client registration, code exchange and refresh.
    /// A required profile admits each endpoint URL before it is sent and
    /// supplies the redirect-free client (with its own resolver and no
    /// ambient proxy), replacing this authority's client. An account
    /// strategy is bound separately (for the native strategy,
    /// [`OidcUserInfoAccountStrategy::with_http_egress`]).
    #[must_use]
    pub fn with_http_egress(mut self, profile: &HttpEgressProfile) -> Self {
        self.egress = profile.clone();
        self
    }

    /// Wait on the network at most `bounds` (see [`McpOAuthNetworkBounds`]).
    /// They hold for every client this authority uses, a host-supplied one
    /// and the egress profile's included, and for its account strategy's
    /// calls.
    #[must_use]
    pub fn with_network_bounds(mut self, bounds: McpOAuthNetworkBounds) -> Self {
        self.bounds = bounds;
        self
    }

    /// The network bounds this authority waits within.
    pub fn network_bounds(&self) -> McpOAuthNetworkBounds {
        self.bounds
    }

    /// Run one network `stage` for `target`, bounded by the stage bound from
    /// now and by `phase_deadline`. A stage past its bound ends as
    /// [`McpOAuthError::NetworkTimedOut`]; its unfinished requests are
    /// dropped.
    async fn bounded_stage<T>(
        &self,
        target: &McpServerIdentity,
        stage: McpOAuthNetworkStage,
        phase_deadline: tokio::time::Instant,
        work: impl std::future::Future<Output = Result<T, McpOAuthError>>,
    ) -> Result<T, McpOAuthError> {
        let deadline = phase_deadline.min(deadline_after(self.bounds.stage));
        tokio::time::timeout_at(deadline, work)
            .await
            .unwrap_or_else(|_| {
                Err(McpOAuthError::NetworkTimedOut {
                    server_name: target.server_name().to_owned(),
                    stage,
                })
            })
    }

    /// The required egress profile this authority enforces for all of its
    /// OAuth traffic, its interactive account strategy included; `None` for
    /// a trusted-host authority or one whose strategy is not bound to the
    /// same profile.
    pub fn bound_http_egress(&self) -> Option<&HttpEgressProfile> {
        if !self.egress.is_required() {
            return None;
        }
        let strategy_bound = self.interactive.as_ref().is_none_or(|(_, strategy)| {
            strategy
                .http_egress()
                .is_some_and(|bound| bound.same_profile(&self.egress))
        });
        strategy_bound.then_some(&self.egress)
    }

    /// Refuse a ceremony whose account strategy would send OAuth traffic
    /// outside this authority's required egress profile.
    fn require_bound_strategy(
        &self,
        target: &McpServerIdentity,
        strategy: &dyn McpOAuthAccountStrategy,
    ) -> Result<(), McpOAuthError> {
        if self.egress.is_required()
            && !strategy
                .http_egress()
                .is_some_and(|bound| bound.same_profile(&self.egress))
        {
            return Err(McpOAuthError::from_egress(
                target,
                HttpEgressError::Unavailable(HttpEgressUnavailable::CredentialOwnerNotBound),
            ));
        }
        Ok(())
    }

    /// The redirect-free HTTP client, or the typed build failure.
    fn http(&self) -> Result<&Client, McpOAuthError> {
        self.http
            .as_ref()
            .map_err(|error| McpOAuthError::HttpClientUnavailable(*error))
    }

    /// The client for one OAuth request to `url`, after the egress profile
    /// admits it. Nothing is sent when it is refused.
    fn http_for(&self, target: &McpServerIdentity, url: &str) -> Result<&Client, McpOAuthError> {
        self.egress
            .admit(url)
            .map_err(|error| McpOAuthError::from_egress(target, error))?;
        match self.egress.credential_client() {
            Some(client) => client.map_err(|reason| {
                McpOAuthError::from_egress(target, HttpEgressError::Unavailable(reason))
            }),
            None => self.http(),
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
        let target = &self.scoped(target)?;
        if self.interactive.is_some() && !target.is_selected() {
            return Err(McpOAuthError::AccountSelectionRequired);
        }
        let key = target.token_key()?;
        let lease_key = target.lease_key()?;
        let admitted = {
            // A request never waits behind an operating-system credential
            // call that holds the lease: it answers pending at once.
            let _guard =
                meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
                    .await
                    .ok_or_else(mcp_owner_busy)?;
            self.load_admitted_stored_credential(
                target,
                &key,
                &_guard,
                TokenStoreAccess::Unattended,
            )
            .await?
        };
        let Some(admitted) = admitted else {
            return Ok(None);
        };
        match admitted.disposition {
            CredentialUseDisposition::Authorized => Ok(admitted.tokens.primary_secret),
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing => {
                // Known before any refresh is coordinated, so nothing is
                // exchanged: a verified refresh re-observes the subject
                // through the account strategy, which under a required
                // egress profile must be bound to that exact profile, and an
                // unadmitted token endpoint is refused typed.
                if let Some((_, strategy)) = &self.interactive
                    && subject_to_reobserve(target, &admitted.tokens).is_some()
                {
                    self.require_bound_strategy(target, strategy.as_ref())?;
                }
                self.egress
                    .admit(&admitted.metadata.discovery.token_endpoint)
                    .map_err(|error| McpOAuthError::from_egress(target, error))?;
                let authority = self.clone();
                let refresh_target = target.clone();
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
                    .map_err(|error| map_coordinated_refresh_error(target, error))?;
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

    /// The status of `target`'s stored credential: what
    /// [`stored_bearer_token`](Self::stored_bearer_token) would do with it
    /// next, through the same rule. `None` when the credential's lifecycle
    /// owner is busy (the read never waits for it) or a legacy refresh marker
    /// is pending; the caller then keeps its own durable reading.
    pub async fn stored_credential_status(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<McpStoredCredentialStatus>, McpOAuthError> {
        let target = &self.scoped(target)?;
        if self.interactive.is_some() && !target.is_selected() {
            return Ok(None);
        }
        let key = target.token_key()?;
        let lease_key = target.lease_key()?;
        // A status read never waits for the lifecycle owner.
        let Some(guard) = meerkat_core::try_acquire_auth_login_lifecycle_guard(&lease_key) else {
            return Ok(None);
        };
        let admitted = match self
            .load_admitted_stored_credential(target, &key, &guard, TokenStoreAccess::Status)
            .await
        {
            Ok(admitted) => admitted,
            Err(McpOAuthError::ReauthRequired { .. }) => {
                let stored = self
                    .token_store()
                    .load_for(&key, TokenStoreAccess::Status)
                    .await
                    .map_err(mcp_store_error)?
                    .filter(|tokens| tokens.auth_mode == PersistedAuthMode::McpOauth);
                return Ok(Some(McpStoredCredentialStatus {
                    phase: McpStoredCredentialPhase::ReauthRequired,
                    expires_at: stored.as_ref().and_then(|tokens| tokens.expires_at),
                    account_id: stored.and_then(|tokens| tokens.account_id),
                }));
            }
            // A legacy refresh marker is transient: the caller's reading
            // stands until the refresh owner settles it.
            Err(McpOAuthError::StalePreparation) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(admitted) = admitted else {
            return Ok(Some(McpStoredCredentialStatus {
                phase: McpStoredCredentialPhase::AuthorizationRequired,
                expires_at: None,
                account_id: None,
            }));
        };
        let phase = match admitted.disposition {
            CredentialUseDisposition::RefreshRequired
                if admitted.tokens.refresh_token.is_none() =>
            {
                McpStoredCredentialPhase::ReauthRequired
            }
            _ => McpStoredCredentialPhase::Authorized,
        };
        Ok(Some(McpStoredCredentialStatus {
            phase,
            expires_at: admitted.tokens.expires_at,
            account_id: admitted.tokens.account_id,
        }))
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
        self.login_start_with_admission(target, redirect_uri, www_authenticate)
            .await
            .map_err(|failure| failure.error)
    }

    /// [`Self::login_start`] that, when it fails, also says what this call
    /// left of the attempt it tried to admit ([`McpOAuthStartFailure`]). The
    /// error is the one [`Self::login_start`] returns.
    pub async fn login_start_with_admission(
        &self,
        target: &McpServerIdentity,
        redirect_uri: &str,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoginStart, McpOAuthStartFailure> {
        self.admit_attempt(
            target,
            redirect_uri,
            RedirectMode::Loopback,
            www_authenticate,
        )
        .await
    }

    /// The exact registered redirect of `target` under `base`: the base
    /// followed by the target's credential binding slug, for the target as
    /// this authority serves it (its realm and account selection included).
    /// Local and secret-free; it admits nothing.
    pub fn registered_redirect_uri(
        &self,
        target: &McpServerIdentity,
        base: &McpRegisteredRedirectBase,
    ) -> Result<String, McpOAuthError> {
        let target = self.scoped(target)?;
        let redirect = base.redirect_for(&target.binding_slug());
        // The slug adds only unreserved path characters, so the derived
        // redirect is itself a valid registered redirect.
        McpRegisteredRedirectBase::parse(&redirect)?;
        Ok(redirect)
    }

    /// [`Self::login_start`] for a host whose users' browsers cannot reach
    /// its loopback address: the attempt redirects to the target's registered
    /// `https` callback under `base` ([`Self::registered_redirect_uri`]), the
    /// host's own endpoint. Everything else is the same admission: realm,
    /// account selection, discovery, preflight, client registration (as a
    /// `web` public PKCE client registering exactly that redirect), PKCE and
    /// one-time state through the AuthMachine-owned flow owner, the same
    /// window and the same start-or-join.
    ///
    /// A started attempt returns [`McpOAuthLoginStart::host_attempt_handle`]:
    /// a host-only secret its completion
    /// ([`Self::login_complete_registered`]) must present. A start joins
    /// only an attempt pending on this same redirect, and the joined
    /// projection carries no handle; an attempt pending on another redirect
    /// is refused ([`OAuthFlowError::RedirectUriMismatch`]) and can be
    /// cancelled by its reference, and so is a loopback start while a
    /// registered attempt is pending. A slot whose credential's client was
    /// registered for another redirect needs a disconnect first.
    pub async fn login_start_registered(
        &self,
        target: &McpServerIdentity,
        base: &McpRegisteredRedirectBase,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoginStart, McpOAuthError> {
        self.login_start_registered_with_admission(target, base, www_authenticate)
            .await
            .map_err(|failure| failure.error)
    }

    /// [`Self::login_start_registered`] that, when it fails, also says what
    /// this call left of the attempt it tried to admit
    /// ([`McpOAuthStartFailure`]). The error is the one
    /// [`Self::login_start_registered`] returns.
    pub async fn login_start_registered_with_admission(
        &self,
        target: &McpServerIdentity,
        base: &McpRegisteredRedirectBase,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoginStart, McpOAuthStartFailure> {
        let redirect = self
            .registered_redirect_uri(target, base)
            .map_err(McpOAuthStartFailure::not_admitted)?;
        self.admit_attempt(
            target,
            &redirect,
            RedirectMode::Registered,
            www_authenticate,
        )
        .await
    }

    /// Admit one attempt, or join the one pending. Every step before the
    /// flow owner's start admits nothing; the start reports what it left of
    /// the attempt it tried to admit, and nothing after it can fail.
    async fn admit_attempt(
        &self,
        target: &McpServerIdentity,
        redirect_uri: &str,
        mode: RedirectMode,
        www_authenticate: Option<&str>,
    ) -> Result<McpOAuthLoginStart, McpOAuthStartFailure> {
        let ready = match self
            .prepare_attempt(target, redirect_uri, mode, www_authenticate)
            .await
        {
            Ok(PreparedAttempt::Joined(joined)) => return Ok(joined),
            Ok(PreparedAttempt::Ready(ready)) => ready,
            Err(error) => return Err(McpOAuthStartFailure::not_admitted(error)),
        };
        let ReadyAttempt {
            authority,
            _admission,
            target,
            binding,
            credential_identity,
            discovery,
            client,
            offline_access,
            identity,
        } = ready;
        let pkce = PkcePair::generate_s256();
        let state = authority
            .start_with_admission(
                credential_identity,
                identity.clone(),
                redirect_uri.to_owned(),
                pkce.verifier.secret().to_owned(),
            )
            .map_err(|failure| McpOAuthStartFailure {
                error: McpOAuthError::Flow(failure.error),
                admission: failure.admission,
            })?;
        // This attempt passed its strategy preflight (or needs none); a
        // later join reuses this receipt instead of repeating discovery.
        retain_preflight_receipt(&state);
        let expires_at = std::time::Instant::now() + MCP_INTERACTIVE_LOGIN_TIMEOUT;
        let authorize_url = authorize_endpoints(&discovery, &client, offline_access)
            .authorize_url_with_pkce(&pkce.challenge, &state);
        let host_attempt_handle = (mode == RedirectMode::Registered)
            .then(|| mcp_remote_attempt_handle(&binding, &state, redirect_uri));
        Ok(McpOAuthLoginStart {
            target,
            authorize_url,
            state,
            redirect_uri: redirect_uri.to_owned(),
            disposition: McpOAuthLoginDisposition::Started,
            host_attempt_handle,
            identity,
            expires_at,
        })
    }

    /// Every step of a start before the flow owner's start, under the
    /// target's start-or-join lock: the projection of an attempt it joins,
    /// or what the admission needs.
    async fn prepare_attempt(
        &self,
        target: &McpServerIdentity,
        redirect_uri: &str,
        mode: RedirectMode,
        www_authenticate: Option<&str>,
    ) -> Result<PreparedAttempt<'_>, McpOAuthError> {
        let target = &self.scoped(target)?;
        self.validate_interactive_selection(target)?;
        let (authority, strategy) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        self.require_bound_strategy(target, strategy.as_ref())?;
        match mode {
            RedirectMode::Loopback => require_loopback_redirect(redirect_uri)?,
            // Only a redirect derived from a host's registered base gets here.
            RedirectMode::Registered => {
                McpRegisteredRedirectBase::parse(redirect_uri)?;
            }
        }
        let binding = target.auth_binding_ref()?;
        let credential_identity: meerkat_core::AuthCredentialIdentity = binding.clone().into();
        let _admission = acquire_admission_lock(target.binding_slug()).await;
        // Every network stage below (a join's issuer metadata, or discovery,
        // preflight and registration) ends by this deadline.
        let start_deadline = deadline_after(self.bounds.start);
        if let Some((state, record)) = authority
            .pending_connector_browser_attempt(&credential_identity)
            .map_err(McpOAuthError::Flow)?
        {
            // A start joins only an attempt its own callback can finish: a
            // registered start the attempt pending on exactly its redirect,
            // a loopback start a loopback attempt (any port). A joined
            // projection must send the browser to a callback this host
            // serves.
            let joinable = match mode {
                RedirectMode::Registered => record.redirect_uri == redirect_uri,
                RedirectMode::Loopback => !is_https_redirect(&record.redirect_uri),
            };
            if !joinable {
                return Err(McpOAuthError::Flow(OAuthFlowError::RedirectUriMismatch));
            }
            // `None`: the pending attempt no longer matches the server, or
            // its preflight is not retained; it was retired, so admit a
            // fresh one.
            if let Some(joined) = self
                .join_pending_attempt(target, &state, record, start_deadline)
                .await?
            {
                return Ok(PreparedAttempt::Joined(joined));
            }
        }
        // The slot's current credential decides what this attempt may
        // publish. The commit re-checks it inside the slot's exclusive
        // mutation; this read only refuses early, before any network I/O.
        let occupant = self.slot_occupant(target).await?;
        let selection = attempt_selection(target, occupant.as_ref())?;
        let (mut discovery, requested) = self
            .discover(target, www_authenticate, start_deadline)
            .await?;
        // Before client registration and before the human is asked to
        // consent: an issuer the strategy cannot verify is refused here.
        // An unverified resource grant runs no account strategy.
        if !selection.is_unverified() {
            self.bounded_stage(
                target,
                McpOAuthNetworkStage::AccountPreflight,
                start_deadline,
                async {
                    strategy
                        .preflight(&discovery.authorization_server)
                        .await
                        .map_err(McpOAuthError::from)
                },
            )
            .await?;
        }
        let client = match &occupant {
            // Reconnect with the admitted client: a fresh registration would
            // change the credential's context. A loopback redirect may use
            // any port (RFC 8252 section 7.3); a registered `https` one is
            // registered exactly, so a client registered for another redirect
            // cannot serve this attempt.
            Some(occupant) => {
                let client_redirect = &occupant.metadata.client.redirect_uri;
                let redirect_fits = match mode {
                    RedirectMode::Loopback => !is_https_redirect(client_redirect),
                    RedirectMode::Registered => client_redirect == redirect_uri,
                };
                if occupant.metadata.discovery.authorization_server
                    != discovery.authorization_server
                    || occupant.metadata.discovery.resource != discovery.resource
                    || !redirect_fits
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
                self.bounded_stage(
                    target,
                    McpOAuthNetworkStage::ClientRegistration,
                    start_deadline,
                    self.register_client(target, &discovery, redirect_uri, mode),
                )
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
        Ok(PreparedAttempt::Ready(ReadyAttempt {
            authority: authority.as_ref(),
            _admission,
            target: target.clone(),
            binding,
            credential_identity,
            discovery,
            client,
            offline_access: requested.offline_access,
            identity: descriptor.into(),
        }))
    }

    /// The `McpOauth` credential currently in `target`'s slot, if any, with
    /// its stored metadata and account binding. An occupant this target may
    /// never reconnect over (no binding, or another verification mode) is
    /// refused here: it needs an explicit disconnect.
    async fn slot_occupant(
        &self,
        target: &McpServerIdentity,
    ) -> Result<Option<SlotOccupant>, McpOAuthError> {
        // A login start reads the slot it may publish into: interactive.
        let Some(tokens) = self
            .token_store()
            .load_for(&target.token_key()?, TokenStoreAccess::Interactive)
            .await
            .map_err(mcp_store_error)?
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
        start_deadline: tokio::time::Instant,
    ) -> Result<Option<McpOAuthLoginStart>, McpOAuthError> {
        if !pending_attempt_matches(target, &record) || !preflight_receipt_retained(state) {
            // A failed retirement is propagated: no fresh attempt is admitted
            // beside one the flow owner still holds.
            if let Some((authority, _)) = self.interactive.as_ref() {
                retire_attempt(
                    authority.as_ref(),
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
            .bounded_stage(
                target,
                McpOAuthNetworkStage::AuthorizationServerMetadata,
                start_deadline,
                self.discover_authorization_server(target, &facts.issuer),
            )
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
            // A joined projection never carries custody of the attempt.
            host_attempt_handle: None,
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
        let target = &self.scoped(target)?;
        // Everything fallible that needs no admission runs before it.
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
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
        // The attempt is admitted: this login takes its cleanup custody at
        // once. The wake-up is registered before the attempt is read back
        // from its flow owner, so a retirement between admission and
        // registration is not lost: this read observes it, or the wait does.
        let wake = AttemptWakeRegistration::register(&start.state);
        let callback = binding.expect_state(start.state.clone());
        let pending = McpOAuthPendingLogin {
            authority: self.clone(),
            start,
            callback: Some(callback),
            listener: None,
            completion: PendingCompletion::NotEntered,
            guard_retirement: RetireObserver::default(),
            wake,
            confirmed_retired: false,
        };
        let live = match self.interactive.as_ref() {
            Some((authority, _)) => authority
                .admitted_connector_browser_attempt(&pending.start.state, &credential_identity)
                .map_err(McpOAuthError::Flow),
            None => Err(ConnectorOAuthRefusal::VerificationUnavailable.into()),
        };
        let read = match live {
            Ok(Some(_)) => return Ok(McpOAuthLoopbackBegin::Started(pending)),
            Ok(None) => McpOAuthError::Flow(OAuthFlowError::Missing),
            Err(error) => error,
        };
        // The read-back failed: retire the attempt and join the listener
        // through the login's own close, and report both beside the fault.
        let cleanup = pending.close().await;
        Err(McpOAuthError::AdmissionReadFailed {
            server_name: target.server_name().to_owned(),
            read: Box::new(read),
            cleanup: Box::new(cleanup),
        })
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
        self.login_complete_observed(target, callback, CompletionCustody::Loopback, None)
            .await
    }

    /// Complete one attempt admitted by [`Self::login_start_registered`] from
    /// the host's `https` callback.
    ///
    /// The flow owner must name a live attempt admitted under `state` for
    /// this exact target, on a registered redirect, and
    /// `host_attempt_handle` must be the handle its start returned. Both are
    /// checked before anything else: a callback without the host's custody
    /// is refused ([`McpOAuthError::RemoteCustodyRequired`]) and leaves the
    /// attempt to its owner. Then, as for [`Self::login_complete`],
    /// `redirect_uri` must equal the admitted redirect, the recorded
    /// issuer's metadata is fetched, the code is exchanged with the retained
    /// PKCE verifier and the canonical commit verifies, persists and consumes
    /// the attempt; every non-success exit after the custody check retires
    /// it. When the callback carries `iss` (RFC 9207) it must be the
    /// attempt's issuer, and an authorization server that advertises
    /// `authorization_response_iss_parameter_supported` must send it. A
    /// consumed attempt cannot be replayed.
    pub async fn login_complete_registered(
        &self,
        target: &McpServerIdentity,
        callback: McpOAuthRegisteredCallback,
    ) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        let McpOAuthRegisteredCallback {
            redirect_uri,
            state,
            code,
            iss,
            host_attempt_handle,
        } = callback;
        self.login_complete_observed(
            target,
            McpOAuthCallback {
                redirect_uri,
                state,
                code,
            },
            CompletionCustody::Registered {
                host_attempt_handle: &host_attempt_handle,
                iss: iss.as_deref(),
            },
            None,
        )
        .await
    }

    /// [`login_complete`](Self::login_complete) that records its retire
    /// guard's own result for the pending login that handed the code over.
    async fn login_complete_observed(
        &self,
        target: &McpServerIdentity,
        callback: McpOAuthCallback,
        custody: CompletionCustody<'_>,
        observed: Option<RetireObserver>,
    ) -> Result<McpOAuthLoginComplete, McpOAuthError> {
        let target = &self.scoped(target)?;
        self.validate_interactive_selection(target)?;
        let (authority, strategy) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        self.require_bound_strategy(target, strategy.as_ref())?;
        let McpOAuthCallback {
            redirect_uri,
            state,
            code,
        } = callback;
        let binding = target.auth_binding_ref()?;
        let credential_identity: meerkat_core::AuthCredentialIdentity = binding.clone().into();
        let admitted = authority
            .admitted_connector_browser_attempt(&state, &credential_identity)
            .map_err(McpOAuthError::Flow)?
            .ok_or(McpOAuthError::Flow(OAuthFlowError::Missing))?;
        // A registered attempt completes only from the custody of the host
        // that started it: its retained handle for exactly this slot, state
        // and admitted redirect. Refused before the attempt is armed for
        // retirement, so a forged callback cannot cancel it.
        let registered_attempt = is_https_redirect(&admitted.redirect_uri);
        let response_issuer = match custody {
            CompletionCustody::Loopback if registered_attempt => {
                return Err(McpOAuthError::RemoteCustodyRequired {
                    server_name: target.server_name().to_owned(),
                });
            }
            CompletionCustody::Loopback => None,
            CompletionCustody::Registered {
                host_attempt_handle,
                iss,
            } => {
                let expected = mcp_remote_attempt_handle(&binding, &state, &admitted.redirect_uri);
                if !registered_attempt
                    || !crate::connector_protocol::bytes_equal(
                        host_attempt_handle.as_bytes(),
                        expected.as_bytes(),
                    )
                {
                    return Err(McpOAuthError::RemoteCustodyRequired {
                        server_name: target.server_name().to_owned(),
                    });
                }
                Some(iss)
            }
        };
        // Every network stage below ends by this deadline, never past the
        // attempt's own validity: a later result would be refused anyway.
        let completion_deadline = deadline_after(self.bounds.completion).min(
            tokio::time::Instant::from_std(admitted.created_at + MCP_INTERACTIVE_LOGIN_TIMEOUT),
        );
        // The wake-up is registered before the attempt is verified, so a
        // retirement is observed by the verify or by the stages below.
        let mut retire = AttemptRetireGuard {
            authority: Arc::clone(authority),
            state: state.clone(),
            target: credential_identity.clone(),
            identity: admitted.provider.clone(),
            redirect_uri: admitted.redirect_uri.clone(),
            armed: true,
            observed,
            wake: AttemptWakeRegistration::register(&state),
        };
        // A registered completion names the admitted redirect byte for byte:
        // a mismatch is refused as what it is, and retires the attempt.
        if response_issuer.is_some() && redirect_uri != admitted.redirect_uri {
            return Err(McpOAuthError::Flow(OAuthFlowError::RedirectUriMismatch));
        }
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
        let endpoints = retire
            .unless_retired(
                target,
                self.bounded_stage(
                    target,
                    McpOAuthNetworkStage::AuthorizationServerMetadata,
                    completion_deadline,
                    self.discover_authorization_server(target, &facts.issuer),
                ),
            )
            .await?;
        // RFC 9207: a registered completion's response names the attempt's
        // own issuer, and a server that says it sends `iss` must have sent
        // it. Checked before the code or the verifier goes anywhere.
        if let Some(iss) = response_issuer {
            let issuer_fits = match iss {
                Some(iss) => iss == facts.issuer,
                None => !endpoints.iss_parameter_supported,
            };
            if !issuer_fits {
                return Err(McpOAuthError::Verification(
                    ConnectorOAuthRefusal::DescriptorMismatch,
                ));
            }
        }
        let discovery = completion_discovery(target, facts, endpoints)?;
        let client = StoredMcpOAuthClient {
            client_id: facts.client.clone(),
            client_secret: None,
            token_endpoint_auth_method: "none".to_owned(),
            redirect_uri: record.redirect_uri.clone(),
        };
        let endpoints = mcp_oauth_endpoints(&discovery, &client);
        let exchange = async {
            exchange_authorization_code_with_state(
                self.http_for(target, &endpoints.token_url)?,
                &endpoints,
                &code,
                &record.pkce_verifier,
                None,
                Some(&state),
            )
            .await
            .map_err(|error| match error {
                crate::auth_oauth::OAuthError::EgressRefused(refusal) => {
                    McpOAuthError::from_egress(target, HttpEgressError::Refused(refusal))
                }
                crate::auth_oauth::OAuthError::Timeout => McpOAuthError::NetworkTimedOut {
                    server_name: target.server_name().to_owned(),
                    stage: McpOAuthNetworkStage::TokenExchange,
                },
                _ => McpOAuthError::TokenExchangeFailed {
                    server_name: target.server_name().to_owned(),
                    reason: "authorization-code exchange failed".into(),
                },
            })
        };
        let token = retire
            .unless_retired(
                target,
                self.bounded_stage(
                    target,
                    McpOAuthNetworkStage::TokenExchange,
                    completion_deadline,
                    exchange,
                ),
            )
            .await?;
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
            let observation = retire
                .unless_retired(
                    target,
                    self.bounded_stage(
                        target,
                        McpOAuthNetworkStage::AccountEvidence,
                        completion_deadline,
                        async {
                            strategy
                                .observe_account(descriptor, &token)
                                .await
                                .map_err(McpOAuthError::from)
                        },
                    ),
                )
                .await?;
            let evidence = descriptor.verify_account(observation, &token)?;
            persisted.account_id = Some(evidence.account().to_owned());
            persisted.scopes = evidence.granted_scopes().iter().cloned().collect();
            evidence.into()
        };
        let commit = save_oauth_tokens_and_consume_browser_flow(
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
        );
        // The commit's work is owned by the credential coordinator and runs to
        // its own end; this completion observes it for a bounded time.
        let observed_until = deadline_after(self.bounds.commit_observation);
        let observed = retire
            .unless_retired(target, async {
                Ok(tokio::time::timeout_at(observed_until, commit).await)
            })
            .await?;
        let committed = match observed {
            Ok(Ok(committed)) => committed,
            // Not observed to its end, or its owner was lost before it
            // reported: the flow owner decides which side of the consume the
            // commit is on.
            Ok(Err(CredentialMutationError::Cancelled)) | Err(_) => {
                return Err(retire.settle_unobserved_commit(target));
            }
            Ok(Err(error)) => return Err(map_coordinated_login_error(target, error)),
        };
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
        let target = &self.scoped(target)?;
        let (authority, _) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        retire_attempt(
            authority.as_ref(),
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
        let target = &self.scoped(target)?;
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
        retire_attempt(
            authority.as_ref(),
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
        let target = &self.scoped(target)?;
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
    /// retire the attempt pending for `target` under that reference. A
    /// reference to an attempt the flow owner holds without its payload
    /// (one a start could not retire, reported as
    /// [`OAuthStartAdmission::RetirementUnconfirmed`]) retires that attempt
    /// and any durable entry for it, although no pending read reports it.
    /// Local only; an unknown or stale reference is refused, and then
    /// nothing with that reference is held.
    pub fn cancel_attempt_by_ref(
        &self,
        target: &McpServerIdentity,
        attempt_ref: &str,
    ) -> Result<(), McpOAuthError> {
        let target = &self.scoped(target)?;
        let (authority, _) = self
            .interactive
            .as_ref()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        if let Some((state, record)) = authority
            .pending_connector_browser_attempt(&credential_identity)
            .map_err(McpOAuthError::Flow)?
            && OAuthBrowserActionRef::project(&state).as_str() == attempt_ref
        {
            if !pending_attempt_matches(target, &record) {
                return Err(McpOAuthError::Flow(OAuthFlowError::Missing));
            }
            return retire_attempt(
                authority.as_ref(),
                &state,
                &credential_identity,
                record.provider,
                &record.redirect_uri,
            )
            .map_err(McpOAuthError::Flow);
        }
        let retired =
            retire_unpayloaded_attempt(authority.as_ref(), &credential_identity, attempt_ref)
                .map_err(McpOAuthError::Flow)?;
        if retired == 0 {
            return Err(McpOAuthError::Flow(OAuthFlowError::Missing));
        }
        Ok(())
    }

    /// Disconnect `target`: retire every browser attempt pending for its
    /// slot, then remove its stored credential and release the credential
    /// lifecycle through the same coordinated mutation that guards refresh
    /// and login commits. Both happen even when no credential is stored, so
    /// a callback admitted before the disconnect cannot repopulate the slot:
    /// its commit finds the attempt retired. Nothing is revoked at the
    /// provider.
    pub async fn logout(&self, target: &McpServerIdentity) -> Result<(), McpOAuthError> {
        let target = &self.scoped(target)?;
        let credential_identity: meerkat_core::AuthCredentialIdentity =
            target.auth_binding_ref()?.into();
        if let Some((authority, _)) = self.interactive.as_ref() {
            // The flow owner holds at most one attempt per slot; each pass
            // retires the one it reports, so this ends when none is left.
            while let Some((state, record)) = authority
                .pending_connector_browser_attempt(&credential_identity)
                .map_err(McpOAuthError::Flow)?
            {
                retire_attempt(
                    authority.as_ref(),
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
        access: TokenStoreAccess,
    ) -> Result<Option<AdmittedMcpCredential>, McpOAuthError> {
        if guard.lease_key() != &target.lease_key()? {
            return Err(McpOAuthError::AuthLifecycle {
                server_name: target.server_name().to_string(),
                reason: "credential lifecycle guard belongs to another lease".into(),
            });
        }
        let Some(mut tokens) = self
            .token_store()
            .load_for(key, access)
            .await
            .map_err(mcp_store_error)?
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
            .map_err(|error| mcp_rehydrate_error(target, error))?
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
        self.http()
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        let lease_key = target
            .lease_key()
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        // Unattended refresh never waits behind an operating-system
        // credential call that holds the lease.
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
            .await
            .ok_or_else(lifecycle_owner_busy)?;
        let mut admitted = self
            .load_admitted_stored_credential(target, key, &_guard, TokenStoreAccess::Unattended)
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
                .load_admitted_stored_credential(target, key, &_guard, TokenStoreAccess::Unattended)
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

        // Before the refresh begins: the token endpoint of the credential
        // that is actually refreshed is admitted, and the client is the
        // egress profile's own. A refusal here begins nothing to close.
        let http = self
            .http_for(target, &admitted.metadata.discovery.token_endpoint)
            .map_err(|error| match error {
                McpOAuthError::EgressRefused { refusal, .. } => RefreshError::EgressRefused {
                    refusal,
                    observation: meerkat_core::RefreshFailureObservation::transient(),
                    disposition: None,
                },
                other => RefreshError::Refresh(other.to_string()),
            })?;
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
        // Bounded like a login stage: a token endpoint that never answers
        // ends as a request timeout (a transient failure), so this refresh
        // never holds the credential's exclusive mutation without bound.
        let exchange = tokio::time::timeout(
            self.bounds.stage,
            exchange_refresh_token(
                http,
                &endpoints,
                &refresh_token,
                metadata.client.client_secret.as_deref(),
            ),
        )
        .await
        .unwrap_or(Err(crate::auth_oauth::OAuthError::Timeout));
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let current_tokens = match self
            .token_store()
            .load_for(key, TokenStoreAccess::Unattended)
            .await
        {
            Ok(current_tokens) => current_tokens,
            Err(error) => {
                // A refused reread publishes nothing and keeps the stored
                // credential. The refresh this owner began still closes, unless
                // another owner already moved the lease.
                let cause = RefreshError::from_token_store(error);
                if self.auth_lease.snapshot(&lease_key) == refreshing_snapshot
                    && let Err(transition_error) = self.auth_lease.refresh_failed(
                        &lease_key,
                        meerkat_core::RefreshFailureObservation::transient(),
                    )
                {
                    return Err(RefreshError::Refresh(format!(
                        "{cause}; AuthMachine refresh_failed rejected closure: {transition_error}"
                    )));
                }
                return Err(cause);
            }
        };
        let lifecycle_current = self.auth_lease.snapshot(&lease_key) == refreshing_snapshot;
        if current_tokens.as_ref() != Some(&admitted.tokens) || !lifecycle_current {
            // Durable bytes that moved outside the protocol while this owner's
            // lifecycle is still current: the result stays stale and the
            // current bytes stay, but the refresh this owner began closes
            // transiently. A changed lifecycle belongs to its new owner and
            // is left untouched.
            if lifecycle_current
                && let Err(transition_error) = self.auth_lease.refresh_failed(
                    &lease_key,
                    meerkat_core::RefreshFailureObservation::transient(),
                )
            {
                return Err(RefreshError::Refresh(format!(
                    "{}; AuthMachine refresh_failed rejected closure: {transition_error}",
                    RefreshError::StalePreparation
                )));
            }
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
                // A local egress refusal closes as a transient failure (it
                // never invalidates the credential) and stays typed for every
                // reader of this coordinated result.
                if let crate::auth_oauth::OAuthError::EgressRefused(refusal) = &error {
                    return Err(RefreshError::EgressRefused {
                        refusal: *refusal,
                        observation,
                        disposition: Some(disposition),
                    });
                }
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
        if let Some(bound) = subject_to_reobserve(target, &admitted.tokens) {
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
                // The provider's evidence for the refreshed token was
                // unavailable: an upstream refresh failure, not a mismatch.
                // The strategy's own request refused by the egress profile
                // stays typed for every reader. Only a failed AuthMachine
                // closure is a local failure.
                Err(unobserved) => {
                    let observation = meerkat_core::RefreshFailureObservation::transient();
                    return Err(
                        match self
                            .auth_lease
                            .refresh_failed(&lease_key, observation.clone())
                        {
                            Ok(_) => match unobserved.egress_refused {
                                Some(refusal) => RefreshError::EgressRefused {
                                    refusal,
                                    observation,
                                    disposition: None,
                                },
                                None => RefreshError::Observed {
                                    message: unobserved.reason.to_owned(),
                                    observation,
                                },
                            },
                            Err(error) => RefreshError::Refresh(format!(
                                "{}; AuthMachine refresh_failed rejected closure: {error}",
                                unobserved.reason
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
                        RefreshError::Refresh(format!(
                            "failed to mark refreshed MCP OAuth token: {error}"
                        )),
                    )
                    .await);
            }
        };
        if let Err(error) = self
            .token_store()
            .save_for(key, &published, TokenStoreAccess::Unattended)
            .await
        {
            let cause = match error.credential_access() {
                Some((refusal, detail)) => RefreshError::CredentialAccess { refusal, detail },
                None => RefreshError::Refresh(format!(
                    "failed to save refreshed MCP OAuth token: {error}"
                )),
            };
            return Err(self
                .rollback_refresh_publication(
                    key,
                    &lease_key,
                    &admitted.tokens,
                    &admitted.restore_snapshot,
                    cause,
                )
                .await);
        }
        Ok(published)
    }

    /// The subject the account strategy observes for a refreshed token of a
    /// verified credential bound to `bound`. `Err` means the subject could
    /// not be observed (no strategy, another strategy or context, the
    /// provider evidence was unavailable, or the strategy's own request was
    /// refused by the egress profile), which is distinct from a different
    /// subject.
    async fn observe_refreshed_subject(
        &self,
        target: &McpServerIdentity,
        metadata: &StoredMcpOAuthMetadata,
        bound: &str,
        refreshed: &OAuthTokenResult,
    ) -> Result<String, SubjectUnobserved> {
        let unobserved = |reason| SubjectUnobserved {
            reason,
            egress_refused: None,
        };
        let Some((_, strategy)) = self.interactive.as_ref() else {
            return Err(unobserved("MCP OAuth account strategy is not installed"));
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
            .map_err(|_| unobserved("MCP OAuth account strategy refused the stored context"))?;
        if let Some(binding) = &metadata.account_binding
            && (binding.stable_context != descriptor.stable_context()
                || binding.strategy_id != descriptor.parameters().strategy_id)
        {
            return Err(unobserved(
                "installed MCP OAuth account strategy differs from the bound one",
            ));
        }
        // Bounded like a login stage: unavailable evidence, never a mismatch.
        let Ok(observed) = tokio::time::timeout(
            self.bounds.stage,
            strategy.observe_account(&descriptor, refreshed),
        )
        .await
        else {
            return Err(unobserved(
                "refreshed MCP OAuth subject was not observed within its bound",
            ));
        };
        observed
            .map(|observation| observation.account)
            .map_err(|refusal| SubjectUnobserved {
                reason: "refreshed MCP OAuth subject could not be observed",
                // The strategy's own endpoint refused by the egress profile:
                // typed for the caller, never a subject mismatch.
                egress_refused: match refusal {
                    ConnectorOAuthRefusal::EgressRefused(refusal) => Some(refusal),
                    _ => None,
                },
            })
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
            if let Err(error) = self
                .token_store()
                .clear_for(key, TokenStoreAccess::Unattended)
                .await
            {
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
        cause: RefreshError,
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
        if let Err(error) = self
            .token_store()
            .save_for(key, &restored_tokens, TokenStoreAccess::Unattended)
            .await
        {
            rollback_errors.push(format!("TokenStore restore failed: {error}"));
        }
        if rollback_errors.is_empty() {
            // The compensation completed: the original cause stands, typed.
            cause
        } else {
            // A failed compensation is its own failure, never the original
            // cause alone.
            RefreshError::Refresh(format!("{cause}; {}", rollback_errors.join("; ")))
        }
    }

    /// Discover the protected resource's authorization server and its
    /// endpoints: the resource metadata stage, then the authorization-server
    /// metadata stage, each bounded, and both by `start_deadline`.
    async fn discover(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
        start_deadline: tokio::time::Instant,
    ) -> Result<(StoredMcpOAuthDiscovery, RequestedScopes), McpOAuthError> {
        require_https_or_loopback(target, target.server_url(), "MCP protected resource")?;
        let (resource_metadata_url, resource) = self
            .bounded_stage(
                target,
                McpOAuthNetworkStage::ResourceMetadata,
                start_deadline,
                self.fetch_protected_resource_metadata(target, www_authenticate),
            )
            .await?;
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
            .bounded_stage(
                target,
                McpOAuthNetworkStage::AuthorizationServerMetadata,
                start_deadline,
                self.discover_authorization_server(target, &auth_server),
            )
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

    /// The protected resource's metadata URL (from the challenge, or the
    /// first well-known candidate that answers) and its document.
    async fn fetch_protected_resource_metadata(
        &self,
        target: &McpServerIdentity,
        www_authenticate: Option<&str>,
    ) -> Result<(String, ProtectedResourceMetadata), McpOAuthError> {
        let stage = McpOAuthNetworkStage::ResourceMetadata;
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
            .http_for(target, &resource_metadata_url)?
            .get(resource_metadata_url.clone())
            .send()
            .await
            .map_err(|error| {
                oauth_send_error(target, stage, error, |error| {
                    McpOAuthError::DiscoveryFailed {
                        server_name: target.server_name().to_string(),
                        reason: error.to_string(),
                    }
                })
            })?
            .error_for_status_refusing_redirects()
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name().to_string(),
                reason: error,
            })?
            .json()
            .await
            .map_err(|error| {
                oauth_send_error(target, stage, error, |error| {
                    McpOAuthError::DiscoveryFailed {
                        server_name: target.server_name().to_string(),
                        reason: format!("decode protected resource metadata: {error}"),
                    }
                })
            })?;
        Ok((resource_metadata_url, resource))
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
        let stage = McpOAuthNetworkStage::AuthorizationServerMetadata;
        for candidate in authorization_server_metadata_urls(&auth_server)? {
            let response = self
                .http_for(target, &candidate)?
                .get(&candidate)
                .send()
                .await
                .map_err(|error| {
                    oauth_send_error(target, stage, error, |error| {
                        McpOAuthError::DiscoveryFailed {
                            server_name: target.server_name().to_string(),
                            reason: error.to_string(),
                        }
                    })
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
            .map_err(|error| {
                oauth_send_error(target, stage, error, |error| {
                    McpOAuthError::DiscoveryFailed {
                        server_name: target.server_name().to_string(),
                        reason: format!("decode authorization server metadata: {error}"),
                    }
                })
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
            iss_parameter_supported: auth.authorization_response_iss_parameter_supported,
        })
    }

    async fn discover_resource_metadata_url_by_well_known(
        &self,
        target: &McpServerIdentity,
    ) -> Result<String, McpOAuthError> {
        for candidate in protected_resource_well_known_candidates(target.server_url())? {
            let response = self
                .http_for(target, &candidate)?
                .get(&candidate)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => return Ok(candidate),
                // The egress profile's own resolver refusal is typed, and so is
                // a probe that passed its client's bound; neither is ever taken
                // as an absent document.
                Err(error)
                    if crate::http_egress::egress_refusal_in(&error).is_some()
                        || error.is_timeout() =>
                {
                    return Err(oauth_send_error(
                        target,
                        McpOAuthNetworkStage::ResourceMetadata,
                        error,
                        |_| McpOAuthError::DiscoveryFailed {
                            server_name: target.server_name().to_string(),
                            reason: "egress refusal".to_string(),
                        },
                    ));
                }
                _ => {}
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
        mode: RedirectMode,
    ) -> Result<StoredMcpOAuthClient, McpOAuthError> {
        // MCP authorization requires the application type. A loopback
        // redirect (RFC 8252) is a native client; a registered `https`
        // redirect is the host's own web callback. OIDC servers default an
        // omitted type to `web`.
        let application_type = match mode {
            RedirectMode::Loopback => "native",
            RedirectMode::Registered => "web",
        };
        let body = serde_json::json!({
            "client_name": CLIENT_NAME,
            "application_type": application_type,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let wire: DynamicClientRegistrationResponse = self
            .http_for(target, &discovery.registration_endpoint)?
            .post(&discovery.registration_endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|error| {
                oauth_send_error(
                    target,
                    McpOAuthNetworkStage::ClientRegistration,
                    error,
                    |error| McpOAuthError::RegistrationFailed {
                        server_name: target.server_name().to_string(),
                        reason: error.to_string(),
                    },
                )
            })?
            .error_for_status_refusing_redirects()
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name().to_string(),
                reason: error,
            })?
            .json()
            .await
            .map_err(|error| {
                oauth_send_error(
                    target,
                    McpOAuthNetworkStage::ClientRegistration,
                    error,
                    |error| McpOAuthError::RegistrationFailed {
                        server_name: target.server_name().to_string(),
                        reason: format!("decode: {error}"),
                    },
                )
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
                reason: match mode {
                    RedirectMode::Loopback => format!(
                        "unsupported token_endpoint_auth_method '{token_endpoint_auth_method}'"
                    ),
                    // Nothing was admitted and no authorize URL exists.
                    RedirectMode::Registered => format!(
                        "the authorization server registered a confidential client \
                         (token_endpoint_auth_method '{token_endpoint_auth_method}') for the \
                         registered https redirect; a hosted login needs a public PKCE client \
                         (token_endpoint_auth_method 'none'): allow public web clients on that \
                         server, or log in through a loopback redirect"
                    ),
                },
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

/// See `verify_stored_account`. Hosts use it to project status from a
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
        client_auth_profile: None,
        protocol: None,
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

/// Process-local wake-ups for an attempt's waiters (a loopback login's
/// callback wait, and the completion it or a host entered), keyed by the
/// non-secret reference of the attempt's state. A wake-up is only a signal:
/// the flow owner stays the single retirement authority, and
/// [`retire_attempt`] sends one only after the owner confirmed the
/// retirement. One sender per attempt, shared by all of its waiters, with the
/// number of waiters registered; it is removed with the last one. Shared by
/// every `McpOAuthAuthority` in the process, including request-scoped ones.
type AttemptWakers = std::collections::HashMap<String, (usize, tokio::sync::watch::Sender<bool>)>;

fn attempt_wakers() -> &'static parking_lot::Mutex<AttemptWakers> {
    static WAKERS: std::sync::OnceLock<parking_lot::Mutex<AttemptWakers>> =
        std::sync::OnceLock::new();
    WAKERS.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()))
}

/// One waiter's wake-up registration for an attempt. Every waiter of the
/// attempt subscribes to the same sender, so a second registration never
/// displaces the first; dropping one removes only its own share.
struct AttemptWakeRegistration {
    key: String,
    retired: tokio::sync::watch::Receiver<bool>,
}

impl AttemptWakeRegistration {
    fn register(state: &str) -> Self {
        let key = OAuthBrowserActionRef::project(state).as_str().to_owned();
        let mut wakers = attempt_wakers().lock();
        let (waiters, sender) = wakers
            .entry(key.clone())
            .or_insert_with(|| (0, tokio::sync::watch::channel(false).0));
        *waiters += 1;
        let retired = sender.subscribe();
        Self { key, retired }
    }

    /// Resolves once a confirmed retirement of this attempt was signalled.
    async fn retired(&mut self) {
        if self.retired.wait_for(|retired| *retired).await.is_err() {
            // The sender lives while this registration does; a closed
            // channel never means retirement.
            std::future::pending::<()>().await;
        }
    }
}

impl Drop for AttemptWakeRegistration {
    fn drop(&mut self) {
        let mut wakers = attempt_wakers().lock();
        if let Some((waiters, _)) = wakers.get_mut(&self.key) {
            *waiters = waiters.saturating_sub(1);
            if *waiters == 0 {
                wakers.remove(&self.key);
            }
        }
    }
}

/// The single retirement seam for an admitted attempt: the flow owner
/// retires it (including its durable removal), and only that confirmed
/// retirement wakes the attempt's local wait. A refused or absent
/// retirement wakes nothing: an absent attempt may have been consumed.
fn retire_attempt(
    authority: &dyn OAuthFlowAuthority,
    state: &str,
    target: &meerkat_core::AuthCredentialIdentity,
    identity: OAuthBrowserFlowIdentity,
    redirect_uri: &str,
) -> Result<(), OAuthFlowError> {
    authority.expire(state, target, identity, redirect_uri)?;
    wake_attempt_waiters(state);
    Ok(())
}

/// Retire the attempt with reference `attempt_ref` that the flow owner holds
/// for `target` without its payload
/// ([`OAuthFlowAuthority::retire_unpayloaded_browser_attempts`]), and wake
/// its local waits. Returns how many attempts were retired.
fn retire_unpayloaded_attempt(
    authority: &dyn OAuthFlowAuthority,
    target: &meerkat_core::AuthCredentialIdentity,
    attempt_ref: &str,
) -> Result<usize, OAuthFlowError> {
    let retired = authority.retire_unpayloaded_browser_attempts(target, Some(attempt_ref))?;
    for retired_ref in &retired {
        wake_attempt_waiters_by_ref(retired_ref.as_str());
    }
    Ok(retired.len())
}

/// Wake every waiter of the attempt admitted under `state`. Only a confirmed
/// retirement calls it.
fn wake_attempt_waiters(state: &str) {
    wake_attempt_waiters_by_ref(OAuthBrowserActionRef::project(state).as_str());
}

/// Wake every waiter of the attempt with reference `attempt_ref`.
fn wake_attempt_waiters_by_ref(attempt_ref: &str) {
    if let Some((_, sender)) = attempt_wakers().lock().get(attempt_ref) {
        sender.send_replace(true);
    }
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

/// The one owner of this error is [`crate::auth_oauth`]; it stays nameable
/// here for existing paths.
pub use crate::auth_oauth::CredentialHttpClientUnavailable;

/// HTTP client for MCP and connector OAuth endpoints: the shared
/// redirect-free credential client. A build failure is returned, never
/// replaced by a default client.
pub(crate) fn no_redirect_client() -> Result<Client, CredentialHttpClientUnavailable> {
    crate::auth_oauth::credential_http_client()
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

/// Whether `redirect_uri` is an `https` URL: a registered redirect, never a
/// loopback one.
fn is_https_redirect(redirect_uri: &str) -> bool {
    reqwest::Url::parse(redirect_uri).is_ok_and(|url| url.scheme() == "https")
}

/// A registered redirect: an absolute `https` URL with a host and no
/// userinfo, query or fragment, already in its canonical form (so the value
/// sent, registered and compared is exactly the configured one).
fn valid_registered_redirect(redirect_uri: &str) -> bool {
    reqwest::Url::parse(redirect_uri).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.as_str() == redirect_uri
    })
}

/// The host-only handle of an attempt admitted on a registered redirect: an
/// HMAC over the slot's binding, the state and the admitted redirect under a
/// key that never leaves this process. It proves the host that started the
/// attempt completes it; a process restart invalidates it.
fn mcp_remote_attempt_handle(binding: &AuthBindingRef, state: &str, redirect_uri: &str) -> String {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let key = KEY.get_or_init(|| {
        crate::oauth_flow::OAuthFlowRegistry::new_state()
            .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
    });
    let message = format!(
        "mcp-remote-attempt-v1\0{}\0{}\0{}\0{}",
        binding.realm.as_str(),
        binding.binding.as_str(),
        state,
        redirect_uri
    );
    URL_SAFE_NO_PAD.encode(crate::connector_protocol::hmac_sha256(
        key.as_bytes(),
        message.as_bytes(),
    ))
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
        CredentialMutationError::CredentialAccess { refusal, detail } => {
            McpOAuthError::CredentialAccess { refusal, detail }
        }
        // The commit failed after it consumed the attempt and could not
        // restore the previous credential and lifecycle: what is stored is
        // known only from a read of the stored credential.
        CredentialMutationError::RollbackFailed(reason) => {
            tracing::warn!(
                mcp_server = %target.server_name(),
                %reason,
                "MCP OAuth credential commit failed and its rollback failed; the stored credential is unsettled"
            );
            McpOAuthError::CommitUnsettled {
                server_name: target.server_name().to_string(),
            }
        }
    }
}

pub(crate) fn lifecycle_snapshot_is_absent(snapshot: &AuthLeaseSnapshot) -> bool {
    snapshot.phase.is_none()
        && !snapshot.credential_present
        && snapshot.generation == 0
        && snapshot.credential_published_at_millis.is_none()
}

/// The bound subject a refresh of `tokens` re-observes through the account
/// strategy: a verified target's bound account. Every admitted credential of
/// a verified target has one.
fn subject_to_reobserve<'a>(
    target: &McpServerIdentity,
    tokens: &'a PersistedTokens,
) -> Option<&'a str> {
    tokens
        .account_id
        .as_deref()
        .filter(|_| target.account_verification() == Some(AccountVerification::Verified))
}

/// Why a refreshed credential's subject was not observed, with the egress
/// refusal of the strategy's own request when that was the cause.
struct SubjectUnobserved {
    reason: &'static str,
    egress_refused: Option<HttpEgressRefusal>,
}

/// Why unattended use of a credential is refused at once: an
/// operating-system credential call still physically holds its lifecycle
/// owner.
const LIFECYCLE_OWNER_BUSY: &str =
    "an operating-system credential call for this credential is still running";

/// Unattended refresh of a credential whose lifecycle owner an
/// operating-system credential call still physically holds: refused at once,
/// never waited for.
fn lifecycle_owner_busy() -> RefreshError {
    RefreshError::CredentialAccess {
        refusal: meerkat_core::auth::TokenStoreRefusal::Pending,
        detail: LIFECYCLE_OWNER_BUSY.into(),
    }
}

/// The same refusal for an unattended use of the stored credential.
fn mcp_owner_busy() -> McpOAuthError {
    McpOAuthError::CredentialAccess {
        refusal: meerkat_core::auth::TokenStoreRefusal::Pending,
        detail: LIFECYCLE_OWNER_BUSY.into(),
    }
}

/// A status rehydration error, keeping a refused credential access typed.
fn mcp_rehydrate_error(
    target: &McpServerIdentity,
    error: meerkat_core::AuthStatusRehydrateError,
) -> McpOAuthError {
    let lifecycle = |reason: String| McpOAuthError::AuthLifecycle {
        server_name: target.server_name().to_string(),
        reason,
    };
    match error {
        meerkat_core::AuthStatusRehydrateError::TokenStore(store) => {
            match store.credential_access() {
                Some((refusal, detail)) => McpOAuthError::CredentialAccess { refusal, detail },
                None => {
                    lifecycle(meerkat_core::AuthStatusRehydrateError::TokenStore(store).to_string())
                }
            }
        }
        meerkat_core::AuthStatusRehydrateError::MarkerSave(store) => {
            match store.credential_access() {
                Some((refusal, detail)) => McpOAuthError::CredentialAccess { refusal, detail },
                None => {
                    lifecycle(meerkat_core::AuthStatusRehydrateError::MarkerSave(store).to_string())
                }
            }
        }
        other => lifecycle(other.to_string()),
    }
}

/// A token-store error, keeping a refused or failed credential access typed.
fn mcp_store_error(error: crate::auth_store::TokenStoreError) -> McpOAuthError {
    match error.credential_access() {
        Some((refusal, detail)) => McpOAuthError::CredentialAccess { refusal, detail },
        None => McpOAuthError::TokenStore(error.to_string()),
    }
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
        McpOAuthError::CredentialAccess { refusal, detail } => {
            RefreshError::CredentialAccess { refusal, detail }
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
        return McpOAuthError::ReauthRequired {
            server_name: target.server_name().to_string(),
        };
    }
    match &error {
        // A destination the host egress profile refused: the same typed
        // local refusal for every reader of the coordinated result.
        RefreshError::EgressRefused { refusal, .. } => McpOAuthError::EgressRefused {
            server_name: target.server_name().to_string(),
            refusal: *refusal,
        },
        // A failure the token endpoint reported: an upstream failure.
        RefreshError::Classified { .. } | RefreshError::Observed { .. } => {
            McpOAuthError::RefreshFailed {
                server_name: target.server_name().to_string(),
                reason: error.to_string(),
            }
        }
        RefreshError::RequiredScopesNotGranted => ConnectorOAuthRefusal::MissingScopes.into(),
        // A changed capture: typed stale outcome, never a refusal.
        RefreshError::StalePreparation => McpOAuthError::StalePreparation,
        // A local lifecycle, lock or closure failure: infrastructure.
        RefreshError::Refresh(_)
        | RefreshError::Cancelled
        | RefreshError::LockFailed(_)
        | RefreshError::CredentialIdentityMismatch
        | RefreshError::ReauthRequired(_)
        | RefreshError::DurableTerminalCommit { .. } => McpOAuthError::AuthLifecycle {
            server_name: target.server_name().to_string(),
            reason: error.to_string(),
        },
        // The credential store refused access: not a refresh failure.
        RefreshError::CredentialAccess { refusal, detail } => McpOAuthError::CredentialAccess {
            refusal: *refusal,
            detail: detail.clone(),
        },
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
    #[serde(default)]
    authorization_response_iss_parameter_supported: bool,
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
            egress: HttpEgressProfile::trusted_host(),
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

    /// Each credential-store refusal, and a failed backend, keeps its type
    /// through the admitted load, the refresh round trip and the login exit.
    #[test]
    fn credential_access_refusals_keep_their_type_through_every_conversion() {
        use crate::auth_store::TokenStoreError;
        use meerkat_core::auth::TokenStoreRefusal as Refusal;
        let target = McpServerIdentity::from_server_config("srv", "https://mcp.example/api");
        let refused = |refusal| {
            TokenStoreError::refused(
                "keyring",
                TokenStoreAccess::Unattended,
                refusal,
                "the platform cause",
            )
        };
        for (error, refusal) in [
            (refused(Refusal::Denied), Refusal::Denied),
            (
                refused(Refusal::InteractionRequired),
                Refusal::InteractionRequired,
            ),
            (refused(Refusal::Pending), Refusal::Pending),
            (refused(Refusal::Unavailable), Refusal::Unavailable),
            (refused(Refusal::NotObserved), Refusal::NotObserved),
            (refused(Refusal::Unsupported), Refusal::Unsupported),
            (
                TokenStoreError::BackendFailed {
                    backend: "keyring",
                    detail: "the platform cause".into(),
                },
                Refusal::Unavailable,
            ),
        ] {
            let loaded = mcp_store_error(error);
            assert!(
                matches!(
                    &loaded,
                    McpOAuthError::CredentialAccess { refusal: kept, detail }
                        if *kept == refusal && detail == "the platform cause"
                ),
                "admitted load: {loaded:?}"
            );
            assert!(!loaded.is_refusal(), "not a refused request");
            let refreshed = refresh_error_from_mcp(loaded);
            assert!(
                matches!(&refreshed, RefreshError::CredentialAccess { refusal: kept, .. } if *kept == refusal),
                "refresh: {refreshed:?}"
            );
            let restored = map_coordinated_refresh_error(&target, refreshed);
            assert!(
                matches!(&restored, McpOAuthError::CredentialAccess { refusal: kept, .. } if *kept == refusal),
                "refresh exit: {restored:?}"
            );
            let login = map_coordinated_login_error(
                &target,
                CredentialMutationError::CredentialAccess {
                    refusal,
                    detail: "the platform cause".into(),
                },
            );
            assert!(
                matches!(&login, McpOAuthError::CredentialAccess { refusal: kept, .. } if *kept == refusal),
                "login exit: {login:?}"
            );
        }
    }

    #[test]
    fn coordinated_refresh_mapping_splits_upstream_reports_from_local_failures() {
        let target = McpServerIdentity::from_server_config("srv", "https://mcp.example/api");
        for local in [
            RefreshError::Refresh("refresh_failed closure rejected".into()),
            RefreshError::LockFailed("lock".into()),
            RefreshError::Cancelled,
        ] {
            let error = map_coordinated_refresh_error(&target, local);
            assert!(
                matches!(error, McpOAuthError::AuthLifecycle { .. }),
                "{error:?}"
            );
            assert!(!error.is_refusal());
        }
        assert!(matches!(
            map_coordinated_refresh_error(&target, RefreshError::CredentialIdentityMismatch),
            McpOAuthError::Verification(ConnectorOAuthRefusal::AccountMismatch)
        ));
        assert!(matches!(
            map_coordinated_refresh_error(&target, RefreshError::RequiredScopesNotGranted),
            McpOAuthError::Verification(ConnectorOAuthRefusal::MissingScopes)
        ));
    }

    #[test]
    fn flow_owner_failures_inside_mcp_errors_are_not_refusals() {
        for error in [
            OAuthFlowError::PersistenceFailed {
                operation: "admit_oauth_browser_flow",
                detail: "disk".into(),
            },
            OAuthFlowError::LifecycleRejected {
                operation: "admit_oauth_browser_flow",
                detail: "rejected".into(),
            },
            OAuthFlowError::StateGenerationFailed,
            OAuthFlowError::RegistryProjectionMissing {
                operation: "verify",
            },
        ] {
            assert!(!McpOAuthError::Flow(error).is_refusal());
        }
        assert!(McpOAuthError::Flow(OAuthFlowError::Missing).is_refusal());
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
        let a = McpServerIdentity::from_server_config(
            "glean",
            "https://tenant.glean.example/mcp/default",
        )
        .token_key()
        .unwrap();
        let b = McpServerIdentity::from_server_config(
            "glean",
            "https://tenant.glean.example/mcp/default",
        )
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
        let identity = McpServerIdentity::from_server_config(
            "glean",
            "https://tenant.glean.example/mcp/default",
        );
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

    #[cfg(feature = "workspace-tests")]
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

    fn realm(id: &str) -> RealmId {
        RealmId::parse(id).unwrap()
    }

    /// The same server in two realms, and realm-less, has three distinct
    /// credential slots and leases; the realm tag cannot collide with an
    /// account selection.
    #[test]
    fn each_realm_has_its_own_mcp_credential_slot() {
        let base = McpServerIdentity::from_server_config("slack", "https://mcp.example/slack");
        let in_a = base.clone().in_realm(realm("alpha"));
        let in_b = base.clone().in_realm(realm("beta"));
        let keys = [&base, &in_a, &in_b].map(|target| target.token_key().unwrap());
        assert_ne!(keys[0], keys[1]);
        assert_ne!(keys[0], keys[2]);
        assert_ne!(keys[1], keys[2]);
        for target in [&base, &in_a, &in_b] {
            assert_eq!(
                target.token_key().unwrap().binding().unwrap().as_str(),
                target.lease_key().unwrap().binding().unwrap().as_str()
            );
        }
        assert_eq!(
            in_a.token_key().unwrap(),
            base.clone().in_realm(realm("alpha")).token_key().unwrap()
        );
        assert!(in_a.binding_slug().starts_with("slack-"));
        // A realm whose name is a selection tag stays distinct from it.
        let known = base.clone().with_expected_account("discover").unwrap();
        let tagged = base.clone().in_realm(realm("discover"));
        assert_ne!(known.token_key().unwrap(), tagged.token_key().unwrap());
        assert_ne!(in_a, base);
    }

    /// A target scoped for a realm host binds to that realm, keeps its own
    /// realm, and is refused across realms.
    #[test]
    fn targets_scope_to_the_host_realm_and_never_cross_it() {
        let base = McpServerIdentity::from_server_config("slack", "https://mcp.example/slack");
        assert_eq!(base.scoped_to(None).unwrap(), base);
        let alpha = realm("alpha");
        let bound = base.scoped_to(Some(&alpha)).unwrap();
        assert_eq!(bound.realm(), Some(&alpha));
        assert_eq!(bound.scoped_to(Some(&alpha)).unwrap(), bound);
        assert!(matches!(
            bound.scoped_to(Some(&realm("beta"))),
            Err(McpOAuthError::RealmMismatch { .. })
        ));
    }

    /// A realm host resolves only its realm's credential: one stored in
    /// another realm, or in the realm-less pre-realm slot, is not used (the
    /// realm needs its own login); its logout leaves both of them in place.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_realm_host_uses_and_clears_only_its_own_mcp_credential() {
        let (base, _state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let auth = test_auth_lease();
        let shared = ce_refresh_authority(store.clone(), auth.clone());
        let target = McpServerIdentity::from_server_config("slack", format!("{base}/mcp"));
        let in_beta = target.clone().in_realm(realm("beta"));
        ce_seed_stored_credential(&shared, &target, &base).await;
        ce_seed_stored_credential(&shared, &in_beta, &base).await;

        let alpha = shared.clone().in_realm(realm("alpha"));
        let beta = shared.clone().in_realm(realm("beta"));
        assert_eq!(
            alpha.stored_bearer_token(&target).await.unwrap(),
            None,
            "alpha has no credential of its own: a fresh realm login is required"
        );
        assert_eq!(
            beta.stored_bearer_token(&target).await.unwrap().as_deref(),
            Some("access-token")
        );
        assert!(matches!(
            alpha.stored_bearer_token(&in_beta).await,
            Err(McpOAuthError::RealmMismatch { .. })
        ));

        alpha.logout(&target).await.unwrap();
        assert!(
            store
                .load(&target.token_key().unwrap())
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .load(&in_beta.token_key().unwrap())
                .await
                .unwrap()
                .is_some()
        );
        beta.logout(&target).await.unwrap();
        assert!(
            store
                .load(&in_beta.token_key().unwrap())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .load(&target.token_key().unwrap())
                .await
                .unwrap()
                .is_some(),
            "a realm logout never deletes the pre-realm shared credential"
        );
    }

    struct CeRefreshRelease(Arc<TestState>);
    impl Drop for CeRefreshRelease {
        fn drop(&mut self) {
            self.0.pause_refresh.store(false, Ordering::SeqCst);
            self.0.refresh_release.notify_one();
        }
    }

    #[cfg(feature = "workspace-tests")]
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

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn ce_mcp_refresh_save_refusal_survives_successful_rollback() {
        use crate::auth_store::TokenStoreError;
        use meerkat_core::auth::TokenStoreRefusal;
        use std::sync::atomic::AtomicUsize;

        struct FailNextSaveStore {
            inner: EphemeralTokenStore,
            fail_next_save: AtomicBool,
            save_attempts: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl TokenStore for FailNextSaveStore {
            async fn load(
                &self,
                key: &TokenKey,
            ) -> Result<Option<PersistedTokens>, TokenStoreError> {
                self.inner.load(key).await
            }

            async fn save(
                &self,
                key: &TokenKey,
                tokens: &PersistedTokens,
            ) -> Result<(), TokenStoreError> {
                self.save_attempts.fetch_add(1, Ordering::SeqCst);
                if self.fail_next_save.swap(false, Ordering::SeqCst) {
                    return Err(TokenStoreError::refused(
                        self.backend_name(),
                        TokenStoreAccess::Unattended,
                        TokenStoreRefusal::InteractionRequired,
                        "post-exchange save needs credential interaction",
                    ));
                }
                self.inner.save(key, tokens).await
            }

            async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
                self.inner.clear(key).await
            }

            async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
                self.inner.list().await
            }

            fn backend_name(&self) -> &'static str {
                "test-refresh-store"
            }
        }

        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(FailNextSaveStore {
            inner: EphemeralTokenStore::new(),
            fail_next_save: AtomicBool::new(false),
            save_attempts: AtomicUsize::new(0),
        });
        let auth = test_auth_lease();
        let authority = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            Client::new(),
            auth.clone(),
        );
        let target =
            McpServerIdentity::from_server_config("ce-mcp-save-refusal", format!("{base}/mcp"));
        ce_seed_stored_credential(&authority, &target, &base).await;
        let previous = republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.primary_secret = Some("previous-access-token".into());
            tokens.refresh_token = Some("previous-refresh-token".into());
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        let key = target.token_key().unwrap();
        let lease = target.lease_key().unwrap();
        // Initial and post-exchange loads succeed. Only the publication save
        // fails; the existing rollback can restore the prior credential.
        store.save_attempts.store(0, Ordering::SeqCst);
        store.fail_next_save.store(true, Ordering::SeqCst);
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            authority.stored_bearer_token(&target),
        )
        .await
        .expect("the mock refresh must finish")
        .expect_err("a refused publication must not yield the refreshed bearer");

        assert_eq!(state.token_requests.lock().len(), 1, "refresh reached HTTP");
        assert_eq!(
            store.save_attempts.load(Ordering::SeqCst),
            2,
            "one refused publication and one successful rollback save"
        );
        let restored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(restored.primary_secret, previous.primary_secret);
        assert_eq!(restored.refresh_token, previous.refresh_token);
        assert_eq!(restored.expires_at, previous.expires_at);
        let snapshot = auth.snapshot(&lease);
        assert!(snapshot.credential_present);
        assert_ne!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Refreshing),
            "successful rollback must close this refresh"
        );
        assert_eq!(
            durable_marker::marker_relation_for_tokens_and_snapshot(&restored, &snapshot, &key),
            durable_marker::AuthLeaseDurableMarkerRelation::Matches
        );
        assert!(
            matches!(
                &error,
                McpOAuthError::CredentialAccess {
                    refusal: TokenStoreRefusal::InteractionRequired,
                    detail,
                } if detail == "post-exchange save needs credential interaction"
            ),
            "successful rollback must preserve the original typed store cause: {error:?}"
        );
        assert!(
            !error.is_refusal(),
            "credential access is not a request denial"
        );
    }

    /// Refuses exactly the `refuse_load`-th read (counted from one; zero
    /// refuses none) with an interaction requirement carrying `detail`. Every
    /// other operation reaches `inner`.
    struct RefuseNthLoadStore {
        inner: EphemeralTokenStore,
        loads: std::sync::atomic::AtomicUsize,
        refuse_load: std::sync::atomic::AtomicUsize,
        detail: &'static str,
    }

    impl RefuseNthLoadStore {
        fn new(detail: &'static str) -> Self {
            Self {
                inner: EphemeralTokenStore::new(),
                loads: std::sync::atomic::AtomicUsize::new(0),
                refuse_load: std::sync::atomic::AtomicUsize::new(0),
                detail,
            }
        }
    }

    #[async_trait::async_trait]
    impl TokenStore for RefuseNthLoadStore {
        async fn load(
            &self,
            key: &TokenKey,
        ) -> Result<Option<PersistedTokens>, crate::auth_store::TokenStoreError> {
            let number = self.loads.fetch_add(1, Ordering::SeqCst) + 1;
            if self.refuse_load.load(Ordering::SeqCst) == number {
                return Err(crate::auth_store::TokenStoreError::refused(
                    self.backend_name(),
                    TokenStoreAccess::Unattended,
                    meerkat_core::auth::TokenStoreRefusal::InteractionRequired,
                    self.detail,
                ));
            }
            self.inner.load(key).await
        }

        async fn save(
            &self,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), crate::auth_store::TokenStoreError> {
            self.inner.save(key, tokens).await
        }

        async fn clear(&self, key: &TokenKey) -> Result<(), crate::auth_store::TokenStoreError> {
            self.inner.clear(key).await
        }

        async fn list(&self) -> Result<Vec<TokenKey>, crate::auth_store::TokenStoreError> {
            self.inner.list().await
        }

        fn backend_name(&self) -> &'static str {
            "test-refuse-load-store"
        }
    }

    /// A refused read while a fresh owner rehydrates the stored credential
    /// keeps the typed store cause through the real admission.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn ce_mcp_refused_rehydration_read_keeps_its_typed_cause() {
        use meerkat_core::auth::TokenStoreRefusal;

        let (base, _state) = spawn_oauth_fixture().await;
        let store = Arc::new(RefuseNthLoadStore::new(
            "rehydration read needs credential interaction",
        ));
        let persistence =
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new()));
        let seeding =
            McpOAuthAuthority::with_http(persistence.clone(), Client::new(), test_auth_lease());
        let target = McpServerIdentity::from_server_config(
            "ce-mcp-rehydrate-refusal",
            format!("{base}/mcp"),
        );
        ce_seed_stored_credential(&seeding, &target, &base).await;
        // A fresh owner has no lifecycle: its admission reads the stored
        // credential, then rehydrates it with a second read, which is refused.
        let fresh = McpOAuthAuthority::with_http(persistence, Client::new(), test_auth_lease());
        store.loads.store(0, Ordering::SeqCst);
        store.refuse_load.store(2, Ordering::SeqCst);
        let error =
            tokio::time::timeout(Duration::from_secs(10), fresh.stored_bearer_token(&target))
                .await
                .expect("the admission finishes")
                .expect_err("a refused rehydration yields no bearer");
        assert_eq!(
            store.loads.load(Ordering::SeqCst),
            2,
            "the admitted read, then the refused rehydration read"
        );
        assert!(
            matches!(
                &error,
                McpOAuthError::CredentialAccess {
                    refusal: TokenStoreRefusal::InteractionRequired,
                    detail,
                } if detail == "rehydration read needs credential interaction"
            ),
            "the rehydration keeps the typed store cause: {error:?}"
        );
    }

    /// A refused durable reread after the refresh exchange closes the refresh
    /// this owner began instead of leaving AuthMachine in `Refreshing`. The
    /// stored credential and its exact bytes survive, so a later use through
    /// the same authority refreshes again once the store answers.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn ce_mcp_refused_post_exchange_reread_closes_the_begun_refresh() {
        use meerkat_core::auth::TokenStoreRefusal;

        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(RefuseNthLoadStore::new(
            "post-exchange reread needs credential interaction",
        ));
        let auth = test_auth_lease();
        let authority = McpOAuthAuthority::with_http(
            ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new())),
            Client::new(),
            auth.clone(),
        );
        let target =
            McpServerIdentity::from_server_config("ce-mcp-reread-refusal", format!("{base}/mcp"));
        ce_seed_stored_credential(&authority, &target, &base).await;
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        let key = target.token_key().unwrap();
        let lease = target.lease_key().unwrap();
        // The exact stored bytes, read beside the refusal counter.
        let original = store.inner.load(&key).await.unwrap();
        let refresh_exchanges = || {
            state
                .token_requests
                .lock()
                .iter()
                .filter(|request| request["grant_type"] == "refresh_token")
                .count()
        };

        state.pause_refresh.store(true, Ordering::SeqCst);
        let release = CeRefreshRelease(state.clone());
        let child = authority.clone();
        let child_target = target.clone();
        let mut task = tokio::spawn(async move { child.stored_bearer_token(&child_target).await });
        let entered =
            tokio::time::timeout(Duration::from_secs(10), state.refresh_started.notified()).await;
        // Every read before the exchange has happened: the next one is the
        // post-exchange reread of the durable credential.
        store
            .refuse_load
            .store(store.loads.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
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
        let error = finished
            .expect("the refused reread must finish")
            .expect("the refresh task must not panic")
            .expect_err("a refused reread yields no bearer");

        // Immediately after the return and before any retry: the begun
        // refresh is closed and the credential is retained.
        let snapshot = auth.snapshot(&lease);
        assert_ne!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Refreshing),
            "a refused post-exchange reread must close the refresh it began"
        );
        assert!(snapshot.credential_present, "the credential is retained");
        assert!(
            matches!(
                &error,
                McpOAuthError::CredentialAccess {
                    refusal: TokenStoreRefusal::InteractionRequired,
                    detail,
                } if detail == "post-exchange reread needs credential interaction"
            ),
            "the refused reread keeps the typed store cause: {error:?}"
        );
        assert_eq!(refresh_exchanges(), 1, "exactly one exchange");
        assert_eq!(
            store.inner.load(&key).await.unwrap(),
            original,
            "a refused reread publishes nothing"
        );
        assert!(
            meerkat_core::try_acquire_auth_login_lifecycle_guard(&lease).is_some(),
            "the lifecycle guard is obtainable after the refusal"
        );

        // The store answers again: a retry through the same authority
        // refreshes once more and yields the bearer.
        store.refuse_load.store(0, Ordering::SeqCst);
        let retried = tokio::time::timeout(
            Duration::from_secs(10),
            authority.stored_bearer_token(&target),
        )
        .await
        .expect("the retry must finish")
        .expect("the retry yields the bearer");
        assert_eq!(retried.as_deref(), Some("access-token"));
        assert_eq!(refresh_exchanges(), 2, "exactly one additional exchange");
    }

    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn ce_mcp_stale_success_and_failure_preserve_replacement() {
        let mut observations = Vec::new();
        // A replacement owner moves both the bytes and the lifecycle. A
        // token-only drift moves only the durable bytes, outside the protocol,
        // while the lifecycle this owner began stays current.
        for token_only in [false, true] {
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
                let replacement = if token_only {
                    replacement
                } else {
                    let transition = auth
                        .acquire_lease(
                            &lease,
                            meerkat_core::persisted_token_expires_at_epoch_secs(&replacement),
                        )
                        .unwrap();
                    meerkat_core::mark_tokens_lifecycle_published_for_transition(
                        &key,
                        &replacement,
                        &transition,
                    )
                    .unwrap()
                };
                store.save(&key, &replacement).await.unwrap();
                if !token_only {
                    auth.begin_refresh(&lease).unwrap();
                }
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
                // Immediately after the return, before any retry.
                let actual = auth.snapshot(&lease);
                let count = state
                    .token_requests
                    .lock()
                    .iter()
                    .filter(|r| r["grant_type"] == "refresh_token")
                    .count();
                observations.push((
                    token_only,
                    refused,
                    expected,
                    actual,
                    replacement,
                    store.load(&key).await.unwrap(),
                    count,
                ));
            }
        }
        for (token_only, refused, expected, actual, replacement, stored, count) in observations {
            assert!(refused, "stale result must not yield a bearer credential");
            assert_eq!(count, 1);
            if token_only {
                // Only the unchanged refresh this owner began closes, and only
                // transiently: neither result is applied to the drifted bytes.
                assert_eq!(
                    expected.phase,
                    Some(meerkat_core::handles::AuthLeasePhase::Refreshing)
                );
                assert_eq!(
                    actual.phase,
                    Some(meerkat_core::handles::AuthLeasePhase::Expiring),
                    "token-only drift must close the refresh it began, transiently"
                );
                assert!(actual.credential_present);
            } else {
                assert_eq!(
                    actual, expected,
                    "stale HTTP may not fail the replacement's refresh owner"
                );
            }
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

    #[cfg(feature = "workspace-tests")]
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

    #[cfg(feature = "workspace-tests")]
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

    #[cfg(feature = "workspace-tests")]
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

    /// A loopback wait and the completion it entered both register for the
    /// same attempt: one confirmed retirement wakes both, and one waiter's
    /// departure never takes the other's registration with it.
    #[tokio::test]
    async fn a_confirmed_retirement_wakes_every_waiter_of_the_attempt() {
        let bound = Duration::from_secs(5);
        let mut wait = AttemptWakeRegistration::register("fan-out-state");
        let mut completion = AttemptWakeRegistration::register("fan-out-state");
        wake_attempt_waiters("fan-out-state");
        tokio::time::timeout(bound, wait.retired())
            .await
            .expect("the wait is woken");
        tokio::time::timeout(bound, completion.retired())
            .await
            .expect("the completion is woken");

        let mut remaining = AttemptWakeRegistration::register("fan-out-later");
        drop(AttemptWakeRegistration::register("fan-out-later"));
        wake_attempt_waiters("fan-out-later");
        tokio::time::timeout(bound, remaining.retired())
            .await
            .expect("the remaining waiter is still registered");
        drop(remaining);
        let key = OAuthBrowserActionRef::project("fan-out-later")
            .as_str()
            .to_owned();
        assert!(
            !attempt_wakers().lock().contains_key(&key),
            "the last waiter removes the entry"
        );
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

    fn egress_origin(text: &str) -> crate::http_egress::HttpEgressOrigin {
        crate::http_egress::HttpEgressOrigin::parse(text).unwrap()
    }

    /// A required egress profile refuses a refresh token endpoint outside it,
    /// typed and before any request reaches the endpoint; the same credential
    /// refreshes through the profile's own client once its endpoint is
    /// deliberately admitted.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_required_egress_profile_refuses_an_unadmitted_refresh_endpoint_unsent() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let auth = test_auth_lease();
        let authority = ce_refresh_authority(store.clone(), auth.clone());
        let target = McpServerIdentity::from_server_config("egress-refresh", format!("{base}/mcp"));
        ce_seed_stored_credential(&authority, &target, &base).await;
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;

        let profile = HttpEgressProfile::required(
            crate::http_egress::HttpEgressPolicy::new()
                .allow_public(egress_origin("https://mcp.example.com")),
        );
        let bound = authority.clone().with_http_egress(&profile);
        assert!(
            bound
                .bound_http_egress()
                .is_some_and(|bound| bound.same_profile(&profile))
        );
        let result = bound.stored_bearer_token(&target).await;
        assert!(
            matches!(
                result,
                Err(McpOAuthError::EgressRefused {
                    refusal: HttpEgressRefusal::HostNotAdmitted,
                    ..
                })
            ),
            "{result:?}"
        );
        assert!(
            state.token_requests.lock().is_empty(),
            "nothing reached the token endpoint"
        );

        let admitted = HttpEgressProfile::required(
            crate::http_egress::HttpEgressPolicy::new()
                .allow_local(egress_origin(&base), ["127.0.0.1".parse().unwrap()]),
        );
        let refreshed = authority
            .with_http_egress(&admitted)
            .stored_bearer_token(&target)
            .await
            .unwrap();
        assert_eq!(refreshed.as_deref(), Some("access-token"));
        assert_eq!(state.token_requests.lock().len(), 1);
    }

    /// Discovery is admitted URL by URL: a protected resource outside the
    /// profile is refused typed before any well-known request is sent.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_required_egress_profile_refuses_discovery_outside_it_before_any_arrival() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let arrivals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&arrivals);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });
        let profile =
            HttpEgressProfile::required(crate::http_egress::HttpEgressPolicy::new().allow_local(
                egress_origin(&format!("http://127.0.0.1:{}", port.wrapping_add(1))),
                ["127.0.0.1".parse().unwrap()],
            ));
        let authority =
            ce_refresh_authority(Arc::new(EphemeralTokenStore::new()), test_auth_lease())
                .with_http_egress(&profile);
        let target = McpServerIdentity::from_server_config(
            "egress-discovery",
            format!("http://127.0.0.1:{port}/mcp"),
        );
        let result = authority
            .discover(
                &target,
                None,
                tokio::time::Instant::now() + MCP_OAUTH_START_BOUND,
            )
            .await;
        assert!(
            matches!(
                result,
                Err(McpOAuthError::EgressRefused {
                    refusal: HttpEgressRefusal::PortNotAdmitted,
                    ..
                })
            ),
            "{:?}",
            result.as_ref().err()
        );
        assert_eq!(arrivals.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_native_account_strategy_reports_only_its_own_required_profile() {
        let profile = HttpEgressProfile::required(crate::http_egress::HttpEgressPolicy::new());
        assert!(OidcUserInfoAccountStrategy::new().http_egress().is_none());
        assert!(
            OidcUserInfoAccountStrategy::new()
                .with_http_egress(&HttpEgressProfile::trusted_host())
                .http_egress()
                .is_none()
        );
        assert!(
            OidcUserInfoAccountStrategy::new()
                .with_http_egress(&profile)
                .http_egress()
                .is_some_and(|bound| bound.same_profile(&profile))
        );
    }

    /// The token endpoint's own resolution is outside the profile: the
    /// refresh is refused typed through the coordinator, nothing reaches the
    /// endpoint, and the credential is never invalidated; deliberate local
    /// authority then refreshes the same credential.
    #[cfg(feature = "workspace-tests")]
    #[tokio::test]
    async fn a_resolver_refused_refresh_is_typed_and_never_invalidates_the_credential() {
        let (base, state) = spawn_oauth_fixture().await;
        let port = reqwest::Url::parse(&base).unwrap().port().unwrap();
        let named = format!("http://localhost:{port}");
        let store = Arc::new(EphemeralTokenStore::new());
        let auth = test_auth_lease();
        let authority = ce_refresh_authority(store.clone(), auth.clone());
        let target =
            McpServerIdentity::from_server_config("egress-dns-refresh", format!("{named}/mcp"));
        ce_seed_stored_credential(&authority, &target, &named).await;
        republish_stored_tokens(&authority, store.as_ref(), &target, |tokens| {
            tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        })
        .await;
        let key = target.token_key().unwrap();
        let before = store.load(&key).await.unwrap();
        assert!(before.is_some());

        let public_only = HttpEgressProfile::required(
            crate::http_egress::HttpEgressPolicy::new().allow_public(egress_origin(&named)),
        );
        let result = authority
            .clone()
            .with_http_egress(&public_only)
            .stored_bearer_token(&target)
            .await;
        assert!(
            matches!(
                result,
                Err(McpOAuthError::EgressRefused {
                    refusal: HttpEgressRefusal::AddressNotAdmitted,
                    ..
                })
            ),
            "{result:?}"
        );
        assert!(state.token_requests.lock().is_empty());
        assert_eq!(
            store.load(&key).await.unwrap(),
            before,
            "a local egress refusal never invalidates or rewrites the credential"
        );

        let deliberate =
            HttpEgressProfile::required(crate::http_egress::HttpEgressPolicy::new().allow_local(
                egress_origin(&named),
                ["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()],
            ));
        let refreshed = authority
            .with_http_egress(&deliberate)
            .stored_bearer_token(&target)
            .await
            .unwrap();
        assert_eq!(refreshed.as_deref(), Some("access-token"));
        assert_eq!(state.token_requests.lock().len(), 1);
    }

    /// The native account strategy's UserInfo discovery is admitted by the
    /// profile: an endpoint outside it, or one whose resolution is refused,
    /// is a typed egress refusal (never evidence about the account), and
    /// nothing reaches it.
    #[tokio::test]
    async fn userinfo_verification_refusals_are_typed_and_unsent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let arrivals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&arrivals);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
        });
        let elsewhere = OidcUserInfoAccountStrategy::new().with_http_egress(
            &HttpEgressProfile::required(crate::http_egress::HttpEgressPolicy::new().allow_local(
                egress_origin(&format!("http://127.0.0.1:{}", port.wrapping_add(1))),
                ["127.0.0.1".parse().unwrap()],
            )),
        );
        assert_eq!(
            elsewhere
                .preflight(&format!("http://127.0.0.1:{port}"))
                .await,
            Err(ConnectorOAuthRefusal::EgressRefused(
                HttpEgressRefusal::PortNotAdmitted
            ))
        );
        let named = format!("http://localhost:{port}");
        let resolved =
            OidcUserInfoAccountStrategy::new().with_http_egress(&HttpEgressProfile::required(
                crate::http_egress::HttpEgressPolicy::new().allow_public(egress_origin(&named)),
            ));
        assert_eq!(
            resolved.preflight(&named).await,
            Err(ConnectorOAuthRefusal::EgressRefused(
                HttpEgressRefusal::AddressNotAdmitted
            ))
        );
        assert_eq!(arrivals.load(Ordering::SeqCst), 0);
    }
}

//! Generic connector OAuth login owner.
//!
//! A trusted host names a credential slot (a realm-scoped storage address)
//! and the connector's descriptor facts: issuer, client, resource, scopes,
//! strategy and account selection. This module discovers the issuer's
//! endpoints, admits the PKCE (and, for ID-token strategies, OIDC nonce)
//! attempt through the AuthMachine-owned flow authority, exchanges the code,
//! has the strategy observe the provider-verified account, and commits
//! through the canonical browser-flow commit. The slot keeps the verified
//! account bound to the stored credential; status projects the slot and the
//! account as separate facts.
//!
//! Hosts never write the store: every publication goes through
//! [`save_oauth_tokens_and_consume_browser_flow`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::auth_oauth::{
    OAuthEndpoints, OAuthTokenRequestFormat, OAuthTokenResult, PkcePair,
    exchange_authorization_code_with_state, exchange_refresh_token, oauth_refresh_observation,
};
use crate::auth_store::{
    CredentialMutationError, CredentialSlotRefusal, PersistedAuthMode, PersistedTokens,
    ProviderAuthPersistence, RefreshError, TokenKey, TokenStoreAccess,
};
use crate::connector_oauth::{
    AccountSelection, ConnectorAccountObservation, ConnectorCredentialMetadata,
    ConnectorOAuthDescriptor, ConnectorOAuthParameters, ConnectorOAuthRefusal,
    ConnectorProtocolEvidence, ScopeEvidence,
};
use crate::connector_protocol::{
    ConnectorCredentialRejection, ConnectorProtocol, ConnectorProtocolFailure,
    ConnectorProtocolGrant, ConnectorProtocols, ConnectorTokenGrantKind, RefreshTokenRule,
    post_token_form,
};
use crate::mcp_oauth::{
    OidcUserInfoAccountStrategy, RefuseRedirect, absolutize_url, epoch_secs, is_loopback_url,
    lifecycle_snapshot_is_absent, no_redirect_client,
};
use crate::oauth_flow::{OAuthBrowserFlowIdentity, OAuthFlowAuthority, OAuthFlowError};
use crate::{BrowserOAuthFlowCommit, save_oauth_tokens_and_consume_browser_flow};
use meerkat_core::AuthCredentialIdentity;
use meerkat_core::auth::RefreshFailureDisposition;
use meerkat_core::connection::CredentialAccountRef;
use meerkat_core::generated::auth_lease_durable_lifecycle_marker as durable_marker;
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, AuthLeaseRestoreSnapshot, CredentialUseDisposition,
    CredentialUseIntent, GeneratedAuthLeaseHandle, LeaseKey,
};

/// Loopback path conventionally used by host listeners for connector
/// callbacks.
pub const CONNECTOR_OAUTH_CALLBACK_PATH: &str = "/connector/oauth/callback";

/// Host-declared connector login target.
///
/// `slot` is where the credential is stored: a realm-scoped address chosen
/// by the trusted host, not proof of any provider account. The remaining
/// fields are the descriptor facts the attempt is admitted with.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectorOAuthTarget {
    pub slot: CredentialAccountRef,
    pub issuer: String,
    pub client: String,
    pub resource: String,
    pub scopes: BTreeSet<String>,
    pub strategy_id: String,
    pub account: AccountSelection,
    /// The host-installed [`ConnectorClientAuthProfile`] this login uses, by
    /// id. `None` keeps the default public-client request forms.
    pub client_auth_profile: Option<String>,
}

impl std::fmt::Debug for ConnectorOAuthTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorOAuthTarget")
            .field("slot", &self.slot)
            .field("strategy_id", &self.strategy_id)
            .field("account", &self.account)
            .field("client_auth_profile", &self.client_auth_profile)
            .finish_non_exhaustive()
    }
}

/// Client authentication for one connector OAuth client, installed by the
/// trusted host from its own configuration (for example a desktop-app
/// client whose provider requires a client secret and offline access).
///
/// A login binds the profile by `id` into its admitted descriptor; the
/// completion uses the profile named by that descriptor, and every refresh
/// of the resulting credential uses the profile named in its stored
/// metadata. The secret stays in host configuration: it is never stored
/// with the credential, put in a descriptor, logged or sent on the wire.
#[derive(Clone)]
pub struct ConnectorClientAuthProfile {
    pub id: String,
    /// The profile applies only to this issuer and client id.
    pub issuer: String,
    pub client: String,
    /// Sent as `client_secret` on the code exchange and on every refresh.
    pub client_secret: Option<ConnectorClientSecret>,
    /// Extra authorize-request parameters (for example `access_type`,
    /// `prompt`). Parameters the login itself owns are refused.
    pub authorize_params: Vec<(String, String)>,
    /// Whether to send the RFC 8707 `resource` parameter on authorize,
    /// code exchange and refresh.
    pub resource_indicator: bool,
    /// The installed [`ConnectorProtocol`] this client speaks, by id. `None`
    /// is the default RFC 6749 protocol with issuer discovery.
    pub protocol: Option<String>,
    /// The exact `https` redirect URIs registered for this OAuth client (for
    /// example a web-application client whose callback is the host's own
    /// authenticated endpoint), from host configuration. A login may use one
    /// of these instead of a loopback redirect; its completion then requires
    /// the host's retained attempt handle
    /// ([`ConnectorLoginStart::host_attempt_handle`]).
    pub registered_redirect_uris: Vec<String>,
}

/// Where a connector client's secret is held.
#[derive(Clone, PartialEq, Eq)]
pub enum ConnectorClientSecret {
    /// Held by the host in memory.
    Inline(String),
    /// Held by the credential store of `realm`, the realm DEFINING the
    /// profile (an inheriting realm reads its parent's), under
    /// [`TokenKey::connector_client_secret`] for this profile's exact
    /// binding. Read at every code exchange and refresh, so a rotated secret
    /// applies on the next one.
    RealmStore { realm: meerkat_core::RealmId },
}

impl std::fmt::Debug for ConnectorClientSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inline(_) => f.write_str("Inline(<redacted>)"),
            Self::RealmStore { realm } => {
                f.debug_struct("RealmStore").field("realm", realm).finish()
            }
        }
    }
}

impl From<String> for ConnectorClientSecret {
    fn from(secret: String) -> Self {
        Self::Inline(secret)
    }
}

impl From<&str> for ConnectorClientSecret {
    fn from(secret: &str) -> Self {
        Self::Inline(secret.to_owned())
    }
}

/// The metadata field binding a stored client secret to the exact client
/// profile meaning it was stored for.
const CLIENT_SECRET_BINDING_FIELD: &str = "connector_client_binding";

fn client_secret_key(
    realm: &meerkat_core::RealmId,
    profile_binding: &str,
) -> Result<TokenKey, ConnectorLoginError> {
    TokenKey::connector_client_secret(realm.clone(), profile_binding)
        .map_err(|error| ConnectorLoginError::ClientConfig(error.to_string()))
}

/// Read the secret `realm` stores for the connector client whose profile
/// binding (id and canonical non-secret meaning) is `profile_binding`.
///
/// Only a row stored as a connector client secret, for exactly this binding,
/// with a non-empty secret is returned; anything else is `None`. A changed
/// client meaning names a different key, so an old client's secret is never
/// read for a new one.
pub async fn load_connector_client_secret(
    store: &dyn crate::auth_store::TokenStore,
    realm: &meerkat_core::RealmId,
    profile_binding: &str,
) -> Result<Option<String>, ConnectorLoginError> {
    let key = client_secret_key(realm, profile_binding)?;
    let stored = store
        .load_for(&key, TokenStoreAccess::Unattended)
        .await
        .map_err(login_store_error)?;
    Ok(stored
        .filter(|tokens| {
            tokens.auth_mode == PersistedAuthMode::ConnectorClientSecret
                && tokens
                    .metadata
                    .get(CLIENT_SECRET_BINDING_FIELD)
                    .and_then(serde_json::Value::as_str)
                    == Some(profile_binding)
        })
        .and_then(|tokens| tokens.primary_secret)
        .filter(|secret| !secret.is_empty()))
}

/// Store (or replace) the secret `realm` holds for the connector client whose
/// profile binding is `profile_binding`. Rotation under the same binding
/// replaces it in place.
pub async fn save_connector_client_secret(
    store: &dyn crate::auth_store::TokenStore,
    realm: &meerkat_core::RealmId,
    profile_binding: &str,
    secret: &str,
) -> Result<(), ConnectorLoginError> {
    if secret.is_empty() {
        return Err(ConnectorLoginError::ClientConfig(
            "a connector client secret cannot be empty".to_owned(),
        ));
    }
    let key = client_secret_key(realm, profile_binding)?;
    let mut tokens = PersistedTokens::api_key(secret);
    tokens.auth_mode = PersistedAuthMode::ConnectorClientSecret;
    tokens.metadata = serde_json::json!({ CLIENT_SECRET_BINDING_FIELD: profile_binding });
    store
        .save_for(&key, &tokens, TokenStoreAccess::Interactive)
        .await
        .map_err(login_store_error)
}

/// Remove the secret `realm` holds for the client binding `profile_binding`.
pub async fn clear_connector_client_secret(
    store: &dyn crate::auth_store::TokenStore,
    realm: &meerkat_core::RealmId,
    profile_binding: &str,
) -> Result<(), ConnectorLoginError> {
    let key = client_secret_key(realm, profile_binding)?;
    store
        .clear_for(&key, TokenStoreAccess::Interactive)
        .await
        .map_err(login_store_error)
}

impl std::fmt::Debug for ConnectorClientAuthProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorClientAuthProfile")
            .field("id", &self.id)
            .field("issuer", &self.issuer)
            .field("client", &self.client)
            .field("client_secret", &self.client_secret)
            .field("authorize_params", &self.authorize_params)
            .field("resource_indicator", &self.resource_indicator)
            .field("protocol", &self.protocol)
            .field("registered_redirect_uris", &self.registered_redirect_uris)
            .finish()
    }
}

impl ConnectorClientAuthProfile {
    /// The profile's canonical non-secret meaning, bound into a login's
    /// admitted descriptor and its credential's metadata: the id, the
    /// client-auth mode (whether a secret is sent), the resource-indicator
    /// choice and the authorize parameters, in order. A changed meaning
    /// under the same id no longer matches; rotating the secret value does
    /// not change it.
    /// Refuse a profile no login could use: an invalid id, an empty issuer
    /// or client, an unregistrable redirect, or an authorize parameter that
    /// is not a token, carries a control character or is one the login owns.
    pub fn validate(&self) -> Result<(), ConnectorLoginError> {
        // Names are single tokens (values are percent-encoded by the
        // authorize formatter); reserved names are refused in any case.
        let reserved = self.authorize_params.iter().any(|(name, value)| {
            !valid_profile_token(name)
                || value.chars().any(char::is_control)
                || RESERVED_AUTHORIZE_PARAMS
                    .iter()
                    .any(|reserved| name.eq_ignore_ascii_case(reserved))
        });
        if !valid_profile_token(&self.id)
            || !self
                .registered_redirect_uris
                .iter()
                .all(|redirect| valid_registered_redirect(redirect))
            || self.issuer.is_empty()
            || self.client.is_empty()
            || reserved
        {
            return Err(ConnectorOAuthRefusal::InvalidDescriptor.into());
        }
        Ok(())
    }

    pub fn binding(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        let mut field = |value: &str| {
            hash.update(value.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(value.as_bytes());
        };
        field(&self.issuer);
        field(&self.client);
        field(if self.client_secret.is_some() {
            "client_secret_post"
        } else {
            "none"
        });
        field(if self.resource_indicator {
            "resource"
        } else {
            "no-resource"
        });
        for (name, value) in &self.authorize_params {
            field(name);
            field(value);
        }
        // Later sections are tagged pairs after the parameter pairs. A tag
        // starts with `#`, which no parameter name can (names are tokens
        // starting with a letter), so no parameter list encodes like a
        // section. A profile without them keeps its earlier meaning.
        // The selected provider protocol.
        if let Some(protocol) = &self.protocol {
            field("#protocol");
            field(protocol);
        }
        // Registered remote redirects.
        for redirect in &self.registered_redirect_uris {
            field("#redirect");
            field(redirect);
        }
        format!("{}@{:x}", self.id, hash.finalize())
    }
}

/// Where a connector authority reads the host's current client-auth
/// profiles, for example the realm's configuration.
///
/// The authority reads it before every login start, completion and refresh,
/// so a profile added or changed after the authority was built applies on
/// its next use. Profiles installed directly on the authority are kept next
/// to these; an id both define is refused.
#[async_trait::async_trait]
pub trait ConnectorClientAuthSource: Send + Sync {
    async fn current_profiles(
        &self,
    ) -> Result<Vec<ConnectorClientAuthProfile>, ConnectorLoginError>;
}

type ClientAuthProfiles = std::collections::BTreeMap<String, ConnectorClientAuthProfile>;

/// How one code exchange authenticates the client.
struct ExchangeClientAuth<'a> {
    resource_indicator: bool,
    client_secret: Option<&'a str>,
}

/// The profile id a bound profile meaning names.
fn bound_profile_id(binding: &str) -> &str {
    binding.split_once('@').map_or(binding, |(id, _)| id)
}

/// Authorize and token parameters the login owns, or that carry a client
/// credential or proof; a profile may not set them.
const RESERVED_AUTHORIZE_PARAMS: [&str; 17] = [
    "client_id",
    "redirect_uri",
    "response_type",
    "response_mode",
    "scope",
    "state",
    "code_challenge",
    "code_challenge_method",
    "nonce",
    "resource",
    "client_secret",
    "client_assertion",
    "client_assertion_type",
    "code_verifier",
    "code",
    "grant_type",
    "refresh_token",
];

/// A profile id or parameter name: one ASCII token, so it is safe in a URL
/// query verbatim and cannot carry `&`, `=`, `%` or `@`.
fn valid_profile_token(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Provider-specific account verification for connector logins, supplied by
/// the trusted embedding host. It authenticates provider evidence; it never
/// decodes an unverified JWT or infers identity from a label.
#[async_trait]
pub trait ConnectorAccountStrategy: Send + Sync {
    /// The id bound into every descriptor this strategy verifies.
    fn strategy_id(&self) -> &str;

    /// Whether the strategy checks a signed ID token's `nonce` claim. The
    /// owner then mints a nonce per attempt and hands it to
    /// [`observe_account`](Self::observe_account) at completion.
    fn requires_nonce(&self) -> bool {
        false
    }

    /// Observe the provider-verified account for `tokens`. `nonce` is the
    /// attempt's nonce at login completion and `None` on refresh, which
    /// proves the subject through the provider-local observation alone.
    async fn observe_account(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
        nonce: Option<&str>,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal>;

    /// [`observe_account`](Self::observe_account) with the evidence the
    /// client's [`ConnectorProtocol`] decoded from the same token response
    /// (`None` under the default protocol). Strategies that need provider
    /// facts the generic token result does not carry override this.
    async fn observe_account_with_evidence(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
        _evidence: Option<&ConnectorProtocolEvidence>,
        nonce: Option<&str>,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        self.observe_account(descriptor, tokens, nonce).await
    }
}

#[async_trait]
impl ConnectorAccountStrategy for OidcUserInfoAccountStrategy {
    fn strategy_id(&self) -> &str {
        crate::mcp_oauth::OIDC_USERINFO_STRATEGY_ID
    }

    async fn observe_account(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
        _nonce: Option<&str>,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        self.observe_userinfo(descriptor, tokens).await
    }
}

/// The connector account strategies a host installs, by strategy id.
#[derive(Clone, Default)]
pub struct ConnectorStrategies {
    by_id: BTreeMap<String, Arc<dyn ConnectorAccountStrategy>>,
}

impl ConnectorStrategies {
    /// The default set: OIDC UserInfo (`oidc-userinfo-v1`), plus the
    /// account strategies of the built-in provider protocols (Slack
    /// `auth.test`, Salesforce identity service, Google UserInfo), which
    /// verify only logins that name them.
    pub fn with_defaults() -> Self {
        Self::default()
            .with(Arc::new(OidcUserInfoAccountStrategy::new()))
            .with(Arc::new(
                crate::connector_protocol::SlackAuthTestStrategy::new(),
            ))
            .with(Arc::new(
                crate::connector_protocol::SalesforceIdentityStrategy::new(),
            ))
            .with(Arc::new(
                crate::connector_protocol::GoogleUserInfoStrategy::new(),
            ))
    }

    pub fn with(mut self, strategy: Arc<dyn ConnectorAccountStrategy>) -> Self {
        self.by_id
            .insert(strategy.strategy_id().to_owned(), strategy);
        self
    }

    fn get(&self, id: &str) -> Result<&Arc<dyn ConnectorAccountStrategy>, ConnectorLoginError> {
        self.by_id
            .get(id)
            .ok_or(ConnectorLoginError::UnknownStrategy)
    }
}

/// Host-only browser navigation for one admitted connector attempt. The
/// authorize URL and state let whoever follows them complete the attempt:
/// never place them in a tool result, transcript, agent event or log.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectorLoginStart {
    pub slot: CredentialAccountRef,
    pub authorize_url: String,
    pub state: String,
    pub redirect_uri: String,
    /// For an attempt redirected to a registered `https` URI: the host-only
    /// proof that this host started it. The host retains it with its
    /// ceremony and passes it back in
    /// [`ConnectorOAuthCallback::host_attempt_handle`]; it never goes to the
    /// browser, a model or a log. `None` for loopback attempts.
    pub host_attempt_handle: Option<String>,
}

impl std::fmt::Debug for ConnectorLoginStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorLoginStart")
            .field("slot", &self.slot)
            .field("authorize_url", &"<redacted>")
            .field("state", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field(
                "host_attempt_handle",
                &self.host_attempt_handle.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// The host's callback for [`ConnectorOAuthAuthority::login_complete`].
/// Everything else is taken from the admitted attempt.
///
/// For a remote (`https`) attempt the host fills `redirect_uri` from its
/// retained attempt and passes the retained `host_attempt_handle`; `state`
/// and `code` come from the callback. Native compares `redirect_uri` with
/// the admitted attempt's and refuses a mismatch. The handle only proves the
/// host started this attempt: it never stands in for a missing attempt and
/// never replaces the state, PKCE verifier or redirect check.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectorOAuthCallback {
    pub redirect_uri: String,
    pub state: String,
    pub code: String,
    /// The handle [`ConnectorLoginStart::host_attempt_handle`] returned.
    /// Required for an `https` attempt; ignored for a loopback one.
    pub host_attempt_handle: Option<String>,
}

impl std::fmt::Debug for ConnectorOAuthCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorOAuthCallback")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"<redacted>")
            .field("code", &"<redacted>")
            .field(
                "host_attempt_handle",
                &self.host_attempt_handle.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// The provider account bound to a connector credential, qualified by the
/// issuer and the strategy that verified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorVerifiedAccount {
    pub issuer: String,
    pub strategy_id: String,
    pub subject: String,
}

/// Secret-free result of a completed connector login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorLoginComplete {
    pub slot: CredentialAccountRef,
    pub verified_account: ConnectorVerifiedAccount,
    pub scopes: Vec<String>,
    pub scope_evidence: ScopeEvidence,
    pub expires_at: Option<DateTime<Utc>>,
    pub has_refresh_token: bool,
}

/// Secret-free authorization phase of a connector slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorAuthPhase {
    Authorized,
    ReauthRequired,
    /// The slot holds no connector credential.
    AuthorizationRequired,
}

/// Secret-free status of a connector slot. The slot and the verified
/// account are separate facts: the slot name is never account proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAuthStatus {
    pub slot: CredentialAccountRef,
    pub phase: ConnectorAuthPhase,
    pub verified_account: Option<ConnectorVerifiedAccount>,
    pub scopes: Vec<String>,
    pub scope_evidence: Option<ScopeEvidence>,
    pub expires_at: Option<DateTime<Utc>>,
    pub has_refresh_token: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectorLoginError {
    #[error("connector OAuth credential preparation is no longer current")]
    StalePreparation,
    #[error("connector OAuth strategy is not installed on this host")]
    UnknownStrategy,
    #[error("connector OAuth redirect must be an http loopback URI or a registered https URI")]
    InvalidRedirect,
    /// A remote (`https`) attempt was completed without the host's retained
    /// attempt handle, or with another one.
    #[error("connector OAuth remote callback requires the host's retained attempt")]
    RemoteCustodyRequired,
    #[error(transparent)]
    Verification(#[from] ConnectorOAuthRefusal),
    #[error("connector OAuth flow owner refused the ceremony")]
    Flow(#[source] OAuthFlowError),
    #[error(transparent)]
    Slot(CredentialSlotRefusal),
    #[error("connector OAuth issuer discovery failed: {0}")]
    DiscoveryFailed(String),
    #[error("connector OAuth token exchange failed")]
    TokenExchangeFailed,
    /// The token endpoint reported a refresh failure.
    #[error("connector OAuth token refresh failed: {0}")]
    RefreshFailed(String),
    #[error("connector OAuth stored credential requires reauthentication")]
    ReauthRequired,
    #[error("connector OAuth token store error: {0}")]
    TokenStore(String),
    /// The credential store refused access (denied, interaction required,
    /// pending or unavailable): never a refresh failure or absence.
    #[error("connector credential access refused ({refusal}): {detail}")]
    CredentialAccess {
        refusal: meerkat_core::auth::TokenStoreRefusal,
        detail: String,
    },
    #[error("connector OAuth credential lifecycle error: {0}")]
    AuthLifecycle(String),
    /// The redirect-free credential HTTP client could not be built.
    #[error(transparent)]
    HttpClientUnavailable(#[from] crate::mcp_oauth::CredentialHttpClientUnavailable),
    /// The host's connector client-auth configuration could not be read, or
    /// names an invalid or duplicate profile.
    #[error("connector client-auth configuration is invalid: {0}")]
    ClientConfig(String),
    /// The profile sends a client secret that the realm's credential store
    /// does not hold. Nothing was sent; a stored credential is kept.
    #[error("connector client '{profile}' has no client secret configured")]
    ClientSecretUnavailable { profile: String },
}

impl ConnectorLoginError {
    /// Whether this refuses the caller's request rather than reporting an
    /// infrastructure or upstream failure.
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::UnknownStrategy
            | Self::InvalidRedirect
            | Self::RemoteCustodyRequired
            | Self::Verification(_)
            | Self::Slot(_)
            | Self::ReauthRequired => true,
            Self::Flow(error) => error.is_refusal(),
            Self::StalePreparation
            | Self::DiscoveryFailed(_)
            | Self::TokenExchangeFailed
            | Self::RefreshFailed(_)
            | Self::TokenStore(_)
            | Self::CredentialAccess { .. }
            | Self::HttpClientUnavailable(_)
            | Self::ClientConfig(_)
            | Self::ClientSecretUnavailable { .. }
            | Self::AuthLifecycle(_) => false,
        }
    }
}

/// The result of [`ConnectorOAuthAuthority::report_credential_rejection`].
#[derive(Clone, PartialEq, Eq)]
pub enum ConnectorRejectionOutcome {
    /// The credential's protocol does not read the response as a credential
    /// rejection. Nothing changed.
    NotCredentialRejection,
    /// The slot no longer holds the rejected credential (or none at all): a
    /// newer login or refresh replaced it. Nothing was refreshed or changed;
    /// whether to use the current credential is the caller's next ordinary
    /// [`ConnectorOAuthAuthority::bearer_token`] decision.
    Superseded,
    /// This call refreshed the exact rejected credential, still bound to the
    /// same account and context; this is its access token after the
    /// refresh, which equals the rejected one when the provider kept it.
    Refreshed(String),
}

impl std::fmt::Debug for ConnectorRejectionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCredentialRejection => f.write_str("NotCredentialRejection"),
            Self::Superseded => f.write_str("Superseded"),
            Self::Refreshed(_) => f.write_str("Refreshed(<redacted>)"),
        }
    }
}

/// Issuer endpoints validated for one connector attempt.
struct IssuerEndpoints {
    authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct IssuerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

/// What a host call expects of a connector slot's credential: the verified
/// account, the stable context it was granted under, and the scopes the call
/// needs. Checked on the exact credential admitted for the call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorCallExpectation {
    pub slot: CredentialAccountRef,
    /// The provider-verified subject the credential must be bound to.
    pub account: String,
    pub issuer: String,
    pub client: String,
    pub resource: String,
    pub strategy_id: String,
    pub required_scopes: BTreeSet<String>,
}

/// A connector credential admitted for one host call
/// ([`ConnectorOAuthAuthority::admit_call_credential`]). It holds the access
/// token for that call only: it is neither `Clone` nor serializable, and its
/// `Debug` never shows the token.
pub struct ConnectorCallCredential {
    slot: CredentialAccountRef,
    key: TokenKey,
    lease_key: LeaseKey,
    tokens: PersistedTokens,
    secret: String,
}

impl ConnectorCallCredential {
    pub fn slot(&self) -> &CredentialAccountRef {
        &self.slot
    }

    /// The verified subject the credential is bound to.
    pub fn account(&self) -> &str {
        self.tokens.account_id.as_deref().unwrap_or_default()
    }

    pub fn scopes(&self) -> &[String] {
        &self.tokens.scopes
    }

    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.tokens.expires_at
    }

    /// The access token, for the host's own request to the connector.
    pub fn expose_access_token(&self) -> &str {
        &self.secret
    }
}

/// A connector credential admitted for one host call, with the slot's
/// lifecycle guard held: no refresh, replacement or logout changes the slot
/// until it is dropped, so [`Self::recheck`] observes the admitted
/// credential's marker, freshness and use admission coherently.
pub struct ConnectorCallEntry {
    authority: ConnectorOAuthAuthority,
    credential: ConnectorCallCredential,
    lifecycle: meerkat_core::AuthLoginLifecycleGuard,
}

impl ConnectorCallEntry {
    pub fn credential(&self) -> &ConnectorCallCredential {
        &self.credential
    }

    /// The final check: the admitted credential is still the slot's current,
    /// fresh and usable credential. A logout, a replacement login, a refresh
    /// onto a newer credential, a lost grant, or entering the refresh window
    /// since admission refuses.
    pub fn recheck(&self) -> Result<(), ConnectorLoginError> {
        self.authority
            .recheck_held(&self.credential, &self.lifecycle)
    }
}

impl std::fmt::Debug for ConnectorCallEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorCallEntry")
            .field("credential", &self.credential)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ConnectorCallCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorCallCredential")
            .field("slot", &self.slot)
            .field("expires_at", &self.tokens.expires_at)
            .finish_non_exhaustive()
    }
}

/// Check the exact credential admitted for a call against what the call
/// expects, returning its access token.
fn check_call_expectation(
    tokens: &PersistedTokens,
    expect: &ConnectorCallExpectation,
) -> Result<String, ConnectorLoginError> {
    check_expectation_facts(tokens, expect)?;
    tokens
        .primary_secret
        .clone()
        .ok_or(ConnectorLoginError::ReauthRequired)
}

/// The passive half of [`check_call_expectation`]: the stored credential's
/// verified account, context and granted scopes against `expect`, without
/// reading its secret.
fn check_expectation_facts(
    tokens: &PersistedTokens,
    expect: &ConnectorCallExpectation,
) -> Result<(), ConnectorLoginError> {
    if tokens.auth_mode != PersistedAuthMode::ConnectorOauth {
        return Err(ConnectorLoginError::ReauthRequired);
    }
    let metadata = ConnectorCredentialMetadata::from_tokens(tokens)
        .ok_or(ConnectorLoginError::ReauthRequired)?;
    if tokens.account_id.as_deref() != Some(expect.account.as_str()) {
        return Err(ConnectorOAuthRefusal::AccountMismatch.into());
    }
    if metadata.issuer != expect.issuer
        || metadata.client != expect.client
        || metadata.resource != expect.resource
        || metadata.strategy_id != expect.strategy_id
    {
        return Err(ConnectorLoginError::Slot(
            CredentialSlotRefusal::ContextMismatch,
        ));
    }
    let granted: BTreeSet<&str> = tokens.scopes.iter().map(String::as_str).collect();
    if !expect
        .required_scopes
        .iter()
        .all(|scope| granted.contains(scope.as_str()))
    {
        return Err(ConnectorOAuthRefusal::MissingScopes.into());
    }
    Ok(())
}

/// The native connector OAuth owner.
#[derive(Clone)]
pub struct ConnectorOAuthAuthority {
    http: Client,
    persistence: ProviderAuthPersistence,
    auth_lease: GeneratedAuthLeaseHandle,
    flows: Arc<dyn OAuthFlowAuthority>,
    strategies: ConnectorStrategies,
    /// Host-installed client-auth profiles, by id.
    client_auth: Arc<ClientAuthProfiles>,
    /// The host's current profiles, read at each use.
    client_auth_source: Option<Arc<dyn ConnectorClientAuthSource>>,
    /// Host-installed provider protocols, by id.
    protocols: ConnectorProtocols,
}

impl ConnectorOAuthAuthority {
    /// The flow authority must own terminal flow state in AuthMachine and
    /// supply its matched credential lifecycle. The HTTP client follows no
    /// redirects.
    pub fn new(
        persistence: ProviderAuthPersistence,
        flows: Arc<dyn OAuthFlowAuthority>,
        strategies: ConnectorStrategies,
    ) -> Result<Self, ConnectorLoginError> {
        Self::with_http(persistence, flows, strategies, no_redirect_client()?)
    }

    /// `http` must not follow redirects.
    pub fn with_http(
        persistence: ProviderAuthPersistence,
        flows: Arc<dyn OAuthFlowAuthority>,
        strategies: ConnectorStrategies,
        http: Client,
    ) -> Result<Self, ConnectorLoginError> {
        if !flows.terminal_flow_state_is_authmachine_owned() {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable.into());
        }
        let auth_lease = flows
            .generated_credential_lifecycle()
            .ok_or(ConnectorOAuthRefusal::VerificationUnavailable)?;
        Ok(Self {
            http,
            persistence,
            auth_lease,
            flows,
            strategies,
            client_auth: Arc::default(),
            client_auth_source: None,
            protocols: ConnectorProtocols::default(),
        })
    }

    /// Install the host's provider protocols. A client-auth profile names
    /// one by id; a login or refresh under a profile naming an unknown
    /// protocol is refused before any request.
    pub fn with_connector_protocols(mut self, protocols: ConnectorProtocols) -> Self {
        self.protocols = protocols;
        self
    }

    /// The installed protocol whose meaning is exactly `recorded` (the
    /// binding an attempt or a credential was admitted under), which must
    /// serve `strategy_id` and still be the one the current `profile`
    /// selects. A profile now selecting another or no protocol, a missing
    /// protocol, or one whose meaning changed under the same id is refused;
    /// none of them falls back to the default protocol. `None` recorded is
    /// the default protocol, which the profile must also select.
    fn protocol_for(
        &self,
        recorded: Option<&str>,
        profile: Option<&ConnectorClientAuthProfile>,
        strategy_id: &str,
    ) -> Result<Option<Arc<dyn ConnectorProtocol>>, ConnectorLoginError> {
        let selected = profile.and_then(|profile| profile.protocol.as_deref());
        let Some(recorded) = recorded else {
            return match selected {
                None => Ok(None),
                Some(_) => Err(ConnectorOAuthRefusal::DescriptorMismatch.into()),
            };
        };
        let recorded_id = recorded.split_once('@').map_or(recorded, |(id, _)| id);
        if selected != Some(recorded_id) {
            return Err(ConnectorOAuthRefusal::DescriptorMismatch.into());
        }
        match self.protocols.get(recorded_id) {
            Some(protocol)
                if protocol.strategy_id() == strategy_id && protocol.binding() == recorded =>
            {
                Ok(Some(Arc::clone(protocol)))
            }
            _ => Err(ConnectorOAuthRefusal::DescriptorMismatch.into()),
        }
    }

    /// The endpoints of an admitted login: the protocol's fixed ones, or the
    /// issuer's discovered ones.
    async fn login_endpoints(
        &self,
        protocol: Option<&Arc<dyn ConnectorProtocol>>,
        facts: &ConnectorOAuthParameters,
    ) -> Result<IssuerEndpoints, ConnectorLoginError> {
        if let Some(protocol) = protocol
            && let Some(fixed) = protocol.endpoints(facts)?
        {
            require_https_or_loopback(&fixed.authorization_endpoint)?;
            require_https_or_loopback(&fixed.token_endpoint)?;
            return Ok(IssuerEndpoints {
                authorization_endpoint: fixed.authorization_endpoint,
                token_endpoint: fixed.token_endpoint,
            });
        }
        self.discover(&facts.issuer).await
    }

    /// The code exchange of a login under `protocol`: the owner's own
    /// request, decoded by the protocol.
    async fn protocol_exchange(
        &self,
        protocol: &dyn ConnectorProtocol,
        endpoints: &IssuerEndpoints,
        facts: &ConnectorOAuthParameters,
        code: &str,
        pkce_verifier: &str,
        client_auth: ExchangeClientAuth<'_>,
    ) -> Result<ConnectorProtocolGrant, ConnectorLoginError> {
        let ExchangeClientAuth {
            resource_indicator,
            client_secret,
        } = client_auth;
        let mut form = vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code.to_owned()),
            ("redirect_uri", facts.redirect_uri.clone()),
            ("client_id", facts.client.clone()),
            ("code_verifier", pkce_verifier.to_owned()),
        ];
        if let Some(secret) = client_secret {
            form.push(("client_secret", secret.to_owned()));
        }
        // The admitted resource indicator, as on authorize (RFC 8707).
        if resource_indicator {
            form.push(("resource", facts.resource.clone()));
        }
        let (status, body) = post_token_form(&self.http, &endpoints.token_endpoint, &form)
            .await
            .map_err(|_| ConnectorLoginError::TokenExchangeFailed)?;
        protocol
            .decode_token_response(
                ConnectorTokenGrantKind::Exchange,
                status,
                &body,
                client_secret,
            )
            .map_err(|_| ConnectorLoginError::TokenExchangeFailed)
    }

    /// The refresh request of a credential under `protocol`, decoded by the
    /// protocol. A provider refusal is reported as a token-endpoint refusal
    /// in OAuth terms, so the refresh owner classifies it as usual.
    async fn protocol_refresh(
        &self,
        protocol: &dyn ConnectorProtocol,
        token_endpoint: &str,
        client: &str,
        refresh_token: &str,
        client_secret: Option<&str>,
        resource: Option<&str>,
    ) -> Result<ConnectorProtocolGrant, crate::auth_oauth::OAuthError> {
        let mut form = vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", refresh_token.to_owned()),
            ("client_id", client.to_owned()),
        ];
        if let Some(secret) = client_secret {
            form.push(("client_secret", secret.to_owned()));
        }
        if let Some(resource) = resource {
            form.push(("resource", resource.to_owned()));
        }
        let (status, body) = post_token_form(&self.http, token_endpoint, &form)
            .await
            .map_err(|error| crate::auth_oauth::OAuthError::Network(error.to_string()))?;
        protocol
            .decode_token_response(
                ConnectorTokenGrantKind::Refresh,
                status,
                &body,
                client_secret,
            )
            .map_err(|failure| match failure {
                ConnectorProtocolFailure::Refused { status, code } => {
                    crate::auth_oauth::OAuthError::TokenEndpoint {
                        status: if (400..600).contains(&status) {
                            status
                        } else {
                            400
                        },
                        body: serde_json::json!({ "error": code.unwrap_or_default() }).to_string(),
                    }
                }
                // A refresh never establishes a new enrollment; a policy
                // failure here is unavailable like a malformed response.
                ConnectorProtocolFailure::Malformed(reason)
                | ConnectorProtocolFailure::Policy(reason) => {
                    crate::auth_oauth::OAuthError::Network(reason.to_owned())
                }
            })
    }

    /// Install the host's client-auth profiles. A profile without an id,
    /// issuer or client, a duplicate id, or one setting a parameter the
    /// login owns is refused.
    pub fn with_client_auth_profiles(
        mut self,
        profiles: impl IntoIterator<Item = ConnectorClientAuthProfile>,
    ) -> Result<Self, ConnectorLoginError> {
        let mut installed = std::collections::BTreeMap::new();
        for profile in profiles {
            profile.validate()?;
            if installed.contains_key(&profile.id) {
                return Err(ConnectorOAuthRefusal::InvalidDescriptor.into());
            }
            installed.insert(profile.id.clone(), profile);
        }
        self.client_auth = Arc::new(installed);
        Ok(self)
    }

    /// Read the host's current client-auth profiles from `source` before
    /// every login start, completion and refresh (see
    /// [`ConnectorClientAuthSource`]).
    pub fn with_client_auth_source(mut self, source: Arc<dyn ConnectorClientAuthSource>) -> Self {
        self.client_auth_source = Some(source);
        self
    }

    /// The client secret `profile` sends now, read from its holder.
    async fn client_secret(
        &self,
        profile: Option<&ConnectorClientAuthProfile>,
    ) -> Result<Option<String>, ConnectorLoginError> {
        let Some(profile) = profile else {
            return Ok(None);
        };
        match &profile.client_secret {
            None => Ok(None),
            Some(ConnectorClientSecret::Inline(secret)) => Ok(Some(secret.clone())),
            // The defining realm's secret for exactly this captured profile
            // meaning; never another realm's and never another meaning's.
            Some(ConnectorClientSecret::RealmStore { realm }) => load_connector_client_secret(
                self.persistence.token_store().as_ref(),
                realm,
                &profile.binding(),
            )
            .await?
            .map(Some)
            .ok_or_else(|| ConnectorLoginError::ClientSecretUnavailable {
                profile: profile.id.clone(),
            }),
        }
    }

    /// The profiles current for one use: the installed ones and the source's,
    /// each valid and each id defined once.
    async fn current_client_auth(&self) -> Result<Arc<ClientAuthProfiles>, ConnectorLoginError> {
        let Some(source) = &self.client_auth_source else {
            return Ok(Arc::clone(&self.client_auth));
        };
        let mut current = (*self.client_auth).clone();
        for profile in source.current_profiles().await? {
            if profile.validate().is_err() {
                return Err(ConnectorLoginError::ClientConfig(format!(
                    "connector client profile '{}' is invalid",
                    profile.id
                )));
            }
            if current.contains_key(&profile.id) {
                return Err(ConnectorLoginError::ClientConfig(format!(
                    "connector client profile '{}' is defined twice",
                    profile.id
                )));
            }
            current.insert(profile.id.clone(), profile);
        }
        Ok(Arc::new(current))
    }

    /// The installed profile whose canonical meaning is exactly `binding`
    /// (completion and refresh resolve from the admitted descriptor or the
    /// stored metadata). A missing profile, or one whose meaning changed
    /// under the same id, is refused.
    fn bound_client_auth_profile<'a>(
        profiles: &'a ClientAuthProfiles,
        binding: Option<&str>,
        issuer: &str,
        client: &str,
    ) -> Result<Option<&'a ConnectorClientAuthProfile>, ConnectorLoginError> {
        let Some(binding) = binding else {
            return Ok(None);
        };
        match Self::client_auth_profile(profiles, Some(bound_profile_id(binding)), issuer, client)?
        {
            Some(profile) if profile.binding() == binding => Ok(Some(profile)),
            _ => Err(ConnectorOAuthRefusal::InvalidDescriptor.into()),
        }
    }

    /// The installed profile named `id`, valid only for `issuer` and
    /// `client`. `None` is the default request forms.
    fn client_auth_profile<'a>(
        profiles: &'a ClientAuthProfiles,
        id: Option<&str>,
        issuer: &str,
        client: &str,
    ) -> Result<Option<&'a ConnectorClientAuthProfile>, ConnectorLoginError> {
        let Some(id) = id else {
            return Ok(None);
        };
        match profiles.get(id) {
            Some(profile) if profile.issuer == issuer && profile.client == client => {
                Ok(Some(profile))
            }
            _ => Err(ConnectorOAuthRefusal::InvalidDescriptor.into()),
        }
    }

    /// Admit one host-driven browser attempt for `target`, redirecting to
    /// the host's loopback `redirect_uri`. Discovers the issuer's endpoints
    /// and admits PKCE/state (plus a nonce for ID-token strategies) through
    /// the flow authority. It opens no listener and launches no browser.
    pub async fn login_start(
        &self,
        target: &ConnectorOAuthTarget,
        redirect_uri: &str,
    ) -> Result<ConnectorLoginStart, ConnectorLoginError> {
        // A loopback redirect as before; an https one only when it is exactly
        // registered for the target's profile (checked below, before any
        // discovery).
        let remote = is_https_redirect(redirect_uri);
        if !remote {
            require_loopback_redirect(redirect_uri)?;
        }
        // A connector credential is always account-verified; the unverified
        // resource grant is an MCP-only host opt-in.
        if target.account.is_unverified() {
            return Err(ConnectorOAuthRefusal::InvalidDescriptor.into());
        }
        let strategy = self.strategies.get(&target.strategy_id)?;
        // An unknown profile, or one for another issuer or client, is
        // refused before any discovery.
        let profiles = self.current_client_auth().await?;
        let profile = Self::client_auth_profile(
            &profiles,
            target.client_auth_profile.as_deref(),
            &target.issuer,
            &target.client,
        )?;
        if remote
            && !profile.is_some_and(|profile| {
                profile
                    .registered_redirect_uris
                    .iter()
                    .any(|registered| registered == redirect_uri)
            })
        {
            return Err(ConnectorLoginError::InvalidRedirect);
        }
        // A profile whose client secret is not configured could never
        // complete; it is refused before any request.
        self.client_secret(profile).await?;
        // The protocol the profile selects, installed and serving the
        // target's strategy; its meaning is bound into the descriptor.
        let selected = match profile.and_then(|profile| profile.protocol.as_deref()) {
            Some(id) => Some(
                self.protocols
                    .get(id)
                    .filter(|protocol| protocol.strategy_id() == target.strategy_id)
                    .cloned()
                    .ok_or(ConnectorOAuthRefusal::InvalidDescriptor)?,
            ),
            None => None,
        };
        let descriptor: ConnectorOAuthDescriptor = ConnectorOAuthParameters {
            issuer: target.issuer.clone(),
            client: target.client.clone(),
            resource: target.resource.clone(),
            scopes: target.scopes.clone(),
            redirect_uri: redirect_uri.to_owned(),
            expected_account: target.account.clone(),
            strategy_id: target.strategy_id.clone(),
            client_auth_profile: profile.map(ConnectorClientAuthProfile::binding),
            protocol: selected.as_ref().map(|protocol| protocol.binding()),
        }
        .try_into()?;
        let protocol = selected;
        let endpoints = self
            .login_endpoints(protocol.as_ref(), descriptor.parameters())
            .await?;
        let pkce = PkcePair::generate_s256();
        let identity: OAuthBrowserFlowIdentity = descriptor.clone().into();
        let slot: AuthCredentialIdentity = AuthCredentialIdentity::Account(target.slot.clone());
        let nonce = if strategy.requires_nonce() {
            Some(
                crate::oauth_flow::OAuthFlowRegistry::new_state()
                    .map_err(ConnectorLoginError::Flow)?,
            )
        } else {
            None
        };
        let state = match &nonce {
            Some(nonce) => self.flows.start_with_nonce(
                slot,
                identity,
                redirect_uri.to_owned(),
                pkce.verifier.secret().to_owned(),
                nonce.clone(),
            ),
            None => self.flows.start(
                slot,
                identity,
                redirect_uri.to_owned(),
                pkce.verifier.secret().to_owned(),
            ),
        }
        .map_err(ConnectorLoginError::Flow)?;
        let mut oauth = match &protocol {
            Some(protocol) => {
                protocol_oauth_endpoints(protocol.as_ref(), &descriptor, &endpoints, profile)?
            }
            None => oauth_endpoints(&descriptor, &endpoints, profile),
        };
        if let Some(nonce) = nonce {
            oauth
                .extra_authorize_params
                .push(("nonce".to_owned(), nonce));
        }
        let host_attempt_handle =
            remote.then(|| remote_attempt_handle(&target.slot, &state, redirect_uri));
        Ok(ConnectorLoginStart {
            slot: target.slot.clone(),
            authorize_url: oauth.authorize_url_with_pkce(&pkce.challenge, &state),
            state,
            redirect_uri: redirect_uri.to_owned(),
            host_attempt_handle,
        })
    }

    /// Complete one admitted attempt from the host's callback (loopback, or
    /// a registered `https` redirect with the retained handle).
    ///
    /// The flow owner must name a live attempt admitted under `state` for
    /// this exact slot. Issuer, client, resource, scopes, strategy and
    /// account selection come only from that admitted descriptor. Every
    /// non-success exit after the remote custody check and before the commit
    /// retires the attempt; a missing or forged remote handle leaves it to
    /// its owner. The commit consumes it, and a consumed attempt cannot be
    /// replayed.
    ///
    /// Dropping this future before the commit retires the attempt and
    /// publishes nothing. Once the commit is entered it runs to its end in
    /// the refresh coordinator's custody even if this future is dropped:
    /// either the attempt is consumed and the credential published, or
    /// nothing is. A caller that must settle the outcome awaits this future
    /// and cancels with [`login_cancel`](Self::login_cancel) instead of
    /// dropping it.
    pub async fn login_complete(
        &self,
        slot: &CredentialAccountRef,
        callback: ConnectorOAuthCallback,
    ) -> Result<ConnectorLoginComplete, ConnectorLoginError> {
        let ConnectorOAuthCallback {
            redirect_uri,
            state,
            code,
            host_attempt_handle,
        } = callback;
        let identity = AuthCredentialIdentity::Account(slot.clone());
        let admitted = self
            .flows
            .admitted_connector_browser_attempt(&state, &identity)
            .map_err(ConnectorLoginError::Flow)?
            .ok_or(ConnectorLoginError::Flow(OAuthFlowError::Missing))?;
        // A remote attempt completes only from the custody of the host that
        // started it: the retained handle for exactly this slot, state and
        // admitted redirect. Refused before anything else, leaving the
        // attempt to its owner.
        if is_https_redirect(&admitted.redirect_uri) {
            let expected = remote_attempt_handle(slot, &state, &admitted.redirect_uri);
            if !host_attempt_handle.as_deref().is_some_and(|handle| {
                crate::connector_protocol::bytes_equal(handle.as_bytes(), expected.as_bytes())
            }) {
                return Err(ConnectorLoginError::RemoteCustodyRequired);
            }
        }
        // Read before the attempt is armed for retirement, so an unreadable
        // configuration leaves the attempt to its owner.
        let profiles = self.current_client_auth().await?;
        let mut retire = AttemptRetireGuard {
            flows: Arc::clone(&self.flows),
            state: state.clone(),
            target: identity.clone(),
            identity: admitted.provider.clone(),
            redirect_uri: admitted.redirect_uri.clone(),
            armed: true,
        };
        let record = self
            .flows
            .verify(&state, &identity, admitted.provider.clone(), &redirect_uri)
            .map_err(ConnectorLoginError::Flow)?;
        let OAuthBrowserFlowIdentity::Connector { connector } = &record.provider else {
            return Err(ConnectorLoginError::Flow(
                OAuthFlowError::BrowserIdentityMismatch,
            ));
        };
        let descriptor = (**connector).clone();
        let facts = descriptor.parameters().clone();
        let strategy = self.strategies.get(&facts.strategy_id)?;
        if strategy.requires_nonce() != record.nonce.is_some() {
            return Err(ConnectorOAuthRefusal::VerificationUnavailable.into());
        }
        // The profile meaning the admitted descriptor binds, never caller
        // input.
        let profile = Self::bound_client_auth_profile(
            &profiles,
            facts.client_auth_profile.as_deref(),
            &facts.issuer,
            &facts.client,
        )?;
        // The protocol the attempt was admitted under, still selected by the
        // profile; checked before any network I/O.
        let protocol = self.protocol_for(facts.protocol.as_deref(), profile, &facts.strategy_id)?;
        let endpoints = self.login_endpoints(protocol.as_ref(), &facts).await?;
        let client_secret = self.client_secret(profile).await?;
        let client_secret = client_secret.as_deref();
        let (token, protocol_evidence) = match &protocol {
            Some(protocol) => {
                let grant = self
                    .protocol_exchange(
                        protocol.as_ref(),
                        &endpoints,
                        &facts,
                        &code,
                        &record.pkce_verifier,
                        ExchangeClientAuth {
                            resource_indicator: profile
                                .is_none_or(|profile| profile.resource_indicator),
                            client_secret,
                        },
                    )
                    .await?;
                (grant.tokens, Some(grant.evidence))
            }
            None => (
                exchange_authorization_code_with_state(
                    &self.http,
                    &oauth_endpoints(&descriptor, &endpoints, profile),
                    &code,
                    &record.pkce_verifier,
                    client_secret,
                    Some(&state),
                )
                .await
                .map_err(|_| ConnectorLoginError::TokenExchangeFailed)?,
                None,
            ),
        };
        let observation = strategy
            .observe_account_with_evidence(
                &descriptor,
                &token,
                protocol_evidence.as_ref(),
                record.nonce.as_deref(),
            )
            .await?;
        let evidence = descriptor.verify_account(observation, &token)?;
        let now = Utc::now();
        let metadata = ConnectorCredentialMetadata {
            issuer: facts.issuer.clone(),
            client: facts.client.clone(),
            resource: facts.resource.clone(),
            strategy_id: facts.strategy_id.clone(),
            requested_scopes: facts.scopes.clone(),
            token_endpoint: endpoints.token_endpoint.clone(),
            stable_context: descriptor.stable_context(),
            scope_evidence: evidence.scope_evidence(),
            granted_at_epoch_secs: now.timestamp(),
            client_auth_profile: facts.client_auth_profile.clone(),
            protocol: facts.protocol.clone(),
            protocol_evidence,
        };
        let persisted = persisted_tokens(
            &token,
            evidence.account(),
            evidence.granted_scopes(),
            &metadata,
            now,
        )?;
        let committed = save_oauth_tokens_and_consume_browser_flow(
            self.persistence.clone(),
            self.auth_lease.clone(),
            identity,
            persisted,
            BrowserOAuthFlowCommit {
                authority: Arc::clone(&self.flows),
                state,
                completion: evidence.clone().into(),
                redirect_uri: record.redirect_uri.clone(),
            },
        )
        .await;
        // Past the consume step the attempt is terminal either way. A
        // failure before it (for example a token-binding mismatch) leaves
        // the attempt for the guard to retire.
        let committed = match committed {
            Ok(committed) => {
                retire.armed = false;
                committed
            }
            Err(CredentialMutationError::SlotRefused(refusal)) => {
                retire.armed = false;
                return Err(ConnectorLoginError::Slot(refusal));
            }
            Err(error) => return Err(map_mutation_error(error)),
        };
        Ok(ConnectorLoginComplete {
            slot: slot.clone(),
            verified_account: ConnectorVerifiedAccount {
                issuer: facts.issuer,
                strategy_id: facts.strategy_id,
                subject: evidence.account().to_owned(),
            },
            scopes: committed.scopes.clone(),
            scope_evidence: evidence.scope_evidence(),
            expires_at: committed.expires_at,
            has_refresh_token: committed.refresh_token.is_some(),
        })
    }

    /// [`login_complete`](Self::login_complete) for a caller that names the
    /// whole target again (wire callers): the admitted attempt's descriptor
    /// must equal `target`'s facts, so a completion cannot be redirected to
    /// a different connector, strategy or account selection.
    pub async fn login_complete_for_target(
        &self,
        target: &ConnectorOAuthTarget,
        callback: ConnectorOAuthCallback,
    ) -> Result<ConnectorLoginComplete, ConnectorLoginError> {
        let identity = AuthCredentialIdentity::Account(target.slot.clone());
        let admitted = self
            .flows
            .admitted_connector_browser_attempt(&callback.state, &identity)
            .map_err(ConnectorLoginError::Flow)?
            .ok_or(ConnectorLoginError::Flow(OAuthFlowError::Missing))?;
        let OAuthBrowserFlowIdentity::Connector { connector } = &admitted.provider else {
            return Err(ConnectorLoginError::Flow(
                OAuthFlowError::BrowserIdentityMismatch,
            ));
        };
        let facts = connector.parameters();
        if facts.issuer != target.issuer
            || facts.client != target.client
            || facts.resource != target.resource
            || facts.scopes != target.scopes
            || facts.strategy_id != target.strategy_id
            || facts.expected_account != target.account
            || facts.client_auth_profile.as_deref().map(bound_profile_id)
                != target.client_auth_profile.as_deref()
        {
            return Err(ConnectorOAuthRefusal::DescriptorMismatch.into());
        }
        self.login_complete(&target.slot, callback).await
    }

    /// Retire the attempt admitted under `state` for `slot` (host timeout,
    /// cancellation or a closed browser). Local only; an unknown state is
    /// refused. Cancellation consumes no authorization response and
    /// publishes no credential.
    pub fn login_cancel(
        &self,
        slot: &CredentialAccountRef,
        state: &str,
    ) -> Result<(), ConnectorLoginError> {
        let identity = AuthCredentialIdentity::Account(slot.clone());
        let record = self
            .flows
            .admitted_connector_browser_attempt(state, &identity)
            .map_err(ConnectorLoginError::Flow)?
            .ok_or(ConnectorLoginError::Flow(OAuthFlowError::Missing))?;
        self.flows
            .expire(state, &identity, record.provider, &record.redirect_uri)
            .map_err(ConnectorLoginError::Flow)
    }

    /// Disconnect `slot`: remove its connector credential and release its
    /// AuthMachine lifecycle, inside the slot's exclusive mutation. An empty
    /// slot is already disconnected; a slot holding another owner's
    /// credential is refused and left untouched.
    pub async fn logout(&self, slot: &CredentialAccountRef) -> Result<(), ConnectorLoginError> {
        meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_mode(
            self.persistence.clone(),
            self.auth_lease.clone(),
            AuthCredentialIdentity::Account(slot.clone()),
            PersistedAuthMode::ConnectorOauth,
        )
        .await
        .map_err(map_mutation_error)
    }

    /// Passively check the slot's stored, admitted credential against
    /// `expect` (the verified account, issuer, client, resource, strategy
    /// and required scopes), as a call admission would. It loads the slot's
    /// persisted tokens through the same admission as a call (which may
    /// rehydrate the slot's auth state from them), but extracts and uses no
    /// token, performs no refresh and enters no call. An absent or unusable
    /// credential is `ReauthRequired`.
    pub async fn check_expectation(
        &self,
        expect: &ConnectorCallExpectation,
    ) -> Result<(), ConnectorLoginError> {
        let identity = AuthCredentialIdentity::Account(expect.slot.clone());
        let key = TokenKey::from_credential_identity(&identity);
        let lease_key = LeaseKey::from_credential_identity(&identity);
        let guard = meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
            .await
            .ok_or_else(lifecycle_owner_busy)?;
        let admitted = self
            .load_admitted(&identity, &key, &guard, TokenStoreAccess::Unattended)
            .await?
            .ok_or(ConnectorLoginError::ReauthRequired)?;
        check_expectation_facts(&admitted.tokens, expect)
    }

    /// Secret-free status of `slot`, projected through the same marker,
    /// AuthMachine projection and use admission as
    /// [`bearer_token`](Self::bearer_token), so status never reports a
    /// credential that bearer would refuse. It performs no refresh and no
    /// network I/O.
    pub async fn status(
        &self,
        slot: &CredentialAccountRef,
    ) -> Result<ConnectorAuthStatus, ConnectorLoginError> {
        let identity = AuthCredentialIdentity::Account(slot.clone());
        let key = TokenKey::from_credential_identity(&identity);
        let lease_key = LeaseKey::from_credential_identity(&identity);
        // Status never waits behind an operating-system credential call and
        // never starts one; ordinary owner contention is waited out.
        let Some(_guard) =
            meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key).await
        else {
            return Err(lifecycle_owner_busy());
        };
        let (phase, tokens) = match self
            .load_admitted(&identity, &key, &_guard, TokenStoreAccess::Status)
            .await
        {
            Ok(None) => (ConnectorAuthPhase::AuthorizationRequired, None),
            Ok(Some(admitted)) => {
                let phase = match admitted.disposition {
                    CredentialUseDisposition::RefreshRequired
                        if admitted.tokens.refresh_token.is_none() =>
                    {
                        ConnectorAuthPhase::ReauthRequired
                    }
                    _ => ConnectorAuthPhase::Authorized,
                };
                (phase, Some(admitted.tokens))
            }
            // Not usable: report which account the slot is bound to, so the
            // host can re-authorize it with a Known login.
            Err(ConnectorLoginError::ReauthRequired) => {
                let stored = self
                    .persistence
                    .token_store()
                    .load_for(&key, TokenStoreAccess::Status)
                    .await
                    .map_err(login_store_error)?
                    .filter(|tokens| tokens.auth_mode == PersistedAuthMode::ConnectorOauth);
                (ConnectorAuthPhase::ReauthRequired, stored)
            }
            Err(error) => return Err(error),
        };
        let metadata = tokens
            .as_ref()
            .and_then(ConnectorCredentialMetadata::from_tokens);
        let usable = phase == ConnectorAuthPhase::Authorized;
        Ok(ConnectorAuthStatus {
            slot: slot.clone(),
            phase,
            verified_account: match (&tokens, &metadata) {
                (Some(tokens), Some(metadata)) => {
                    tokens
                        .account_id
                        .clone()
                        .map(|subject| ConnectorVerifiedAccount {
                            issuer: metadata.issuer.clone(),
                            strategy_id: metadata.strategy_id.clone(),
                            subject,
                        })
                }
                _ => None,
            },
            scopes: tokens
                .as_ref()
                .filter(|_| usable)
                .map(|tokens| tokens.scopes.clone())
                .unwrap_or_default(),
            scope_evidence: metadata
                .as_ref()
                .filter(|_| usable)
                .map(|metadata| metadata.scope_evidence),
            expires_at: tokens
                .as_ref()
                .filter(|_| usable)
                .and_then(|tokens| tokens.expires_at),
            has_refresh_token: usable
                && tokens
                    .as_ref()
                    .is_some_and(|tokens| tokens.refresh_token.is_some()),
        })
    }

    /// The slot's access token for native use, refreshed through the
    /// AuthMachine-owned lifecycle when it needs it. `None` when the slot
    /// holds no connector credential. The token never leaves the host.
    ///
    /// Contract: the credential admitted at the start of this call is
    /// binding. A refresh, including one another waiter ran or one the
    /// coordinator's reload ran on a replacement credential (for example
    /// after a logout and a new login into the slot), is returned only if it
    /// is still bound to the admitted account and stable context (issuer,
    /// client, resource, strategy). Otherwise the call is refused:
    /// `AccountMismatch` for another subject, `ContextMismatch` for the same
    /// subject under another context. A later call admits the replacement.
    pub async fn bearer_token(
        &self,
        slot: &CredentialAccountRef,
    ) -> Result<Option<String>, ConnectorLoginError> {
        let identity = AuthCredentialIdentity::Account(slot.clone());
        let key = TokenKey::from_credential_identity(&identity);
        let lease_key = LeaseKey::from_credential_identity(&identity);
        let admitted = {
            // A request never waits behind an operating-system credential
            // call that holds the lease: it answers pending at once.
            let _guard =
                meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
                    .await
                    .ok_or_else(lifecycle_owner_busy)?;
            self.load_admitted(&identity, &key, &_guard, TokenStoreAccess::Unattended)
                .await?
        };
        let Some(admitted) = admitted else {
            return Ok(None);
        };
        match admitted.disposition {
            CredentialUseDisposition::Authorized => Ok(admitted.tokens.primary_secret),
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing => Ok(self
                .refresh_bound(&identity, key, &admitted.tokens)
                .await?
                .primary_secret),
            CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => Err(ConnectorLoginError::ReauthRequired),
        }
    }

    /// Refresh an admitted credential through the refresh coordinator and
    /// return the exact resulting credential, refused unless it is still
    /// bound to the admitted account and stable context.
    async fn refresh_bound(
        &self,
        identity: &AuthCredentialIdentity,
        key: TokenKey,
        admitted: &PersistedTokens,
    ) -> Result<PersistedTokens, ConnectorLoginError> {
        let authority = self.clone();
        let refresh_identity = identity.clone();
        let refresh_key = key.clone();
        let bound = admitted.account_id.clone();
        let admitted_context = ConnectorCredentialMetadata::from_tokens(admitted)
            .map(|metadata| metadata.stable_context);
        let refreshed = self
            .persistence
            .refresh_coordinator()
            .with_refresh(
                key,
                Box::new(move || {
                    Box::pin(async move {
                        authority
                            .refresh_under_coordinator(&refresh_identity, &refresh_key, None)
                            .await
                    })
                }),
            )
            .await
            .map_err(map_refresh_error)?;
        // A coordinator may return another waiter's result or a
        // replacement's: recheck the admitted binding on that exact result
        // before use.
        if refreshed.account_id != bound || refreshed.auth_mode != PersistedAuthMode::ConnectorOauth
        {
            return Err(ConnectorOAuthRefusal::AccountMismatch.into());
        }
        if ConnectorCredentialMetadata::from_tokens(&refreshed)
            .map(|metadata| metadata.stable_context)
            != admitted_context
        {
            return Err(ConnectorLoginError::Slot(
                CredentialSlotRefusal::ContextMismatch,
            ));
        }
        Ok(refreshed)
    }

    /// Admit the slot's credential for one host call that expects `expect`:
    /// the verified account, the stable context and the required scopes are
    /// checked on the exact credential this call admits (after any refresh,
    /// including one another waiter or a replacement ran), not on an earlier
    /// status or a cached token.
    ///
    /// The returned entry holds the slot's lifecycle guard, so no refresh,
    /// replacement or logout changes the slot until it is dropped. Run
    /// [`ConnectorCallEntry::recheck`] right before the credential leaves the
    /// host's custody, with no await in between, and drop the entry once it
    /// has.
    pub async fn admit_call_credential(
        &self,
        expect: &ConnectorCallExpectation,
    ) -> Result<ConnectorCallEntry, ConnectorLoginError> {
        let identity = AuthCredentialIdentity::Account(expect.slot.clone());
        let key = TokenKey::from_credential_identity(&identity);
        let lease_key = LeaseKey::from_credential_identity(&identity);
        let admitted = {
            let _guard =
                meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
                    .await
                    .ok_or_else(lifecycle_owner_busy)?;
            self.load_admitted(&identity, &key, &_guard, TokenStoreAccess::Unattended)
                .await?
        }
        .ok_or(ConnectorLoginError::ReauthRequired)?;
        let tokens = match admitted.disposition {
            CredentialUseDisposition::Authorized => admitted.tokens,
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing => {
                self.refresh_bound(&identity, key.clone(), &admitted.tokens)
                    .await?
            }
            CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => {
                return Err(ConnectorLoginError::ReauthRequired);
            }
        };
        let secret = check_call_expectation(&tokens, expect)?;
        let lifecycle = meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
            .await
            .ok_or_else(lifecycle_owner_busy)?;
        let entry = ConnectorCallEntry {
            authority: self.clone(),
            credential: ConnectorCallCredential {
                slot: expect.slot.clone(),
                key,
                lease_key,
                tokens,
                secret,
            },
            lifecycle,
        };
        // The refresh above ran without the guard: the credential it
        // returned must still be the slot's, now that the guard is held.
        entry.recheck()?;
        Ok(entry)
    }

    /// Synchronous, I/O-free check that `credential` is still the slot's
    /// current, usable credential: the credential lifecycle still matches
    /// the admitted credential's durable marker, its freshness is observed
    /// again, and it still authorizes use. The caller holds the slot's
    /// lifecycle guard, so all three observe the same credential.
    fn recheck_held(
        &self,
        credential: &ConnectorCallCredential,
        lifecycle: &meerkat_core::AuthLoginLifecycleGuard,
    ) -> Result<(), ConnectorLoginError> {
        if lifecycle.lease_key() != &credential.lease_key {
            return Err(ConnectorLoginError::AuthLifecycle(
                "credential lifecycle guard belongs to another lease".into(),
            ));
        }
        let snapshot = self.auth_lease.snapshot(&credential.lease_key);
        if lifecycle_snapshot_is_absent(&snapshot)
            || durable_marker::marker_relation_for_tokens_and_snapshot(
                &credential.tokens,
                &snapshot,
                &credential.key,
            ) != durable_marker::AuthLeaseDurableMarkerRelation::Matches
        {
            return Err(ConnectorLoginError::ReauthRequired);
        }
        // Freshness may have changed during the caller's waits: re-observe it
        // now, so a credential that entered its refresh window is refused here
        // (the next call refreshes it) rather than used.
        self.auth_lease
            .observe_credential_freshness(
                &credential.lease_key,
                epoch_secs(Utc::now()),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(|error| ConnectorLoginError::AuthLifecycle(error.to_string()))?;
        match self
            .auth_lease
            .resolve_credential_use_admission(
                &credential.lease_key,
                CredentialUseIntent::UseCredential,
            )
            .map_err(|error| ConnectorLoginError::AuthLifecycle(error.to_string()))?
        {
            CredentialUseDisposition::Authorized => Ok(()),
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing
            | CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => Err(ConnectorLoginError::ReauthRequired),
        }
    }

    /// Report that a resource server answered a call made with the slot's
    /// access token `rejected_token` with `status` and `body`.
    ///
    /// The credential's own protocol classifies the response; the default
    /// protocol classifies nothing, and nothing is refreshed or replayed for
    /// an unclassified response. Under the slot's lifecycle guard the
    /// rejected token is compared with the credential the slot holds: a
    /// rejection of a credential the slot no longer holds starts no work and
    /// changes nothing ([`ConnectorRejectionOutcome::Superseded`]). For the
    /// exact current credential, AuthMachine moves a valid lifecycle to
    /// refresh, and the refresh coordinator refreshes that credential only:
    /// if, by the time the coordinated refresh runs, the slot holds another
    /// credential (a refresh by another waiter, or a logout and new login),
    /// it is left alone and reported as superseded. A refresh this call ran
    /// is [`ConnectorRejectionOutcome::Refreshed`], even when the provider
    /// returned the same access token. A refreshed credential must still be
    /// bound to the rejected one's account and context.
    /// Whether to retry the call stays with the caller.
    pub async fn report_credential_rejection(
        &self,
        slot: &CredentialAccountRef,
        rejected_token: &str,
        status: u16,
        body: &[u8],
    ) -> Result<ConnectorRejectionOutcome, ConnectorLoginError> {
        let identity = AuthCredentialIdentity::Account(slot.clone());
        let key = TokenKey::from_credential_identity(&identity);
        let lease_key = LeaseKey::from_credential_identity(&identity);
        let (bound, admitted_context) = {
            let guard = meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
                .await
                .ok_or_else(lifecycle_owner_busy)?;
            let Some(admitted) = self
                .load_admitted(&identity, &key, &guard, TokenStoreAccess::Unattended)
                .await?
            else {
                return Ok(ConnectorRejectionOutcome::Superseded);
            };
            let Some(metadata) = ConnectorCredentialMetadata::from_tokens(&admitted.tokens) else {
                return Err(ConnectorLoginError::ReauthRequired);
            };
            let profiles = self.current_client_auth().await?;
            let profile = Self::bound_client_auth_profile(
                &profiles,
                metadata.client_auth_profile.as_deref(),
                &metadata.issuer,
                &metadata.client,
            )?;
            let Some(protocol) =
                self.protocol_for(metadata.protocol.as_deref(), profile, &metadata.strategy_id)?
            else {
                return Ok(ConnectorRejectionOutcome::NotCredentialRejection);
            };
            match protocol.classify_rejection(status, body) {
                Some(ConnectorCredentialRejection::InvalidCredential) => {}
                None => return Ok(ConnectorRejectionOutcome::NotCredentialRejection),
            }
            // A late answer about a credential the slot no longer holds
            // starts no work on the current one.
            if admitted.tokens.primary_secret.as_deref() != Some(rejected_token) {
                return Ok(ConnectorRejectionOutcome::Superseded);
            }
            // The exact current credential: AuthMachine moves it to refresh.
            // An already expiring, expired or refreshing lifecycle refreshes
            // on its own.
            if self.auth_lease.snapshot(&lease_key).phase
                == Some(meerkat_core::handles::AuthLeasePhase::Valid)
            {
                self.auth_lease
                    .mark_expiring(&lease_key)
                    .map_err(|error| ConnectorLoginError::AuthLifecycle(error.to_string()))?;
            }
            (admitted.tokens.account_id, metadata.stable_context)
        };
        // The guard is released; the coordinated refresh reacquires it and
        // refreshes only if the slot still holds the rejected credential.
        let refreshed_here = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let authority = self.clone();
        let refresh_identity = identity.clone();
        let refresh_key = key.clone();
        let rejected = rejected_token.to_owned();
        let flag = Arc::clone(&refreshed_here);
        let current = self
            .persistence
            .refresh_coordinator()
            .with_refresh(
                key,
                Box::new(move || {
                    Box::pin(async move {
                        authority
                            .refresh_under_coordinator(
                                &refresh_identity,
                                &refresh_key,
                                Some((rejected.as_str(), flag.as_ref())),
                            )
                            .await
                    })
                }),
            )
            .await
            .map_err(map_refresh_error)?;
        // Superseded only when this call did not refresh the credential. A
        // refresh it ran is reported as refreshed even when the provider
        // returned the same access token (with, for example, a rotated
        // refresh token): the slot holds that refreshed credential.
        if !refreshed_here.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(ConnectorRejectionOutcome::Superseded);
        }
        if current.account_id != bound || current.auth_mode != PersistedAuthMode::ConnectorOauth {
            return Err(ConnectorOAuthRefusal::AccountMismatch.into());
        }
        if ConnectorCredentialMetadata::from_tokens(&current)
            .map(|metadata| metadata.stable_context)
            != Some(admitted_context)
        {
            return Err(ConnectorLoginError::Slot(
                CredentialSlotRefusal::ContextMismatch,
            ));
        }
        current
            .primary_secret
            .map(ConnectorRejectionOutcome::Refreshed)
            .ok_or(ConnectorLoginError::ReauthRequired)
    }

    /// Load one durable connector credential through its marker,
    /// AuthMachine projection, freshness observation and use admission. The
    /// caller holds the slot's lifecycle guard for this whole read.
    async fn load_admitted(
        &self,
        identity: &AuthCredentialIdentity,
        key: &TokenKey,
        guard: &meerkat_core::AuthLoginLifecycleGuard,
        access: TokenStoreAccess,
    ) -> Result<Option<AdmittedConnectorCredential>, ConnectorLoginError> {
        let lease_key = LeaseKey::from_credential_identity(identity);
        if guard.lease_key() != &lease_key {
            return Err(ConnectorLoginError::AuthLifecycle(
                "credential lifecycle guard belongs to another lease".into(),
            ));
        }
        let store = self.persistence.token_store();
        let lifecycle = |error: meerkat_core::handles::DslTransitionError| {
            ConnectorLoginError::AuthLifecycle(error.to_string())
        };
        let Some(mut tokens) = store
            .load_for(key, access)
            .await
            .map_err(login_store_error)?
        else {
            let snapshot = self.auth_lease.snapshot(&lease_key);
            if snapshot.credential_present
                && snapshot
                    .phase
                    .is_some_and(|phase| phase != meerkat_core::handles::AuthLeasePhase::Released)
            {
                self.auth_lease
                    .release_credential_lifecycle(&lease_key)
                    .map_err(lifecycle)?;
            }
            return Ok(None);
        };
        // The connector loader refuses every other owner's rows.
        if tokens.auth_mode != PersistedAuthMode::ConnectorOauth {
            return Ok(None);
        }
        let usable = |tokens: &PersistedTokens| {
            tokens.primary_secret.is_some()
                && tokens.account_id.is_some()
                && ConnectorCredentialMetadata::from_tokens(tokens).is_some()
                && durable_marker::marker_payload_valid_for_tokens(tokens, key)
        };
        if !usable(&tokens) {
            return Err(ConnectorLoginError::ReauthRequired);
        }
        let snapshot = self.auth_lease.snapshot(&lease_key);
        if lifecycle_snapshot_is_absent(&snapshot)
            || matches!(
                durable_marker::marker_relation_for_tokens_and_snapshot(&tokens, &snapshot, key),
                durable_marker::AuthLeaseDurableMarkerRelation::TokenNewer
            )
        {
            tokens = meerkat_core::rehydrate_marked_tokens_for_status_for_identity_with_guard(
                store.as_ref(),
                &self.auth_lease,
                identity,
                PersistedAuthMode::ConnectorOauth,
                Utc::now(),
                guard,
            )
            .await
            .map_err(login_rehydrate_error)?
            .ok_or(ConnectorLoginError::ReauthRequired)?;
            if !usable(&tokens) {
                return Err(ConnectorLoginError::ReauthRequired);
            }
        }
        self.auth_lease
            .observe_credential_freshness(
                &lease_key,
                epoch_secs(Utc::now()),
                AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
            )
            .map_err(lifecycle)?;
        let restore_snapshot = self
            .auth_lease
            .capture_auth_lifecycle_restore_snapshot(&lease_key);
        if durable_marker::marker_relation_for_tokens_and_snapshot(
            &tokens,
            restore_snapshot.snapshot(),
            key,
        ) != durable_marker::AuthLeaseDurableMarkerRelation::Matches
        {
            return Err(ConnectorLoginError::ReauthRequired);
        }
        let disposition = self
            .auth_lease
            .resolve_credential_use_admission(&lease_key, CredentialUseIntent::UseCredential)
            .map_err(lifecycle)?;
        match disposition {
            CredentialUseDisposition::Authorized
            | CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing => {
                Ok(Some(AdmittedConnectorCredential {
                    tokens,
                    restore_snapshot,
                    disposition,
                }))
            }
            CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => Err(ConnectorLoginError::ReauthRequired),
        }
    }

    /// Coordinator closure: reload and re-admit under the coordinator lock,
    /// so a waiter observes a winner's durable token instead of refreshing
    /// twice. A refresh keeps the bound account: the subject is re-observed
    /// through the strategy with no ID-token proof, and a mismatch or an
    /// unobservable subject leaves the credential at its prior state.
    ///
    /// `exact` binds the refresh to one credential: when the slot no longer
    /// holds that access token, the current credential is returned untouched,
    /// and the flag is set only when this call begins the refresh.
    async fn refresh_under_coordinator(
        &self,
        identity: &AuthCredentialIdentity,
        key: &TokenKey,
        exact: Option<(&str, &std::sync::atomic::AtomicBool)>,
    ) -> Result<PersistedTokens, RefreshError> {
        let lease_key = LeaseKey::from_credential_identity(identity);
        // Unattended refresh never waits behind an operating-system
        // credential call that holds the lease.
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard_for_unattended(&lease_key)
            .await
            .ok_or_else(|| RefreshError::CredentialAccess {
                refusal: meerkat_core::auth::TokenStoreRefusal::Pending,
                detail: LIFECYCLE_OWNER_BUSY.into(),
            })?;
        let admitted = self
            .load_admitted(identity, key, &_guard, TokenStoreAccess::Unattended)
            .await
            .map_err(refresh_error_from_login)?
            .ok_or_else(|| {
                RefreshError::ReauthRequired(
                    "stored connector OAuth credential disappeared before refresh".to_owned(),
                )
            })?;
        if let Some((expected, _)) = exact
            && admitted.tokens.primary_secret.as_deref() != Some(expected)
        {
            return Ok(admitted.tokens);
        }
        if admitted.disposition == CredentialUseDisposition::Authorized {
            return Ok(admitted.tokens);
        }
        let begin = self
            .auth_lease
            .resolve_credential_use_admission(&lease_key, CredentialUseIntent::BeginRefresh)
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        match begin {
            CredentialUseDisposition::RefreshRequired => {
                self.auth_lease
                    .begin_refresh(&lease_key)
                    .map_err(|error| RefreshError::Refresh(error.to_string()))?;
                if let Some((_, refreshed_here)) = exact {
                    refreshed_here.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            CredentialUseDisposition::ReauthRequired => {
                return Err(RefreshError::ReauthRequired(
                    "connector OAuth credential requires reauthentication".to_owned(),
                ));
            }
            other => {
                return Err(RefreshError::Refresh(format!(
                    "AuthMachine rejected connector OAuth BeginRefresh admission: {other:?}"
                )));
            }
        }
        let refreshing_snapshot = self.auth_lease.snapshot(&lease_key);
        let previous = admitted.tokens.clone();
        let (Some(metadata), Some(bound)) = (
            ConnectorCredentialMetadata::from_tokens(&previous),
            previous.account_id.clone(),
        ) else {
            return Err(self
                .refresh_failed(&lease_key, "stored connector credential lost its binding")
                .await);
        };
        let Some(refresh_token) = previous.refresh_token.clone() else {
            let observation = meerkat_core::RefreshFailureObservation::local_credential_unusable();
            let disposition = self
                .close_refresh_failure(key, &lease_key, &observation)
                .await?;
            return Err(RefreshError::Classified {
                message: "stored connector OAuth credential has no refresh token".to_owned(),
                observation,
                disposition,
            });
        };
        let descriptor: ConnectorOAuthDescriptor = match (ConnectorOAuthParameters {
            issuer: metadata.issuer.clone(),
            client: metadata.client.clone(),
            resource: metadata.resource.clone(),
            scopes: metadata.requested_scopes.clone(),
            // Refresh never redirects; the descriptor only needs a valid URI.
            redirect_uri: "http://127.0.0.1/".to_owned(),
            expected_account: AccountSelection::Known(bound.clone()),
            strategy_id: metadata.strategy_id.clone(),
            client_auth_profile: metadata.client_auth_profile.clone(),
            protocol: metadata.protocol.clone(),
        })
        .try_into()
        {
            Ok(descriptor) => descriptor,
            Err(_) => {
                return Err(self
                    .refresh_failed(&lease_key, "stored connector descriptor is invalid")
                    .await);
            }
        };
        // The profile the credential was minted under. A missing or changed
        // profile is a local failure: the request is never sent without it.
        let Ok(profiles) = self.current_client_auth().await else {
            return Err(self
                .refresh_failed(
                    &lease_key,
                    "connector client-auth configuration is unavailable",
                )
                .await);
        };
        let Ok(profile) = Self::bound_client_auth_profile(
            &profiles,
            metadata.client_auth_profile.as_deref(),
            &metadata.issuer,
            &metadata.client,
        ) else {
            return Err(self
                .refresh_failed(
                    &lease_key,
                    "connector client-auth profile is not installed for this credential",
                )
                .await);
        };
        let resource_indicator = profile.is_none_or(|profile| profile.resource_indicator);
        let endpoints = OAuthEndpoints {
            client_id: metadata.client.clone(),
            authorize_url: String::new(),
            token_url: metadata.token_endpoint.clone(),
            device_code_url: None,
            redirect_uri: String::new(),
            scopes: Vec::new(),
            extra_authorize_params: Vec::new(),
            token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
            include_state_in_token_exchange: false,
            extra_token_params: if resource_indicator {
                vec![("resource".to_owned(), metadata.resource.clone())]
            } else {
                Vec::new()
            },
            refresh_scopes: Vec::new(),
            extra_headers: Vec::new(),
        };
        // A missing secret is local configuration: nothing is sent and the
        // credential is kept.
        let client_secret = match self.client_secret(profile).await {
            Ok(secret) => secret,
            Err(error) => {
                return Err(self.refresh_failed(&lease_key, &error.to_string()).await);
            }
        };
        let client_secret = client_secret.as_deref();
        // The protocol the credential's profile names decodes the refresh.
        let Ok(protocol) =
            self.protocol_for(metadata.protocol.as_deref(), profile, &metadata.strategy_id)
        else {
            return Err(self
                .refresh_failed(
                    &lease_key,
                    "connector protocol is not installed for this credential",
                )
                .await);
        };
        let response = match &protocol {
            Some(protocol) => self
                .protocol_refresh(
                    protocol.as_ref(),
                    &metadata.token_endpoint,
                    &metadata.client,
                    &refresh_token,
                    client_secret,
                    resource_indicator.then_some(metadata.resource.as_str()),
                )
                .await
                .map(|grant| (grant.tokens, Some(grant.evidence))),
            None => exchange_refresh_token(&self.http, &endpoints, &refresh_token, client_secret)
                .await
                .map(|refreshed| (refreshed, None)),
        };
        let (refreshed, refreshed_evidence) = match response {
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
        // Protocol rules: a protocol may require the refresh to state its
        // scopes and to replace the refresh token (whose predecessor may be
        // consumed, so it is never reused), and the facts that pin the grant
        // must not change.
        let protocol_evidence = match &protocol {
            Some(protocol) => {
                // Replacement loss first: a missing (or handed-back)
                // replacement closes the credential durably, whatever else
                // the response lacks, because the predecessor may be
                // consumed and is never reused.
                if protocol.refresh_token_rule() == RefreshTokenRule::ReplacementRequired
                    && refreshed
                        .refresh_token
                        .as_deref()
                        .is_none_or(|replacement| replacement == refresh_token)
                {
                    let observation =
                        meerkat_core::RefreshFailureObservation::local_credential_unusable();
                    let disposition = self
                        .close_refresh_failure(key, &lease_key, &observation)
                        .await?;
                    return Err(RefreshError::Classified {
                        message: "connector refresh response carried no replacement refresh token"
                            .to_owned(),
                        observation,
                        disposition,
                    });
                }
                if protocol.refresh_requires_scope() && refreshed.scope.is_none() {
                    return Err(self
                        .refresh_failed(&lease_key, "connector refresh response stated no scopes")
                        .await);
                }
                match merge_protocol_evidence(
                    metadata.protocol_evidence.as_ref(),
                    refreshed_evidence.unwrap_or_default(),
                ) {
                    Some(evidence) => Some(evidence),
                    None => {
                        return Err(self
                            .refused_after_closure(
                                &lease_key,
                                RefreshError::CredentialIdentityMismatch,
                            )
                            .await);
                    }
                }
            }
            None => metadata.protocol_evidence.clone(),
        };
        // Scope: an included `scope` is authoritative for the new token and
        // must still cover the required scopes; an omitted one keeps the
        // original grant (RFC 6749 section 5.1), recorded by reference.
        let granted: BTreeSet<String> = match refreshed.scope.as_deref() {
            Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
            None => previous.scopes.iter().cloned().collect(),
        };
        if !metadata.requested_scopes.is_subset(&granted) {
            return Err(self
                .refused_after_closure(&lease_key, RefreshError::RequiredScopesNotGranted)
                .await);
        }
        // Subject: the refreshed token must still be the bound account.
        let Ok(strategy) = self.strategies.get(&metadata.strategy_id) else {
            return Err(self
                .refresh_failed(&lease_key, "connector strategy is not installed")
                .await);
        };
        match strategy
            .observe_account_with_evidence(
                &descriptor,
                &refreshed,
                protocol_evidence.as_ref(),
                None,
            )
            .await
        {
            Ok(observation) if observation.account == bound => {}
            Ok(_) => {
                return Err(self
                    .refused_after_closure(&lease_key, RefreshError::CredentialIdentityMismatch)
                    .await);
            }
            Err(_) => {
                return Err(self
                    .refresh_failed(&lease_key, "refreshed subject could not be observed")
                    .await);
            }
        }
        let refreshed_at = Utc::now();
        let refreshed_metadata = ConnectorCredentialMetadata {
            protocol_evidence,
            scope_evidence: metadata.refreshed_scope_evidence(refreshed.scope.is_some()),
            granted_at_epoch_secs: if refreshed.scope.is_some() {
                refreshed_at.timestamp()
            } else {
                metadata.granted_at_epoch_secs
            },
            ..metadata
        };
        let mut persisted = match persisted_tokens(
            &refreshed,
            &bound,
            &granted,
            &refreshed_metadata,
            refreshed_at,
        ) {
            Ok(persisted) => persisted,
            Err(_) => {
                return Err(self
                    .refresh_failed(&lease_key, "refreshed token expiry is out of range")
                    .await);
            }
        };
        // Rotation: a replacement refresh token replaces the stored one in
        // the same save as the access token; none keeps the existing one
        // (a protocol requiring replacement refused that above).
        if persisted.refresh_token.is_none() {
            persisted.refresh_token = Some(refresh_token);
        }
        let current = match self
            .persistence
            .token_store()
            .load_for(key, TokenStoreAccess::Unattended)
            .await
        {
            Ok(current) => current,
            // The store refused or failed the reread: close the refresh and
            // keep that cause, typed when it is a credential access.
            Err(error) => {
                return Err(self
                    .refused_after_closure(&lease_key, RefreshError::from_token_store(error))
                    .await);
            }
        };
        if current.as_ref() != Some(&previous)
            || self.auth_lease.snapshot(&lease_key) != refreshing_snapshot
        {
            return Err(self
                .refresh_failed(
                    &lease_key,
                    "connector credential or lifecycle changed during refresh",
                )
                .await);
        }
        let transition = match self.auth_lease.complete_refresh(
            &lease_key,
            meerkat_core::persisted_token_expires_at_epoch_secs(&persisted),
            epoch_secs(refreshed_at),
        ) {
            Ok(transition) => transition,
            Err(error) => return Err(self.refresh_failed(&lease_key, &error.to_string()).await),
        };
        let published = match meerkat_core::mark_tokens_lifecycle_published_for_transition(
            key,
            &persisted,
            &transition,
        ) {
            Ok(published) => published,
            Err(error) => {
                return Err(self
                    .rollback_refresh(
                        key,
                        &lease_key,
                        &previous,
                        &admitted.restore_snapshot,
                        RefreshError::Refresh(error.to_string()),
                    )
                    .await);
            }
        };
        if let Err(error) = self
            .persistence
            .token_store()
            .save_for(key, &published, TokenStoreAccess::Unattended)
            .await
        {
            return Err(self
                .rollback_refresh(
                    key,
                    &lease_key,
                    &previous,
                    &admitted.restore_snapshot,
                    RefreshError::from_token_store(error),
                )
                .await);
        }
        Ok(published)
    }

    /// Close a begun refresh for a use refusal: the credential keeps its prior
    /// durable state, and the typed refusal survives only if AuthMachine
    /// accepted the closure.
    async fn refused_after_closure(
        &self,
        lease_key: &LeaseKey,
        refusal: RefreshError,
    ) -> RefreshError {
        let observation = meerkat_core::RefreshFailureObservation::transient();
        refusal_after_closure(
            self.auth_lease
                .refresh_failed(lease_key, observation)
                .map(|_transition| ())
                .map_err(|error| error.to_string()),
            refusal,
        )
    }

    /// Close a begun refresh as a transient failure: the credential keeps
    /// its prior durable state.
    async fn refresh_failed(&self, lease_key: &LeaseKey, reason: &str) -> RefreshError {
        let observation = meerkat_core::RefreshFailureObservation::transient();
        match self.auth_lease.refresh_failed(lease_key, observation) {
            Ok(_transition) => RefreshError::Refresh(reason.to_owned()),
            Err(error) => RefreshError::Refresh(format!(
                "{reason}; AuthMachine refresh_failed rejected closure: {error}"
            )),
        }
    }

    /// Close a begun refresh through AuthMachine. Permanently unusable
    /// credentials are removed from the durable store first, so a new
    /// process cannot resurrect bytes the token endpoint rejected.
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
        if disposition == RefreshFailureDisposition::ReauthRequired
            && let Err(error) = self
                .persistence
                .token_store()
                .clear_for(key, TokenStoreAccess::Unattended)
                .await
        {
            let _ = self
                .auth_lease
                .refresh_failed(lease_key, observation.clone());
            return Err(RefreshError::DurableTerminalCommit {
                message: format!(
                    "permanently rejected connector OAuth credential removal failed: {error}"
                ),
                observation: observation.clone(),
                disposition,
            });
        }
        self.auth_lease
            .refresh_failed(lease_key, observation.clone())
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        Ok(disposition)
    }

    async fn rollback_refresh(
        &self,
        key: &TokenKey,
        lease_key: &LeaseKey,
        previous: &PersistedTokens,
        previous_snapshot: &AuthLeaseRestoreSnapshot,
        cause: RefreshError,
    ) -> RefreshError {
        let mut errors = Vec::new();
        if let Err(error) = self.auth_lease.release_credential_lifecycle(lease_key) {
            errors.push(format!("AuthMachine release failed: {error}"));
        }
        let mut restored = previous.clone();
        match meerkat_core::restore_token_lifecycle_snapshot(&self.auth_lease, previous_snapshot) {
            Ok(Some(transition)) => {
                match meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    key,
                    previous,
                    &transition,
                ) {
                    Ok(marked) => restored = marked,
                    Err(error) => errors.push(format!("marker restore failed: {error}")),
                }
            }
            Ok(None) => {}
            Err(error) => errors.push(format!("AuthMachine restore failed: {error}")),
        }
        if let Err(error) = self
            .persistence
            .token_store()
            .save_for(key, &restored, TokenStoreAccess::Unattended)
            .await
        {
            errors.push(format!("TokenStore restore failed: {error}"));
        }
        if errors.is_empty() {
            // The compensation completed: the original cause stands, typed.
            cause
        } else {
            // A failed compensation is its own failure, never the original
            // cause alone.
            RefreshError::Refresh(format!("{cause}; {}", errors.join("; ")))
        }
    }

    /// The issuer's authorization and token endpoints: RFC 8414 metadata,
    /// falling back to OpenID Connect discovery. The metadata's `issuer`
    /// must equal the admitted issuer, PKCE S256 must be advertised, and
    /// every endpoint must be https (or loopback). Redirects are refused.
    async fn discover(&self, issuer: &str) -> Result<IssuerEndpoints, ConnectorLoginError> {
        require_https_or_loopback(issuer)?;
        let mut last_error = String::from("no issuer metadata found");
        for metadata_url in issuer_metadata_urls(issuer)? {
            let response = match self.http.get(&metadata_url).send().await {
                Ok(response) => response,
                Err(error) => {
                    last_error = error.to_string();
                    continue;
                }
            };
            let response = match response.error_for_status_refusing_redirects() {
                Ok(response) => response,
                Err(error) => {
                    last_error = error;
                    continue;
                }
            };
            let metadata: IssuerMetadata = response.json().await.map_err(|error| {
                ConnectorLoginError::DiscoveryFailed(format!("decode issuer metadata: {error}"))
            })?;
            if metadata.issuer != issuer {
                return Err(ConnectorLoginError::DiscoveryFailed(
                    "issuer metadata names a different issuer".to_owned(),
                ));
            }
            if !metadata
                .code_challenge_methods_supported
                .iter()
                .any(|method| method == "S256")
            {
                return Err(ConnectorLoginError::DiscoveryFailed(
                    "issuer does not advertise PKCE S256".to_owned(),
                ));
            }
            let authorization_endpoint =
                absolutize_url(&metadata_url, &metadata.authorization_endpoint)
                    .map_err(ConnectorLoginError::DiscoveryFailed)?;
            let token_endpoint = absolutize_url(&metadata_url, &metadata.token_endpoint)
                .map_err(ConnectorLoginError::DiscoveryFailed)?;
            require_https_or_loopback(&authorization_endpoint)?;
            require_https_or_loopback(&token_endpoint)?;
            return Ok(IssuerEndpoints {
                authorization_endpoint,
                token_endpoint,
            });
        }
        Err(ConnectorLoginError::DiscoveryFailed(last_error))
    }
}

struct AdmittedConnectorCredential {
    tokens: PersistedTokens,
    restore_snapshot: AuthLeaseRestoreSnapshot,
    disposition: CredentialUseDisposition,
}

/// Retires an admitted attempt through its flow owner unless disarmed.
struct AttemptRetireGuard {
    flows: Arc<dyn OAuthFlowAuthority>,
    state: String,
    target: AuthCredentialIdentity,
    identity: OAuthBrowserFlowIdentity,
    redirect_uri: String,
    armed: bool,
}

impl Drop for AttemptRetireGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.flows.expire(
                &self.state,
                &self.target,
                self.identity.clone(),
                &self.redirect_uri,
            );
        }
    }
}

fn oauth_endpoints(
    descriptor: &ConnectorOAuthDescriptor,
    issuer: &IssuerEndpoints,
    profile: Option<&ConnectorClientAuthProfile>,
) -> OAuthEndpoints {
    let facts = descriptor.parameters();
    let scopes: Vec<String> = facts.scopes.iter().cloned().collect();
    let resource = profile
        .is_none_or(|profile| profile.resource_indicator)
        .then(|| ("resource".to_owned(), facts.resource.clone()));
    let mut extra_authorize_params: Vec<(String, String)> = resource.iter().cloned().collect();
    if let Some(profile) = profile {
        extra_authorize_params.extend(profile.authorize_params.iter().cloned());
    }
    OAuthEndpoints {
        client_id: facts.client.clone(),
        authorize_url: issuer.authorization_endpoint.clone(),
        token_url: issuer.token_endpoint.clone(),
        device_code_url: None,
        redirect_uri: facts.redirect_uri.clone(),
        scopes: scopes.clone(),
        extra_authorize_params,
        token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
        include_state_in_token_exchange: false,
        extra_token_params: resource.into_iter().collect(),
        refresh_scopes: scopes,
        extra_headers: Vec::new(),
    }
}

/// The evidence a refreshed credential keeps: the stored evidence with the
/// refresh response's facts. `None` when its pinned facts differ in any way
/// (changed, missing or new).
fn merge_protocol_evidence(
    stored: Option<&ConnectorProtocolEvidence>,
    refreshed: ConnectorProtocolEvidence,
) -> Option<ConnectorProtocolEvidence> {
    let mut merged = stored.cloned().unwrap_or_default();
    // Every pinned fact must be asserted again, unchanged; none may appear
    // or disappear.
    if refreshed.pinned != merged.pinned {
        return None;
    }
    merged.facts.extend(refreshed.facts);
    Some(merged)
}

/// [`oauth_endpoints`] under a provider protocol: the protocol renders the
/// admitted scopes (and any account hint) in place of the default `scope`
/// parameter; the profile's parameters follow. A protocol never sets a
/// parameter the login owns.
fn protocol_oauth_endpoints(
    protocol: &dyn ConnectorProtocol,
    descriptor: &ConnectorOAuthDescriptor,
    issuer: &IssuerEndpoints,
    profile: Option<&ConnectorClientAuthProfile>,
) -> Result<OAuthEndpoints, ConnectorLoginError> {
    let facts = descriptor.parameters();
    let protocol_params = protocol.authorize_params(facts)?;
    if protocol_params.iter().any(|(name, _)| {
        RESERVED_AUTHORIZE_PARAMS
            .iter()
            .filter(|reserved| !matches!(**reserved, "scope" | "resource"))
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
    }) {
        return Err(ConnectorOAuthRefusal::InvalidDescriptor.into());
    }
    let mut endpoints = oauth_endpoints(descriptor, issuer, profile);
    // The protocol owns how scopes are rendered.
    endpoints.scopes = Vec::new();
    endpoints.refresh_scopes = Vec::new();
    let mut extra = protocol_params;
    extra.extend(endpoints.extra_authorize_params);
    endpoints.extra_authorize_params = extra;
    Ok(endpoints)
}

fn persisted_tokens(
    token: &OAuthTokenResult,
    account: &str,
    granted: &BTreeSet<String>,
    metadata: &ConnectorCredentialMetadata,
    now: DateTime<Utc>,
) -> Result<PersistedTokens, ConnectorLoginError> {
    let expires_at = token
        .expires_at_from(now)
        .map_err(|_| ConnectorLoginError::TokenExchangeFailed)?;
    Ok(PersistedTokens {
        auth_mode: PersistedAuthMode::ConnectorOauth,
        primary_secret: Some(token.access_token.clone()),
        refresh_token: token.refresh_token.clone(),
        id_token: token.id_token.clone(),
        expires_at,
        last_refresh: Some(now),
        scopes: granted.iter().cloned().collect(),
        account_id: Some(account.to_owned()),
        metadata: metadata.to_value(),
    })
}

/// RFC 8414 metadata (with the issuer path inserted after the well-known
/// segment), then OpenID Connect discovery.
fn issuer_metadata_urls(issuer: &str) -> Result<[String; 2], ConnectorLoginError> {
    let url = reqwest::Url::parse(issuer)
        .map_err(|error| ConnectorLoginError::DiscoveryFailed(error.to_string()))?;
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_matches('/');
    let rfc8414 = if path.is_empty() {
        format!("{origin}/.well-known/oauth-authorization-server")
    } else {
        format!("{origin}/.well-known/oauth-authorization-server/{path}")
    };
    let oidc = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    Ok([rfc8414, oidc])
}

fn require_https_or_loopback(value: &str) -> Result<(), ConnectorLoginError> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| ConnectorLoginError::DiscoveryFailed(error.to_string()))?;
    if url.scheme() == "https" || is_loopback_url(&url) {
        Ok(())
    } else {
        Err(ConnectorLoginError::DiscoveryFailed(
            "connector OAuth endpoints must be https or loopback".to_owned(),
        ))
    }
}

/// Whether `redirect_uri` is an `https` URL (a remote callback).
fn is_https_redirect(redirect_uri: &str) -> bool {
    reqwest::Url::parse(redirect_uri).is_ok_and(|url| url.scheme() == "https")
}

/// A registered remote redirect: an absolute `https` URL with a host and no
/// userinfo or fragment.
fn valid_registered_redirect(redirect_uri: &str) -> bool {
    reqwest::Url::parse(redirect_uri).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url.as_str() == redirect_uri
    })
}

/// The host-only handle of a remote attempt: an HMAC over the slot, state
/// and admitted redirect under a key that never leaves this process.
fn remote_attempt_handle(slot: &CredentialAccountRef, state: &str, redirect_uri: &str) -> String {
    use base64::Engine as _;
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let key = KEY.get_or_init(|| {
        crate::oauth_flow::OAuthFlowRegistry::new_state()
            .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string())
    });
    let message = format!(
        "connector-remote-attempt-v1\0{}\0{}\0{}\0{}",
        slot.realm.as_str(),
        slot.account.as_str(),
        state,
        redirect_uri
    );
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::connector_protocol::hmac_sha256(
        key.as_bytes(),
        message.as_bytes(),
    ))
}

/// RFC 8252 section 7.3: the redirect must be an http loopback address.
fn require_loopback_redirect(redirect_uri: &str) -> Result<(), ConnectorLoginError> {
    let url =
        reqwest::Url::parse(redirect_uri).map_err(|_| ConnectorLoginError::InvalidRedirect)?;
    if url.scheme() != "http" || !is_loopback_url(&url) {
        return Err(ConnectorLoginError::InvalidRedirect);
    }
    Ok(())
}

fn map_mutation_error(error: CredentialMutationError) -> ConnectorLoginError {
    match error {
        CredentialMutationError::StalePreparation => ConnectorLoginError::StalePreparation,
        CredentialMutationError::SlotRefused(refusal) => ConnectorLoginError::Slot(refusal),
        CredentialMutationError::TokenStore(reason) => ConnectorLoginError::TokenStore(reason),
        CredentialMutationError::AuthLifecycle(reason)
        | CredentialMutationError::Operation(reason)
        | CredentialMutationError::LockFailed(reason)
        | CredentialMutationError::RollbackFailed(reason) => {
            ConnectorLoginError::AuthLifecycle(reason)
        }
        CredentialMutationError::Cancelled => ConnectorLoginError::AuthLifecycle(
            "credential mutation coordinator cancelled the login commit".to_owned(),
        ),
        CredentialMutationError::CredentialAccess { refusal, detail } => {
            ConnectorLoginError::CredentialAccess { refusal, detail }
        }
    }
}

/// The typed refusal when the refresh closure was accepted; an
/// infrastructure failure, never a refusal, when it was not.
fn refusal_after_closure(closure: Result<(), String>, refusal: RefreshError) -> RefreshError {
    match closure {
        Ok(()) => refusal,
        Err(error) => RefreshError::Refresh(format!(
            "{refusal}; AuthMachine refresh_failed rejected closure: {error}"
        )),
    }
}

/// Why unattended use of a slot is refused at once: an operating-system
/// credential call still physically holds the slot's lifecycle owner.
const LIFECYCLE_OWNER_BUSY: &str =
    "an operating-system credential call for this slot is still running";

/// Unattended use of a slot whose lifecycle owner an operating-system
/// credential call still physically holds: refused at once, never waited for.
fn lifecycle_owner_busy() -> ConnectorLoginError {
    ConnectorLoginError::CredentialAccess {
        refusal: meerkat_core::auth::TokenStoreRefusal::Pending,
        detail: LIFECYCLE_OWNER_BUSY.into(),
    }
}

/// A status rehydration error, keeping a refused credential access typed.
fn login_rehydrate_error(error: meerkat_core::AuthStatusRehydrateError) -> ConnectorLoginError {
    match error {
        meerkat_core::AuthStatusRehydrateError::TokenStore(store) => login_store_error(store),
        meerkat_core::AuthStatusRehydrateError::MarkerSave(store) => {
            match store.credential_access() {
                Some((refusal, detail)) => {
                    ConnectorLoginError::CredentialAccess { refusal, detail }
                }
                None => ConnectorLoginError::AuthLifecycle(
                    meerkat_core::AuthStatusRehydrateError::MarkerSave(store).to_string(),
                ),
            }
        }
        other => ConnectorLoginError::AuthLifecycle(other.to_string()),
    }
}

/// A token-store error, keeping a refused or failed credential access typed.
fn login_store_error(error: crate::auth_store::TokenStoreError) -> ConnectorLoginError {
    match error.credential_access() {
        Some((refusal, detail)) => ConnectorLoginError::CredentialAccess { refusal, detail },
        None => ConnectorLoginError::TokenStore(error.to_string()),
    }
}

fn refresh_error_from_login(error: ConnectorLoginError) -> RefreshError {
    match error {
        ConnectorLoginError::StalePreparation => RefreshError::StalePreparation,
        ConnectorLoginError::Verification(ConnectorOAuthRefusal::AccountMismatch) => {
            RefreshError::CredentialIdentityMismatch
        }
        ConnectorLoginError::Verification(ConnectorOAuthRefusal::MissingScopes) => {
            RefreshError::RequiredScopesNotGranted
        }
        ConnectorLoginError::ReauthRequired => RefreshError::ReauthRequired(error.to_string()),
        ConnectorLoginError::CredentialAccess { refusal, detail } => {
            RefreshError::CredentialAccess { refusal, detail }
        }
        other => RefreshError::Refresh(other.to_string()),
    }
}

fn map_refresh_error(error: RefreshError) -> ConnectorLoginError {
    if matches!(error, RefreshError::StalePreparation) {
        return ConnectorLoginError::StalePreparation;
    }
    if matches!(error, RefreshError::CredentialIdentityMismatch) {
        return ConnectorOAuthRefusal::AccountMismatch.into();
    }
    if matches!(error, RefreshError::RequiredScopesNotGranted) {
        return ConnectorOAuthRefusal::MissingScopes.into();
    }
    if let RefreshError::DurableTerminalCommit { message, .. } = &error {
        return ConnectorLoginError::TokenStore(message.clone());
    }
    if matches!(&error, RefreshError::ReauthRequired(_))
        || error.refresh_failure_disposition() == Some(RefreshFailureDisposition::ReauthRequired)
    {
        return ConnectorLoginError::ReauthRequired;
    }
    match &error {
        // A destination the host egress profile refused: a typed local
        // refusal, never a credential verdict.
        RefreshError::EgressRefused { refusal, .. } => {
            ConnectorOAuthRefusal::EgressRefused(*refusal).into()
        }
        // A failure the token endpoint reported: an upstream failure.
        RefreshError::Classified { .. } | RefreshError::Observed { .. } => {
            ConnectorLoginError::RefreshFailed(error.to_string())
        }
        // A changed capture: typed stale outcome, never a refusal.
        RefreshError::StalePreparation => ConnectorLoginError::StalePreparation,
        // A local lifecycle, lock or closure failure: infrastructure.
        RefreshError::Refresh(_)
        | RefreshError::Cancelled
        | RefreshError::LockFailed(_)
        | RefreshError::CredentialIdentityMismatch
        | RefreshError::RequiredScopesNotGranted
        | RefreshError::ReauthRequired(_)
        | RefreshError::DurableTerminalCommit { .. } => {
            ConnectorLoginError::AuthLifecycle(error.to_string())
        }
        // The credential store refused access: not a refresh failure.
        RefreshError::CredentialAccess { refusal, detail } => {
            ConnectorLoginError::CredentialAccess {
                refusal: *refusal,
                detail: detail.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each credential-store refusal, and a failed backend, keeps its type
    /// through the admitted load, the refresh round trip and the mutation
    /// exit.
    #[test]
    fn credential_access_refusals_keep_their_type_through_every_conversion() {
        use crate::auth_store::TokenStoreError;
        use meerkat_core::auth::TokenStoreRefusal as Refusal;
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
            let loaded = login_store_error(error);
            assert!(
                matches!(
                    &loaded,
                    ConnectorLoginError::CredentialAccess { refusal: kept, detail }
                        if *kept == refusal && detail == "the platform cause"
                ),
                "admitted load: {loaded:?}"
            );
            assert!(!loaded.is_refusal(), "not a refused request");
            let refreshed = refresh_error_from_login(loaded);
            assert!(
                matches!(&refreshed, RefreshError::CredentialAccess { refusal: kept, .. } if *kept == refusal),
                "refresh: {refreshed:?}"
            );
            let restored = map_refresh_error(refreshed);
            assert!(
                matches!(&restored, ConnectorLoginError::CredentialAccess { refusal: kept, .. } if *kept == refusal),
                "refresh exit: {restored:?}"
            );
            let mutated = map_mutation_error(CredentialMutationError::CredentialAccess {
                refusal,
                detail: "the platform cause".into(),
            });
            assert!(
                matches!(&mutated, ConnectorLoginError::CredentialAccess { refusal: kept, .. } if *kept == refusal),
                "mutation exit: {mutated:?}"
            );
        }
    }

    #[test]
    fn stale_preparation_preserves_infrastructure_classification() {
        let error = map_mutation_error(CredentialMutationError::StalePreparation);
        assert!(matches!(error, ConnectorLoginError::StalePreparation));
        assert!(!error.is_refusal());
        let refresh = refresh_error_from_login(error);
        assert!(matches!(refresh, RefreshError::StalePreparation));
        let restored = map_refresh_error(refresh);
        assert!(matches!(restored, ConnectorLoginError::StalePreparation));
        assert!(!restored.is_refusal());
    }

    #[test]
    fn flow_owner_failures_are_infrastructure_and_attempt_errors_are_refusals() {
        for error in [
            OAuthFlowError::RegistryProjectionMissing { operation: "x" },
            OAuthFlowError::StateGenerationFailed,
            OAuthFlowError::LifecycleRejected {
                operation: "x",
                detail: "y".into(),
            },
            OAuthFlowError::PersistenceFailed {
                operation: "x",
                detail: "y".into(),
            },
        ] {
            assert!(!ConnectorLoginError::Flow(error).is_refusal());
        }
        for error in [
            OAuthFlowError::Missing,
            OAuthFlowError::BrowserIdentityMismatch,
            OAuthFlowError::RedirectUriMismatch,
            OAuthFlowError::Connector(ConnectorOAuthRefusal::DescriptorMismatch),
        ] {
            assert!(ConnectorLoginError::Flow(error).is_refusal());
        }
    }

    #[test]
    fn a_rejected_refresh_closure_is_never_reported_as_a_refusal() {
        assert!(matches!(
            map_refresh_error(refusal_after_closure(
                Ok(()),
                RefreshError::RequiredScopesNotGranted
            )),
            ConnectorLoginError::Verification(ConnectorOAuthRefusal::MissingScopes)
        ));
        assert!(matches!(
            map_refresh_error(refusal_after_closure(
                Ok(()),
                RefreshError::CredentialIdentityMismatch
            )),
            ConnectorLoginError::Verification(ConnectorOAuthRefusal::AccountMismatch)
        ));
        for refusal in [
            RefreshError::RequiredScopesNotGranted,
            RefreshError::CredentialIdentityMismatch,
        ] {
            let error = map_refresh_error(refusal_after_closure(
                Err("transition rejected".into()),
                refusal,
            ));
            assert!(matches!(error, ConnectorLoginError::AuthLifecycle(_)));
            assert!(!error.is_refusal());
        }
    }
}

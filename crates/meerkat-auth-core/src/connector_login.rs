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
    ProviderAuthPersistence, RefreshError, TokenKey,
};
use crate::connector_oauth::{
    AccountSelection, ConnectorAccountObservation, ConnectorCredentialMetadata,
    ConnectorOAuthDescriptor, ConnectorOAuthParameters, ConnectorOAuthRefusal, ScopeEvidence,
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
}

impl std::fmt::Debug for ConnectorOAuthTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorOAuthTarget")
            .field("slot", &self.slot)
            .field("strategy_id", &self.strategy_id)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
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
    /// The default set: OIDC UserInfo (`oidc-userinfo-v1`).
    pub fn with_defaults() -> Self {
        Self::default().with(Arc::new(OidcUserInfoAccountStrategy::new()))
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
}

impl std::fmt::Debug for ConnectorLoginStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorLoginStart")
            .field("slot", &self.slot)
            .field("authorize_url", &"<redacted>")
            .field("state", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

/// The host's loopback callback for [`ConnectorOAuthAuthority::login_complete`].
/// Everything else is taken from the admitted attempt.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectorOAuthCallback {
    pub redirect_uri: String,
    pub state: String,
    pub code: String,
}

impl std::fmt::Debug for ConnectorOAuthCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorOAuthCallback")
            .field("redirect_uri", &self.redirect_uri)
            .field("state", &"<redacted>")
            .field("code", &"<redacted>")
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
    #[error("connector OAuth strategy is not installed on this host")]
    UnknownStrategy,
    #[error("connector OAuth redirect must be an http loopback URI")]
    InvalidRedirect,
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
    #[error("connector OAuth token refresh failed: {0}")]
    RefreshFailed(String),
    #[error("connector OAuth stored credential requires reauthentication")]
    ReauthRequired,
    #[error("connector OAuth token store error: {0}")]
    TokenStore(String),
    #[error("connector OAuth credential lifecycle error: {0}")]
    AuthLifecycle(String),
    /// The redirect-free credential HTTP client could not be built.
    #[error(transparent)]
    HttpClientUnavailable(#[from] crate::mcp_oauth::CredentialHttpClientUnavailable),
}

impl ConnectorLoginError {
    /// Whether this refuses the caller's request rather than reporting an
    /// infrastructure or upstream failure.
    pub fn is_refusal(&self) -> bool {
        match self {
            Self::UnknownStrategy
            | Self::InvalidRedirect
            | Self::Verification(_)
            | Self::Slot(_)
            | Self::ReauthRequired => true,
            Self::Flow(error) => flow_error_is_refusal(error),
            Self::DiscoveryFailed(_)
            | Self::TokenExchangeFailed
            | Self::RefreshFailed(_)
            | Self::TokenStore(_)
            | Self::HttpClientUnavailable(_)
            | Self::AuthLifecycle(_) => false,
        }
    }
}

/// Whether a flow-owner error refuses the caller's attempt (unknown,
/// mismatched or expired state) rather than reporting a flow-owner
/// persistence or lifecycle failure.
fn flow_error_is_refusal(error: &OAuthFlowError) -> bool {
    match error {
        OAuthFlowError::Missing
        | OAuthFlowError::BrowserIdentityMismatch
        | OAuthFlowError::Connector(_)
        | OAuthFlowError::ProviderMismatch { .. }
        | OAuthFlowError::RedirectUriMismatch
        | OAuthFlowError::TargetMismatch { .. }
        | OAuthFlowError::DevicePollInProgress
        | OAuthFlowError::DeviceCodeAlreadyAdmitted
        | OAuthFlowError::DeviceExpiryOutOfRange => true,
        OAuthFlowError::RegistryProjectionMissing { .. }
        | OAuthFlowError::StateGenerationFailed
        | OAuthFlowError::LifecycleRejected { .. }
        | OAuthFlowError::PersistenceFailed { .. } => false,
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

/// The native connector OAuth owner.
#[derive(Clone)]
pub struct ConnectorOAuthAuthority {
    http: Client,
    persistence: ProviderAuthPersistence,
    auth_lease: GeneratedAuthLeaseHandle,
    flows: Arc<dyn OAuthFlowAuthority>,
    strategies: ConnectorStrategies,
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
        })
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
        require_loopback_redirect(redirect_uri)?;
        // A connector credential is always account-verified; the unverified
        // resource grant is an MCP-only host opt-in.
        if target.account.is_unverified() {
            return Err(ConnectorOAuthRefusal::InvalidDescriptor.into());
        }
        let strategy = self.strategies.get(&target.strategy_id)?;
        let descriptor: ConnectorOAuthDescriptor = ConnectorOAuthParameters {
            issuer: target.issuer.clone(),
            client: target.client.clone(),
            resource: target.resource.clone(),
            scopes: target.scopes.clone(),
            redirect_uri: redirect_uri.to_owned(),
            expected_account: target.account.clone(),
            strategy_id: target.strategy_id.clone(),
        }
        .try_into()?;
        let endpoints = self.discover(&target.issuer).await?;
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
        let mut oauth = oauth_endpoints(&descriptor, &endpoints);
        if let Some(nonce) = nonce {
            oauth
                .extra_authorize_params
                .push(("nonce".to_owned(), nonce));
        }
        Ok(ConnectorLoginStart {
            slot: target.slot.clone(),
            authorize_url: oauth.authorize_url_with_pkce(&pkce.challenge, &state),
            state,
            redirect_uri: redirect_uri.to_owned(),
        })
    }

    /// Complete one admitted attempt from the host's loopback callback.
    ///
    /// The flow owner must name a live attempt admitted under `state` for
    /// this exact slot. Issuer, client, resource, scopes, strategy and
    /// account selection come only from that admitted descriptor. Every
    /// non-success exit before the commit retires the attempt; the commit
    /// consumes it, and a consumed attempt cannot be replayed.
    pub async fn login_complete(
        &self,
        slot: &CredentialAccountRef,
        callback: ConnectorOAuthCallback,
    ) -> Result<ConnectorLoginComplete, ConnectorLoginError> {
        let ConnectorOAuthCallback {
            redirect_uri,
            state,
            code,
        } = callback;
        let identity = AuthCredentialIdentity::Account(slot.clone());
        let admitted = self
            .flows
            .admitted_connector_browser_attempt(&state, &identity)
            .map_err(ConnectorLoginError::Flow)?
            .ok_or(ConnectorLoginError::Flow(OAuthFlowError::Missing))?;
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
        let endpoints = self.discover(&facts.issuer).await?;
        let token = exchange_authorization_code_with_state(
            &self.http,
            &oauth_endpoints(&descriptor, &endpoints),
            &code,
            &record.pkce_verifier,
            None,
            Some(&state),
        )
        .await
        .map_err(|_| ConnectorLoginError::TokenExchangeFailed)?;
        let observation = strategy
            .observe_account(&descriptor, &token, record.nonce.as_deref())
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
        let identity = AuthCredentialIdentity::Account(slot.clone());
        let key = TokenKey::from_credential_identity(&identity);
        let store = self.persistence.token_store();
        let auth_lease = self.auth_lease.clone();
        let load_key = key.clone();
        self.persistence
            .refresh_coordinator()
            .with_exclusive_mutation(
                key,
                Box::new(move || {
                    Box::pin(async move {
                        let lease_key = LeaseKey::from_credential_identity(&identity);
                        let _guard =
                            meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
                        let stored = store
                            .load(&load_key)
                            .await
                            .map_err(|error| CredentialMutationError::TokenStore(error.to_string()))?;
                        match stored {
                            None => {}
                            Some(tokens) if tokens.auth_mode != PersistedAuthMode::ConnectorOauth => {
                                return Err(CredentialMutationError::SlotRefused(
                                    CredentialSlotRefusal::ModeMismatch,
                                ));
                            }
                            Some(_) => {
                                meerkat_core::clear_tokens_and_publish_lifecycle_released_for_identity(
                                    store.as_ref(),
                                    &auth_lease,
                                    &identity,
                                )
                                .await
                                .map_err(|error| {
                                    CredentialMutationError::AuthLifecycle(error.to_string())
                                })?;
                            }
                        }
                        Ok(crate::auth_store::CredentialMutationOutcome::Cleared)
                    })
                }),
            )
            .await
            .map(|_| ())
            .map_err(map_mutation_error)
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
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let (phase, tokens) = match self.load_admitted(&identity, &key).await {
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
                    .load(&key)
                    .await
                    .map_err(|error| ConnectorLoginError::TokenStore(error.to_string()))?
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
            let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
            self.load_admitted(&identity, &key).await?
        };
        let Some(admitted) = admitted else {
            return Ok(None);
        };
        match admitted.disposition {
            CredentialUseDisposition::Authorized => Ok(admitted.tokens.primary_secret),
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::AlreadyRefreshing => {
                let authority = self.clone();
                let refresh_identity = identity.clone();
                let refresh_key = key.clone();
                let bound = admitted.tokens.account_id.clone();
                let admitted_context = ConnectorCredentialMetadata::from_tokens(&admitted.tokens)
                    .map(|metadata| metadata.stable_context);
                let refreshed = self
                    .persistence
                    .refresh_coordinator()
                    .with_refresh(
                        key,
                        Box::new(move || {
                            Box::pin(async move {
                                authority
                                    .refresh_under_coordinator(&refresh_identity, &refresh_key)
                                    .await
                            })
                        }),
                    )
                    .await
                    .map_err(map_refresh_error)?;
                // A coordinator may return another waiter's result or a
                // replacement's: recheck the admitted binding on that exact
                // result before use.
                if refreshed.account_id != bound
                    || refreshed.auth_mode != PersistedAuthMode::ConnectorOauth
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
                Ok(refreshed.primary_secret)
            }
            CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent => Err(ConnectorLoginError::ReauthRequired),
        }
    }

    /// Load one durable connector credential through its marker,
    /// AuthMachine projection, freshness observation and use admission. The
    /// caller holds the slot's lifecycle guard for this whole read.
    async fn load_admitted(
        &self,
        identity: &AuthCredentialIdentity,
        key: &TokenKey,
    ) -> Result<Option<AdmittedConnectorCredential>, ConnectorLoginError> {
        let store = self.persistence.token_store();
        let lease_key = LeaseKey::from_credential_identity(identity);
        let lifecycle = |error: meerkat_core::handles::DslTransitionError| {
            ConnectorLoginError::AuthLifecycle(error.to_string())
        };
        let Some(mut tokens) = store
            .load(key)
            .await
            .map_err(|error| ConnectorLoginError::TokenStore(error.to_string()))?
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
            tokens = meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
                store.as_ref(),
                &self.auth_lease,
                identity,
                PersistedAuthMode::ConnectorOauth,
                Utc::now(),
            )
            .await
            .map_err(|error| ConnectorLoginError::AuthLifecycle(error.to_string()))?
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
    async fn refresh_under_coordinator(
        &self,
        identity: &AuthCredentialIdentity,
        key: &TokenKey,
    ) -> Result<PersistedTokens, RefreshError> {
        let lease_key = LeaseKey::from_credential_identity(identity);
        let _guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key).await;
        let admitted = self
            .load_admitted(identity, key)
            .await
            .map_err(refresh_error_from_login)?
            .ok_or_else(|| {
                RefreshError::ReauthRequired(
                    "stored connector OAuth credential disappeared before refresh".to_owned(),
                )
            })?;
        if admitted.disposition == CredentialUseDisposition::Authorized {
            return Ok(admitted.tokens);
        }
        let begin = self
            .auth_lease
            .resolve_credential_use_admission(&lease_key, CredentialUseIntent::BeginRefresh)
            .map_err(|error| RefreshError::Refresh(error.to_string()))?;
        match begin {
            CredentialUseDisposition::RefreshRequired => self
                .auth_lease
                .begin_refresh(&lease_key)
                .map_err(|error| RefreshError::Refresh(error.to_string()))?,
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
            extra_token_params: vec![("resource".to_owned(), metadata.resource.clone())],
            refresh_scopes: Vec::new(),
            extra_headers: Vec::new(),
        };
        let refreshed =
            match exchange_refresh_token(&self.http, &endpoints, &refresh_token, None).await {
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
            .observe_account(&descriptor, &refreshed, None)
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
        // the same save as the access token; none keeps the existing one.
        if persisted.refresh_token.is_none() {
            persisted.refresh_token = Some(refresh_token);
        }
        let current = self.persistence.token_store().load(key).await;
        if !matches!(&current, Ok(Some(current)) if *current == previous)
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
                        error.to_string(),
                    )
                    .await);
            }
        };
        if let Err(error) = self.persistence.token_store().save(key, &published).await {
            return Err(self
                .rollback_refresh(
                    key,
                    &lease_key,
                    &previous,
                    &admitted.restore_snapshot,
                    error.to_string(),
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
                .map_err(|error| error.to_string()),
            refusal,
        )
    }

    /// Close a begun refresh as a transient failure: the credential keeps
    /// its prior durable state.
    async fn refresh_failed(&self, lease_key: &LeaseKey, reason: &str) -> RefreshError {
        let observation = meerkat_core::RefreshFailureObservation::transient();
        match self.auth_lease.refresh_failed(lease_key, observation) {
            Ok(()) => RefreshError::Refresh(reason.to_owned()),
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
            && let Err(error) = self.persistence.token_store().clear(key).await
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
        reason: String,
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
        if let Err(error) = self.persistence.token_store().save(key, &restored).await {
            errors.push(format!("TokenStore restore failed: {error}"));
        }
        if errors.is_empty() {
            RefreshError::Refresh(reason)
        } else {
            RefreshError::Refresh(format!("{reason}; {}", errors.join("; ")))
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
) -> OAuthEndpoints {
    let facts = descriptor.parameters();
    let scopes: Vec<String> = facts.scopes.iter().cloned().collect();
    OAuthEndpoints {
        client_id: facts.client.clone(),
        authorize_url: issuer.authorization_endpoint.clone(),
        token_url: issuer.token_endpoint.clone(),
        device_code_url: None,
        redirect_uri: facts.redirect_uri.clone(),
        scopes: scopes.clone(),
        extra_authorize_params: vec![("resource".to_owned(), facts.resource.clone())],
        token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
        include_state_in_token_exchange: false,
        extra_token_params: vec![("resource".to_owned(), facts.resource.clone())],
        refresh_scopes: scopes,
        extra_headers: Vec::new(),
    }
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
        CredentialMutationError::SlotRefused(refusal) => ConnectorLoginError::Slot(refusal),
        CredentialMutationError::TokenStore(reason) => ConnectorLoginError::TokenStore(reason),
        CredentialMutationError::AuthLifecycle(reason)
        | CredentialMutationError::Operation(reason)
        | CredentialMutationError::LockFailed(reason) => ConnectorLoginError::AuthLifecycle(reason),
        CredentialMutationError::Cancelled => ConnectorLoginError::AuthLifecycle(
            "credential mutation coordinator cancelled the login commit".to_owned(),
        ),
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

fn refresh_error_from_login(error: ConnectorLoginError) -> RefreshError {
    match error {
        ConnectorLoginError::Verification(ConnectorOAuthRefusal::AccountMismatch) => {
            RefreshError::CredentialIdentityMismatch
        }
        ConnectorLoginError::Verification(ConnectorOAuthRefusal::MissingScopes) => {
            RefreshError::RequiredScopesNotGranted
        }
        ConnectorLoginError::ReauthRequired => RefreshError::ReauthRequired(error.to_string()),
        other => RefreshError::Refresh(other.to_string()),
    }
}

fn map_refresh_error(error: RefreshError) -> ConnectorLoginError {
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
        ConnectorLoginError::ReauthRequired
    } else {
        ConnectorLoginError::RefreshFailed(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(matches!(error, ConnectorLoginError::RefreshFailed(_)));
            assert!(!error.is_refusal());
        }
    }
}

//! Host-declared connector OAuth facts for the existing browser-flow owner.
//!
//! This module does not discover providers, authenticate users, or own a flow
//! registry. A trusted host strategy verifies provider evidence; the existing
//! OAuth owner compares that typed observation against the admitted descriptor.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth_store::PersistedTokens;
use crate::oauth_flow::OAuthProviderIdentity;

/// The existing MCP login window, also used by connector browser-flow owners.
pub const CONNECTOR_BROWSER_LOGIN_WINDOW: Duration = Duration::from_secs(300);

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorOAuthParameters {
    pub issuer: String,
    pub client: String,
    pub resource: String,
    pub scopes: BTreeSet<String>,
    pub redirect_uri: String,
    /// Which provider account the attempt must prove: a known account,
    /// discovery of the account the provider verifies, or none at all for an
    /// explicitly unverified resource grant. Wire form: the account string
    /// for `Known`, `null` for `Discover`, `{"mode":"unverified"}` for
    /// `Unverified`.
    pub expected_account: AccountSelection,
    pub strategy_id: String,
}

/// Account selection of one connector browser attempt.
///
/// `Known` binds the provider account before the attempt starts and refuses
/// any other verified account. `Discover` admits the attempt with no account;
/// the provider-verified account is bound to the credential at the commit,
/// which publishes only into an empty credential slot. `Unverified` is an
/// explicit host opt-in for a resource-bound grant without account
/// evidence: no account is observed, bound or reported, and its commit
/// publishes only into an empty credential slot.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum AccountSelection {
    Known(String),
    Discover,
    Unverified,
}

impl AccountSelection {
    /// The selected account, when the attempt names one.
    pub fn known(&self) -> Option<&str> {
        match self {
            Self::Known(account) => Some(account),
            Self::Discover | Self::Unverified => None,
        }
    }

    pub fn is_discover(&self) -> bool {
        matches!(self, Self::Discover)
    }

    pub fn is_unverified(&self) -> bool {
        matches!(self, Self::Unverified)
    }
}

impl From<String> for AccountSelection {
    fn from(account: String) -> Self {
        Self::Known(account)
    }
}

impl From<&str> for AccountSelection {
    fn from(account: &str) -> Self {
        Self::Known(account.to_owned())
    }
}

impl fmt::Debug for AccountSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(_) => f.write_str("Known(..)"),
            Self::Discover => f.write_str("Discover"),
            Self::Unverified => f.write_str("Unverified"),
        }
    }
}

/// Object form of the selections that are neither an account nor `null`.
#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum AccountSelectionMarker {
    Unverified,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AccountSelectionRepr {
    Known(String),
    Marker(AccountSelectionMarker),
}

impl Serialize for AccountSelection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Known(account) => serializer.serialize_some(account),
            Self::Discover => serializer.serialize_none(),
            Self::Unverified => AccountSelectionMarker::Unverified.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AccountSelection {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(
            match Option::<AccountSelectionRepr>::deserialize(deserializer)? {
                Some(AccountSelectionRepr::Known(account)) => Self::Known(account),
                Some(AccountSelectionRepr::Marker(AccountSelectionMarker::Unverified)) => {
                    Self::Unverified
                }
                None => Self::Discover,
            },
        )
    }
}

/// Framed into the attempt key in place of an account for `Discover`. It
/// contains a control character, which no valid account can, so a Discover
/// key never equals a Known key.
const DISCOVER_KEY_TAG: &str = "\u{0}connector-account-discover-v1";
/// Framed into the attempt key in place of an account for `Unverified`;
/// distinct from every Known and Discover key for the same reason.
const UNVERIFIED_KEY_TAG: &str = "\u{0}connector-account-unverified-v1";

/// Immutable validated facts bound to one browser attempt. These are host
/// inputs, not a provider selected by agent text and not verified account data.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    try_from = "ConnectorOAuthParameters",
    into = "ConnectorOAuthParameters"
)]
pub struct ConnectorOAuthDescriptor(ConnectorOAuthParameters);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorOAuthRefusal {
    #[error("invalid connector OAuth descriptor")]
    InvalidDescriptor,
    #[error("connector OAuth descriptor mismatch")]
    DescriptorMismatch,
    #[error("connector OAuth actual account mismatch")]
    AccountMismatch,
    #[error("connector OAuth required scopes were not granted")]
    MissingScopes,
    #[error("connector OAuth credential differs from verified account evidence")]
    CredentialMismatch,
    #[error("connector OAuth account verification is unavailable")]
    VerificationUnavailable,
}

fn valid_atom(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

fn valid_url(value: &str, allow_query: bool) -> bool {
    valid_atom(value)
        && reqwest::Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && (allow_query || url.query().is_none())
        })
}

impl TryFrom<ConnectorOAuthParameters> for ConnectorOAuthDescriptor {
    type Error = ConnectorOAuthRefusal;

    fn try_from(value: ConnectorOAuthParameters) -> Result<Self, Self::Error> {
        if !valid_url(&value.issuer, false)
            || !valid_url(&value.resource, false)
            || !valid_url(&value.redirect_uri, true)
            || !valid_atom(&value.client)
            || value
                .expected_account
                .known()
                .is_some_and(|account| !valid_atom(account))
            || !valid_atom(&value.strategy_id)
            // An unverified resource grant may request no scope: the
            // resource need not publish any, and no account evidence scope
            // is requested for it.
            || (value.scopes.is_empty() && !value.expected_account.is_unverified())
            || value.scopes.len() > 256
            || value
                .scopes
                .iter()
                .any(|scope| !valid_atom(scope) || scope.chars().any(char::is_whitespace))
        {
            return Err(ConnectorOAuthRefusal::InvalidDescriptor);
        }
        Ok(Self(value))
    }
}

impl From<ConnectorOAuthDescriptor> for ConnectorOAuthParameters {
    fn from(value: ConnectorOAuthDescriptor) -> Self {
        value.0
    }
}

impl ConnectorOAuthDescriptor {
    pub fn parameters(&self) -> &ConnectorOAuthParameters {
        &self.0
    }

    /// Mechanical, opaque equality key for the existing generated AuthMachine
    /// provider field. No caller interprets this key to select a provider.
    pub fn binding_key(&self) -> String {
        let mut hash = Sha256::new();
        for value in [
            self.0.issuer.as_str(),
            &self.0.client,
            &self.0.resource,
            &self.0.redirect_uri,
            match &self.0.expected_account {
                AccountSelection::Known(account) => account.as_str(),
                AccountSelection::Discover => DISCOVER_KEY_TAG,
                AccountSelection::Unverified => UNVERIFIED_KEY_TAG,
            },
            &self.0.strategy_id,
        ]
        .into_iter()
        .chain(self.0.scopes.iter().map(String::as_str))
        {
            hash.update(value.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(value.as_bytes());
        }
        format!("connector:{:x}", hash.finalize())
    }

    /// Fingerprint of the facts that must stay equal for a credential slot
    /// to accept a reconnect: issuer, client, resource and strategy. The
    /// redirect URI (loopback port) and the requested scopes are attempt
    /// facts, not part of it.
    pub fn stable_context(&self) -> ConnectorStableContext {
        let mut hash = Sha256::new();
        for value in [
            self.0.issuer.as_str(),
            &self.0.client,
            &self.0.resource,
            &self.0.strategy_id,
        ] {
            hash.update(value.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(value.as_bytes());
        }
        ConnectorStableContext(format!("connector-context:{:x}", hash.finalize()))
    }

    /// Scopes the native owner derives from a token-endpoint response: its
    /// `scope` field, or the requested scopes when the response omits it
    /// (RFC 6749 section 5.1).
    pub fn granted_scopes_from_response(
        &self,
        exchanged: &crate::auth_oauth::OAuthTokenResult,
    ) -> BTreeSet<String> {
        match exchanged.scope.as_deref() {
            Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
            None => self.0.scopes.clone(),
        }
    }

    pub fn verify_account(
        &self,
        observation: ConnectorAccountObservation,
        exchanged: &crate::auth_oauth::OAuthTokenResult,
    ) -> Result<VerifiedConnectorAccount, ConnectorOAuthRefusal> {
        self.verify_observation(&observation)?;
        if exchanged.access_token.is_empty() {
            return Err(ConnectorOAuthRefusal::CredentialMismatch);
        }
        // Granted scopes are authorized only by the response the owner's
        // own exchange parsed; a strategy cannot widen or relabel them.
        if observation.granted_scopes != self.granted_scopes_from_response(exchanged) {
            return Err(ConnectorOAuthRefusal::CredentialMismatch);
        }
        Ok(VerifiedConnectorAccount {
            descriptor: self.clone(),
            scope_evidence: ScopeEvidence::TokenEndpointResponse,
            observation,
            credential_fingerprint: secret_fingerprint(
                Some(&exchanged.access_token),
                exchanged.refresh_token.as_deref(),
                exchanged.id_token.as_deref(),
            ),
        })
    }

    /// Accept a token response for an explicitly `Unverified` descriptor.
    /// Nothing about an account is observed or claimed: the grant only
    /// records the scopes the owner's own exchange parsed, which must cover
    /// the descriptor's required scopes.
    pub fn accept_unverified_grant(
        &self,
        exchanged: &crate::auth_oauth::OAuthTokenResult,
    ) -> Result<UnverifiedResourceGrant, ConnectorOAuthRefusal> {
        if !self.0.expected_account.is_unverified() {
            return Err(ConnectorOAuthRefusal::InvalidDescriptor);
        }
        if exchanged.access_token.is_empty() {
            return Err(ConnectorOAuthRefusal::CredentialMismatch);
        }
        let granted_scopes = self.granted_scopes_from_response(exchanged);
        if !self.0.scopes.is_subset(&granted_scopes) {
            return Err(ConnectorOAuthRefusal::MissingScopes);
        }
        Ok(UnverifiedResourceGrant {
            descriptor: self.clone(),
            granted_scopes,
            credential_fingerprint: secret_fingerprint(
                Some(&exchanged.access_token),
                exchanged.refresh_token.as_deref(),
                exchanged.id_token.as_deref(),
            ),
        })
    }

    fn verify_observation(
        &self,
        observation: &ConnectorAccountObservation,
    ) -> Result<(), ConnectorOAuthRefusal> {
        match &self.0.expected_account {
            AccountSelection::Known(account) if observation.account != *account => {
                return Err(ConnectorOAuthRefusal::AccountMismatch);
            }
            AccountSelection::Known(_) => {}
            AccountSelection::Discover if !valid_atom(&observation.account) => {
                return Err(ConnectorOAuthRefusal::VerificationUnavailable);
            }
            AccountSelection::Discover => {}
            // No observation verifies a descriptor that admits no account.
            AccountSelection::Unverified => {
                return Err(ConnectorOAuthRefusal::VerificationUnavailable);
            }
        }
        if !self.0.scopes.is_subset(&observation.granted_scopes) {
            return Err(ConnectorOAuthRefusal::MissingScopes);
        }
        Ok(())
    }
}

impl fmt::Debug for ConnectorOAuthDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectorOAuthDescriptor")
            .finish_non_exhaustive()
    }
}

/// Evidence returned only by the host's provider-specific authenticated probe.
/// Construction is not verification: the native owner still compares it with
/// the exact admitted descriptor. Never fill this from labels or decoded JWTs.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectorAccountObservation {
    pub account: String,
    pub granted_scopes: BTreeSet<String>,
}

impl fmt::Debug for ConnectorAccountObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectorAccountObservation")
            .finish_non_exhaustive()
    }
}

/// Stable-context fingerprint of a connector credential (see
/// [`ConnectorOAuthDescriptor::stable_context`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConnectorStableContext(String);

impl ConnectorStableContext {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether an MCP credential's account was verified by an account strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountVerification {
    /// A Known or Discover login verified the bound account.
    Verified,
    /// An explicitly unverified resource grant: no account is bound.
    Unverified,
}

/// Account binding of an `McpOauth` credential, stored under the
/// `account_binding` member of its token metadata by the native MCP owner.
/// It decides slot compatibility for later logins and is preserved by
/// refresh. A credential without it predates account binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpCredentialBinding {
    pub verification: AccountVerification,
    pub strategy_id: String,
    pub stable_context: ConnectorStableContext,
}

impl McpCredentialBinding {
    /// The binding a credential admitted under `descriptor` is stored with.
    pub fn for_descriptor(descriptor: &ConnectorOAuthDescriptor) -> Self {
        Self {
            verification: if descriptor.parameters().expected_account.is_unverified() {
                AccountVerification::Unverified
            } else {
                AccountVerification::Verified
            },
            strategy_id: descriptor.parameters().strategy_id.clone(),
            stable_context: descriptor.stable_context(),
        }
    }

    /// The binding of an `McpOauth` credential, if it carries one.
    pub fn from_tokens(tokens: &PersistedTokens) -> Option<Self> {
        if tokens.auth_mode != crate::auth_store::PersistedAuthMode::McpOauth {
            return None;
        }
        tokens
            .metadata
            .get("account_binding")
            .and_then(|binding| serde_json::from_value(binding.clone()).ok())
    }
}

/// What authorizes a connector credential's granted scopes. The native
/// owner assigns it; no host or strategy input can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeEvidence {
    /// Parsed by the owner from the token-endpoint response that issued this
    /// access token (the requested scopes when that response omits `scope`,
    /// RFC 6749 section 5.1).
    TokenEndpointResponse,
    /// A refresh response without `scope` kept the original grant, recorded
    /// by reference and never relabelled as new evidence.
    RetainedOnRefresh { from: ScopeEvidenceRef },
}

/// Reference to the token-endpoint response that originally granted a
/// credential's scopes: the second it was committed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeEvidenceRef {
    pub granted_at_epoch_secs: i64,
}

/// Facts a `ConnectorOauth` credential is stored with, under the
/// `connector` member of its token metadata. They rebuild the refresh
/// request and decide slot compatibility; they are not secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCredentialMetadata {
    pub issuer: String,
    pub client: String,
    pub resource: String,
    pub strategy_id: String,
    /// Scopes the login requested (the required set refreshes keep).
    pub requested_scopes: BTreeSet<String>,
    pub token_endpoint: String,
    pub stable_context: ConnectorStableContext,
    pub scope_evidence: ScopeEvidence,
    /// When the scopes' original token-endpoint grant was committed.
    pub granted_at_epoch_secs: i64,
}

#[derive(Serialize, Deserialize)]
struct StoredConnectorMetadata {
    connector: ConnectorCredentialMetadata,
}

impl ConnectorCredentialMetadata {
    /// The metadata of a `ConnectorOauth` credential, if it carries it.
    pub fn from_tokens(tokens: &PersistedTokens) -> Option<Self> {
        if tokens.auth_mode != crate::auth_store::PersistedAuthMode::ConnectorOauth {
            return None;
        }
        serde_json::from_value::<StoredConnectorMetadata>(tokens.metadata.clone())
            .ok()
            .map(|stored| stored.connector)
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::json!({ "connector": self })
    }

    /// The scope evidence a successful refresh records: the refresh
    /// response's own `scope` when present, else the original grant kept by
    /// reference.
    pub fn refreshed_scope_evidence(&self, response_has_scope: bool) -> ScopeEvidence {
        if response_has_scope {
            ScopeEvidence::TokenEndpointResponse
        } else {
            ScopeEvidence::RetainedOnRefresh {
                from: match self.scope_evidence {
                    ScopeEvidence::RetainedOnRefresh { from } => from,
                    ScopeEvidence::TokenEndpointResponse => ScopeEvidenceRef {
                        granted_at_epoch_secs: self.granted_at_epoch_secs,
                    },
                },
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedConnectorAccount {
    descriptor: ConnectorOAuthDescriptor,
    observation: ConnectorAccountObservation,
    scope_evidence: ScopeEvidence,
    credential_fingerprint: [u8; 32],
}

// Private, nonserialized binding to the material observed by the strategy.
// Distinguish absent and empty secrets and frame every byte string exactly.
fn secret_fingerprint(access: Option<&str>, refresh: Option<&str>, id: Option<&str>) -> [u8; 32] {
    let mut hash = Sha256::new();
    for value in [access, refresh, id] {
        match value {
            None => hash.update([0_u8]),
            Some(value) => {
                hash.update([1_u8]);
                hash.update(value.len().to_string().as_bytes());
                hash.update(b":");
                hash.update(value.as_bytes());
            }
        }
    }
    hash.finalize().into()
}

impl VerifiedConnectorAccount {
    pub fn descriptor(&self) -> &ConnectorOAuthDescriptor {
        &self.descriptor
    }
    pub fn account(&self) -> &str {
        &self.observation.account
    }
    pub fn granted_scopes(&self) -> &BTreeSet<String> {
        &self.observation.granted_scopes
    }
    pub fn scope_evidence(&self) -> ScopeEvidence {
        self.scope_evidence
    }

    pub fn verify_tokens(&self, tokens: &PersistedTokens) -> Result<(), ConnectorOAuthRefusal> {
        if tokens.auth_mode == crate::auth_store::PersistedAuthMode::ConnectorOauth {
            let metadata = ConnectorCredentialMetadata::from_tokens(tokens)
                .ok_or(ConnectorOAuthRefusal::CredentialMismatch)?;
            let facts = self.descriptor.parameters();
            if metadata.stable_context != self.descriptor.stable_context()
                || metadata.issuer != facts.issuer
                || metadata.client != facts.client
                || metadata.resource != facts.resource
                || metadata.strategy_id != facts.strategy_id
                || metadata.requested_scopes != facts.scopes
                || metadata.scope_evidence != self.scope_evidence
            {
                return Err(ConnectorOAuthRefusal::CredentialMismatch);
            }
        }
        if tokens.auth_mode == crate::auth_store::PersistedAuthMode::McpOauth
            && McpCredentialBinding::from_tokens(tokens)
                != Some(McpCredentialBinding::for_descriptor(&self.descriptor))
        {
            return Err(ConnectorOAuthRefusal::CredentialMismatch);
        }
        let scopes = tokens.scopes.iter().cloned().collect::<BTreeSet<_>>();
        if tokens.account_id.as_deref() != Some(self.account())
            || scopes != self.observation.granted_scopes
            || secret_fingerprint(
                tokens.primary_secret.as_deref(),
                tokens.refresh_token.as_deref(),
                tokens.id_token.as_deref(),
            ) != self.credential_fingerprint
        {
            return Err(ConnectorOAuthRefusal::CredentialMismatch);
        }
        Ok(())
    }
}

impl fmt::Debug for VerifiedConnectorAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedConnectorAccount")
            .finish_non_exhaustive()
    }
}

/// Completion evidence of an explicitly `Unverified` attempt: the admitted
/// descriptor, the scopes the owner's own exchange parsed, and the exact
/// credential bytes. It carries no account and is not account evidence; it
/// cannot be converted into [`VerifiedConnectorAccount`].
#[derive(Clone, PartialEq, Eq)]
pub struct UnverifiedResourceGrant {
    descriptor: ConnectorOAuthDescriptor,
    granted_scopes: BTreeSet<String>,
    credential_fingerprint: [u8; 32],
}

impl UnverifiedResourceGrant {
    pub fn descriptor(&self) -> &ConnectorOAuthDescriptor {
        &self.descriptor
    }
    pub fn granted_scopes(&self) -> &BTreeSet<String> {
        &self.granted_scopes
    }

    fn verify_scopes(&self) -> Result<(), ConnectorOAuthRefusal> {
        let facts = self.descriptor.parameters();
        if !facts.expected_account.is_unverified() {
            return Err(ConnectorOAuthRefusal::InvalidDescriptor);
        }
        if !facts.scopes.is_subset(&self.granted_scopes) {
            return Err(ConnectorOAuthRefusal::MissingScopes);
        }
        Ok(())
    }

    /// The persisted credential is exactly the unverified grant: an
    /// `McpOauth` credential with no account, the granted scopes, the
    /// exchanged bytes and the unverified binding of this descriptor.
    fn verify_tokens(&self, tokens: &PersistedTokens) -> Result<(), ConnectorOAuthRefusal> {
        let scopes = tokens.scopes.iter().cloned().collect::<BTreeSet<_>>();
        if tokens.auth_mode != crate::auth_store::PersistedAuthMode::McpOauth
            || tokens.account_id.is_some()
            || scopes != self.granted_scopes
            || McpCredentialBinding::from_tokens(tokens)
                != Some(McpCredentialBinding::for_descriptor(&self.descriptor))
            || secret_fingerprint(
                tokens.primary_secret.as_deref(),
                tokens.refresh_token.as_deref(),
                tokens.id_token.as_deref(),
            ) != self.credential_fingerprint
        {
            return Err(ConnectorOAuthRefusal::CredentialMismatch);
        }
        Ok(())
    }
}

impl fmt::Debug for UnverifiedResourceGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnverifiedResourceGrant")
            .finish_non_exhaustive()
    }
}

/// Browser-flow identity. Untagged serialization preserves every existing LLM
/// provider string; connector identities have a distinct typed object shape.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum OAuthBrowserFlowIdentity {
    Provider(OAuthProviderIdentity),
    Connector {
        connector: Box<ConnectorOAuthDescriptor>,
    },
}

impl OAuthBrowserFlowIdentity {
    pub fn binding_key(&self) -> String {
        match self {
            Self::Provider(provider) => provider.canonical_alias().to_owned(),
            Self::Connector { connector } => connector.binding_key(),
        }
    }

    pub fn lifetime(&self, provider_lifetime: Duration) -> Duration {
        match self {
            Self::Provider(_) => provider_lifetime,
            Self::Connector { .. } => provider_lifetime.min(CONNECTOR_BROWSER_LOGIN_WINDOW),
        }
    }

    pub fn validate_redirect(&self, redirect_uri: &str) -> Result<(), ConnectorOAuthRefusal> {
        if let Self::Connector { connector } = self
            && connector.parameters().redirect_uri != redirect_uri
        {
            return Err(ConnectorOAuthRefusal::DescriptorMismatch);
        }
        Ok(())
    }
}

impl From<OAuthProviderIdentity> for OAuthBrowserFlowIdentity {
    fn from(value: OAuthProviderIdentity) -> Self {
        Self::Provider(value)
    }
}
impl From<ConnectorOAuthDescriptor> for OAuthBrowserFlowIdentity {
    fn from(value: ConnectorOAuthDescriptor) -> Self {
        Self::Connector {
            connector: Box::new(value),
        }
    }
}
impl fmt::Debug for OAuthBrowserFlowIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(provider) => fmt::Debug::fmt(provider, f),
            Self::Connector { .. } => f.write_str("Connector { .. }"),
        }
    }
}

/// Completion carries the original LLM-provider contract, checked
/// connector account evidence, or an explicitly unverified resource grant.
/// There is no conversion from an unverified connector identity to account
/// evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthBrowserFlowCompletion {
    Provider(OAuthProviderIdentity),
    Connector(VerifiedConnectorAccount),
    UnverifiedResource(UnverifiedResourceGrant),
}

impl OAuthBrowserFlowCompletion {
    /// Recheck typed evidence at the terminal owner boundary. The evidence is
    /// immutable and cannot be constructed from an unverified descriptor.
    pub fn verify_account_and_scopes(&self) -> Result<(), ConnectorOAuthRefusal> {
        match self {
            Self::Provider(_) => Ok(()),
            Self::Connector(evidence) => evidence
                .descriptor
                .verify_observation(&evidence.observation),
            Self::UnverifiedResource(grant) => grant.verify_scopes(),
        }
    }

    pub fn identity(&self) -> OAuthBrowserFlowIdentity {
        match self {
            Self::Provider(provider) => (*provider).into(),
            Self::Connector(evidence) => evidence.descriptor.clone().into(),
            Self::UnverifiedResource(grant) => grant.descriptor.clone().into(),
        }
    }

    pub fn verify_tokens(&self, tokens: &PersistedTokens) -> Result<(), ConnectorOAuthRefusal> {
        match self {
            Self::Provider(_) => Ok(()),
            Self::Connector(evidence) => evidence.verify_tokens(tokens),
            Self::UnverifiedResource(grant) => grant.verify_tokens(tokens),
        }
    }

    /// Whether `previous`, the credential currently in the slot, may be
    /// replaced by this completion's `tokens`. Decided inside the slot's
    /// exclusive mutation, so racing completions are serialized.
    ///
    /// - Any publication into an empty slot is admitted.
    /// - A `ConnectorOauth` credential is replaced only by a `Known`
    ///   connector completion for the same verified account and stable
    ///   context. A `Discover` completion never replaces anything, even the
    ///   same account and context with an independent grant.
    /// - An `McpOauth` credential published by a connector-shaped
    ///   completion follows the same rules: `Discover` and unverified
    ///   completions never replace anything; a `Known` completion replaces
    ///   only a verified `McpOauth` credential with the same stable context
    ///   and verified account.
    /// - Other modes keep their existing replacement rules, but never
    ///   replace a `ConnectorOauth` credential.
    pub fn admit_into_slot(
        &self,
        previous: Option<&PersistedTokens>,
        tokens: &PersistedTokens,
    ) -> Result<(), crate::auth_store::CredentialSlotRefusal> {
        use crate::auth_store::{CredentialSlotRefusal, PersistedAuthMode};
        if tokens.auth_mode == PersistedAuthMode::ConnectorOauth
            && !matches!(self, Self::Connector(_))
        {
            return Err(CredentialSlotRefusal::UnverifiedConnectorPublication);
        }
        if let Self::UnverifiedResource(_) = self
            && tokens.auth_mode != PersistedAuthMode::McpOauth
        {
            return Err(CredentialSlotRefusal::ModeMismatch);
        }
        let Some(previous) = previous else {
            return Ok(());
        };
        match self {
            // Replacing an unverified grant needs an explicit disconnect:
            // without an account a re-login could switch accounts silently.
            Self::UnverifiedResource(_) => return Err(CredentialSlotRefusal::Occupied),
            Self::Connector(evidence) if tokens.auth_mode == PersistedAuthMode::McpOauth => {
                return admit_mcp_replacement(evidence, previous);
            }
            Self::Provider(_) | Self::Connector(_) => {}
        }
        let previous_is_connector = previous.auth_mode == PersistedAuthMode::ConnectorOauth;
        let connector = match self {
            Self::Connector(evidence) if tokens.auth_mode == PersistedAuthMode::ConnectorOauth => {
                evidence
            }
            Self::Provider(_) | Self::Connector(_) | Self::UnverifiedResource(_) => {
                return if previous_is_connector {
                    Err(CredentialSlotRefusal::ModeMismatch)
                } else {
                    Ok(())
                };
            }
        };
        let AccountSelection::Known(account) = &connector.descriptor.parameters().expected_account
        else {
            return Err(CredentialSlotRefusal::Occupied);
        };
        if !previous_is_connector {
            return Err(CredentialSlotRefusal::ModeMismatch);
        }
        let previous_context = ConnectorCredentialMetadata::from_tokens(previous)
            .map(|metadata| metadata.stable_context);
        if previous_context.as_ref() != Some(&connector.descriptor.stable_context()) {
            return Err(CredentialSlotRefusal::ContextMismatch);
        }
        if previous.account_id.as_deref() != Some(account.as_str()) {
            return Err(CredentialSlotRefusal::AccountMismatch);
        }
        Ok(())
    }
}
/// The `McpOauth` arm of [`OAuthBrowserFlowCompletion::admit_into_slot`]
/// for an occupied slot.
fn admit_mcp_replacement(
    evidence: &VerifiedConnectorAccount,
    previous: &PersistedTokens,
) -> Result<(), crate::auth_store::CredentialSlotRefusal> {
    use crate::auth_store::{CredentialSlotRefusal, PersistedAuthMode};
    let AccountSelection::Known(account) = &evidence.descriptor.parameters().expected_account
    else {
        // A Discover commit publishes only into a still-empty slot, even
        // when the racing occupant has the same subject.
        return Err(CredentialSlotRefusal::Occupied);
    };
    if previous.auth_mode != PersistedAuthMode::McpOauth {
        return Err(CredentialSlotRefusal::ModeMismatch);
    }
    let Some(binding) = McpCredentialBinding::from_tokens(previous) else {
        return Err(CredentialSlotRefusal::ContextMismatch);
    };
    if binding.verification != AccountVerification::Verified
        || binding.stable_context != evidence.descriptor.stable_context()
    {
        return Err(CredentialSlotRefusal::ContextMismatch);
    }
    if previous.account_id.as_deref() != Some(account.as_str()) {
        return Err(CredentialSlotRefusal::AccountMismatch);
    }
    Ok(())
}

impl From<UnverifiedResourceGrant> for OAuthBrowserFlowCompletion {
    fn from(value: UnverifiedResourceGrant) -> Self {
        Self::UnverifiedResource(value)
    }
}

impl From<OAuthProviderIdentity> for OAuthBrowserFlowCompletion {
    fn from(value: OAuthProviderIdentity) -> Self {
        Self::Provider(value)
    }
}
impl From<VerifiedConnectorAccount> for OAuthBrowserFlowCompletion {
    fn from(value: VerifiedConnectorAccount) -> Self {
        Self::Connector(value)
    }
}

/// Opaque correlation projection only. Possessing it grants no flow access and
/// cannot be used as an OAuth state value or credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OAuthBrowserActionRef(String);
impl OAuthBrowserActionRef {
    pub fn project(state: &str) -> Self {
        Self(format!(
            "oauth-action:{:x}",
            Sha256::digest(state.as_bytes())
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn exchanged() -> crate::auth_oauth::OAuthTokenResult {
        crate::auth_oauth::OAuthTokenResult {
            access_token: "fixture-secret".into(),
            refresh_token: None,
            id_token: None,
            expires_in_secs: None,
            scope: Some("mcp.read".into()),
        }
    }

    fn parameters() -> ConnectorOAuthParameters {
        ConnectorOAuthParameters {
            issuer: "https://issuer.example".into(),
            client: "registered-client".into(),
            resource: "https://service.example/mcp".into(),
            scopes: ["mcp.read".into()].into(),
            redirect_uri: "http://127.0.0.1:12345/callback".into(),
            expected_account: "account-a".into(),
            strategy_id: "verified-profile-v1".into(),
        }
    }

    #[test]
    fn every_admitted_descriptor_field_changes_the_native_binding_key() {
        let original: ConnectorOAuthDescriptor = parameters().try_into().unwrap();
        for field in 0..7 {
            let mut value = parameters();
            match field {
                0 => value.issuer = "https://other-issuer.example".into(),
                1 => value.client = "other-client".into(),
                2 => value.resource = "https://other-resource.example".into(),
                3 => {
                    value.scopes.insert("mcp.write".into());
                }
                4 => value.redirect_uri = "http://127.0.0.1:12346/callback".into(),
                5 => value.expected_account = "account-b".into(),
                _ => value.strategy_id = "verified-profile-v2".into(),
            }
            let changed: ConnectorOAuthDescriptor = value.try_into().unwrap();
            assert_ne!(original, changed);
            assert_ne!(original.binding_key(), changed.binding_key());
        }
    }

    /// The pre-`AccountSelection` key algorithm, inlined: a Known attempt
    /// admitted before the change keeps matching after it.
    fn legacy_binding_key(value: &ConnectorOAuthParameters, account: &str) -> String {
        let mut hash = Sha256::new();
        for field in [
            value.issuer.as_str(),
            &value.client,
            &value.resource,
            &value.redirect_uri,
            account,
            &value.strategy_id,
        ]
        .into_iter()
        .chain(value.scopes.iter().map(String::as_str))
        {
            hash.update(field.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(field.as_bytes());
        }
        format!("connector:{:x}", hash.finalize())
    }

    #[test]
    fn known_keys_are_unchanged_and_discover_keys_never_equal_a_known_key() {
        let known: ConnectorOAuthDescriptor = parameters().try_into().unwrap();
        assert_eq!(
            known.binding_key(),
            legacy_binding_key(&parameters(), "account-a")
        );
        let mut value = parameters();
        value.expected_account = AccountSelection::Discover;
        let discover: ConnectorOAuthDescriptor = value.clone().try_into().unwrap();
        assert_ne!(discover.binding_key(), known.binding_key());
        // No valid account can spell the Discover tag: it has a control char.
        value.expected_account = AccountSelection::Known(DISCOVER_KEY_TAG.to_owned());
        assert_eq!(
            ConnectorOAuthDescriptor::try_from(value),
            Err(ConnectorOAuthRefusal::InvalidDescriptor)
        );
        assert_eq!(discover.stable_context(), known.stable_context());
    }

    #[test]
    fn account_selection_wire_form_is_the_account_string_or_null() {
        let known: ConnectorOAuthDescriptor = parameters().try_into().unwrap();
        let json = serde_json::to_value(&known).unwrap();
        assert_eq!(json["expected_account"], "account-a");
        let mut value = parameters();
        value.expected_account = AccountSelection::Discover;
        let discover: ConnectorOAuthDescriptor = value.try_into().unwrap();
        let json = serde_json::to_value(&discover).unwrap();
        assert!(json["expected_account"].is_null());
        assert_eq!(
            serde_json::from_value::<ConnectorOAuthDescriptor>(json).unwrap(),
            discover
        );
    }

    #[test]
    fn discover_accepts_any_verified_account_but_not_an_empty_one() {
        let mut value = parameters();
        value.expected_account = AccountSelection::Discover;
        let descriptor: ConnectorOAuthDescriptor = value.try_into().unwrap();
        let evidence = descriptor
            .verify_account(
                ConnectorAccountObservation {
                    account: "account-z".into(),
                    granted_scopes: ["mcp.read".into()].into(),
                },
                &exchanged(),
            )
            .unwrap();
        assert_eq!(evidence.account(), "account-z");
        assert_eq!(
            descriptor.verify_account(
                ConnectorAccountObservation {
                    account: String::new(),
                    granted_scopes: ["mcp.read".into()].into(),
                },
                &exchanged()
            ),
            Err(ConnectorOAuthRefusal::VerificationUnavailable)
        );
    }

    #[test]
    fn stable_context_ignores_redirect_and_scopes_but_not_client() {
        let original: ConnectorOAuthDescriptor = parameters().try_into().unwrap();
        let mut moved = parameters();
        moved.redirect_uri = "http://127.0.0.1:23456/callback".into();
        moved.scopes.insert("mcp.write".into());
        let moved: ConnectorOAuthDescriptor = moved.try_into().unwrap();
        assert_eq!(original.stable_context(), moved.stable_context());
        let mut other = parameters();
        other.client = "other-client".into();
        let other: ConnectorOAuthDescriptor = other.try_into().unwrap();
        assert_ne!(original.stable_context(), other.stable_context());
    }

    #[test]
    fn observations_refuse_wrong_account_and_missing_scopes_without_downgrade() {
        let descriptor: ConnectorOAuthDescriptor = parameters().try_into().unwrap();
        assert_eq!(
            descriptor.verify_account(
                ConnectorAccountObservation {
                    account: "account-b".into(),
                    granted_scopes: ["mcp.read".into()].into(),
                },
                &exchanged()
            ),
            Err(ConnectorOAuthRefusal::AccountMismatch)
        );
        assert_eq!(
            descriptor.verify_account(
                ConnectorAccountObservation {
                    account: "account-a".into(),
                    granted_scopes: BTreeSet::new(),
                },
                &exchanged()
            ),
            Err(ConnectorOAuthRefusal::MissingScopes)
        );
        let evidence = descriptor
            .verify_account(
                ConnectorAccountObservation {
                    account: "account-a".into(),
                    granted_scopes: ["mcp.read".into()].into(),
                },
                &exchanged(),
            )
            .unwrap();
        assert_eq!(evidence.account(), "account-a");
    }

    #[test]
    fn projections_and_debug_do_not_reveal_descriptor_or_state() {
        let descriptor: ConnectorOAuthDescriptor = parameters().try_into().unwrap();
        let projection = OAuthBrowserActionRef::project("secret-oauth-state");
        let debug = format!("{descriptor:?} {projection:?}");
        for secret in [
            "account-a",
            "registered-client",
            "secret-oauth-state",
            "service.example",
        ] {
            assert!(!debug.contains(secret));
        }
        assert!(
            !serde_json::to_string(&projection)
                .unwrap()
                .contains("secret-oauth-state")
        );
    }

    #[test]
    fn descriptor_deserialization_revalidates_and_preserves_exact_redirect() {
        let descriptor: ConnectorOAuthDescriptor = parameters().try_into().unwrap();
        let json = serde_json::to_value(&descriptor).unwrap();
        let restored: ConnectorOAuthDescriptor = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(descriptor, restored);
        assert_eq!(
            OAuthBrowserFlowIdentity::from(restored)
                .validate_redirect("http://127.0.0.1:12346/callback"),
            Err(ConnectorOAuthRefusal::DescriptorMismatch)
        );
        let mut invalid = json;
        invalid["scopes"] = serde_json::json!([]);
        assert!(serde_json::from_value::<ConnectorOAuthDescriptor>(invalid).is_err());
    }

    fn mcp_tokens(
        descriptor: &ConnectorOAuthDescriptor,
        account: Option<&str>,
        exchanged: &crate::auth_oauth::OAuthTokenResult,
    ) -> PersistedTokens {
        PersistedTokens {
            auth_mode: crate::auth_store::PersistedAuthMode::McpOauth,
            primary_secret: Some(exchanged.access_token.clone()),
            refresh_token: exchanged.refresh_token.clone(),
            id_token: exchanged.id_token.clone(),
            expires_at: None,
            last_refresh: None,
            scopes: vec!["mcp.read".into()],
            account_id: account.map(str::to_owned),
            metadata: serde_json::json!({
                "account_binding": McpCredentialBinding::for_descriptor(descriptor),
            }),
        }
    }

    fn with_selection(selection: AccountSelection) -> ConnectorOAuthDescriptor {
        let mut value = parameters();
        value.expected_account = selection;
        value.try_into().unwrap()
    }

    #[test]
    fn unverified_selection_has_its_own_wire_form_and_key() {
        let unverified = with_selection(AccountSelection::Unverified);
        let json = serde_json::to_value(&unverified).unwrap();
        assert_eq!(
            json["expected_account"],
            serde_json::json!({"mode": "unverified"})
        );
        assert_eq!(
            serde_json::from_value::<ConnectorOAuthDescriptor>(json).unwrap(),
            unverified
        );
        let discover = with_selection(AccountSelection::Discover);
        let known = with_selection(AccountSelection::Known("account-a".into()));
        assert_ne!(unverified.binding_key(), discover.binding_key());
        assert_ne!(unverified.binding_key(), known.binding_key());
        // Only an unverified descriptor may request no scope.
        let mut empty = parameters();
        empty.scopes.clear();
        assert_eq!(
            ConnectorOAuthDescriptor::try_from(empty.clone()),
            Err(ConnectorOAuthRefusal::InvalidDescriptor)
        );
        empty.expected_account = AccountSelection::Unverified;
        assert!(ConnectorOAuthDescriptor::try_from(empty).is_ok());
    }

    #[test]
    fn unverified_grant_is_never_account_evidence() {
        let unverified = with_selection(AccountSelection::Unverified);
        // No observation verifies it, whatever account it names.
        assert_eq!(
            unverified.verify_account(
                ConnectorAccountObservation {
                    account: "account-a".into(),
                    granted_scopes: ["mcp.read".into()].into(),
                },
                &exchanged()
            ),
            Err(ConnectorOAuthRefusal::VerificationUnavailable)
        );
        // Only an unverified descriptor yields a grant, and only with the
        // required scopes granted.
        assert_eq!(
            with_selection(AccountSelection::Discover).accept_unverified_grant(&exchanged()),
            Err(ConnectorOAuthRefusal::InvalidDescriptor)
        );
        let mut narrow = exchanged();
        narrow.scope = Some("other".into());
        assert_eq!(
            unverified.accept_unverified_grant(&narrow),
            Err(ConnectorOAuthRefusal::MissingScopes)
        );
        let grant = unverified.accept_unverified_grant(&exchanged()).unwrap();
        let completion = OAuthBrowserFlowCompletion::from(grant);
        completion.verify_account_and_scopes().unwrap();
        completion
            .verify_tokens(&mcp_tokens(&unverified, None, &exchanged()))
            .unwrap();
        // The persisted credential may not claim an account or another mode.
        assert_eq!(
            completion.verify_tokens(&mcp_tokens(&unverified, Some("account-a"), &exchanged())),
            Err(ConnectorOAuthRefusal::CredentialMismatch)
        );
        let discover = with_selection(AccountSelection::Discover);
        assert_eq!(
            completion.verify_tokens(&mcp_tokens(&discover, None, &exchanged())),
            Err(ConnectorOAuthRefusal::CredentialMismatch)
        );
    }

    #[test]
    fn mcp_slot_rules_match_the_connector_race_controls() {
        use crate::auth_store::CredentialSlotRefusal;
        let known = with_selection(AccountSelection::Known("account-a".into()));
        let discover = with_selection(AccountSelection::Discover);
        let unverified = with_selection(AccountSelection::Unverified);
        let observation = |account: &str| ConnectorAccountObservation {
            account: account.into(),
            granted_scopes: ["mcp.read".into()].into(),
        };
        let occupant = mcp_tokens(&known, Some("account-a"), &exchanged());
        let tokens = |descriptor| mcp_tokens(descriptor, Some("account-a"), &exchanged());

        // Discover publishes only into a still-empty slot, even for the
        // same subject.
        let discovered = OAuthBrowserFlowCompletion::from(
            discover
                .verify_account(observation("account-a"), &exchanged())
                .unwrap(),
        );
        assert_eq!(discovered.admit_into_slot(None, &tokens(&discover)), Ok(()));
        assert_eq!(
            discovered.admit_into_slot(Some(&occupant), &tokens(&discover)),
            Err(CredentialSlotRefusal::Occupied)
        );

        // Known replaces only the same verified subject in the same context.
        let reconnect = OAuthBrowserFlowCompletion::from(
            known
                .verify_account(observation("account-a"), &exchanged())
                .unwrap(),
        );
        assert_eq!(
            reconnect.admit_into_slot(Some(&occupant), &tokens(&known)),
            Ok(())
        );
        let mut other_subject = occupant.clone();
        other_subject.account_id = Some("account-b".into());
        assert_eq!(
            reconnect.admit_into_slot(Some(&other_subject), &tokens(&known)),
            Err(CredentialSlotRefusal::AccountMismatch)
        );
        let mut other_client = parameters();
        other_client.client = "fresh-registration".into();
        let other_context: ConnectorOAuthDescriptor = other_client.try_into().unwrap();
        let other_occupant = mcp_tokens(&other_context, Some("account-a"), &exchanged());
        assert_eq!(
            reconnect.admit_into_slot(Some(&other_occupant), &tokens(&known)),
            Err(CredentialSlotRefusal::ContextMismatch)
        );
        let mut unbound = occupant;
        unbound.metadata = serde_json::json!({});
        assert_eq!(
            reconnect.admit_into_slot(Some(&unbound), &tokens(&known)),
            Err(CredentialSlotRefusal::ContextMismatch)
        );
        let unverified_occupant = mcp_tokens(&unverified, None, &exchanged());
        assert_eq!(
            reconnect.admit_into_slot(Some(&unverified_occupant), &tokens(&known)),
            Err(CredentialSlotRefusal::ContextMismatch)
        );

        // An unverified grant never replaces anything.
        let grant = OAuthBrowserFlowCompletion::from(
            unverified.accept_unverified_grant(&exchanged()).unwrap(),
        );
        let grant_tokens = mcp_tokens(&unverified, None, &exchanged());
        assert_eq!(grant.admit_into_slot(None, &grant_tokens), Ok(()));
        assert_eq!(
            grant.admit_into_slot(Some(&unverified_occupant), &grant_tokens),
            Err(CredentialSlotRefusal::Occupied)
        );
    }
}

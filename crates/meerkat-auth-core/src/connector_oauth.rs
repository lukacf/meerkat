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
    pub expected_account: String,
    pub strategy_id: String,
}

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
            || !valid_atom(&value.expected_account)
            || !valid_atom(&value.strategy_id)
            || value.scopes.is_empty()
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
            &self.0.issuer,
            &self.0.client,
            &self.0.resource,
            &self.0.redirect_uri,
            &self.0.expected_account,
            &self.0.strategy_id,
        ]
        .into_iter()
        .chain(self.0.scopes.iter())
        {
            hash.update(value.len().to_string().as_bytes());
            hash.update(b":");
            hash.update(value.as_bytes());
        }
        format!("connector:{:x}", hash.finalize())
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
        Ok(VerifiedConnectorAccount {
            descriptor: self.clone(),
            observation,
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
        if observation.account != self.0.expected_account {
            return Err(ConnectorOAuthRefusal::AccountMismatch);
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

#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedConnectorAccount {
    descriptor: ConnectorOAuthDescriptor,
    observation: ConnectorAccountObservation,
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

    pub fn verify_tokens(&self, tokens: &PersistedTokens) -> Result<(), ConnectorOAuthRefusal> {
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

/// Completion carries either the original LLM-provider contract or checked
/// connector evidence. There is no conversion from an unverified connector
/// identity to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthBrowserFlowCompletion {
    Provider(OAuthProviderIdentity),
    Connector(VerifiedConnectorAccount),
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
        }
    }

    pub fn identity(&self) -> OAuthBrowserFlowIdentity {
        match self {
            Self::Provider(provider) => (*provider).into(),
            Self::Connector(evidence) => evidence.descriptor.clone().into(),
        }
    }

    pub fn verify_tokens(&self, tokens: &PersistedTokens) -> Result<(), ConnectorOAuthRefusal> {
        match self {
            Self::Provider(_) => Ok(()),
            Self::Connector(evidence) => evidence.verify_tokens(tokens),
        }
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
}

//! TokenStore trait + PersistedTokens/TokenKey types + RefreshCoordinator.
//!
//! Moved from `meerkat-providers::auth_store` (B2 split, 2026-04-18) so
//! the trait surface is reachable without heavy-IO dependencies.
//! Concrete backends (File/Keyring/Auto/Ephemeral/InMemory/FileLock)
//! live in `meerkat-auth-core::auth_store`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::connection::{
    AuthBindingRef, AuthCredentialIdentity, BindingId, CredentialAccountId, IdentityError,
    ProfileId, RealmId,
};
use crate::realm_admission::AdmissionGuard;

/// Key for a persisted token bundle.
///
/// Binding-backed keys retain their historical serialized representation.
/// Account-backed keys let several provider routes share one durable
/// credential without duplicating lifecycle authority.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct TokenKey(TokenKeyKind);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
enum TokenKeyKind {
    /// A sign-in credential.
    Credential(AuthCredentialIdentity),
    /// A connector OAuth client's own secret: client configuration, not a
    /// credential. It has no credential identity, lease or lifecycle.
    ConnectorClientSecret(ConnectorClientSecretKey),
    /// The update ownership of one realm's connector client profile id: a
    /// coordinator key only, never stored.
    ConnectorClientUpdate { realm: RealmId, profile: ProfileId },
    /// The realm's protected key for keyed MCP attachment identities: host
    /// key material, not a credential. It has no credential identity, lease
    /// or lifecycle.
    McpAttachmentIdentityKey { realm: RealmId },
    /// One service secret of an MCP stdio server the realm's own
    /// configuration defines: server configuration, not a credential. It has
    /// no credential identity, lease or lifecycle.
    McpServiceSecret(McpServiceSecretKey),
    /// The host-registered OAuth client of one MCP credential slot (its
    /// secret included): client configuration, not a credential. It has no
    /// credential identity, lease or lifecycle.
    McpOauthClient(McpOauthClientKey),
}

/// Storage stem of a realm's MCP attachment identity key. `~` is outside the
/// binding/profile slug grammar, so no credential key can produce it.
const MCP_ATTACHMENT_IDENTITY_KEY_STEM: &str = "mcp-attachment-identity-key~v1";

/// Where a realm keeps one connector client's secret: the defining realm,
/// the profile id and the client's canonical non-secret meaning (the digest
/// in its profile binding). A changed client meaning is a different key, so
/// a secret is only ever read for the exact client it was stored for.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorClientSecretKey {
    realm: RealmId,
    profile: ProfileId,
    meaning: String,
}

/// Storage prefix of a connector client secret key. `~` is outside the
/// binding/profile slug grammar, so no credential key can produce it.
const CONNECTOR_CLIENT_SECRET_STEM: &str = "connector-client~v1~";

impl ConnectorClientSecretKey {
    fn stem(&self) -> String {
        format!(
            "{CONNECTOR_CLIENT_SECRET_STEM}{}~{}",
            self.profile, self.meaning
        )
    }

    fn valid_meaning(meaning: &str) -> bool {
        meaning.len() == 64
            && meaning
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// Where a realm keeps one service secret of an MCP stdio server its own
/// configuration defines: the defining realm, the environment slot and the
/// server's canonical launch meaning
/// ([`crate::McpServerConfig::stdio_launch_meaning`], which covers the
/// server name). A changed definition is a different key, so a secret is only
/// ever read for the exact definition it was stored for.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServiceSecretKey {
    realm: RealmId,
    env: String,
    meaning: String,
}

/// Storage prefix of an MCP service secret key. `~` is outside the
/// binding/profile slug grammar and the environment-name grammar, so no other
/// key can produce it.
const MCP_SERVICE_SECRET_STEM: &str = "mcp-service-secret~v1~";

impl McpServiceSecretKey {
    pub(crate) fn new(realm: RealmId, env: &str, meaning: &str) -> Option<Self> {
        let key = Self {
            realm,
            env: env.to_owned(),
            meaning: meaning.to_owned(),
        };
        key.valid().then_some(key)
    }

    fn valid(&self) -> bool {
        crate::mcp_config::is_env_var_name(&self.env)
            && ConnectorClientSecretKey::valid_meaning(&self.meaning)
    }

    pub(crate) fn realm(&self) -> &RealmId {
        &self.realm
    }

    pub(crate) fn env(&self) -> &str {
        &self.env
    }

    pub(crate) fn meaning(&self) -> &str {
        &self.meaning
    }

    fn stem(&self) -> String {
        format!("{MCP_SERVICE_SECRET_STEM}{}~{}", self.env, self.meaning)
    }
}

/// Where an MCP credential slot keeps its host-registered OAuth client: the
/// slot's own token realm and binding, under a key kind of its own. Every
/// slot (server, realm and account selection) has its own client key, so a
/// client installed for one slot is never read for another.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpOauthClientKey {
    realm: RealmId,
    binding: BindingId,
}

/// Storage prefix of an MCP OAuth client key. `~` is outside the
/// binding/profile slug grammar, so no credential key can produce it.
const MCP_OAUTH_CLIENT_STEM: &str = "mcp-oauth-client~v1~";

impl McpOauthClientKey {
    fn stem(&self) -> String {
        format!("{MCP_OAUTH_CLIENT_STEM}{}", self.binding)
    }
}

/// Serialized form: a credential key keeps its historical (transparent
/// identity) encoding; a client-secret key is a distinct tagged object.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum TokenKeyWire {
    ConnectorClientSecret {
        connector_client_secret: ConnectorClientSecretKey,
    },
    McpAttachmentIdentityKey {
        mcp_attachment_identity_key: RealmId,
    },
    McpServiceSecret {
        mcp_service_secret: McpServiceSecretKey,
    },
    McpOauthClient {
        mcp_oauth_client: McpOauthClientKey,
    },
    Credential(AuthCredentialIdentity),
}

impl Serialize for TokenKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.0 {
            TokenKeyKind::Credential(identity) => identity.serialize(serializer),
            TokenKeyKind::ConnectorClientSecret(key) => TokenKeyWire::ConnectorClientSecret {
                connector_client_secret: key.clone(),
            }
            .serialize(serializer),
            TokenKeyKind::McpAttachmentIdentityKey { realm } => {
                TokenKeyWire::McpAttachmentIdentityKey {
                    mcp_attachment_identity_key: realm.clone(),
                }
                .serialize(serializer)
            }
            TokenKeyKind::ConnectorClientUpdate { realm, profile } => serde::Serialize::serialize(
                &serde_json::json!({
                    "connector_client_update": { "realm": realm, "profile": profile }
                }),
                serializer,
            ),
            TokenKeyKind::McpServiceSecret(key) => TokenKeyWire::McpServiceSecret {
                mcp_service_secret: key.clone(),
            }
            .serialize(serializer),
            TokenKeyKind::McpOauthClient(key) => TokenKeyWire::McpOauthClient {
                mcp_oauth_client: key.clone(),
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for TokenKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match TokenKeyWire::deserialize(deserializer)? {
            TokenKeyWire::Credential(identity) => Ok(Self::from_credential_identity(&identity)),
            TokenKeyWire::McpAttachmentIdentityKey {
                mcp_attachment_identity_key,
            } => Ok(Self::mcp_attachment_identity_key(
                mcp_attachment_identity_key,
            )),
            TokenKeyWire::ConnectorClientSecret {
                connector_client_secret,
            } => {
                if ConnectorClientSecretKey::valid_meaning(&connector_client_secret.meaning) {
                    Ok(Self(TokenKeyKind::ConnectorClientSecret(
                        connector_client_secret,
                    )))
                } else {
                    Err(serde::de::Error::custom(
                        "connector client secret key has an invalid meaning",
                    ))
                }
            }
            TokenKeyWire::McpServiceSecret { mcp_service_secret } => {
                if mcp_service_secret.valid() {
                    Ok(Self(TokenKeyKind::McpServiceSecret(mcp_service_secret)))
                } else {
                    Err(serde::de::Error::custom(
                        "MCP service secret key has an invalid slot or meaning",
                    ))
                }
            }
            TokenKeyWire::McpOauthClient { mcp_oauth_client } => {
                Ok(Self(TokenKeyKind::McpOauthClient(mcp_oauth_client)))
            }
        }
    }
}

impl TokenKey {
    /// Construct a token key from already-typed atoms. The primary
    /// constructor — call sites that have the typed forms (e.g. from
    /// `AuthBindingRef.realm` / `AuthBindingRef.binding`) use this
    /// directly. Raw-string call sites build the atoms at their CLI /
    /// wire boundary via `RealmId::parse` / `BindingId::parse` and
    /// fold the resulting `Result<TokenKey, IdentityError>` into their
    /// ambient error handling.
    pub fn new(realm: RealmId, binding: BindingId) -> Self {
        Self::new_with_profile(realm, binding, None)
    }

    pub fn new_with_profile(
        realm: RealmId,
        binding: BindingId,
        profile: Option<ProfileId>,
    ) -> Self {
        Self(TokenKeyKind::Credential(AuthCredentialIdentity::Binding(
            AuthBindingRef {
                realm,
                binding,
                profile,
                origin: crate::connection::BindingOrigin::Configured,
            },
        )))
    }

    /// Where `realm` keeps the secret of the connector client whose profile
    /// binding (its id and canonical non-secret meaning, `<id>@<digest>`) is
    /// `profile_binding`. A distinct key kind: no credential key, binding or
    /// account, can name it.
    pub fn connector_client_secret(
        realm: RealmId,
        profile_binding: &str,
    ) -> Result<Self, IdentityError> {
        let (profile, meaning) = profile_binding
            .split_once('@')
            .ok_or(IdentityError::InvalidChar('@'))?;
        if !ConnectorClientSecretKey::valid_meaning(meaning) {
            return Err(IdentityError::InvalidChar('@'));
        }
        Ok(Self(TokenKeyKind::ConnectorClientSecret(
            ConnectorClientSecretKey {
                realm,
                profile: ProfileId::parse(profile)?,
                meaning: meaning.to_owned(),
            },
        )))
    }

    /// The coordinator key that serializes updates of `realm`'s connector
    /// client `profile` (its config entry and secrets) across processes. It
    /// is never stored.
    pub fn connector_client_update(realm: RealmId, profile: &str) -> Result<Self, IdentityError> {
        Ok(Self(TokenKeyKind::ConnectorClientUpdate {
            realm,
            profile: ProfileId::parse(profile)?,
        }))
    }

    /// Where `realm` keeps its protected key for keyed MCP attachment
    /// identities. A distinct key kind: no credential key, binding or account,
    /// can name it.
    pub fn mcp_attachment_identity_key(realm: RealmId) -> Self {
        Self(TokenKeyKind::McpAttachmentIdentityKey { realm })
    }

    /// The MCP attachment identity key a storage stem names, if it names one.
    pub fn mcp_attachment_identity_key_from_stem(realm: &RealmId, stem: &str) -> Option<Self> {
        (stem == MCP_ATTACHMENT_IDENTITY_KEY_STEM)
            .then(|| Self::mcp_attachment_identity_key(realm.clone()))
    }

    /// Whether this key holds a realm's MCP attachment identity key.
    pub fn is_mcp_attachment_identity_key(&self) -> bool {
        matches!(self.0, TokenKeyKind::McpAttachmentIdentityKey { .. })
    }

    /// Where `realm` keeps the service secret of exactly `reference`'s slot
    /// and definition. A distinct key kind: no credential key, binding or
    /// account, can name it.
    pub fn mcp_service_secret(reference: &crate::auth::McpServiceSecretRef) -> Self {
        Self(TokenKeyKind::McpServiceSecret(reference.key().clone()))
    }

    /// The MCP service secret key a storage stem names, if it names one.
    pub fn mcp_service_secret_from_stem(realm: &RealmId, stem: &str) -> Option<Self> {
        let rest = stem.strip_prefix(MCP_SERVICE_SECRET_STEM)?;
        let (env, meaning) = rest.split_once('~')?;
        McpServiceSecretKey::new(realm.clone(), env, meaning)
            .map(|key| Self(TokenKeyKind::McpServiceSecret(key)))
    }

    /// Whether this key holds an MCP server's service secret.
    pub fn is_mcp_service_secret(&self) -> bool {
        matches!(self.0, TokenKeyKind::McpServiceSecret(_))
    }

    /// Where the MCP credential slot `realm`/`binding` (the slot's token
    /// realm and binding) keeps its host-registered OAuth client. A distinct
    /// key kind: no credential key, binding or account, can name it.
    pub fn mcp_oauth_client(realm: RealmId, binding: BindingId) -> Self {
        Self(TokenKeyKind::McpOauthClient(McpOauthClientKey {
            realm,
            binding,
        }))
    }

    /// The MCP OAuth client key a storage stem names, if it names one.
    pub fn mcp_oauth_client_from_stem(realm: &RealmId, stem: &str) -> Option<Self> {
        let binding = stem.strip_prefix(MCP_OAUTH_CLIENT_STEM)?;
        BindingId::parse(binding)
            .ok()
            .map(|binding| Self::mcp_oauth_client(realm.clone(), binding))
    }

    /// Whether this key holds an MCP credential slot's OAuth client.
    pub fn is_mcp_oauth_client(&self) -> bool {
        matches!(self.0, TokenKeyKind::McpOauthClient(_))
    }

    /// The client-secret key a storage stem names, if it names one.
    pub fn connector_client_secret_from_stem(realm: &RealmId, stem: &str) -> Option<Self> {
        let rest = stem.strip_prefix(CONNECTOR_CLIENT_SECRET_STEM)?;
        let (profile, meaning) = rest.split_once('~')?;
        Self::connector_client_secret(realm.clone(), &format!("{profile}@{meaning}")).ok()
    }

    pub fn from_auth_binding(auth_binding: &AuthBindingRef) -> Self {
        Self(TokenKeyKind::Credential(
            AuthCredentialIdentity::from_auth_binding(auth_binding),
        ))
    }

    pub fn from_credential_identity(identity: &AuthCredentialIdentity) -> Self {
        Self(TokenKeyKind::Credential(
            identity.normalized_for_credential_storage(),
        ))
    }

    /// The credential this key stores, or `None` for a connector client
    /// secret key (client configuration, not a credential).
    pub fn credential_identity(&self) -> Option<&AuthCredentialIdentity> {
        match &self.0 {
            TokenKeyKind::Credential(identity) => Some(identity),
            TokenKeyKind::ConnectorClientSecret(_)
            | TokenKeyKind::ConnectorClientUpdate { .. }
            | TokenKeyKind::McpAttachmentIdentityKey { .. }
            | TokenKeyKind::McpServiceSecret(_)
            | TokenKeyKind::McpOauthClient(_) => None,
        }
    }

    /// Whether this key holds a connector client secret.
    pub fn is_connector_client_secret(&self) -> bool {
        matches!(self.0, TokenKeyKind::ConnectorClientSecret(_))
    }

    pub fn realm(&self) -> &RealmId {
        match &self.0 {
            TokenKeyKind::Credential(identity) => identity.realm(),
            TokenKeyKind::ConnectorClientSecret(key) => &key.realm,
            TokenKeyKind::McpServiceSecret(key) => &key.realm,
            TokenKeyKind::McpOauthClient(key) => &key.realm,
            TokenKeyKind::ConnectorClientUpdate { realm, .. }
            | TokenKeyKind::McpAttachmentIdentityKey { realm } => realm,
        }
    }

    pub fn binding(&self) -> Option<&BindingId> {
        self.credential_identity()?.binding()
    }

    pub fn profile(&self) -> Option<&ProfileId> {
        self.credential_identity()?.profile()
    }

    pub fn account(&self) -> Option<&CredentialAccountId> {
        self.credential_identity()?.account()
    }

    /// Construct a token key from raw strings, validating each component
    /// against the slug grammar enforced by
    /// `meerkat_core::connection::{RealmId,BindingId}::parse`. This is
    /// the right entry point for callers that only have flat-string
    /// input — the CLI `--auth-binding` parser, wire-layer handlers,
    /// test fixtures.
    pub fn parse(realm: impl AsRef<str>, binding: impl AsRef<str>) -> Result<Self, IdentityError> {
        Self::parse_with_profile(realm, binding, None::<&str>)
    }

    pub fn parse_with_profile(
        realm: impl AsRef<str>,
        binding: impl AsRef<str>,
        profile: Option<impl AsRef<str>>,
    ) -> Result<Self, IdentityError> {
        Ok(Self::new_with_profile(
            RealmId::parse(realm.as_ref())?,
            BindingId::parse(binding.as_ref())?,
            profile
                .map(|profile| ProfileId::parse(profile.as_ref()))
                .transpose()?,
        ))
    }

    /// The flat account identifier used by OS keyrings.
    ///
    /// Default binding credentials preserve the legacy format:
    /// `<realm>:<binding>`. Profile override credentials include the
    /// canonical override atom: `<realm>:<binding>:<profile>`. Shared accounts
    /// use `<realm>:account~<account>`, where `~` is outside the binding/profile
    /// slug grammar and therefore cannot collide with a legacy key.
    ///
    /// The default credential format stays identical to the pre-profile-key
    /// output; this method is the source of truth for the keyring
    /// `service:account` convention, so the default branch must preserve that
    /// output byte-for-byte to keep existing OAuth credentials reachable.
    pub fn keyring_account(&self) -> String {
        let identity = match &self.0 {
            TokenKeyKind::Credential(identity) => identity,
            TokenKeyKind::ConnectorClientSecret(key) => {
                return format!("{}:{}", key.realm, key.stem());
            }
            TokenKeyKind::ConnectorClientUpdate { realm, profile } => {
                return format!("{realm}:connector-client-update~v1~{profile}");
            }
            TokenKeyKind::McpAttachmentIdentityKey { realm } => {
                return format!("{realm}:{MCP_ATTACHMENT_IDENTITY_KEY_STEM}");
            }
            TokenKeyKind::McpServiceSecret(key) => {
                return format!("{}:{}", key.realm, key.stem());
            }
            TokenKeyKind::McpOauthClient(key) => {
                return format!("{}:{}", key.realm, key.stem());
            }
        };
        match identity {
            AuthCredentialIdentity::Binding(binding) => match &binding.profile {
                Some(profile) => {
                    format!("{}:{}:{}", binding.realm, binding.binding, profile)
                }
                None => format!("{}:{}", binding.realm, binding.binding),
            },
            AuthCredentialIdentity::Account(account) => {
                format!("{}:account~{}", account.realm, account.account)
            }
        }
    }

    /// Path-safe stable component below the owning realm directory.
    pub fn storage_stem(&self) -> String {
        let identity = match &self.0 {
            TokenKeyKind::Credential(identity) => identity,
            TokenKeyKind::ConnectorClientSecret(key) => return key.stem(),
            TokenKeyKind::ConnectorClientUpdate { profile, .. } => {
                return format!("connector-client-update~v1~{profile}");
            }
            TokenKeyKind::McpAttachmentIdentityKey { .. } => {
                return MCP_ATTACHMENT_IDENTITY_KEY_STEM.to_owned();
            }
            TokenKeyKind::McpServiceSecret(key) => return key.stem(),
            TokenKeyKind::McpOauthClient(key) => return key.stem(),
        };
        match identity {
            AuthCredentialIdentity::Binding(binding) => match &binding.profile {
                Some(profile) => format!("{}@{}", binding.binding, profile),
                None => binding.binding.to_string(),
            },
            AuthCredentialIdentity::Account(account) => format!("account~{}", account.account),
        }
    }
}

impl std::fmt::Display for TokenKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            TokenKeyKind::Credential(identity) => identity.fmt(formatter),
            TokenKeyKind::ConnectorClientSecret(key) => {
                write!(formatter, "{}:{}", key.realm, key.stem())
            }
            TokenKeyKind::ConnectorClientUpdate { realm, profile } => {
                write!(formatter, "{realm}:connector-client-update~v1~{profile}")
            }
            TokenKeyKind::McpAttachmentIdentityKey { realm } => {
                write!(formatter, "{realm}:{MCP_ATTACHMENT_IDENTITY_KEY_STEM}")
            }
            TokenKeyKind::McpServiceSecret(key) => {
                write!(formatter, "{}:{}", key.realm, key.stem())
            }
            TokenKeyKind::McpOauthClient(key) => {
                write!(formatter, "{}:{}", key.realm, key.stem())
            }
        }
    }
}

/// Kind of credential material persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedAuthMode {
    ApiKey,
    StaticBearer,
    ChatgptOauth,
    ClaudeAiOauth,
    OauthToApiKey,
    GoogleOauth,
    GithubCopilotOauth,
    Adc,
    ComputeAdc,
    Bedrock,
    Vertex,
    Foundry,
    McpOauth,
    /// Generic connector OAuth credential owned by the connector login
    /// owner. Disjoint from `McpOauth`: each loader refuses the other's rows.
    ConnectorOauth,
    /// A connector OAuth client's own secret (realm configuration, not a
    /// sign-in credential), stored under
    /// [`TokenKey::connector_client_secret`]. No provider resolver accepts it
    /// and it has no AuthMachine lifecycle.
    ConnectorClientSecret,
    /// A realm's protected key for keyed MCP attachment identities (host key
    /// material, not a credential), stored under
    /// [`TokenKey::mcp_attachment_identity_key`]. No provider resolver
    /// accepts it and it has no AuthMachine lifecycle.
    McpAttachmentIdentityKey,
    /// One service secret of an MCP stdio server a realm's own configuration
    /// defines (server configuration, not a sign-in credential), stored under
    /// [`TokenKey::mcp_service_secret`]. No provider resolver accepts it and
    /// it has no AuthMachine lifecycle.
    McpServiceSecret,
    ExternalTokens,
    ExternalAuthorizer,
    Command,
    /// The host-registered OAuth client of one MCP credential slot (client
    /// configuration with its secret, not a sign-in credential), stored
    /// under [`TokenKey::mcp_oauth_client`]. No provider resolver accepts it
    /// and it has no AuthMachine lifecycle.
    McpOauthClient,
}

/// Serializable token bundle. `Debug` redacts the secret and tokens.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct PersistedTokens {
    pub auth_mode: PersistedAuthMode,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub primary_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id_token: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "chrono::serde::ts_seconds_option"
    )]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "chrono::serde::ts_seconds_option"
    )]
    pub last_refresh: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

impl std::fmt::Debug for PersistedTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::redact::optional;
        f.debug_struct("PersistedTokens")
            .field("auth_mode", &self.auth_mode)
            .field("primary_secret", &optional(&self.primary_secret))
            .field("refresh_token", &optional(&self.refresh_token))
            .field("id_token", &optional(&self.id_token))
            .field("expires_at", &self.expires_at)
            .field("last_refresh", &self.last_refresh)
            .field("scopes", &self.scopes)
            .field("account_id", &self.account_id)
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl PersistedTokens {
    pub fn api_key(secret: impl Into<String>) -> Self {
        Self {
            auth_mode: PersistedAuthMode::ApiKey,
            primary_secret: Some(secret.into()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn static_bearer(token: impl Into<String>) -> Self {
        Self {
            auth_mode: PersistedAuthMode::StaticBearer,
            primary_secret: Some(token.into()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }
}

/// Errors from the token-store layer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TokenStoreError {
    #[error("io error: {0}")]
    Io(String),
    #[error("serialization error: {0}")]
    Serde(String),
    /// The keyring backend is absent on this host. This is the only keyring
    /// error that lets an Auto store use its file backend instead.
    #[error("keyring backend unavailable: {0}")]
    KeyringUnavailable(String),
    #[error("no credentials found for {realm}:{binding}")]
    NotFound { realm: String, binding: String },
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    /// The store refused this access without an answer. It keeps the backend
    /// that refused and its original error text, and never means "absent".
    #[error("{backend} credential store refused {access} access ({refusal}): {detail}")]
    AccessRefused {
        backend: &'static str,
        access: TokenStoreAccess,
        refusal: TokenStoreRefusal,
        detail: String,
    },
    /// The selected backend failed. It keeps the backend and its original
    /// error; it is not backend absence and admits no fallback.
    #[error("{backend} credential store failed: {detail}")]
    BackendFailed {
        backend: &'static str,
        detail: String,
    },
    /// A write whose admission guard does not cover the credential fence of
    /// the written credential's realm (C(r) under this store's root): the
    /// store wrote nothing. A persisted store answers it before any byte is
    /// written; it is never an I/O error and never a partial write.
    #[error("credential store write refused, its admission guard does not cover it: {0}")]
    AdmissionFence(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<std::io::Error> for TokenStoreError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied(e.to_string()),
            std::io::ErrorKind::NotFound => Self::Io(e.to_string()),
            _ => Self::Io(e.to_string()),
        }
    }
}

impl From<serde_json::Error> for TokenStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e.to_string())
    }
}

impl TokenStoreError {
    /// A typed refusal by `backend` of an `access` it did not answer.
    pub fn refused(
        backend: &'static str,
        access: TokenStoreAccess,
        refusal: TokenStoreRefusal,
        detail: impl Into<String>,
    ) -> Self {
        Self::AccessRefused {
            backend,
            access,
            refusal,
            detail: detail.into(),
        }
    }

    /// Why the store refused, when this error is a refusal.
    pub fn refusal(&self) -> Option<TokenStoreRefusal> {
        match self {
            Self::AccessRefused { refusal, .. } => Some(*refusal),
            _ => None,
        }
    }

    /// The typed auth error for a refused or failed credential access, or
    /// `None` for an error that is not about credential access.
    pub fn credential_access_error(&self) -> Option<super::error::AuthError> {
        credential_access_parts(self).map(|(refusal, detail)| refusal_auth_error(refusal, detail))
    }

    /// The refusal and its detail for a refused or failed credential access
    /// (a failed backend call is an unavailability), or `None` for an error
    /// that is not about credential access.
    pub fn credential_access(&self) -> Option<(TokenStoreRefusal, String)> {
        credential_access_parts(self)
    }
}

/// The typed auth error for a credential-access refusal.
fn refusal_auth_error(refusal: TokenStoreRefusal, detail: String) -> super::error::AuthError {
    use super::error::AuthError;
    match refusal {
        TokenStoreRefusal::Denied => AuthError::CredentialAccessDenied,
        TokenStoreRefusal::InteractionRequired => AuthError::CredentialInteractionRequired,
        TokenStoreRefusal::Pending => AuthError::CredentialAccessPending,
        TokenStoreRefusal::Unavailable
        | TokenStoreRefusal::NotObserved
        | TokenStoreRefusal::Unsupported => AuthError::CredentialStoreUnavailable(detail),
    }
}

/// How a caller uses a credential store. Callers state it at every load,
/// save and clear; a store never infers it from context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenStoreAccess {
    /// A status or readiness observation. It never waits and never starts an
    /// operating-system credential read or operating-system UI.
    Status,
    /// Provider setup and maintenance: factory setup, request preparation and
    /// refresh. A keyring store admits at most one bounded, owner-held lookup
    /// per credential and does not retry a recorded denial.
    Unattended,
    /// An explicit, user-initiated credential operation (login, logout, an
    /// explicit save or clear). It may raise operating-system UI, and its
    /// success re-arms a recorded denial.
    Interactive,
}

impl TokenStoreAccess {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Unattended => "unattended",
            Self::Interactive => "interactive",
        }
    }
}

impl std::fmt::Display for TokenStoreAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a credential store refused an access without answering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenStoreRefusal {
    /// The operating system reported that the user denied the request.
    Denied,
    /// The operating system reported that answering needs user interaction
    /// it would not raise.
    InteractionRequired,
    /// An operation for this credential is already physically in flight. No
    /// new operating-system call was started.
    Pending,
    /// The bounded operation capacity or the credential owner is busy. No
    /// operating-system call was started.
    Unavailable,
    /// A status read that did not look: no operating-system read was
    /// attempted, so nothing is known about the credential.
    NotObserved,
    /// The backend does not support this access.
    Unsupported,
}

impl TokenStoreRefusal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::InteractionRequired => "interaction_required",
            Self::Pending => "pending",
            Self::Unavailable => "unavailable",
            Self::NotObserved => "not_observed",
            Self::Unsupported => "unsupported",
        }
    }
}

impl std::fmt::Display for TokenStoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a non-blocking status observation learned about one credential.
///
/// Only `Missing` proves absence. `NotObserved` means no read was attempted,
/// and `InteractionRequired` is reported only on backend evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenStoreObservation {
    /// The store answered without operating-system interaction.
    Present(Box<PersistedTokens>),
    /// The store answered and holds no credential for the key.
    Missing,
    /// The store did not look.
    NotObserved,
    /// An owner-held lookup for this credential is in flight.
    Pending,
    /// The operating system recorded a denial for this credential.
    Denied,
    /// The backend reported that reading needs user interaction.
    InteractionRequired,
    /// The store cannot answer now.
    Unavailable,
}

impl TokenStoreObservation {
    /// The refusal a load for `access` reports for an unanswered observation,
    /// or `None` when the observation answered (`Present` or `Missing`).
    pub fn refusal(&self) -> Option<TokenStoreRefusal> {
        match self {
            Self::Present(_) | Self::Missing => None,
            Self::NotObserved => Some(TokenStoreRefusal::NotObserved),
            Self::Pending => Some(TokenStoreRefusal::Pending),
            Self::Denied => Some(TokenStoreRefusal::Denied),
            Self::InteractionRequired => Some(TokenStoreRefusal::InteractionRequired),
            Self::Unavailable => Some(TokenStoreRefusal::Unavailable),
        }
    }
}

/// Cross-process-safe persistence for tokens.
///
/// # Vault property: the durable lifecycle marker is the proof-of-acquisition
///
/// `TokenStore` persists credential *material*; the AuthMachine lease owns the
/// credential *lifecycle*. The bridge between the two is the durable lifecycle
/// marker embedded in [`PersistedTokens::metadata`] by
/// `mark_tokens_lifecycle_published_for_transition`: it is stamped from an
/// AuthMachine acquisition transition and is the only durable proof that the
/// machine recorded the acquisition.
///
/// Writers MUST follow acquire-first ordering: acquire the AuthMachine lease
/// (`publish_token_lifecycle_acquired`), stamp the marker from the returned
/// transition, then perform a single `save` of the marked tokens. Unmarked
/// token bytes must never be persisted — on a crash the durable record either
/// carries the marker or does not exist, so no orphan window is
/// representable.
///
/// Readers MUST only use persisted tokens after marker validation through the
/// lifecycle restore path (`restore_marked_token_lifecycle` /
/// `rehydrate_marked_tokens_for_status`) and the AuthMachine admission gate
/// (`resolve_credential_use_admission` in the resolver). A persisted token
/// without a valid marker is dead data: no acquisition was recorded for it,
/// and it must be rejected, never silently adopted.
///
/// # Writes take the operation's admission guard
///
/// Every write ([`Self::save`], [`Self::clear`], [`Self::save_for`],
/// [`Self::clear_for`]) takes the [`AdmissionGuard`] of the operation it
/// belongs to, which the credential coordinator acquires at the operation's
/// entry and hands its closure. A persisted store refuses a guard that does
/// not cover the credential fence C(r) of the written credential's realm
/// under its own root ([`TokenStoreError::AdmissionFence`]), before any byte
/// is written. A store with no fence (in memory, ephemeral) accepts any
/// guard, [`AdmissionGuard::none`] included. Reads take none.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait TokenStore: Send + Sync {
    async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError>;
    async fn save(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
    ) -> Result<(), TokenStoreError>;
    async fn clear(&self, guard: &AdmissionGuard, key: &TokenKey) -> Result<(), TokenStoreError>;
    async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError>;
    fn backend_name(&self) -> &'static str;

    /// Load for a stated access.
    ///
    /// A status load answers only through [`TokenStore::observe_status`] and
    /// refuses anything that is not `Present` or `Missing`. The default never
    /// delegates a status load to [`TokenStore::load`].
    async fn load_for(
        &self,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<Option<PersistedTokens>, TokenStoreError> {
        match access {
            TokenStoreAccess::Status => match self.observe_status(key).await? {
                TokenStoreObservation::Present(tokens) => Ok(Some(*tokens)),
                TokenStoreObservation::Missing => Ok(None),
                observation => Err(TokenStoreError::refused(
                    self.backend_name(),
                    access,
                    observation
                        .refusal()
                        .unwrap_or(TokenStoreRefusal::NotObserved),
                    "the status observation did not answer",
                )),
            },
            TokenStoreAccess::Unattended | TokenStoreAccess::Interactive => self.load(key).await,
        }
    }

    /// Save for a stated access. Status never writes.
    async fn save_for(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        tokens: &PersistedTokens,
        access: TokenStoreAccess,
    ) -> Result<(), TokenStoreError> {
        match access {
            TokenStoreAccess::Status => Err(TokenStoreError::refused(
                self.backend_name(),
                access,
                TokenStoreRefusal::Unsupported,
                "a status access never writes",
            )),
            TokenStoreAccess::Unattended | TokenStoreAccess::Interactive => {
                self.save(guard, key, tokens).await
            }
        }
    }

    /// Clear for a stated access. Status never deletes.
    async fn clear_for(
        &self,
        guard: &AdmissionGuard,
        key: &TokenKey,
        access: TokenStoreAccess,
    ) -> Result<(), TokenStoreError> {
        match access {
            TokenStoreAccess::Status => Err(TokenStoreError::refused(
                self.backend_name(),
                access,
                TokenStoreRefusal::Unsupported,
                "a status access never deletes",
            )),
            TokenStoreAccess::Unattended | TokenStoreAccess::Interactive => {
                self.clear(guard, key).await
            }
        }
    }

    /// Observe one credential for status. It never waits and never starts an
    /// operating-system credential read. Only audited non-interactive
    /// backends answer; the default reports `NotObserved`.
    async fn observe_status(
        &self,
        key: &TokenKey,
    ) -> Result<TokenStoreObservation, TokenStoreError> {
        let _ = key;
        Ok(TokenStoreObservation::NotObserved)
    }
}

/// Errors raised by the refresh coordinator.
#[derive(Clone, Debug, Error)]
pub enum RefreshError {
    /// The credential loaded under the coordinator differs from the caller's
    /// selected identity. This is a use refusal, not a freshness classification.
    #[error("stored credential does not match the selected identity")]
    CredentialIdentityMismatch,
    /// The captured lifecycle or durable predecessor changed. No refresh
    /// success, failure transition or compensation may use that capture.
    #[error("refresh preparation is no longer current")]
    StalePreparation,
    #[error("refresh function failed: {0}")]
    Refresh(String),
    #[error("refresh function failed: {message}")]
    Observed {
        message: String,
        observation: RefreshFailureObservation,
    },
    #[error("refresh function failed: {message}")]
    Classified {
        message: String,
        observation: RefreshFailureObservation,
        disposition: RefreshFailureDisposition,
    },
    #[error("refresh requires interactive reauthorization: {0}")]
    ReauthRequired(String),
    /// The refresh response granted fewer scopes than the credential
    /// requires. A use refusal: the credential keeps its prior state.
    #[error("refresh response does not grant the required scopes")]
    RequiredScopesNotGranted,
    #[error("refresh in progress was cancelled")]
    Cancelled,
    #[error("cross-process lock acquisition failed: {0}")]
    LockFailed(String),
    /// The credential store or the refresh owner refused credential access
    /// without an answer. It is not a refresh-failure observation.
    #[error("credential access refused ({refusal}): {detail}")]
    CredentialAccess {
        refusal: TokenStoreRefusal,
        detail: String,
    },
    #[error("durable credential terminal commit failed: {message}")]
    DurableTerminalCommit {
        message: String,
        observation: RefreshFailureObservation,
        disposition: RefreshFailureDisposition,
    },
    /// The host egress profile refused a destination of the refresh before
    /// anything was connected to it: a local refusal, never evidence about
    /// the credential. A begun refresh was closed with `observation`, under
    /// the disposition AuthMachine issued when it classified one.
    #[error("refresh destination refused by the host egress profile: {refusal}")]
    EgressRefused {
        refusal: super::HttpEgressRefusal,
        observation: RefreshFailureObservation,
        disposition: Option<RefreshFailureDisposition>,
    },
    /// An admission, or another fenced operation, holds the credential fence
    /// of the credential's realm beyond the coordinator's wait. The refresh
    /// did not start: it sent no token request and wrote nothing. A refusal,
    /// never evidence about the credential; it may be retried once the hold
    /// ends, which for an admission may be only when its process restarts.
    #[error(
        "an admission or another fenced operation holds the credential fence; the refresh did \
         not start, sent no token request and wrote nothing"
    )]
    AdmissionInProgress,
    /// The credential fence could not be opened, created, locked or
    /// identity-checked. The refresh did not start and wrote nothing; it
    /// fails closed.
    #[error("the credential admission fence is unavailable; the refresh did not start: {0}")]
    AdmissionFenceUnavailable(String),
}

/// Machine-issued classification of a typed refresh-failure observation.
///
/// AuthMachine is the semantic owner of this verdict. Provider and persistence
/// shells may mirror it for durable ordering and public error projection, but
/// must never construct it by re-evaluating raw HTTP or OAuth fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshFailureDisposition {
    /// The refresh may be retried after returning the lease to Expiring.
    Transient,
    /// The credential is terminally unusable and interactive authorization is
    /// required.
    ReauthRequired,
}

/// Typed boundary evidence reported to AuthMachine after a refresh failure.
///
/// This is not a lifecycle classification.  It records facts the provider
/// boundary actually observed; AuthMachine owns the semantic permanent vs.
/// transient transition decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefreshFailureObservation {
    pub http_status: Option<u64>,
    pub oauth_error_code: Option<String>,
    pub local_credential_unusable: bool,
}

impl RefreshFailureObservation {
    pub fn transient() -> Self {
        Self::default()
    }

    pub fn http_status(status: u16) -> Self {
        Self {
            http_status: Some(u64::from(status)),
            ..Self::default()
        }
    }

    pub fn oauth_token_endpoint(status: u16, oauth_error_code: Option<String>) -> Self {
        Self {
            http_status: Some(u64::from(status)),
            oauth_error_code,
            ..Self::default()
        }
    }

    pub fn oauth_error_code(code: impl Into<String>) -> Self {
        Self {
            oauth_error_code: Some(code.into()),
            ..Self::default()
        }
    }

    pub fn local_credential_unusable() -> Self {
        Self {
            local_credential_unusable: true,
            ..Self::default()
        }
    }
}

impl RefreshError {
    pub fn observation(&self) -> RefreshFailureObservation {
        match self {
            Self::Observed { observation, .. }
            | Self::Classified { observation, .. }
            | Self::DurableTerminalCommit { observation, .. }
            | Self::EgressRefused { observation, .. } => observation.clone(),
            Self::ReauthRequired(_) => RefreshFailureObservation::local_credential_unusable(),
            Self::Refresh(_)
            | Self::CredentialIdentityMismatch
            | Self::RequiredScopesNotGranted
            | Self::Cancelled
            | Self::LockFailed(_)
            | Self::CredentialAccess { .. }
            | Self::AdmissionInProgress
            | Self::AdmissionFenceUnavailable(_)
            | Self::StalePreparation => RefreshFailureObservation::transient(),
        }
    }

    /// Return only a disposition issued by AuthMachine's generated classifier.
    /// Unclassified provider errors deliberately return `None` rather than
    /// re-deriving policy from their observation fields.
    pub fn refresh_failure_disposition(&self) -> Option<RefreshFailureDisposition> {
        match self {
            Self::Classified { disposition, .. }
            | Self::DurableTerminalCommit { disposition, .. } => Some(*disposition),
            Self::EgressRefused { disposition, .. } => *disposition,
            Self::Refresh(_)
            | Self::CredentialIdentityMismatch
            | Self::RequiredScopesNotGranted
            | Self::Observed { .. }
            | Self::ReauthRequired(_)
            | Self::Cancelled
            | Self::LockFailed(_)
            | Self::CredentialAccess { .. }
            | Self::AdmissionInProgress
            | Self::AdmissionFenceUnavailable(_)
            | Self::StalePreparation => None,
        }
    }

    /// Carry a token-store error into a refresh: a refused or failed credential
    /// access stays typed, a write its guard did not cover is the fence's
    /// refusal, and any other store error keeps its text.
    pub fn from_token_store(error: TokenStoreError) -> Self {
        if let TokenStoreError::AdmissionFence(detail) = error {
            return Self::AdmissionFenceUnavailable(detail);
        }
        match credential_access_parts(&error) {
            Some((refusal, detail)) => Self::CredentialAccess { refusal, detail },
            None => Self::Refresh(error.to_string()),
        }
    }

    /// The typed auth error for a refused credential access in a refresh: a
    /// held credential fence is pending work (retryable), an unavailable one
    /// an unavailable store.
    pub fn credential_access_error(&self) -> Option<super::error::AuthError> {
        match self {
            Self::CredentialAccess { refusal, detail } => {
                Some(refusal_auth_error(*refusal, detail.clone()))
            }
            Self::AdmissionInProgress => Some(super::error::AuthError::CredentialAccessPending),
            Self::AdmissionFenceUnavailable(detail) => Some(
                super::error::AuthError::CredentialStoreUnavailable(detail.clone()),
            ),
            _ => None,
        }
    }
}

/// The typed refusal parts of a credential-access error: a refusal keeps its
/// reason, and a failed backend is reported as unavailable.
fn credential_access_parts(error: &TokenStoreError) -> Option<(TokenStoreRefusal, String)> {
    match error {
        TokenStoreError::AccessRefused {
            refusal, detail, ..
        } => Some((*refusal, detail.clone())),
        TokenStoreError::BackendFailed { detail, .. } => {
            Some((TokenStoreRefusal::Unavailable, detail.clone()))
        }
        _ => None,
    }
}

/// Boxed refresh closure. The coordinator runs it under the credential fence
/// of the key's realm and hands it that operation's [`AdmissionGuard`],
/// which it passes to every write it makes (the token request, the commit,
/// the lifecycle publication and any rollback all run under it).
pub type RefreshFn = Box<
    dyn FnOnce(Arc<AdmissionGuard>) -> BoxFuture<'static, Result<PersistedTokens, RefreshError>>
        + Send
        + 'static,
>;

/// Errors raised while serializing one durable credential mutation.
///
/// Refresh classification remains AuthMachine-owned. This error only describes
/// the mechanics of entering and completing the shared mutation transaction
/// used by refresh and interactive credential replacement.
#[derive(Clone, Debug, Error)]
pub enum CredentialMutationError {
    #[error("credential preparation is no longer current")]
    StalePreparation,
    #[error("credential mutation failed: {0}")]
    Operation(String),
    #[error("credential token-store mutation failed: {0}")]
    TokenStore(String),
    #[error("credential lifecycle mutation failed: {0}")]
    AuthLifecycle(String),
    #[error("credential mutation was cancelled")]
    Cancelled,
    #[error("cross-process credential mutation lock acquisition failed: {0}")]
    LockFailed(String),
    /// The credential slot refused the publication. Its previous credential
    /// and lifecycle are untouched.
    #[error("credential slot refused the publication: {0}")]
    SlotRefused(CredentialSlotRefusal),
    /// The credential store or the mutation owner refused credential access
    /// without an answer.
    #[error("credential access refused ({refusal}): {detail}")]
    CredentialAccess {
        refusal: TokenStoreRefusal,
        detail: String,
    },
    /// A publication failed after it acquired the credential lifecycle, and
    /// restoring the previous credential and lifecycle failed too. The stored
    /// credential and its lifecycle may hold the new state, the previous one,
    /// or a mix of both: the outcome is unsettled, never a clean failure. A
    /// read of the stored credential reports what is durable.
    #[error("credential publication and its rollback failed: {0}")]
    RollbackFailed(String),
    /// An admission, or another fenced operation, holds the credential fence
    /// of the key's realm beyond the coordinator's wait. The mutation did not
    /// start and wrote nothing. A refusal, never an unknown outcome; it may
    /// be retried once the hold ends, which for an admission may be only
    /// when its process restarts.
    #[error(
        "an admission or another fenced operation holds the credential fence; the credential \
         mutation did not start and wrote nothing"
    )]
    AdmissionInProgress,
    /// The credential fence could not be opened, created, locked or
    /// identity-checked, or the operation's guard does not cover it. The
    /// mutation did not start and wrote nothing; it fails closed.
    #[error("the credential admission fence is unavailable; the mutation did not start: {0}")]
    AdmissionFenceUnavailable(String),
}

impl CredentialMutationError {
    /// Carry a token-store error into a mutation: a refused or failed
    /// credential access stays typed, a write its guard did not cover is the
    /// fence's refusal, and any other store error keeps its text.
    pub fn from_token_store(error: TokenStoreError) -> Self {
        if let TokenStoreError::AdmissionFence(detail) = error {
            return Self::AdmissionFenceUnavailable(detail);
        }
        match credential_access_parts(&error) {
            Some((refusal, detail)) => Self::CredentialAccess { refusal, detail },
            None => Self::TokenStore(error.to_string()),
        }
    }
}

/// Why a credential slot refused a publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialSlotRefusal {
    /// A discovery login publishes only into an empty slot.
    #[error("the credential slot is occupied")]
    Occupied,
    /// The slot holds a credential of another persisted mode.
    #[error("the credential slot holds a credential of another mode")]
    ModeMismatch,
    /// The slot's credential is bound to a different verified account.
    #[error("the credential slot is bound to a different account")]
    AccountMismatch,
    /// The slot's credential was issued under an incompatible context.
    #[error("the credential slot is bound to an incompatible context")]
    ContextMismatch,
    /// A connector credential is published only by a verified connector
    /// browser completion, never by a generic save path.
    #[error("a connector credential requires a verified connector completion")]
    UnverifiedConnectorPublication,
}

/// Typed durable result of one exclusive credential mutation.
///
/// Refresh and credential replacement publish the exact persisted token bytes;
/// logout/profile deletion publish the absence of durable credentials. Keeping
/// both outcomes in the coordinator contract lets every mutation share the
/// same per-key cross-process transaction without inventing sentinel tokens or
/// a second locking seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialMutationOutcome {
    Persisted(PersistedTokens),
    Cleared,
}

/// Boxed non-coalescing credential mutation closure. The coordinator runs
/// it under the credential fence of the key's realm and hands it that
/// operation's [`AdmissionGuard`], which it passes to every write it makes.
pub type CredentialMutationFn = Box<
    dyn FnOnce(
            Arc<AdmissionGuard>,
        )
            -> BoxFuture<'static, Result<CredentialMutationOutcome, CredentialMutationError>>
        + Send
        + 'static,
>;

/// Coordinator for token-refresh calls. Implementations coalesce
/// concurrent calls for the same `TokenKey`.
///
/// # The credential fence
///
/// Every credential write of a persisted store happens under the credential
/// fence C(r) of the written credential's realm, held EXCLUSIVE for the
/// whole operation. The coordinator of a persisted store takes it once per
/// operation: after in-process coalescing (refreshes that join one in flight
/// take nothing) and before its per-key lock and the closure, waiting no
/// longer than it waits for that lock. It runs the closure on a task it owns
/// and hands the closure the guard, so a caller that stops waiting releases
/// nothing before the closure's last write returned. A held fence refuses
/// typed (`AdmissionInProgress`, an unavailable one
/// `AdmissionFenceUnavailable`) before the closure runs: nothing was
/// requested or written. The coordinator of a store with no fence (in
/// memory, ephemeral) hands its closures [`AdmissionGuard::none`]. A
/// coordinator that wraps another delegates every entry, `_under` included,
/// and never acquires a fence itself.
#[async_trait]
pub trait RefreshCoordinator: Send + Sync {
    /// Run one per-key credential mutation under the coordinator's in-process
    /// and, when configured, cross-process exclusion boundary.
    ///
    /// Unlike refresh calls, distinct mutations are never coalesced: every
    /// closure runs exactly once after the prior transaction releases the key.
    async fn with_exclusive_mutation(
        &self,
        key: TokenKey,
        mutation_fn: CredentialMutationFn,
    ) -> Result<CredentialMutationOutcome, CredentialMutationError>;

    /// Run one per-key credential mutation, stating how the caller uses the
    /// credential. A coordinator that bounds admission (retained custody, for
    /// stores that can wait on the operating system) lets only an interactive
    /// mutation wait for earlier work on the key; a status or unattended
    /// mutation is admitted at once or refused with a typed pending answer.
    /// The default ignores `access` and runs [`Self::with_exclusive_mutation`].
    async fn with_exclusive_mutation_for(
        &self,
        key: TokenKey,
        access: TokenStoreAccess,
        mutation_fn: CredentialMutationFn,
    ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
        let _ = access;
        self.with_exclusive_mutation(key, mutation_fn).await
    }

    /// [`Self::with_exclusive_mutation_for`] for an operation that already
    /// holds the credential fence of `key`'s realm in `guard` (it acquired
    /// its whole fence set at its own outermost entry): it takes no fence and
    /// hands the closure `guard`. A second acquisition of a fence the process
    /// already holds would conflict with it and refuse. A persisted
    /// coordinator refuses a guard that does not cover C of `key`'s realm
    /// (`AdmissionFenceUnavailable`) before the closure runs. A coordinator
    /// that wraps another delegates it. The default refuses
    /// (`AdmissionFenceUnavailable`) without running the closure: a
    /// coordinator that does not implement it never runs a fenced operation
    /// unfenced.
    async fn with_exclusive_mutation_under(
        &self,
        guard: Arc<AdmissionGuard>,
        key: TokenKey,
        access: TokenStoreAccess,
        mutation_fn: CredentialMutationFn,
    ) -> Result<CredentialMutationOutcome, CredentialMutationError> {
        let _ = (guard, key, access, mutation_fn);
        Err(CredentialMutationError::AdmissionFenceUnavailable(
            "this credential coordinator does not run operations that already hold the \
             credential fence; nothing was started or written"
                .into(),
        ))
    }

    /// The credential fence C of `realm` this coordinator takes for a key of
    /// that realm: for an operation that takes it in one set with other
    /// fences ([`AdmissionGuard::acquire`], in the global order) and then
    /// enters through [`Self::with_exclusive_mutation_under`]. `Ok(None)`
    /// when the coordinator's store has no fence (in memory, ephemeral). A
    /// coordinator that wraps another delegates it. The default refuses
    /// (`AdmissionFenceUnavailable`): a coordinator that does not name its
    /// fence serves no such operation.
    #[cfg(not(target_arch = "wasm32"))]
    fn credential_fence(
        &self,
        realm: &crate::connection::RealmId,
    ) -> Result<Option<crate::realm_admission::AdmissionFence>, CredentialMutationError> {
        let _ = realm;
        Err(CredentialMutationError::AdmissionFenceUnavailable(
            "this credential coordinator does not name its credential fence; nothing was \
             started or written"
                .into(),
        ))
    }

    async fn with_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError>;

    async fn with_forced_refresh(
        &self,
        key: TokenKey,
        refresh_fn: RefreshFn,
    ) -> Result<PersistedTokens, RefreshError> {
        self.with_refresh(key, refresh_fn).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::{CredentialAccountRef, connection::BindingOrigin};

    #[test]
    fn persisted_tokens_debug_redacts_secret_and_tokens() {
        const SECRET: &str = "sk-live-secret-value";
        let mut tokens = PersistedTokens::api_key(SECRET);
        tokens.refresh_token = Some(format!("refresh-{SECRET}"));
        tokens.id_token = Some(format!("id-{SECRET}"));
        tokens.account_id = Some("acct-visible".into());
        let rendered = format!("{tokens:?} {tokens:#?}");
        assert!(!rendered.contains(SECRET), "secret leaked: {rendered}");
        assert!(rendered.contains("acct-visible"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn binding_key_serialization_remains_legacy_compatible() {
        let key = TokenKey::from_auth_binding(&AuthBindingRef {
            realm: RealmId::parse("global").expect("valid realm"),
            binding: BindingId::parse("openai").expect("valid binding"),
            profile: None,
            origin: BindingOrigin::Configured,
        });
        assert_eq!(
            serde_json::to_value(&key).expect("serialize"),
            serde_json::json!({"realm": "global", "binding": "openai"})
        );
        assert_eq!(
            serde_json::from_value::<TokenKey>(
                serde_json::json!({"realm": "global", "binding": "openai"})
            )
            .expect("deserialize legacy key"),
            key
        );
        let synthetic = TokenKey::from_auth_binding(&AuthBindingRef {
            realm: RealmId::parse("global").expect("valid realm"),
            binding: BindingId::parse("openai").expect("valid binding"),
            profile: None,
            origin: BindingOrigin::SyntheticEnvDefault,
        });
        assert_eq!(
            synthetic, key,
            "route provenance is not credential identity"
        );
    }

    #[test]
    fn connector_client_secret_keys_are_disjoint_from_every_credential_key() {
        let meaning = "a".repeat(64);
        let realm = RealmId::parse("global").expect("valid realm");
        let secret =
            TokenKey::connector_client_secret(realm.clone(), &format!("desktop@{meaning}"))
                .expect("secret key");
        // The legacy binding a reserved name would have collided with.
        let legacy = TokenKey::parse_with_profile("global", "connector-client", Some("desktop"))
            .expect("legacy binding key");
        assert_eq!(legacy.storage_stem(), "connector-client@desktop");
        assert_eq!(legacy.keyring_account(), "global:connector-client:desktop");
        assert_eq!(
            secret.storage_stem(),
            format!("connector-client~v1~desktop~{meaning}")
        );
        assert_eq!(
            secret.keyring_account(),
            format!("global:connector-client~v1~desktop~{meaning}")
        );
        assert_ne!(secret, legacy);
        assert!(secret.credential_identity().is_none());
        assert!(secret.is_connector_client_secret());
        assert_eq!(secret.realm(), &realm);
        assert_eq!(
            TokenKey::connector_client_secret_from_stem(&realm, &secret.storage_stem()),
            Some(secret.clone())
        );
        assert_eq!(
            TokenKey::connector_client_secret_from_stem(&realm, &legacy.storage_stem()),
            None
        );
        let encoded = serde_json::to_value(&secret).expect("encode");
        assert_eq!(
            serde_json::from_value::<TokenKey>(encoded).expect("decode"),
            secret
        );
        // Credential keys keep their historical encoding.
        let encoded = serde_json::to_value(&legacy).expect("encode");
        assert!(encoded.get("connector_client_secret").is_none());
        assert_eq!(
            serde_json::from_value::<TokenKey>(encoded).expect("decode"),
            legacy
        );
        assert!(
            TokenKey::connector_client_secret(realm, "desktop@not-a-digest").is_err(),
            "the meaning must be the canonical digest"
        );
    }

    #[test]
    fn mcp_oauth_client_keys_are_disjoint_from_their_slot_credential_key() {
        let realm = RealmId::parse("mcp-oauth").expect("valid realm");
        let binding = BindingId::parse("remote-AbC123dEf456GhI7").expect("valid binding");
        let client = TokenKey::mcp_oauth_client(realm.clone(), binding.clone());
        let credential = TokenKey::new(realm.clone(), binding);
        assert_ne!(client, credential);
        assert!(client.is_mcp_oauth_client());
        assert!(!credential.is_mcp_oauth_client());
        assert!(client.credential_identity().is_none());
        assert_eq!(client.realm(), &realm);
        assert_eq!(
            client.storage_stem(),
            "mcp-oauth-client~v1~remote-AbC123dEf456GhI7"
        );
        assert_eq!(
            client.keyring_account(),
            "mcp-oauth:mcp-oauth-client~v1~remote-AbC123dEf456GhI7"
        );
        assert_ne!(client.storage_stem(), credential.storage_stem());
        assert_ne!(client.keyring_account(), credential.keyring_account());
        assert_eq!(
            TokenKey::mcp_oauth_client_from_stem(&realm, &client.storage_stem()),
            Some(client.clone())
        );
        assert_eq!(
            TokenKey::mcp_oauth_client_from_stem(&realm, &credential.storage_stem()),
            None
        );
        let encoded = serde_json::to_value(&client).expect("encode");
        assert!(encoded.get("mcp_oauth_client").is_some(), "{encoded}");
        assert_eq!(
            serde_json::from_value::<TokenKey>(encoded).expect("decode"),
            client
        );
        // The slot's credential key keeps its historical encoding.
        let encoded = serde_json::to_value(&credential).expect("encode");
        assert!(encoded.get("mcp_oauth_client").is_none());
        assert_eq!(
            serde_json::from_value::<TokenKey>(encoded).expect("decode"),
            credential
        );
        assert_eq!(
            serde_json::to_value(PersistedAuthMode::McpOauthClient).expect("encode"),
            serde_json::json!("mcp_oauth_client")
        );
    }

    #[test]
    fn account_keys_have_disjoint_storage_names() {
        let identity = AuthCredentialIdentity::Account(CredentialAccountRef {
            realm: RealmId::parse("global").expect("valid realm"),
            account: CredentialAccountId::parse("github_copilot").expect("valid account"),
        });
        let key = TokenKey::from_credential_identity(&identity);
        assert_eq!(key.storage_stem(), "account~github_copilot");
        assert_eq!(key.keyring_account(), "global:account~github_copilot");
        assert_eq!(
            serde_json::from_value::<TokenKey>(
                serde_json::to_value(&key).expect("serialize account key")
            )
            .expect("deserialize account key"),
            key
        );
    }
}

/// Complete provider-auth persistence capability.
///
/// Rotating provider credentials are only safe when their vault and mutation
/// authority agree. Keeping the [`TokenStore`] and [`RefreshCoordinator`] in
/// one value makes that pairing an invariant of provider-resolution and
/// factory composition instead of a convention reconstructed by each caller.
/// Native persisted backends construct this value from one backend decision;
/// ephemeral hosts and tests must pair their process-local store and
/// coordinator explicitly through [`Self::new`].
static NEXT_PROVIDER_AUTH_PERSISTENCE_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local identity of one token-vault and mutation-authority pairing.
///
/// Clones retain the same identity. Independently constructed persistence
/// capabilities never share derived credential caches, even when they contain
/// the same logical token key and lifecycle generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderAuthPersistenceId(u64);

impl ProviderAuthPersistenceId {
    fn next() -> Self {
        Self(NEXT_PROVIDER_AUTH_PERSISTENCE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct ProviderAuthPersistence {
    authority_id: ProviderAuthPersistenceId,
    token_store: Arc<dyn TokenStore>,
    refresh_coordinator: Arc<dyn RefreshCoordinator>,
}

impl ProviderAuthPersistence {
    /// Pair one token vault with the authority that serializes its mutations.
    pub fn new(
        token_store: Arc<dyn TokenStore>,
        refresh_coordinator: Arc<dyn RefreshCoordinator>,
    ) -> Self {
        Self {
            authority_id: ProviderAuthPersistenceId::next(),
            token_store,
            refresh_coordinator,
        }
    }

    pub fn authority_id(&self) -> ProviderAuthPersistenceId {
        self.authority_id
    }

    pub fn token_store(&self) -> Arc<dyn TokenStore> {
        Arc::clone(&self.token_store)
    }

    pub fn refresh_coordinator(&self) -> Arc<dyn RefreshCoordinator> {
        Arc::clone(&self.refresh_coordinator)
    }
}

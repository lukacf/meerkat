//! Wire-facing projections of the realm connection types
//! (`AuthBindingRef`, `BackendProfile`, `AuthProfile`, `ProviderBinding`,
//! `RealmConnectionSet`, `AuthStatus`).
//!
//! `meerkat-core` owns the domain types; this module re-projects them
//! with wire-friendly field shapes and adds `#[schemars]` attributes
//! for SDK codegen. The projection is lossy by design: `Provider` is
//! string-typed on the wire, DateTime is ISO-8601 string, and sensitive
//! secret material never crosses a wire boundary (the wire projection
//! only carries IDs + non-secret metadata).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Canonical OAuth issuer identity carried by login request/response contracts.
///
/// Deserialization accepts the CLI-compatible aliases owned by
/// [`meerkat_core::OAuthProviderIdentity`], while serialization always emits
/// one canonical wire value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WireOAuthProvider {
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "google")]
    Google,
    #[serde(rename = "copilot")]
    Copilot,
}

impl WireOAuthProvider {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
            Self::Copilot => "copilot",
        }
    }

    pub const fn identity(self) -> meerkat_core::OAuthProviderIdentity {
        match self {
            Self::Anthropic => meerkat_core::OAuthProviderIdentity::AnthropicClaudeAi,
            Self::OpenAi => meerkat_core::OAuthProviderIdentity::OpenAiChatGpt,
            Self::Google => meerkat_core::OAuthProviderIdentity::GoogleCodeAssist,
            Self::Copilot => meerkat_core::OAuthProviderIdentity::GitHubCopilot,
        }
    }
}

impl std::fmt::Display for WireOAuthProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WireOAuthProvider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match meerkat_core::OAuthProviderIdentity::from_alias(&value) {
            Some(meerkat_core::OAuthProviderIdentity::AnthropicClaudeAi) => Ok(Self::Anthropic),
            Some(meerkat_core::OAuthProviderIdentity::OpenAiChatGpt) => Ok(Self::OpenAi),
            Some(meerkat_core::OAuthProviderIdentity::GoogleCodeAssist) => Ok(Self::Google),
            Some(meerkat_core::OAuthProviderIdentity::GitHubCopilot) => Ok(Self::Copilot),
            Some(meerkat_core::OAuthProviderIdentity::AnthropicConsoleApiKey) | None => {
                Err(serde::de::Error::unknown_variant(
                    &value,
                    &["anthropic", "openai", "google", "copilot"],
                ))
            }
        }
    }
}

/// Wire projection of [`meerkat_core::AuthBindingRef`].
///
/// Pure structural shape — no `"realm:binding"` string form. Wave-b deleted
/// `parse` and `Display` on both the core type and the wire projection so
/// the colon-joined form cannot travel across wire boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthBindingRef {
    pub realm: meerkat_core::connection::RealmId,
    pub binding: meerkat_core::connection::BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<meerkat_core::connection::ProfileId>,
}

impl From<meerkat_core::AuthBindingRef> for WireAuthBindingRef {
    fn from(value: meerkat_core::AuthBindingRef) -> Self {
        Self {
            realm: value.realm,
            binding: value.binding,
            profile: value.profile,
        }
    }
}

impl From<WireAuthBindingRef> for meerkat_core::AuthBindingRef {
    fn from(value: WireAuthBindingRef) -> Self {
        Self {
            realm: value.realm,
            binding: value.binding,
            profile: value.profile,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        }
    }
}

/// Request payload for `auth/profile/list` and `realm/get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealmIdParams {
    pub realm_id: String,
}

/// Request payload for binding-scoped auth methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BindingIdParams {
    pub realm_id: String,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Request payload for `auth/profile/create`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateProfileParams {
    pub realm_id: String,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub auth_method: String,
    pub secret: String,
}

/// Redacts the secret.
impl std::fmt::Debug for CreateProfileParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateProfileParams")
            .field("realm_id", &self.realm_id)
            .field("binding_id", &self.binding_id)
            .field("profile_id", &self.profile_id)
            .field("auth_method", &self.auth_method)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// The target enums below are untagged but disjoint (each arm has required
/// fields and denies unknown ones), so their schema is `oneOf`, which the
/// SDK generators expand into typed variants.
#[cfg(feature = "schema")]
fn untagged_target_one_of(schema: &mut schemars::Schema) {
    if let Some(object) = schema.as_object_mut()
        && let Some(any_of) = object.remove("anyOf")
    {
        object.insert("oneOf".to_owned(), any_of);
    }
}

/// OAuth-protected MCP server addressed by `auth/login/*` and
/// `auth/status/get` instead of a provider binding.
///
/// `server_name` and `server_url` identify the configured server;
/// `oauth_account` is the selected account the login must prove (the OIDC
/// subject for the default account strategy). Login is host-driven: the
/// authorize URL and state are host-channel data and must never reach an
/// agent, tool result, transcript or log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireMcpAuthTarget {
    pub server_name: String,
    pub server_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_account: Option<String>,
}

/// Credential slot of a connector credential: a realm-scoped storage
/// address chosen by the trusted host. It is not proof of any provider
/// account; the provider-verified account is reported separately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireConnectorSlot {
    pub realm_id: String,
    pub slot_id: String,
}

/// Which provider account a connector login must prove.
///
/// `known` refuses any other verified account. `discover` admits the login
/// with no account and binds the provider-verified account at completion;
/// it publishes only into an empty slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireConnectorAccountSelection {
    Known { account: String },
    Discover,
}

/// Generic connector OAuth target addressed by `auth/login/start` and
/// `auth/login/complete`: the slot plus the descriptor facts the attempt is
/// admitted with. `strategy_id` names an account strategy installed on the
/// host (`oidc-userinfo-v1` by default). Login is host-driven: the
/// authorize URL and state are host-channel data and must never reach an
/// agent, tool result, transcript or log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireConnectorAuthTarget {
    pub slot: WireConnectorSlot,
    pub issuer: String,
    pub client: String,
    pub resource: String,
    pub scopes: Vec<String>,
    pub strategy_id: String,
    pub account_selection: WireConnectorAccountSelection,
}

/// Connector arm of [`WireLoginTarget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireConnectorLoginTarget {
    pub connector: WireConnectorAuthTarget,
}

/// A connector slot addressed by `auth/status/get`, `auth/login/cancel` and
/// the start/complete results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireConnectorSlotTarget {
    pub connector: WireConnectorSlot,
}

/// The provider account bound to a connector credential, qualified by the
/// issuer and the strategy that verified it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireConnectorVerifiedAccount {
    pub issuer: String,
    pub strategy_id: String,
    pub subject: String,
}

/// What authorizes a connector credential's granted scopes. Assigned by the
/// native owner from its own parsed token response, never by a caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireScopeEvidence {
    /// Parsed from the token-endpoint response that issued the access token.
    TokenEndpointResponse,
    /// A refresh response without `scope` kept the original grant, committed
    /// at `granted_at` (RFC 3339).
    RetainedOnRefresh { granted_at: String },
}

/// Typed reason of an auth error on the RPC and REST surfaces: RPC
/// `error.data.reason`, REST body `reason`. Hosts branch on it, never on the
/// error text. One native mapping (`HostAuthError::reason`) owns it; the
/// existing status codes and RPC error codes are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireAuthErrorReason {
    /// Malformed or inconsistent target (ids, provider, backend, auth
    /// method, source, connector target, redirect or descriptor).
    InvalidTarget,
    RealmNotFound,
    BindingNotFound,
    /// The binding exists but its configuration is invalid.
    BindingInvalid,
    /// A credential write addressed a binding the realm only inherits.
    BindingInherited,
    /// The provider does not support the requested login flow.
    FlowUnsupported,
    McpServerNotConfigured,
    /// The MCP target differs from the configured server, or it does not
    /// use OAuth login.
    McpServerMismatch,
    AccountSelectionRequired,
    UnknownStrategy,
    /// No live attempt under this state (unknown, expired or consumed).
    AttemptMissing,
    /// The attempt exists but the target, provider, identity, redirect or
    /// descriptor differs.
    AttemptMismatch,
    DevicePollInProgress,
    DeviceCodeAlreadyAdmitted,
    DeviceExpiryInvalid,
    /// The provider-verified account is not the expected one.
    AccountMismatch,
    MissingScopes,
    /// Token material does not match the verified evidence.
    CredentialMismatch,
    /// The account strategy could not verify the provider account.
    VerificationUnavailable,
    SlotOccupied,
    SlotAccountMismatch,
    SlotContextMismatch,
    SlotModeMismatch,
    UnverifiedConnectorPublication,
    ReauthRequired,
    /// No usable credential: a human must authorize through the host.
    AuthorizationRequired,
    /// The host's loopback callback could not be bound, failed or timed out.
    CallbackUnavailable,
    /// The provider's authorization server failed (discovery, registration,
    /// token exchange or refresh).
    UpstreamFailure,
    ConfigurationInvalid,
    /// An internal failure; its detail is only in protected diagnostics.
    Infrastructure,
}

/// Provider binding addressed by `auth/login/*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireProviderLoginTarget {
    pub provider: WireOAuthProvider,
    pub realm_id: String,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Login target: a provider binding (the original flat fields), an MCP
/// server (`{"mcp": {...}}`) or a connector (`{"connector": {...}}`).
/// Exactly one shape is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(transform = untagged_target_one_of))]
pub enum WireLoginTarget {
    Provider(WireProviderLoginTarget),
    Mcp(WireMcpLoginTarget),
    Connector(WireConnectorLoginTarget),
}

/// MCP arm of [`WireLoginTarget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireMcpLoginTarget {
    pub mcp: WireMcpAuthTarget,
}

/// Request payload for `auth/login/start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoginStartParams {
    #[serde(flatten)]
    pub target: WireLoginTarget,
    pub redirect_uri: String,
}

/// Request payload for `auth/login/complete`. For an MCP target, issuer,
/// client and resource come from the admitted attempt named by `state`;
/// nothing else is echoed. `Debug` redacts `code` and `state`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LoginCompleteParams {
    #[serde(flatten)]
    pub target: WireLoginTarget,
    pub code: String,
    pub state: String,
    pub redirect_uri: String,
}

impl std::fmt::Debug for LoginCompleteParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginCompleteParams")
            .field("target", &self.target)
            .field("code", &"<redacted>")
            .field("state", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

/// Request payload for `auth/login/cancel`: retire the pending attempt
/// admitted under `state` for a configured MCP server (`{"mcp": {...}}`) or
/// a connector slot (`{"connector": {...}}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(transform = untagged_target_one_of))]
pub enum LoginCancelParams {
    Mcp(McpLoginCancelParams),
    Connector(ConnectorLoginCancelParams),
}

impl LoginCancelParams {
    pub fn state(&self) -> &str {
        match self {
            Self::Mcp(params) => &params.state,
            Self::Connector(params) => &params.state,
        }
    }
}

/// MCP arm of [`LoginCancelParams`]. `Debug` redacts `state`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct McpLoginCancelParams {
    pub mcp: WireMcpAuthTarget,
    pub state: String,
}

impl std::fmt::Debug for McpLoginCancelParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpLoginCancelParams")
            .field("mcp", &self.mcp)
            .field("state", &"<redacted>")
            .finish()
    }
}

/// Connector arm of [`LoginCancelParams`]. `Debug` redacts `state`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConnectorLoginCancelParams {
    pub connector: WireConnectorSlot,
    pub state: String,
}

impl std::fmt::Debug for ConnectorLoginCancelParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorLoginCancelParams")
            .field("connector", &self.connector)
            .field("state", &"<redacted>")
            .finish()
    }
}

/// `auth/login/cancel` success body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLoginCancelled {
    #[serde(flatten)]
    pub target: WireLoginCancelledTarget,
    pub cancelled: bool,
}

/// Target echo of [`WireLoginCancelled`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(transform = untagged_target_one_of))]
pub enum WireLoginCancelledTarget {
    Mcp(WireMcpLoginTarget),
    Connector(WireConnectorSlotTarget),
}

/// Request payload for `auth/status/get`: a provider binding (the original
/// flat fields), an MCP server (`{"mcp": {...}}`) or a connector slot
/// (`{"connector": {...}}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(transform = untagged_target_one_of))]
pub enum AuthStatusParams {
    Binding(BindingIdParams),
    Mcp(WireMcpLoginTarget),
    Connector(WireConnectorSlotTarget),
}

/// Request payload for `auth/login/device_start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeviceStartParams {
    pub provider: WireOAuthProvider,
    pub realm_id: String,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Request payload for `auth/login/device_complete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DeviceCompleteParams {
    pub provider: WireOAuthProvider,
    pub device_code: String,
    pub realm_id: String,
    pub binding_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Request payload for `auth/login/provision_api_key`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProvisionApiKeyParams {
    /// Access token acquired from a prior Console-OAuth flow.
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

/// Redacts the access token.
impl std::fmt::Debug for ProvisionApiKeyParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionApiKeyParams")
            .field("access_token", &"<redacted>")
            .field("realm_id", &self.realm_id)
            .field("binding_id", &self.binding_id)
            .field("profile_id", &self.profile_id)
            .finish()
    }
}

/// Wire projection of [`meerkat_core::BackendProfile`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireBackendProfile {
    pub id: String,
    /// Provider as a normalized string: `"openai"` / `"anthropic"` /
    /// `"gemini"` / `"self_hosted"`. Wire types don't carry the strict
    /// `Provider` enum so consumers aren't forced to update their
    /// schema when we add new providers.
    pub provider: String,
    pub backend_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

impl From<&meerkat_core::BackendProfile> for WireBackendProfile {
    fn from(value: &meerkat_core::BackendProfile) -> Self {
        Self {
            id: value.id.clone(),
            provider: value.provider.as_str().to_string(),
            backend_kind: value.backend_kind.clone(),
            base_url: value.base_url.clone(),
            options: value.options.clone(),
        }
    }
}

/// Wire projection of [`meerkat_core::AuthProfile`]. Sensitive credential
/// material is NOT wire-projected; callers that inspect profile metadata use
/// the server-side `auth.profile.get` RPC or the
/// `GET /auth/bindings/{binding_id}` REST path; both return typed redacted
/// shapes. `source_kind` is a
/// discriminator for the credential-source variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfile {
    pub id: String,
    pub provider: String,
    pub auth_method: String,
    /// Discriminator: "inline_secret" / "managed_store" / "env" /
    /// "external_resolver" / "platform_default" / "command" /
    /// "file_descriptor".
    pub source_kind: String,
}

impl From<&meerkat_core::AuthProfile> for WireAuthProfile {
    fn from(value: &meerkat_core::AuthProfile) -> Self {
        Self {
            id: value.id.clone(),
            provider: value.provider.as_str().to_string(),
            auth_method: value.auth_method.clone(),
            source_kind: value.source.kind_label().to_string(),
        }
    }
}

/// Wire projection of [`meerkat_core::ProviderBinding`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProviderBinding {
    pub id: String,
    pub backend_profile: String,
    pub auth_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_account: Option<meerkat_core::CredentialAccountId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub allow_auth_override: bool,
    #[serde(default)]
    pub require_metadata_account: bool,
    #[serde(default)]
    pub require_metadata_workspace: bool,
}

impl From<&meerkat_core::ProviderBinding> for WireProviderBinding {
    fn from(value: &meerkat_core::ProviderBinding) -> Self {
        Self {
            id: value.id.clone(),
            backend_profile: value.backend_profile.clone(),
            auth_profile: value.auth_profile.clone(),
            credential_account: value.credential_account.clone(),
            default_model: value.default_model.clone(),
            allow_auth_override: value.policy.allow_auth_override,
            require_metadata_account: value.policy.require_metadata_account,
            require_metadata_workspace: value.policy.require_metadata_workspace,
        }
    }
}

/// Wire projection of [`meerkat_core::RealmConnectionSet`]. Returned
/// from the `realm/get` / `GET /realm/:id` endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRealmConnectionSet {
    pub realm_id: String,
    pub backends: BTreeMap<String, WireBackendProfile>,
    pub auth_profiles: BTreeMap<String, WireAuthProfile>,
    pub bindings: BTreeMap<String, WireProviderBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
}

impl From<&meerkat_core::RealmConnectionSet> for WireRealmConnectionSet {
    fn from(value: &meerkat_core::RealmConnectionSet) -> Self {
        Self {
            realm_id: value.realm_id.to_string(),
            backends: value
                .backends
                .iter()
                .map(|(k, v)| (k.clone(), WireBackendProfile::from(v)))
                .collect(),
            auth_profiles: value
                .auth_profiles
                .iter()
                .map(|(k, v)| (k.clone(), WireAuthProfile::from(v)))
                .collect(),
            bindings: value
                .bindings
                .iter()
                .map(|(k, v)| (k.clone(), WireProviderBinding::from(v)))
                .collect(),
            default_binding: value.default_binding.clone(),
        }
    }
}

// --- Auth status projection -------------------------------------------

/// Stable wire kind for auth errors. Mirrors `meerkat_core::AuthErrorKind`
/// on the wire as a normalized string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireAuthError {
    MissingSecret,
    UnsupportedCombination { backend: String, auth: String },
    MissingRequiredMetadata { field: String },
    WorkspaceMismatch,
    Expired,
    StaleCredential,
    RefreshRequired,
    LeaseAbsent,
    UserReauthRequired,
    RefreshFailed { detail: String },
    ResolveRequired { detail: String },
    InteractiveLoginRequired,
    HostOwnedUnavailable,
    Io { detail: String },
    Other { detail: String },
}

impl From<meerkat_core::AuthError> for WireAuthError {
    fn from(value: meerkat_core::AuthError) -> Self {
        match value {
            meerkat_core::AuthError::MissingSecret => Self::MissingSecret,
            meerkat_core::AuthError::UnsupportedCombination { backend, auth } => {
                Self::UnsupportedCombination { backend, auth }
            }
            meerkat_core::AuthError::MissingRequiredMetadata(field) => {
                Self::MissingRequiredMetadata { field }
            }
            meerkat_core::AuthError::WorkspaceMismatch => Self::WorkspaceMismatch,
            meerkat_core::AuthError::Expired => Self::Expired,
            meerkat_core::AuthError::StaleCredential => Self::StaleCredential,
            meerkat_core::AuthError::RefreshRequired => Self::RefreshRequired,
            meerkat_core::AuthError::LeaseAbsent => Self::LeaseAbsent,
            meerkat_core::AuthError::UserReauthRequired => Self::UserReauthRequired,
            meerkat_core::AuthError::RefreshFailed(detail) => Self::RefreshFailed { detail },
            meerkat_core::AuthError::ResolveRequired(detail) => Self::ResolveRequired { detail },
            meerkat_core::AuthError::InteractiveLoginRequired => Self::InteractiveLoginRequired,
            meerkat_core::AuthError::HostOwnedUnavailable => Self::HostOwnedUnavailable,
            meerkat_core::AuthError::Io(detail) => Self::Io { detail },
            meerkat_core::AuthError::Other(detail) => Self::Other { detail },
        }
    }
}

/// Wire projection of the auth-profile status. Returned from
/// `auth.status.get` / `GET /auth/bindings/{binding_id}/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthStatus {
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    /// High-level health projected from typed auth-lease lifecycle truth.
    pub state: meerkat_core::AuthStatusPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(with = "Option<String>"))]
    pub last_refresh_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Most recent auth error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<WireAuthError>,
}

// --- Auth REST response envelopes ------------------------------------

/// Identifies a binding inside a realm on the wire. Shared by every
/// auth REST response that returns a `{realm_id, binding_id, auth_binding}`
/// trio. Built from a typed [`meerkat_core::AuthBindingRef`] so the three
/// fields always agree; the `realm_id`/`binding_id` strings carry the
/// slug form for wire consumers that key by string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireBindingIdentity {
    pub realm_id: String,
    pub binding_id: String,
    pub auth_binding: WireAuthBindingRef,
}

impl From<&meerkat_core::AuthBindingRef> for WireBindingIdentity {
    fn from(cref: &meerkat_core::AuthBindingRef) -> Self {
        Self {
            realm_id: cref.realm.to_string(),
            binding_id: cref.binding.to_string(),
            auth_binding: WireAuthBindingRef::from(cref.clone()),
        }
    }
}

/// `POST /auth/profiles` (create) success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfileCreated {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    pub stored: bool,
}

/// `GET /auth/bindings/{binding_id}` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfileDetail {
    pub auth_binding: WireAuthBindingRef,
    pub binding_id: String,
    pub profile_id: String,
    pub auth_profile: WireAuthProfile,
}

/// `DELETE /auth/bindings/{binding_id}` /
/// `POST /auth/bindings/{binding_id}/logout` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfileCleared {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub cleared: bool,
}

/// `POST /auth/login/start` success body. Host-channel data: the
/// authorize URL and state must never reach an agent, tool result,
/// transcript or log.
#[derive(Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLoginStart {
    pub authorize_url: String,
    pub state: String,
    pub redirect_uri: String,
    #[serde(flatten)]
    pub target: WireLoginStartTarget,
}

impl std::fmt::Debug for WireLoginStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireLoginStart")
            .field("authorize_url", &"<redacted>")
            .field("state", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field("target", &self.target)
            .finish()
    }
}

/// Target echo of [`WireLoginStart`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(transform = untagged_target_one_of))]
pub enum WireLoginStartTarget {
    Provider(WireProviderLoginStart),
    Mcp(WireMcpLoginStart),
    Connector(WireConnectorSlotTarget),
}

/// Provider arm of [`WireLoginStartTarget`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireProviderLoginStart {
    pub provider: WireOAuthProvider,
}

/// MCP arm of [`WireLoginStartTarget`]. A joined start returns the pending
/// attempt's authorize URL and state: wire callers are host-privileged by
/// contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireMcpLoginStart {
    pub mcp: WireMcpAuthTarget,
    pub disposition: WireMcpLoginDisposition,
}

/// Whether `auth/login/start` admitted a new MCP attempt or returned the one
/// already pending for the target (no second attempt is ever admitted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMcpLoginDisposition {
    Started,
    Joined,
}

/// `POST /auth/login/complete` / ready leg of device-code success body.
///
/// The optional `state` field distinguishes the flat `POST
/// /auth/login/complete` response (no `state` set) from the device-code
/// ready leg (`state = "ready"`) which is part of the pending/slow_down/
/// access_denied/expired/ready tagged protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireLoginReady {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(flatten)]
    pub target: WireLoginReadyTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub has_refresh_token: bool,
    pub scopes: Vec<String>,
}

/// Target of a completed login.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(transform = untagged_target_one_of))]
pub enum WireLoginReadyTarget {
    Provider(WireProviderLoginReady),
    Mcp(WireMcpLoginReady),
    Connector(WireConnectorLoginReady),
}

/// Connector arm of [`WireLoginReadyTarget`]: the slot and the verified
/// account as separate facts, plus the owner-assigned scope evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireConnectorLoginReady {
    pub connector: WireConnectorSlot,
    pub verified_account: WireConnectorVerifiedAccount,
    pub scope_evidence: WireScopeEvidence,
}

/// Provider arm of [`WireLoginReadyTarget`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProviderLoginReady {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: WireOAuthProvider,
}

/// MCP arm of [`WireLoginReadyTarget`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireMcpLoginReady {
    pub mcp: WireMcpAuthTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// `POST /auth/login/device/start` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
    pub provider: WireOAuthProvider,
}

/// `auth/login/device_complete` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WireDeviceCompleteResult {
    Pending,
    SlowDown,
    AccessDenied,
    Expired,
    Ready {
        #[serde(flatten)]
        identity: Box<WireBindingIdentity>,
        profile_id: String,
        provider: WireOAuthProvider,
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
        has_refresh_token: bool,
        scopes: Vec<String>,
    },
}

/// `auth/login/provision_api_key` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireProvisionApiKeyResult {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: String,
    pub auth_mode: String,
    pub has_api_key: bool,
    pub scopes: Vec<String>,
}

/// Realm summary entry returned by `GET /realms`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRealmSummary {
    pub realm_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
    pub backend_count: usize,
    pub auth_profile_count: usize,
    pub binding_count: usize,
}

/// `GET /realms` success body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireRealmList {
    pub realms: Vec<WireRealmSummary>,
}

/// `GET /auth/profiles` success body — realm-scoped lists of backend,
/// auth, and binding profiles.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthProfilesList {
    pub realm_id: String,
    pub auth_profiles: Vec<WireAuthProfile>,
    pub backend_profiles: Vec<WireBackendProfile>,
    pub bindings: Vec<WireProviderBinding>,
}

/// `GET /auth/bindings/{binding_id}/status` success body. Richer than
/// [`WireAuthStatus`] — also carries `realm_id` / `binding_id` /
/// `auth_binding` / `has_refresh_token` so the caller can key by
/// binding directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAuthStatusDetail {
    #[serde(flatten)]
    pub identity: WireBindingIdentity,
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    pub state: meerkat_core::AuthStatusPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    pub has_refresh_token: bool,
}

/// Secret-free authorization phase of an MCP server target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireMcpAuthPhase {
    Authorized,
    ReauthRequired,
    /// No usable credential: awaiting human authorization through the host.
    AuthorizationRequired,
}

/// `auth/status/get` result for an MCP server target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireMcpAuthStatus {
    pub mcp: WireMcpAuthTarget,
    pub phase: WireMcpAuthPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

/// `auth/status/get` result: a provider binding status or an MCP status.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
#[cfg_attr(feature = "schema", schemars(transform = untagged_target_one_of))]
pub enum WireAuthStatusResult {
    Binding(WireAuthStatusDetail),
    Mcp(WireMcpAuthStatus),
    Connector(WireConnectorAuthStatus),
}

/// `auth/status/get` result for a connector slot. The slot and the verified
/// account are separate facts: the slot name is never account proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireConnectorAuthStatus {
    pub connector: WireConnectorSlot,
    pub phase: WireMcpAuthPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_account: Option<WireConnectorVerifiedAccount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_evidence: Option<WireScopeEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub has_refresh_token: bool,
}

/// Deserialize a flattened login/status target by its shape: an `mcp`
/// member selects the MCP arm, anything else the provider/binding arm. Each
/// arm then reports its own precise errors (`missing field ...`,
/// `unknown field ...`) instead of serde's generic untagged mismatch.
/// `auth/status/get` params: selected by the presence of `mcp`, like the
/// login targets. The binding arm keeps tolerating unrelated extra fields (as
/// `BindingIdParams` always has), but a case-variant `mcp` key is refused so a
/// misspelled MCP target cannot fall back to a binding status.
impl<'de> Deserialize<'de> for AuthStatusParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let value = serde_json::Value::deserialize(deserializer)?;
        let Some(object) = value.as_object() else {
            return Err(D::Error::custom("auth/status/get params must be an object"));
        };
        if object.contains_key("mcp") {
            return serde_json::from_value(value)
                .map(Self::Mcp)
                .map_err(D::Error::custom);
        }
        if object.contains_key("connector") {
            return serde_json::from_value(value)
                .map(Self::Connector)
                .map_err(D::Error::custom);
        }
        if let Some(misspelled) = object
            .keys()
            .find(|key| key.eq_ignore_ascii_case("mcp") || key.eq_ignore_ascii_case("connector"))
        {
            return Err(D::Error::custom(format!(
                "unknown field `{misspelled}`, expected `mcp` or `connector`"
            )));
        }
        serde_json::from_value(value)
            .map(Self::Binding)
            .map_err(D::Error::custom)
    }
}

/// Select a flattened target arm by its member: `mcp`, then `connector`,
/// anything else the provider/binding arm (when the type has one).
macro_rules! target_by_member {
    ($name:ident, $mcp:ident, $connector:ident, $other:ident) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                use serde::de::Error as _;
                let value = serde_json::Value::deserialize(deserializer)?;
                let has = |member: &str| {
                    value
                        .as_object()
                        .is_some_and(|object| object.contains_key(member))
                };
                if has("mcp") {
                    serde_json::from_value(value).map(Self::$mcp)
                } else if has("connector") {
                    serde_json::from_value(value).map(Self::$connector)
                } else {
                    serde_json::from_value(value).map(Self::$other)
                }
                .map_err(D::Error::custom)
            }
        }
    };
    ($name:ident, $mcp:ident, $connector:ident) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                use serde::de::Error as _;
                let value = serde_json::Value::deserialize(deserializer)?;
                let has = |member: &str| {
                    value
                        .as_object()
                        .is_some_and(|object| object.contains_key(member))
                };
                if has("mcp") {
                    serde_json::from_value(value).map(Self::$mcp)
                } else if has("connector") {
                    serde_json::from_value(value).map(Self::$connector)
                } else {
                    Err(serde_json::Error::custom(
                        "expected an `mcp` or a `connector` target",
                    ))
                }
                .map_err(D::Error::custom)
            }
        }
    };
}

target_by_member!(WireLoginTarget, Mcp, Connector, Provider);
target_by_member!(WireLoginStartTarget, Mcp, Connector, Provider);
target_by_member!(WireLoginReadyTarget, Mcp, Connector, Provider);
target_by_member!(WireAuthStatusResult, Mcp, Connector, Binding);
target_by_member!(LoginCancelParams, Mcp, Connector);
target_by_member!(WireLoginCancelledTarget, Mcp, Connector);

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn credential_params_debug_redacts_secret_and_access_token() {
        const SECRET: &str = "sk-live-secret-value";
        let create = CreateProfileParams {
            realm_id: "realm-visible".into(),
            binding_id: "binding-visible".into(),
            profile_id: None,
            auth_method: "api_key".into(),
            secret: SECRET.into(),
        };
        let provision = ProvisionApiKeyParams {
            access_token: SECRET.into(),
            realm_id: Some("realm-visible".into()),
            binding_id: None,
            profile_id: None,
        };
        let rendered = format!("{create:?} {create:#?} {provision:?} {provision:#?}");
        assert!(!rendered.contains(SECRET), "secret leaked: {rendered}");
        assert!(rendered.contains("realm-visible"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
    }

    #[test]
    fn oauth_provider_aliases_serialize_canonically() {
        let provider: WireOAuthProvider = serde_json::from_str("\"chatgpt\"").expect("known alias");
        assert_eq!(provider, WireOAuthProvider::OpenAi);
        assert_eq!(serde_json::to_string(&provider).unwrap(), "\"openai\"");
        assert!(serde_json::from_str::<WireOAuthProvider>("\"future_oauth\"").is_err());
    }

    #[test]
    fn auth_binding_roundtrip() {
        let r = meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let w: WireAuthBindingRef = r.clone().into();
        assert_eq!(w.realm.as_str(), "dev");
        assert_eq!(w.binding.as_str(), "default_openai");
        assert!(w.profile.is_none());
        let back: meerkat_core::AuthBindingRef = w.into();
        assert_eq!(back, r);
    }

    #[test]
    fn wire_auth_binding_ref_drops_client_supplied_origin() {
        // A client cannot forge the server-owned `origin` provenance: the wire
        // projection has no `origin` field, so a forged value is dropped on
        // deserialize and `From<WireAuthBindingRef>` always reconstructs
        // `BindingOrigin::Configured` — closing the env-default laundering path the
        // mob spawn/fork-helper RPC + REST surfaces would otherwise expose.
        let json = r#"{"realm":"dev","binding":"default_openai","profile":"ci","origin":"synthetic_env_default"}"#;
        let wire: WireAuthBindingRef =
            serde_json::from_str(json).expect("unknown origin field is ignored on deserialize");
        let core: meerkat_core::AuthBindingRef = wire.into();
        assert_eq!(
            core.origin,
            meerkat_core::connection::BindingOrigin::Configured,
            "client-supplied origin must be dropped and reconstructed as Configured"
        );
    }

    #[test]
    fn auth_binding_wire_json_has_no_string_form() {
        let w = WireAuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("prod").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("openai_main").unwrap(),
            profile: Some(meerkat_core::connection::ProfileId::parse("ci").unwrap()),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains("\"realm\":\"prod\""));
        assert!(json.contains("\"binding\":\"openai_main\""));
        assert!(json.contains("\"profile\":\"ci\""));
        // No colon-joined form anywhere.
        assert!(!json.contains("prod:openai_main"));
    }

    #[test]
    fn backend_profile_projects_provider_as_string() {
        let bp = meerkat_core::BackendProfile {
            id: "openai_api".into(),
            provider: meerkat_core::Provider::OpenAI,
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        };
        let w: WireBackendProfile = (&bp).into();
        assert_eq!(w.provider, "openai");
    }

    #[test]
    fn auth_error_projects_to_tagged_variants() {
        use meerkat_core::AuthError;
        let e = AuthError::RefreshFailed("timeout".into());
        let w: WireAuthError = e.into();
        let s = serde_json::to_string(&w).unwrap();
        assert!(s.contains("\"kind\":\"refresh_failed\""));
        assert!(s.contains("\"detail\":\"timeout\""));
    }

    #[test]
    fn device_complete_result_serializes_terminal_and_ready_shapes() {
        let pending = serde_json::to_value(WireDeviceCompleteResult::Pending).unwrap();
        assert_eq!(pending, serde_json::json!({ "state": "pending" }));

        let cref = meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("prod").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("anthropic_main").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let ready = serde_json::to_value(WireDeviceCompleteResult::Ready {
            identity: Box::new(WireBindingIdentity::from(&cref)),
            profile_id: "console".to_string(),
            provider: WireOAuthProvider::Anthropic,
            expires_at: Some("2026-04-29T00:00:00Z".to_string()),
            has_refresh_token: true,
            scopes: vec!["org:create_api_key".to_string()],
        })
        .unwrap();

        assert_eq!(ready["state"], "ready");
        assert_eq!(ready["realm_id"], "prod");
        assert_eq!(ready["binding_id"], "anthropic_main");
        assert_eq!(ready["auth_binding"]["realm"], "prod");
        assert_eq!(ready["auth_binding"]["binding"], "anthropic_main");
        assert_eq!(ready["profile_id"], "console");
        assert_eq!(ready["has_refresh_token"], true);
    }

    #[test]
    fn provision_api_key_result_serializes_binding_identity() {
        let cref = meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("prod").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("anthropic_main").unwrap(),
            profile: Some(meerkat_core::connection::ProfileId::parse("console").unwrap()),
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let value = serde_json::to_value(WireProvisionApiKeyResult {
            identity: WireBindingIdentity::from(&cref),
            profile_id: "console".to_string(),
            provider: "anthropic".to_string(),
            auth_mode: "oauth_to_api_key".to_string(),
            has_api_key: true,
            scopes: vec!["user:profile".to_string()],
        })
        .unwrap();

        assert_eq!(value["realm_id"], "prod");
        assert_eq!(value["binding_id"], "anthropic_main");
        assert_eq!(value["auth_binding"]["profile"], "console");
        assert_eq!(value["auth_mode"], "oauth_to_api_key");
        assert_eq!(value["has_api_key"], true);
    }

    #[test]
    fn auth_status_serde_roundtrip() {
        let status = WireAuthStatus {
            profile_id: "p".into(),
            provider: "openai".into(),
            auth_method: "managed_chatgpt_oauth".into(),
            state: meerkat_core::AuthStatusPhase::Valid,
            expires_at: Some(chrono::Utc::now()),
            last_refresh_at: None,
            account_id: Some("acct_x".into()),
            last_error: None,
        };
        let s = serde_json::to_string(&status).unwrap();
        let back: WireAuthStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, status);
    }
}

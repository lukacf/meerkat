use std::collections::BTreeMap;

use meerkat_core::config::CustomModelConfig;
use meerkat_core::{AuthCredentialIdentity, Provider};
use meerkat_llm_core::provider_runtime::{AccountModelLookup, AccountModelRefusal};
use serde::{Deserialize, Serialize};
use url::Url;

pub use meerkat_auth_core::github_copilot::GITHUB_COPILOT_CLIENT_ID as PROVISIONAL_GITHUB_COPILOT_CLIENT_ID;
pub const COPILOT_TOKEN_API_VERSION: &str = "2025-04-01";
pub const COPILOT_INFERENCE_API_VERSION: &str = "2026-08-01";
pub const DEFAULT_COPILOT_API_BASE: &str = "https://api.githubcopilot.com";
pub const GITHUB_COPILOT_AUTHORIZER_LABEL: &str = "github-copilot";
pub const GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID: &str = "github_copilot";

const DEFAULT_INTEGRATION_ID: &str = "vscode-chat";
/// The `Copilot-Integration-Id` a `github_copilot_token` route sends when its
/// backend options set none: the integration id the Copilot API accepts for a
/// GitHub token sent as its bearer, a personal access token and an OAuth token
/// alike (GitHub Copilot CLI's). The API refuses `vscode-chat`, the device
/// sign-in's default, for such a token.
const OPERATOR_TOKEN_INTEGRATION_ID: &str = "copilot-developer-cli";
const DEFAULT_USER_AGENT: &str = meerkat_auth_core::github_copilot::GITHUB_COPILOT_USER_AGENT;
const DEFAULT_EDITOR_VERSION: &str = "vscode/1.107.0";
const DEFAULT_EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.35.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotProviderRoute {
    OpenAi,
    Anthropic,
    Gemini,
}

impl CopilotProviderRoute {
    pub const ALL: [Self; 3] = [Self::OpenAi, Self::Anthropic, Self::Gemini];

    pub const fn binding_id(self) -> &'static str {
        match self {
            Self::OpenAi => "copilot_openai",
            Self::Anthropic => "copilot_anthropic",
            Self::Gemini => "copilot_gemini",
        }
    }

    pub const fn provider(self) -> Provider {
        match self {
            Self::OpenAi => Provider::OpenAI,
            Self::Anthropic => Provider::Anthropic,
            Self::Gemini => Provider::Gemini,
        }
    }

    pub fn backend_kind(self) -> &'static str {
        match self {
            Self::OpenAi => {
                meerkat_core::provider_matrix::openai::OpenAiBackendKind::Copilot.as_str()
            }
            Self::Anthropic => {
                meerkat_core::provider_matrix::anthropic::AnthropicBackendKind::Copilot.as_str()
            }
            Self::Gemini => {
                meerkat_core::provider_matrix::google::GoogleBackendKind::Copilot.as_str()
            }
        }
    }

    pub fn auth_method(self) -> &'static str {
        match self {
            Self::OpenAi => {
                meerkat_core::provider_matrix::openai::OpenAiAuthMethod::GitHubCopilotOauth.as_str()
            }
            Self::Anthropic => {
                meerkat_core::provider_matrix::anthropic::AnthropicAuthMethod::GitHubCopilotOauth
                    .as_str()
            }
            Self::Gemini => {
                meerkat_core::provider_matrix::google::GoogleAuthMethod::GitHubCopilotOauth.as_str()
            }
        }
    }

    /// The operator-token auth method of this route (`github_copilot_token`):
    /// an operator-supplied GitHub token is the Copilot API credential, with
    /// no device sign-in.
    pub fn token_auth_method(self) -> &'static str {
        match self {
            Self::OpenAi => {
                meerkat_core::provider_matrix::openai::OpenAiAuthMethod::GitHubCopilotToken.as_str()
            }
            Self::Anthropic => {
                meerkat_core::provider_matrix::anthropic::AnthropicAuthMethod::GitHubCopilotToken
                    .as_str()
            }
            Self::Gemini => {
                meerkat_core::provider_matrix::google::GoogleAuthMethod::GitHubCopilotToken.as_str()
            }
        }
    }
}

/// The GitHub identity an operator-supplied token resolves to
/// (`github_copilot_token` routes).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CopilotAccountIdentity {
    /// The GitHub login. `None` for a GitHub App installation token, which
    /// has no user.
    pub login: Option<String>,
    /// The Copilot plan GitHub reports for the login, such as `business`.
    pub plan: Option<String>,
    /// The Copilot API host the token is sent to.
    pub api_base: String,
    /// Whether the token is a GitHub App installation token
    /// (server-to-server, billed to the organization that installed the
    /// App).
    pub installation: bool,
    /// Whether GitHub's identity lookup (`copilot_internal/user`) answered
    /// for the token. `false` for an installation token, which skips it, and
    /// for a token it does not answer (a 403 or a 404, as a fine-grained
    /// personal access token can get): such a token is used only once the
    /// account's model list on the default Copilot API host loads, and a 401
    /// or 403 from that list is the Copilot API refusing it, as for a
    /// looked-up token.
    pub looked_up: bool,
}

/// How a Copilot route authenticates, chosen by its auth method.
#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopilotCredentialMode {
    /// `github_copilot_oauth`: the GitHub token from the device sign-in is
    /// exchanged for a short-lived Copilot token.
    DeviceSignIn,
    /// `github_copilot_token`: an operator-supplied GitHub token is the
    /// Copilot API bearer itself.
    OperatorToken,
}

#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
impl CopilotCredentialMode {
    /// The mode of a validated route's auth method, or `None` for a method
    /// that is not a Copilot method.
    pub(crate) fn for_auth_method(
        method: meerkat_llm_core::provider_runtime::NormalizedAuthMethod,
    ) -> Option<Self> {
        use meerkat_core::provider_matrix::{
            AnthropicAuthMethod, GoogleAuthMethod, OpenAiAuthMethod,
        };
        use meerkat_llm_core::provider_runtime::NormalizedAuthMethod;
        match method {
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::GitHubCopilotOauth)
            | NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::GitHubCopilotOauth)
            | NormalizedAuthMethod::Google(GoogleAuthMethod::GitHubCopilotOauth) => {
                Some(Self::DeviceSignIn)
            }
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::GitHubCopilotToken)
            | NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::GitHubCopilotToken)
            | NormalizedAuthMethod::Google(GoogleAuthMethod::GitHubCopilotToken) => {
                Some(Self::OperatorToken)
            }
            _ => None,
        }
    }

    /// The persisted credential mode this mode's auth method stores.
    pub(crate) fn persisted_auth_mode(self) -> meerkat_core::auth::PersistedAuthMode {
        match self {
            Self::DeviceSignIn => meerkat_core::auth::PersistedAuthMode::GithubCopilotOauth,
            Self::OperatorToken => meerkat_core::auth::PersistedAuthMode::StaticBearer,
        }
    }

    /// The wire auth method of this mode.
    pub(crate) fn auth_method(self) -> &'static str {
        match self {
            Self::DeviceSignIn => CopilotProviderRoute::OpenAi.auth_method(),
            Self::OperatorToken => CopilotProviderRoute::OpenAi.token_auth_method(),
        }
    }
}

/// The kind of a GitHub token, read from its documented prefix only, never
/// from its length or character set (installation tokens, for one, changed
/// their length when they became stateless).
#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitHubTokenKind {
    /// `ghp_`: a classic personal access token, which Copilot does not
    /// accept.
    ClassicPersonalAccessToken,
    /// `ghs_`: a GitHub App installation token (server-to-server). It has no
    /// user, so it has no identity lookup.
    Installation,
    /// Any other token: a fine-grained personal access token (`github_pat_`),
    /// an OAuth app token (`gho_`) or a GitHub App user token (`ghu_`).
    User,
}

#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
impl GitHubTokenKind {
    pub(crate) fn classify(token: &str) -> Self {
        if token.starts_with("ghp_") {
            Self::ClassicPersonalAccessToken
        } else if token.starts_with("ghs_") {
            Self::Installation
        } else {
            Self::User
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ClassicPersonalAccessToken => "classic_personal_access_token",
            Self::Installation => "installation",
            Self::User => "user",
        }
    }
}

/// The `copilot_internal/user` endpoint, parsed like the other GitHub
/// endpoints (`https`, no user information).
#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
pub(crate) fn copilot_user_url() -> Result<Url, CopilotProtocolError> {
    parse_https(
        meerkat_auth_core::github_copilot::GITHUB_COPILOT_USER_URL,
        "copilot_user_url",
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubCopilotEndpoints {
    pub device_code_url: Url,
    pub oauth_token_url: Url,
    pub copilot_token_url: Url,
}

impl<'de> Deserialize<'de> for CopilotEndpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::parse(&value))
    }
}

impl GitHubCopilotEndpoints {
    pub fn public() -> Result<Self, CopilotProtocolError> {
        let device_code_url = parse_https(
            meerkat_auth_core::github_copilot::GITHUB_COPILOT_DEVICE_CODE_URL,
            "device_code_url",
        )?;
        let oauth_token_url = parse_https(
            meerkat_auth_core::github_copilot::GITHUB_COPILOT_TOKEN_URL,
            "oauth_token_url",
        )?;
        let copilot_token_url = parse_https(
            meerkat_auth_core::github_copilot::GITHUB_COPILOT_TOKEN_EXCHANGE_URL,
            "copilot_token_url",
        )?;
        Ok(Self {
            device_code_url,
            oauth_token_url,
            copilot_token_url,
        })
    }
}

fn parse_https(raw: &str, field: &'static str) -> Result<Url, CopilotProtocolError> {
    let url = Url::parse(raw).map_err(|error| CopilotProtocolError::InvalidEndpoint {
        field,
        reason: error.to_string(),
    })?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(CopilotProtocolError::InvalidEndpoint {
            field,
            reason: "must be an HTTPS URL without user information".to_string(),
        });
    }
    Ok(url)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CopilotBackendConfig {
    pub integration_id: String,
    pub user_agent: String,
    pub editor_version: String,
    pub editor_plugin_version: String,
    pub token_api_version: String,
    pub inference_api_version: String,
}

impl Default for CopilotBackendConfig {
    fn default() -> Self {
        Self {
            integration_id: DEFAULT_INTEGRATION_ID.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            editor_version: DEFAULT_EDITOR_VERSION.to_string(),
            editor_plugin_version: DEFAULT_EDITOR_PLUGIN_VERSION.to_string(),
            token_api_version: COPILOT_TOKEN_API_VERSION.to_string(),
            inference_api_version: COPILOT_INFERENCE_API_VERSION.to_string(),
        }
    }
}

impl CopilotBackendConfig {
    pub fn from_options(options: &serde_json::Value) -> Result<Self, CopilotProtocolError> {
        if options.is_null() {
            return Ok(Self::default());
        }
        let config: Self = serde_json::from_value(options.clone())
            .map_err(|error| CopilotProtocolError::InvalidBackendOptions(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// The configuration of a `github_copilot_token` route: as
    /// [`Self::from_options`], except that options which set no
    /// `integration_id` send `copilot-developer-cli`, the integration id the
    /// Copilot API accepts for a GitHub token sent as its bearer (it refuses
    /// `vscode-chat`, the device sign-in's default, for a personal access
    /// token). An `integration_id` the options set is kept, whatever its
    /// value. Pass this configuration to `verify_operator_token` to
    /// check a token the way the token routes use it.
    pub fn from_token_options(options: &serde_json::Value) -> Result<Self, CopilotProtocolError> {
        let mut config = Self::from_options(options)?;
        if configured_integration_id(options).is_none() {
            config.integration_id = OPERATOR_TOKEN_INTEGRATION_ID.to_string();
        }
        Ok(config)
    }

    pub fn endpoints(&self) -> Result<GitHubCopilotEndpoints, CopilotProtocolError> {
        GitHubCopilotEndpoints::public()
    }

    fn validate(&self) -> Result<(), CopilotProtocolError> {
        for (field, value) in [
            ("integration_id", self.integration_id.as_str()),
            ("user_agent", self.user_agent.as_str()),
            ("editor_version", self.editor_version.as_str()),
            ("editor_plugin_version", self.editor_plugin_version.as_str()),
            ("token_api_version", self.token_api_version.as_str()),
            ("inference_api_version", self.inference_api_version.as_str()),
        ] {
            if value.trim().is_empty() || value.contains(['\r', '\n']) {
                return Err(CopilotProtocolError::InvalidBackendOptions(format!(
                    "{field} must be a non-empty HTTP header value"
                )));
            }
        }
        Ok(())
    }
}

/// The `integration_id` a route's backend options set, if any. A resolved
/// [`CopilotBackendConfig`] always holds one, so only the options tell a value
/// the operator chose from the default of the route's auth method.
fn configured_integration_id(options: &serde_json::Value) -> Option<&str> {
    options
        .get("integration_id")
        .and_then(serde_json::Value::as_str)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CopilotEndpoint {
    #[serde(rename = "/responses")]
    Responses,
    #[serde(rename = "/chat/completions")]
    ChatCompletions,
    #[serde(rename = "/v1/messages")]
    Messages,
    #[serde(rename = "unknown")]
    Unknown,
}

impl CopilotEndpoint {
    pub fn path(self) -> &'static str {
        match self {
            Self::Responses => "/responses",
            Self::ChatCompletions => "/chat/completions",
            Self::Messages => "/v1/messages",
            Self::Unknown => "<unknown>",
        }
    }

    fn parse(raw: &str) -> Self {
        match raw {
            "/responses" | "responses" => Self::Responses,
            "/chat/completions" | "chat_completions" => Self::ChatCompletions,
            "/v1/messages" | "messages" => Self::Messages,
            _ => Self::Unknown,
        }
    }

    pub fn optimistic_for_provider(provider: Provider) -> Option<Self> {
        match provider {
            Provider::OpenAI => Some(Self::Responses),
            Provider::Anthropic => Some(Self::Messages),
            Provider::Gemini => Some(Self::ChatCompletions),
            Provider::SelfHosted | Provider::Other => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct CopilotTokenEnvelope {
    pub token: String,
    pub expires_at: u64,
    pub refresh_in: u64,
    #[serde(default)]
    pub endpoints: CopilotTokenEndpoints,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(default)]
    pub individual: Option<bool>,
}

impl std::fmt::Debug for CopilotTokenEnvelope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CopilotTokenEnvelope")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("refresh_in", &self.refresh_in)
            .field("endpoints", &self.endpoints)
            .field("sku", &self.sku)
            .field("individual", &self.individual)
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CopilotTokenEndpoints {
    #[serde(default)]
    pub api: Option<String>,
}

/// The fields read from `GET copilot_internal/user`; everything else in the
/// response is ignored.
#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct CopilotUserEnvelope {
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub chat_enabled: Option<bool>,
    #[serde(default)]
    pub copilot_plan: Option<String>,
    #[serde(default)]
    pub endpoints: Option<CopilotTokenEndpoints>,
}

/// GitHub's JSON error body: only its human-readable `message` is read.
#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct GitHubErrorEnvelope {
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct CopilotTokenErrorEnvelope {
    #[serde(default)]
    pub error_details: Option<CopilotTokenErrorDetails>,
}

#[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct CopilotTokenErrorDetails {
    #[serde(default)]
    pub notification_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CopilotModelsEnvelope {
    pub data: Vec<CopilotModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotModel {
    pub id: String,
    #[serde(default)]
    pub vendor: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub model_picker_enabled: Option<bool>,
    #[serde(default)]
    pub capabilities: CopilotModelCapabilities,
    #[serde(default)]
    pub policy: Option<CopilotModelPolicy>,
    #[serde(default)]
    pub supported_endpoints: Vec<CopilotEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotModelOffering {
    pub vendor: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub model_picker_enabled: Option<bool>,
    pub capabilities: CopilotModelCapabilities,
    pub policy: Option<CopilotModelPolicy>,
    pub supported_endpoints: Vec<CopilotEndpoint>,
}

impl CopilotModelOffering {
    pub fn endpoints(&self) -> impl Iterator<Item = CopilotEndpoint> + '_ {
        self.supported_endpoints
            .iter()
            .copied()
            .filter(|endpoint| *endpoint != CopilotEndpoint::Unknown)
    }

    pub fn route_for(&self, provider: Provider) -> Option<CopilotEndpoint> {
        if !self.enabled() {
            return None;
        }
        let supports = |candidate| self.endpoints().any(|endpoint| endpoint == candidate);
        match provider {
            Provider::OpenAI => [CopilotEndpoint::Responses, CopilotEndpoint::ChatCompletions]
                .into_iter()
                .find(|endpoint| supports(*endpoint)),
            Provider::Anthropic => {
                supports(CopilotEndpoint::Messages).then_some(CopilotEndpoint::Messages)
            }
            Provider::Gemini => supports(CopilotEndpoint::ChatCompletions)
                .then_some(CopilotEndpoint::ChatCompletions),
            Provider::SelfHosted | Provider::Other => None,
        }
    }

    /// Whether the account's policy leaves this model enabled. A model whose
    /// policy states nothing is enabled.
    pub fn enabled(&self) -> bool {
        self.policy
            .as_ref()
            .is_none_or(|policy| policy.state == CopilotModelPolicyState::Enabled)
    }

    /// The provider family whose protocol serves this model on the account,
    /// for a model the model registry does not define. It is read from the
    /// endpoints the account advertises for the model alone, never from the
    /// model's vendor, name or id: a model offered on the Messages endpoint
    /// is the Anthropic family, which speaks Messages; otherwise a model
    /// offered on the Responses or the Chat Completions endpoint is the
    /// OpenAI family, whose route takes Responses first
    /// ([`Self::route_for`]). `None` when the account's policy disables the
    /// model or it advertises no endpoint Meerkat speaks.
    pub fn protocol_provider(&self) -> Option<Provider> {
        if !self.enabled() {
            return None;
        }
        let supports = |candidate| self.endpoints().any(|endpoint| endpoint == candidate);
        if supports(CopilotEndpoint::Messages) {
            Some(Provider::Anthropic)
        } else if supports(CopilotEndpoint::Responses) || supports(CopilotEndpoint::ChatCompletions)
        {
            Some(Provider::OpenAI)
        } else {
            None
        }
    }

    /// This model's definition for the model registry, for a model the
    /// registry does not define: the account's own statements and nothing
    /// else. Its provider is [`Self::protocol_provider`]; its display name,
    /// context window, prompt limit (the registry's input ceiling), output
    /// limit and image input are what the account states. Every other
    /// capability keeps the registry's conservative default for a model it
    /// does not catalog, and the account capability gate enforces what else
    /// the account states (tool calls, streaming, structured outputs,
    /// thinking budgets) on every request. `None` when the model has no
    /// protocol provider.
    pub fn registry_definition(&self) -> Option<CustomModelConfig> {
        let limits = &self.capabilities.limits;
        Some(CustomModelConfig {
            provider: self.protocol_provider()?,
            display_name: self.name.clone(),
            context_window: limits.max_context_window_tokens,
            max_input_tokens: limits.max_prompt_tokens,
            max_output_tokens: limits.max_output_tokens,
            vision: self.capabilities.supports.vision,
            web_search: None,
            call_timeout_secs: None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotModelCapabilities {
    #[serde(default)]
    pub limits: CopilotModelLimits,
    #[serde(default)]
    pub supports: CopilotModelSupports,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotModelLimits {
    #[serde(default)]
    pub max_context_window_tokens: Option<u32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub max_prompt_tokens: Option<u32>,
    #[serde(default)]
    pub max_non_streaming_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotModelSupports {
    #[serde(default)]
    pub tool_calls: Option<bool>,
    #[serde(default)]
    pub vision: Option<bool>,
    #[serde(default)]
    pub streaming: Option<bool>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub structured_outputs: Option<bool>,
    #[serde(default)]
    pub adaptive_thinking: Option<bool>,
    #[serde(default)]
    pub max_thinking_budget: Option<u32>,
    #[serde(default)]
    pub min_thinking_budget: Option<u32>,
}

/// The state of the account's policy for a model, as `/models` states it.
/// Only `enabled` lets the account's routes serve the model; any other state
/// is kept as stated, so a refusal can name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum CopilotModelPolicyState {
    Enabled,
    Disabled,
    /// Any other state, as stated (for example `unconfigured`).
    Other(String),
}

impl CopilotModelPolicyState {
    /// The state as `/models` states it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Other(state) => state,
        }
    }
}

impl From<String> for CopilotModelPolicyState {
    fn from(state: String) -> Self {
        match state.as_str() {
            "enabled" => Self::Enabled,
            "disabled" => Self::Disabled,
            _ => Self::Other(state),
        }
    }
}

impl From<CopilotModelPolicyState> for String {
    fn from(state: CopilotModelPolicyState) -> Self {
        match state {
            CopilotModelPolicyState::Other(state) => state,
            known @ (CopilotModelPolicyState::Enabled | CopilotModelPolicyState::Disabled) => {
                known.as_str().to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotModelPolicy {
    pub state: CopilotModelPolicyState,
}

#[derive(Debug, Clone, Default)]
pub struct CopilotModelSnapshot {
    models: BTreeMap<String, CopilotModelOffering>,
}

impl CopilotModelSnapshot {
    pub fn from_models(models: Vec<CopilotModel>) -> Result<Self, CopilotProtocolError> {
        let mut indexed = BTreeMap::new();
        for model in models {
            let CopilotModel {
                id,
                vendor,
                name,
                version,
                model_picker_enabled,
                capabilities,
                policy,
                supported_endpoints,
            } = model;
            let offering = CopilotModelOffering {
                vendor,
                name,
                version,
                model_picker_enabled,
                capabilities,
                policy,
                supported_endpoints,
            };
            if indexed.insert(id.clone(), offering).is_some() {
                return Err(CopilotProtocolError::DuplicateModelId(id));
            }
        }
        Ok(Self { models: indexed })
    }

    pub fn model(&self, id: &str) -> Option<&CopilotModelOffering> {
        self.models.get(id)
    }

    pub fn models(&self) -> impl Iterator<Item = &CopilotModelOffering> {
        self.models.values()
    }

    pub fn available_model_ids(&self, provider: Provider) -> impl Iterator<Item = &str> {
        self.models.iter().filter_map(move |(id, model)| {
            model.route_for(provider).is_some().then_some(id.as_str())
        })
    }

    /// The ids of the models the account offers on a protocol Meerkat
    /// speaks ([`CopilotModelOffering::protocol_provider`]).
    pub fn offered_model_ids(&self) -> impl Iterator<Item = &str> {
        self.models
            .iter()
            .filter_map(|(id, model)| model.protocol_provider().is_some().then_some(id.as_str()))
    }

    /// What this model list states about `model`, which the model registry
    /// does not define: the model's registry definition when the account
    /// offers it on a protocol Meerkat speaks
    /// ([`CopilotModelOffering::registry_definition`]). A model the list does
    /// not name, or whose policy disables it, is refused as not offered,
    /// naming the ids the account offers; a model offered on no endpoint
    /// Meerkat speaks is refused as such. `credential` names the account in
    /// a refusal.
    pub fn account_model(
        &self,
        credential: &AuthCredentialIdentity,
        model: &str,
    ) -> AccountModelLookup {
        match self.model(model) {
            Some(offering) if offering.enabled() => match offering.registry_definition() {
                Some(definition) => AccountModelLookup::Offered(definition),
                None => AccountModelLookup::Refused(AccountModelRefusal::UnsupportedEndpoints {
                    credential: credential.clone(),
                    model: model.to_string(),
                }),
            },
            Some(_) | None => AccountModelLookup::Refused(AccountModelRefusal::NotOffered {
                credential: credential.clone(),
                model: model.to_string(),
                available: self.offered_model_ids().map(str::to_string).collect(),
            }),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CopilotProtocolError {
    #[error("invalid Copilot backend options: {0}")]
    InvalidBackendOptions(String),
    #[error("invalid Copilot endpoint '{field}': {reason}")]
    InvalidEndpoint { field: &'static str, reason: String },
    #[error("Copilot model discovery returned duplicate model id '{0}'")]
    DuplicateModelId(String),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn public_endpoints_match_observed_contract() {
        let endpoints = GitHubCopilotEndpoints::public().expect("public endpoints");
        assert_eq!(
            endpoints.device_code_url.as_str(),
            "https://github.com/login/device/code"
        );
        assert_eq!(
            endpoints.oauth_token_url.as_str(),
            "https://github.com/login/oauth/access_token"
        );
        assert_eq!(
            endpoints.copilot_token_url.as_str(),
            "https://api.github.com/copilot_internal/v2/token"
        );
        assert_eq!(DEFAULT_COPILOT_API_BASE, "https://api.githubcopilot.com");
    }

    #[test]
    fn model_route_is_metadata_driven() {
        let model = CopilotModel {
            id: "gpt-5".to_string(),
            vendor: None,
            name: None,
            version: None,
            model_picker_enabled: Some(true),
            capabilities: CopilotModelCapabilities::default(),
            policy: None,
            supported_endpoints: vec![CopilotEndpoint::ChatCompletions, CopilotEndpoint::Responses],
        };
        let snapshot = CopilotModelSnapshot::from_models(vec![model]).expect("unique model id");
        let model = snapshot.model("gpt-5").expect("discovered model");
        assert_eq!(
            model.route_for(Provider::OpenAI),
            Some(CopilotEndpoint::Responses)
        );
        assert_eq!(model.route_for(Provider::Anthropic), None);
        assert_eq!(
            model.route_for(Provider::Gemini),
            Some(CopilotEndpoint::ChatCompletions)
        );
    }

    #[test]
    fn provider_dialects_do_not_cross_family_boundaries() {
        let model = CopilotModel {
            id: "claude".to_string(),
            vendor: Some("Anthropic".to_string()),
            name: None,
            version: None,
            model_picker_enabled: Some(true),
            capabilities: CopilotModelCapabilities::default(),
            policy: None,
            supported_endpoints: vec![CopilotEndpoint::Messages, CopilotEndpoint::ChatCompletions],
        };
        let snapshot = CopilotModelSnapshot::from_models(vec![model]).expect("unique model id");
        let model = snapshot.model("claude").expect("discovered model");
        assert_eq!(
            model.route_for(Provider::Anthropic),
            Some(CopilotEndpoint::Messages)
        );
        assert_eq!(
            model.route_for(Provider::Gemini),
            Some(CopilotEndpoint::ChatCompletions)
        );
        assert_eq!(
            CopilotEndpoint::optimistic_for_provider(Provider::Anthropic),
            Some(CopilotEndpoint::Messages)
        );
        assert_eq!(
            CopilotEndpoint::optimistic_for_provider(Provider::Gemini),
            Some(CopilotEndpoint::ChatCompletions)
        );
    }

    #[test]
    fn disabled_account_policy_removes_model_from_available_routes() {
        let model = CopilotModel {
            id: "gpt-disabled".to_string(),
            vendor: None,
            name: None,
            version: None,
            model_picker_enabled: Some(true),
            capabilities: CopilotModelCapabilities::default(),
            policy: Some(CopilotModelPolicy {
                state: CopilotModelPolicyState::Disabled,
            }),
            supported_endpoints: vec![CopilotEndpoint::Responses],
        };
        let snapshot = CopilotModelSnapshot::from_models(vec![model]).expect("unique model id");
        assert_eq!(
            snapshot
                .model("gpt-disabled")
                .unwrap()
                .route_for(Provider::OpenAI),
            None
        );
        assert!(
            snapshot
                .available_model_ids(Provider::OpenAI)
                .next()
                .is_none()
        );
    }

    #[test]
    fn duplicate_discovered_model_ids_fail_closed() {
        let model = CopilotModel {
            id: "duplicate".to_string(),
            vendor: None,
            name: None,
            version: None,
            model_picker_enabled: None,
            capabilities: CopilotModelCapabilities::default(),
            policy: None,
            supported_endpoints: vec![CopilotEndpoint::Responses],
        };

        assert!(matches!(
            CopilotModelSnapshot::from_models(vec![model.clone(), model]),
            Err(CopilotProtocolError::DuplicateModelId(id)) if id == "duplicate"
        ));
    }

    #[test]
    fn backend_options_reject_header_injection_and_github_endpoint_overrides() {
        let injected = serde_json::json!({"integration_id": "copilot\r\nX-Evil: yes"});
        assert!(matches!(
            CopilotBackendConfig::from_options(&injected),
            Err(CopilotProtocolError::InvalidBackendOptions(_))
        ));
        let insecure = serde_json::json!({"github_host": "http://github.example.test"});
        assert!(matches!(
            CopilotBackendConfig::from_options(&insecure),
            Err(CopilotProtocolError::InvalidBackendOptions(_))
        ));
    }

    /// A token route defaults to `copilot-developer-cli`, the device sign-in
    /// to `vscode-chat`; an `integration_id` the options set is kept on a
    /// token route, the device sign-in's default value included, and is
    /// still validated as a header value.
    #[test]
    fn token_routes_default_to_the_cli_integration_id() {
        let token_default = |options: serde_json::Value| {
            CopilotBackendConfig::from_token_options(&options)
                .expect("valid token route options")
                .integration_id
        };
        assert_eq!(
            token_default(serde_json::Value::Null),
            "copilot-developer-cli"
        );
        assert_eq!(
            token_default(serde_json::json!({"editor_version": "vscode/1.200.0"})),
            "copilot-developer-cli"
        );
        assert_eq!(
            token_default(serde_json::json!({"integration_id": "enterprise-approved"})),
            "enterprise-approved"
        );
        assert_eq!(
            token_default(serde_json::json!({"integration_id": "vscode-chat"})),
            "vscode-chat"
        );
        assert_eq!(
            CopilotBackendConfig::from_options(&serde_json::Value::Null)
                .expect("valid device route options")
                .integration_id,
            "vscode-chat"
        );
        let other_fields = CopilotBackendConfig::from_token_options(&serde_json::json!({
            "editor_version": "vscode/1.200.0"
        }))
        .expect("valid token route options");
        assert_eq!(other_fields.editor_version, "vscode/1.200.0");
        assert!(matches!(
            CopilotBackendConfig::from_token_options(&serde_json::json!({"integration_id": " "})),
            Err(CopilotProtocolError::InvalidBackendOptions(_))
        ));
    }

    #[test]
    fn every_route_names_its_operator_token_method() {
        for route in CopilotProviderRoute::ALL {
            assert_eq!(route.token_auth_method(), "github_copilot_token");
            assert_ne!(route.token_auth_method(), route.auth_method());
        }
    }

    /// Classification reads the documented prefix only: no length or
    /// character-set rule, so a long stateless installation token is still an
    /// installation token.
    #[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
    #[test]
    fn github_token_kind_is_read_from_the_documented_prefix() {
        for (token, expected) in [
            ("ghp_classic", GitHubTokenKind::ClassicPersonalAccessToken),
            ("ghs_installation", GitHubTokenKind::Installation),
            ("github_pat_fine_grained", GitHubTokenKind::User),
            ("gho_oauth_app", GitHubTokenKind::User),
            ("ghu_app_user", GitHubTokenKind::User),
            ("unprefixed", GitHubTokenKind::User),
        ] {
            assert_eq!(GitHubTokenKind::classify(token), expected, "{token}");
        }
        let stateless = format!("ghs_{}", "a".repeat(516));
        assert_eq!(
            GitHubTokenKind::classify(&stateless),
            GitHubTokenKind::Installation
        );
    }

    #[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
    #[test]
    fn copilot_user_lookup_reads_identity_and_tolerates_extra_fields() {
        assert_eq!(
            copilot_user_url().expect("user url").as_str(),
            "https://api.github.com/copilot_internal/user"
        );
        let user: CopilotUserEnvelope = serde_json::from_value(serde_json::json!({
            "login": "svc-bot",
            "copilot_plan": "business",
            "chat_enabled": true,
            "endpoints": {"api": "https://api.business.githubcopilot.com", "proxy": "x"},
            "organization_list": []
        }))
        .expect("identity response");
        assert_eq!(user.login.as_deref(), Some("svc-bot"));
        assert_eq!(user.copilot_plan.as_deref(), Some("business"));
        assert_eq!(user.chat_enabled, Some(true));
        assert_eq!(
            user.endpoints
                .and_then(|endpoints| endpoints.api)
                .as_deref(),
            Some("https://api.business.githubcopilot.com")
        );
        let sparse: CopilotUserEnvelope =
            serde_json::from_value(serde_json::json!({"endpoints": null})).expect("sparse");
        assert_eq!(sparse, CopilotUserEnvelope::default());
    }

    #[cfg(all(feature = "oauth", not(target_arch = "wasm32")))]
    #[test]
    fn copilot_modes_follow_the_route_auth_method() {
        use meerkat_core::provider_matrix::{AnthropicAuthMethod, GoogleAuthMethod};
        use meerkat_llm_core::provider_runtime::NormalizedAuthMethod;
        assert_eq!(
            CopilotCredentialMode::for_auth_method(NormalizedAuthMethod::Anthropic(
                AnthropicAuthMethod::GitHubCopilotOauth
            )),
            Some(CopilotCredentialMode::DeviceSignIn)
        );
        assert_eq!(
            CopilotCredentialMode::for_auth_method(NormalizedAuthMethod::Google(
                GoogleAuthMethod::GitHubCopilotToken
            )),
            Some(CopilotCredentialMode::OperatorToken)
        );
        assert_eq!(
            CopilotCredentialMode::for_auth_method(NormalizedAuthMethod::Google(
                GoogleAuthMethod::ApiKey
            )),
            None
        );
        assert_eq!(
            CopilotCredentialMode::OperatorToken.persisted_auth_mode(),
            meerkat_core::auth::PersistedAuthMode::StaticBearer
        );
        assert_eq!(
            CopilotCredentialMode::OperatorToken.auth_method(),
            "github_copilot_token"
        );
    }

    #[test]
    fn provisional_client_id_has_one_oauth_authority() {
        assert_eq!(
            PROVISIONAL_GITHUB_COPILOT_CLIENT_ID,
            meerkat_auth_core::oauth_flow::oauth_provider_declaration(
                meerkat_core::OAuthProviderIdentity::GitHubCopilot
            )
            .client_id
        );
    }

    /// A model of the account's list, as Copilot's model discovery states it.
    fn discovered(
        id: &str,
        vendor: Option<&str>,
        endpoints: Vec<CopilotEndpoint>,
        policy: Option<CopilotModelPolicyState>,
    ) -> CopilotModel {
        CopilotModel {
            id: id.to_string(),
            vendor: vendor.map(str::to_string),
            name: None,
            version: None,
            model_picker_enabled: Some(true),
            capabilities: CopilotModelCapabilities::default(),
            policy: policy.map(|state| CopilotModelPolicy { state }),
            supported_endpoints: endpoints,
        }
    }

    fn github_copilot_credential() -> meerkat_core::AuthCredentialIdentity {
        meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
            realm: meerkat_core::RealmId::global(),
            account: meerkat_core::CredentialAccountId::parse("github_copilot")
                .expect("valid account id"),
        })
    }

    /// The provider family Meerkat speaks to a model the registry does not
    /// define is the protocol of the endpoints the account advertises for
    /// it: the Messages endpoint is the Anthropic family, the Responses and
    /// Chat Completions endpoints the OpenAI family. The vendor never
    /// decides it, and a model the policy disables or that advertises no
    /// endpoint Meerkat speaks has none.
    #[test]
    fn a_discovered_model_protocol_comes_from_its_endpoints_not_its_vendor() {
        use super::CopilotEndpoint::{ChatCompletions, Messages, Responses, Unknown};
        for (vendor, endpoints, expected) in [
            (
                Some("ExampleVendor"),
                vec![Responses, ChatCompletions],
                Some(Provider::OpenAI),
            ),
            (
                Some("ExampleVendor"),
                vec![ChatCompletions],
                Some(Provider::OpenAI),
            ),
            (
                Some("Anthropic"),
                vec![ChatCompletions],
                Some(Provider::OpenAI),
            ),
            (Some("OpenAI"), vec![Messages], Some(Provider::Anthropic)),
            (
                Some("Anthropic"),
                vec![ChatCompletions, Messages],
                Some(Provider::Anthropic),
            ),
            (None, vec![Responses], Some(Provider::OpenAI)),
            (Some("ExampleVendor"), vec![Unknown], None),
            (Some("ExampleVendor"), Vec::new(), None),
        ] {
            let snapshot = CopilotModelSnapshot::from_models(vec![discovered(
                "account-model-1",
                vendor,
                endpoints.clone(),
                None,
            )])
            .expect("unique model id");
            let offering = snapshot.model("account-model-1").expect("discovered model");
            assert!(offering.enabled());
            assert_eq!(
                offering.protocol_provider(),
                expected,
                "vendor {vendor:?}, endpoints {endpoints:?}"
            );
        }
        let disabled = CopilotModelSnapshot::from_models(vec![discovered(
            "account-model-1",
            None,
            vec![Responses],
            Some(CopilotModelPolicyState::Disabled),
        )])
        .expect("unique model id");
        let offering = disabled.model("account-model-1").expect("discovered model");
        assert!(!offering.enabled());
        assert_eq!(offering.protocol_provider(), None);
    }

    /// A model the registry does not define enters it with the account's own
    /// statements and nothing else: its name, context window, prompt and
    /// output limits and image input, under the provider family of its
    /// protocol. A model that states no limits claims none.
    #[test]
    fn a_discovered_model_definition_carries_the_account_limits() {
        let mut model = discovered(
            "account-model-1",
            Some("ExampleVendor"),
            vec![CopilotEndpoint::Responses, CopilotEndpoint::ChatCompletions],
            Some(CopilotModelPolicyState::Enabled),
        );
        model.name = Some("Example Model".to_string());
        model.capabilities = CopilotModelCapabilities {
            limits: CopilotModelLimits {
                max_context_window_tokens: Some(256_000),
                max_output_tokens: Some(64_000),
                max_prompt_tokens: Some(192_000),
                max_non_streaming_output_tokens: Some(16_000),
            },
            supports: CopilotModelSupports {
                tool_calls: Some(true),
                vision: Some(true),
                streaming: Some(true),
                ..Default::default()
            },
        };
        let sparse = discovered(
            "account-model-2",
            None,
            vec![CopilotEndpoint::Messages],
            None,
        );
        let snapshot =
            CopilotModelSnapshot::from_models(vec![model, sparse]).expect("unique model ids");
        assert_eq!(
            snapshot
                .model("account-model-1")
                .expect("discovered model")
                .registry_definition(),
            Some(meerkat_core::config::CustomModelConfig {
                provider: Provider::OpenAI,
                display_name: Some("Example Model".to_string()),
                context_window: Some(256_000),
                max_input_tokens: Some(192_000),
                max_output_tokens: Some(64_000),
                vision: Some(true),
                web_search: None,
                call_timeout_secs: None,
            })
        );
        assert_eq!(
            snapshot
                .model("account-model-2")
                .expect("discovered model")
                .registry_definition(),
            Some(meerkat_core::config::CustomModelConfig {
                provider: Provider::Anthropic,
                display_name: None,
                context_window: None,
                max_input_tokens: None,
                max_output_tokens: None,
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            })
        );
    }

    /// The account's model list defines a model the registry does not, and
    /// refuses one it cannot serve, typed: a model it does not list or its
    /// policy disables (naming the ids it offers), and a model it offers on
    /// no endpoint Meerkat speaks.
    #[test]
    fn an_account_model_lookup_refuses_unknown_disabled_and_unsupported_models() {
        use meerkat_llm_core::provider_runtime::{AccountModelLookup, AccountModelRefusal};
        let credential = github_copilot_credential();
        let snapshot = CopilotModelSnapshot::from_models(vec![
            discovered(
                "account-model-1",
                Some("ExampleVendor"),
                vec![CopilotEndpoint::Responses],
                None,
            ),
            discovered(
                "account-model-2",
                None,
                vec![CopilotEndpoint::Messages],
                Some(CopilotModelPolicyState::Disabled),
            ),
            discovered(
                "account-model-3",
                None,
                vec![CopilotEndpoint::Unknown],
                None,
            ),
        ])
        .expect("unique model ids");
        assert!(matches!(
            snapshot.account_model(&credential, "account-model-1"),
            AccountModelLookup::Offered(definition) if definition.provider == Provider::OpenAI
        ));
        for model in ["account-model-2", "absent-model"] {
            assert_eq!(
                snapshot.account_model(&credential, model),
                AccountModelLookup::Refused(AccountModelRefusal::NotOffered {
                    credential: credential.clone(),
                    model: model.to_string(),
                    available: vec!["account-model-1".to_string()],
                }),
                "{model}"
            );
        }
        assert_eq!(
            snapshot.account_model(&credential, "account-model-3"),
            AccountModelLookup::Refused(AccountModelRefusal::UnsupportedEndpoints {
                credential: credential.clone(),
                model: "account-model-3".to_string(),
            })
        );
    }
}

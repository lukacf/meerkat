use std::collections::BTreeMap;

use meerkat_core::Provider;
use serde::{Deserialize, Serialize};
use url::Url;

pub use meerkat_auth_core::github_copilot::GITHUB_COPILOT_CLIENT_ID as PROVISIONAL_GITHUB_COPILOT_CLIENT_ID;
pub const COPILOT_TOKEN_API_VERSION: &str = "2025-04-01";
pub const COPILOT_INFERENCE_API_VERSION: &str = "2026-08-01";
pub const DEFAULT_COPILOT_API_BASE: &str = "https://api.githubcopilot.com";
pub const GITHUB_COPILOT_AUTHORIZER_LABEL: &str = "github-copilot";
pub const GITHUB_COPILOT_CREDENTIAL_ACCOUNT_ID: &str = "github_copilot";

const DEFAULT_INTEGRATION_ID: &str = "vscode-chat";
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
        if self
            .policy
            .as_ref()
            .is_some_and(|policy| policy.state != CopilotModelPolicyState::Enabled)
        {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopilotModelPolicyState {
    Enabled,
    Disabled,
    #[serde(other)]
    Unknown,
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
}

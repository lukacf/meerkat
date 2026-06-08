//! Anthropic backend kinds (typed, provider-owned).

use super::anthropic_auth::AnthropicAuthMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnthropicBackendKind {
    /// Native `api.anthropic.com` API (Claude.ai / Console).
    AnthropicApi,
    /// AWS Bedrock InvokeModelWithResponseStream (event-stream, SigV4).
    Bedrock,
    /// Google Vertex AI (Anthropic-on-Vertex), Bearer auth via GoogleAuth.
    Vertex,
    /// Azure Foundry (Anthropic-on-Foundry), Bearer auth via Azure AD.
    Foundry,
}

impl AnthropicBackendKind {
    pub const ALL: &'static [Self] = &[
        Self::AnthropicApi,
        Self::Bedrock,
        Self::Vertex,
        Self::Foundry,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "anthropic_api" => Some(Self::AnthropicApi),
            "bedrock" => Some(Self::Bedrock),
            "vertex" => Some(Self::Vertex),
            "foundry" => Some(Self::Foundry),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicApi => "anthropic_api",
            Self::Bedrock => "bedrock",
            Self::Vertex => "vertex",
            Self::Foundry => "foundry",
        }
    }

    /// The Anthropic auth methods this backend supports — the provider-owned
    /// (backend, auth) compatibility policy that the provider-runtime
    /// `supports()` seam delegates to (dogma rows #122/#178).
    pub fn supported_auth_methods(self) -> &'static [AnthropicAuthMethod] {
        match self {
            Self::AnthropicApi => &[
                AnthropicAuthMethod::ApiKey,
                AnthropicAuthMethod::StaticBearer,
                AnthropicAuthMethod::ClaudeAiOauth,
                AnthropicAuthMethod::OauthToApiKey,
                AnthropicAuthMethod::ExternalAuthorizer,
            ],
            Self::Bedrock => &[
                AnthropicAuthMethod::BedrockBearer,
                AnthropicAuthMethod::BedrockAwsSigv4,
                AnthropicAuthMethod::ExternalAuthorizer,
            ],
            Self::Vertex => &[
                AnthropicAuthMethod::VertexGoogleAuth,
                AnthropicAuthMethod::ExternalAuthorizer,
            ],
            Self::Foundry => &[
                AnthropicAuthMethod::FoundryApiKey,
                AnthropicAuthMethod::FoundryAzureAd,
                AnthropicAuthMethod::ExternalAuthorizer,
            ],
        }
    }
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::AnthropicApi => "https://api.anthropic.com",
            // Bedrock uses <region>.amazonaws.com base (region-specific; the
            // binding's BackendProfile.base_url carries the full URL).
            Self::Bedrock => "",
            // Vertex uses <region>-aiplatform.googleapis.com.
            Self::Vertex => "",
            // Foundry uses a per-deployment base URL from binding options.
            Self::Foundry => "",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        for v in AnthropicBackendKind::ALL {
            let v = *v;
            assert_eq!(AnthropicBackendKind::parse(v.as_str()), Some(v));
        }
        assert_eq!(AnthropicBackendKind::parse("other"), None);
    }
}

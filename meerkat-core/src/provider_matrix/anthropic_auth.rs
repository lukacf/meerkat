//! Anthropic auth methods (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnthropicAuthMethod {
    ApiKey,
    StaticBearer,
    ClaudeAiOauth,
    OauthToApiKey,
    ExternalAuthorizer,
    /// Bedrock static bearer token (`AWS_BEARER_TOKEN_BEDROCK`).
    BedrockBearer,
    /// Bedrock SigV4 signing with AWS credentials (env / static / assume-role).
    BedrockAwsSigv4,
    /// Vertex Bearer token via Google Auth (ADC / metadata).
    VertexGoogleAuth,
    /// Foundry static API key.
    FoundryApiKey,
    /// Foundry Bearer token via Azure AD (DefaultAzureCredential equivalent).
    FoundryAzureAd,
}

impl AnthropicAuthMethod {
    pub const ALL: &'static [Self] = &[
        Self::ApiKey,
        Self::StaticBearer,
        Self::ClaudeAiOauth,
        Self::OauthToApiKey,
        Self::ExternalAuthorizer,
        Self::BedrockBearer,
        Self::BedrockAwsSigv4,
        Self::VertexGoogleAuth,
        Self::FoundryApiKey,
        Self::FoundryAzureAd,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "api_key" => Some(Self::ApiKey),
            "static_bearer" => Some(Self::StaticBearer),
            "claude_ai_oauth" => Some(Self::ClaudeAiOauth),
            "oauth_to_api_key" => Some(Self::OauthToApiKey),
            "external_authorizer" => Some(Self::ExternalAuthorizer),
            "bedrock_bearer" => Some(Self::BedrockBearer),
            "bedrock_aws_sigv4" => Some(Self::BedrockAwsSigv4),
            "vertex_google_auth" => Some(Self::VertexGoogleAuth),
            "foundry_api_key" => Some(Self::FoundryApiKey),
            "foundry_azure_ad" => Some(Self::FoundryAzureAd),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::StaticBearer => "static_bearer",
            Self::ClaudeAiOauth => "claude_ai_oauth",
            Self::OauthToApiKey => "oauth_to_api_key",
            Self::ExternalAuthorizer => "external_authorizer",
            Self::BedrockBearer => "bedrock_bearer",
            Self::BedrockAwsSigv4 => "bedrock_aws_sigv4",
            Self::VertexGoogleAuth => "vertex_google_auth",
            Self::FoundryApiKey => "foundry_api_key",
            Self::FoundryAzureAd => "foundry_azure_ad",
        }
    }

    /// The persisted credential mode this auth method stores in the
    /// `TokenStore`, or `None` for authorizer/SigV4-backed methods that hold
    /// no persisted secret. Typed owner of the auth-method -> persisted-mode
    /// mapping (replaces the string-keyed `persisted_auth_mode_for_auth_method`
    /// decision table).
    pub fn persisted_auth_mode(self) -> Option<crate::auth::token_store::PersistedAuthMode> {
        use crate::auth::token_store::PersistedAuthMode;
        match self {
            Self::ApiKey | Self::FoundryApiKey => Some(PersistedAuthMode::ApiKey),
            Self::StaticBearer | Self::BedrockBearer => Some(PersistedAuthMode::StaticBearer),
            Self::ClaudeAiOauth => Some(PersistedAuthMode::ClaudeAiOauth),
            Self::OauthToApiKey => Some(PersistedAuthMode::OauthToApiKey),
            Self::ExternalAuthorizer
            | Self::BedrockAwsSigv4
            | Self::VertexGoogleAuth
            | Self::FoundryAzureAd => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in AnthropicAuthMethod::ALL {
            let v = *v;
            assert_eq!(AnthropicAuthMethod::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(AnthropicAuthMethod::parse("unknown"), None);
    }
}

//! Anthropic backend kinds (typed, provider-owned).

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
        for v in [
            AnthropicBackendKind::AnthropicApi,
            AnthropicBackendKind::Bedrock,
            AnthropicBackendKind::Vertex,
            AnthropicBackendKind::Foundry,
        ] {
            assert_eq!(AnthropicBackendKind::parse(v.as_str()), Some(v));
        }
        assert_eq!(AnthropicBackendKind::parse("other"), None);
    }
}

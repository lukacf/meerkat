//! OpenAI auth methods (typed, provider-owned).

/// Wire header for the ChatGPT account id lifted from OAuth metadata.
///
/// Kept in this unconditional module (rather than `oauth.rs`) so it remains
/// available on default/wasm builds where the interactive OAuth flow is
/// feature-gated off but the ChatGPT backend wire headers are still emitted
/// whenever metadata is present.
pub const CHATGPT_ACCOUNT_HEADER: &str = "ChatGPT-Account-ID";

/// Wire header for ChatGPT accounts flagged `chatgpt_account_is_fedramp`.
pub const FEDRAMP_HEADER: &str = "X-OpenAI-Fedramp";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenAiAuthMethod {
    ApiKey,
    AzureApiKey,
    StaticBearer,
    ManagedChatGptOauth,
    ExternalChatGptTokens,
    ExternalAuthorizer,
}

impl OpenAiAuthMethod {
    pub const ALL: &'static [Self] = &[
        Self::ApiKey,
        Self::AzureApiKey,
        Self::StaticBearer,
        Self::ManagedChatGptOauth,
        Self::ExternalChatGptTokens,
        Self::ExternalAuthorizer,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "api_key" => Some(Self::ApiKey),
            "azure_api_key" => Some(Self::AzureApiKey),
            "static_bearer" => Some(Self::StaticBearer),
            "managed_chatgpt_oauth" => Some(Self::ManagedChatGptOauth),
            "external_chatgpt_tokens" => Some(Self::ExternalChatGptTokens),
            "external_authorizer" => Some(Self::ExternalAuthorizer),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::AzureApiKey => "azure_api_key",
            Self::StaticBearer => "static_bearer",
            Self::ManagedChatGptOauth => "managed_chatgpt_oauth",
            Self::ExternalChatGptTokens => "external_chatgpt_tokens",
            Self::ExternalAuthorizer => "external_authorizer",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip_all_variants() {
        for v in OpenAiAuthMethod::ALL {
            let v = *v;
            assert_eq!(OpenAiAuthMethod::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(OpenAiAuthMethod::parse("unknown"), None);
    }
}

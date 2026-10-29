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
    GitHubCopilotOauth,
    /// An operator-supplied GitHub token sent as the Copilot API credential
    /// (no device sign-in). Stored, when stored, as a static bearer secret.
    GitHubCopilotToken,
}

impl OpenAiAuthMethod {
    pub const ALL: &'static [Self] = &[
        Self::ApiKey,
        Self::AzureApiKey,
        Self::StaticBearer,
        Self::ManagedChatGptOauth,
        Self::ExternalChatGptTokens,
        Self::ExternalAuthorizer,
        Self::GitHubCopilotOauth,
        Self::GitHubCopilotToken,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "api_key" => Some(Self::ApiKey),
            "azure_api_key" => Some(Self::AzureApiKey),
            "static_bearer" => Some(Self::StaticBearer),
            "managed_chatgpt_oauth" => Some(Self::ManagedChatGptOauth),
            "external_chatgpt_tokens" => Some(Self::ExternalChatGptTokens),
            "external_authorizer" => Some(Self::ExternalAuthorizer),
            "github_copilot_oauth" => Some(Self::GitHubCopilotOauth),
            "github_copilot_token" => Some(Self::GitHubCopilotToken),
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
            Self::GitHubCopilotOauth => "github_copilot_oauth",
            Self::GitHubCopilotToken => "github_copilot_token",
        }
    }

    /// The persisted credential mode this auth method stores in the
    /// `TokenStore`, or `None` for authorizer-backed methods that hold no
    /// persisted secret. Typed owner of the
    /// auth-method -> persisted-mode mapping.
    pub fn persisted_auth_mode(self) -> Option<crate::auth::token_store::PersistedAuthMode> {
        use crate::auth::token_store::PersistedAuthMode;
        match self {
            Self::ApiKey | Self::AzureApiKey => Some(PersistedAuthMode::ApiKey),
            Self::StaticBearer => Some(PersistedAuthMode::StaticBearer),
            Self::ManagedChatGptOauth => Some(PersistedAuthMode::ChatgptOauth),
            Self::ExternalChatGptTokens => Some(PersistedAuthMode::ExternalTokens),
            Self::ExternalAuthorizer => None,
            Self::GitHubCopilotOauth => Some(PersistedAuthMode::GithubCopilotOauth),
            Self::GitHubCopilotToken => Some(PersistedAuthMode::StaticBearer),
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

    /// The operator-token method is a wire member of the matrix and stores an
    /// operator-supplied secret, never an OAuth-login token.
    #[test]
    fn github_copilot_token_is_a_static_bearer_wire_method() {
        let method = OpenAiAuthMethod::parse("github_copilot_token");
        assert_eq!(
            method.map(OpenAiAuthMethod::as_str),
            Some("github_copilot_token")
        );
        assert_eq!(
            method.and_then(OpenAiAuthMethod::persisted_auth_mode),
            Some(crate::auth::token_store::PersistedAuthMode::StaticBearer)
        );
    }

    #[test]
    fn persisted_auth_mode_mapping_is_typed_owner_truth() {
        use crate::auth::token_store::PersistedAuthMode;
        let cases = [
            (OpenAiAuthMethod::ApiKey, Some(PersistedAuthMode::ApiKey)),
            (
                OpenAiAuthMethod::AzureApiKey,
                Some(PersistedAuthMode::ApiKey),
            ),
            (
                OpenAiAuthMethod::StaticBearer,
                Some(PersistedAuthMode::StaticBearer),
            ),
            (
                OpenAiAuthMethod::ManagedChatGptOauth,
                Some(PersistedAuthMode::ChatgptOauth),
            ),
            (
                OpenAiAuthMethod::ExternalChatGptTokens,
                Some(PersistedAuthMode::ExternalTokens),
            ),
            (OpenAiAuthMethod::ExternalAuthorizer, None),
            (
                OpenAiAuthMethod::GitHubCopilotOauth,
                Some(PersistedAuthMode::GithubCopilotOauth),
            ),
            (
                OpenAiAuthMethod::GitHubCopilotToken,
                Some(PersistedAuthMode::StaticBearer),
            ),
        ];
        for (method, expected) in cases {
            assert_eq!(
                method.persisted_auth_mode(),
                expected,
                "persisted mode for {method:?}"
            );
        }
    }
}

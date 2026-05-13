//! Typed OAuth provider identities shared by auth surfaces and lifecycle state.

use serde::{Deserialize, Serialize};

use super::PersistedAuthMode;
use crate::provider::Provider;

/// Concrete OAuth login/provisioning authority.
///
/// This intentionally distinguishes product login flows from the broader
/// provider family. For example, OpenAI API-key auth and ChatGPT OAuth are
/// both `Provider::OpenAI`, but only `OpenAiChatGpt` owns the ChatGPT OAuth
/// endpoints, token mode, and target backend contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OAuthProviderIdentity {
    #[serde(alias = "anthropic", alias = "claude", alias = "claude.ai")]
    AnthropicClaudeAi,
    AnthropicConsoleApiKey,
    #[serde(alias = "openai", alias = "chatgpt")]
    OpenAiChatGpt,
    #[serde(alias = "google", alias = "gemini", alias = "code_assist")]
    GoogleCodeAssist,
}

impl OAuthProviderIdentity {
    pub const ALL: &'static [Self] = &[
        Self::AnthropicClaudeAi,
        Self::AnthropicConsoleApiKey,
        Self::OpenAiChatGpt,
        Self::GoogleCodeAssist,
    ];

    /// Stable wire/persistence projection for this typed identity.
    pub const fn canonical_alias(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi => "anthropic_claude_ai",
            Self::AnthropicConsoleApiKey => "anthropic_console_api_key",
            Self::OpenAiChatGpt => "open_ai_chat_gpt",
            Self::GoogleCodeAssist => "google_code_assist",
        }
    }

    /// Legacy input aliases accepted at request-deserialization boundaries.
    ///
    /// These are compatibility spellings, not canonical persisted/wire output.
    pub const fn compatibility_aliases(self) -> &'static [&'static str] {
        match self {
            Self::AnthropicClaudeAi => &["anthropic", "claude", "claude.ai"],
            Self::AnthropicConsoleApiKey => &[],
            Self::OpenAiChatGpt => &["openai", "chatgpt"],
            Self::GoogleCodeAssist => &["google", "gemini", "code_assist"],
        }
    }

    /// Compatibility path segment used only by local OAuth endpoint fixtures.
    pub const fn fixture_path_segment(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi => "anthropic",
            Self::AnthropicConsoleApiKey => "anthropic_console_api_key",
            Self::OpenAiChatGpt => "openai",
            Self::GoogleCodeAssist => "google",
        }
    }

    pub const fn provider(self) -> Provider {
        match self {
            Self::AnthropicClaudeAi | Self::AnthropicConsoleApiKey => Provider::Anthropic,
            Self::OpenAiChatGpt => Provider::OpenAI,
            Self::GoogleCodeAssist => Provider::Gemini,
        }
    }

    pub const fn auth_mode(self) -> PersistedAuthMode {
        match self {
            Self::AnthropicClaudeAi => PersistedAuthMode::ClaudeAiOauth,
            Self::AnthropicConsoleApiKey => PersistedAuthMode::OauthToApiKey,
            Self::OpenAiChatGpt => PersistedAuthMode::ChatgptOauth,
            Self::GoogleCodeAssist => PersistedAuthMode::GoogleOauth,
        }
    }

    pub const fn backend_kind(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi | Self::AnthropicConsoleApiKey => "anthropic_api",
            Self::OpenAiChatGpt => "chatgpt_backend",
            Self::GoogleCodeAssist => "google_code_assist",
        }
    }

    pub const fn auth_method(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi => "claude_ai_oauth",
            Self::AnthropicConsoleApiKey => "oauth_to_api_key",
            Self::OpenAiChatGpt => "managed_chatgpt_oauth",
            Self::GoogleCodeAssist => "google_oauth",
        }
    }
}

impl std::fmt::Display for OAuthProviderIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.canonical_alias())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn oauth_provider_identity_serializes_canonical_identity_not_legacy_provider_alias() {
        let encoded = serde_json::to_string(&OAuthProviderIdentity::OpenAiChatGpt).unwrap();
        assert_eq!(encoded, "\"open_ai_chat_gpt\"");
    }

    #[test]
    fn oauth_provider_identity_accepts_legacy_alias_only_at_deserialization_boundary() {
        let decoded: OAuthProviderIdentity = serde_json::from_str("\"chatgpt\"").unwrap();
        assert_eq!(decoded, OAuthProviderIdentity::OpenAiChatGpt);
    }
}

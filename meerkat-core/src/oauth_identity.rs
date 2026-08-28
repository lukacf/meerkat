//! Typed OAuth provider identity vocabulary.
//!
//! `OAuthProviderIdentity` is the canonical, provider-owned typed identity for
//! the four interactive OAuth login providers. It lives here in `meerkat-core`
//! — below `meerkat-contracts` in the dependency graph — so wire types can
//! reference it directly instead of ferrying a bare `String`. Precedent: the
//! provider-owned typed enums relocated into `meerkat_core::provider_matrix`.
//!
//! Only the *pure* projections that depend on nothing above `meerkat-core`
//! live here as inherent methods (`from_alias`, `canonical_alias`, `provider`,
//! `auth_mode`, `client_secret`). The higher-type projections that reach into
//! crates above core — the canonical [`crate::provider_matrix`]-tagged backend
//! kind, the per-provider OAuth declaration, and the concrete login endpoints —
//! live as free functions in the crates that can see both this identity (via
//! re-export) and their return types (`meerkat-llm-core`, `meerkat-auth-core`).

use serde::{Deserialize, Serialize};

use crate::Provider;
use crate::auth::token_store::PersistedAuthMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OAuthProviderIdentity {
    #[serde(rename = "anthropic_claude_ai")]
    AnthropicClaudeAi,
    #[serde(rename = "anthropic_console_api_key")]
    AnthropicConsoleApiKey,
    #[serde(rename = "openai_chatgpt")]
    OpenAiChatGpt,
    #[serde(rename = "google_code_assist")]
    GoogleCodeAssist,
    #[serde(rename = "github_copilot")]
    GitHubCopilot,
}

impl OAuthProviderIdentity {
    pub const INTERACTIVE: &'static [Self] = &[
        Self::AnthropicClaudeAi,
        Self::OpenAiChatGpt,
        Self::GoogleCodeAssist,
        Self::GitHubCopilot,
    ];

    pub fn from_alias(alias: &str) -> Option<Self> {
        match alias {
            "anthropic" | "claude" | "claude.ai" => Some(Self::AnthropicClaudeAi),
            "anthropic_console_api_key" => Some(Self::AnthropicConsoleApiKey),
            "openai" | "chatgpt" => Some(Self::OpenAiChatGpt),
            "google" | "gemini" | "code_assist" => Some(Self::GoogleCodeAssist),
            "copilot" | "github_copilot" => Some(Self::GitHubCopilot),
            _ => None,
        }
    }

    pub fn canonical_alias(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi => "anthropic",
            Self::AnthropicConsoleApiKey => "anthropic_console_api_key",
            Self::OpenAiChatGpt => "openai",
            Self::GoogleCodeAssist => "google",
            Self::GitHubCopilot => "copilot",
        }
    }

    pub fn provider(self) -> Option<Provider> {
        match self {
            Self::AnthropicClaudeAi | Self::AnthropicConsoleApiKey => Some(Provider::Anthropic),
            Self::OpenAiChatGpt => Some(Provider::OpenAI),
            Self::GoogleCodeAssist => Some(Provider::Gemini),
            Self::GitHubCopilot => None,
        }
    }

    pub const fn supports_browser_flow(self) -> bool {
        !matches!(self, Self::GitHubCopilot)
    }

    pub fn auth_mode(self) -> PersistedAuthMode {
        match self {
            Self::AnthropicClaudeAi => PersistedAuthMode::ClaudeAiOauth,
            Self::AnthropicConsoleApiKey => PersistedAuthMode::OauthToApiKey,
            Self::OpenAiChatGpt => PersistedAuthMode::ChatgptOauth,
            Self::GoogleCodeAssist => PersistedAuthMode::GoogleOauth,
            Self::GitHubCopilot => PersistedAuthMode::GithubCopilotOauth,
        }
    }
}

impl std::fmt::Display for OAuthProviderIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.canonical_alias())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::OAuthProviderIdentity;

    #[test]
    fn alias_canonical_round_trips_for_every_variant() {
        for identity in [
            OAuthProviderIdentity::AnthropicClaudeAi,
            OAuthProviderIdentity::AnthropicConsoleApiKey,
            OAuthProviderIdentity::OpenAiChatGpt,
            OAuthProviderIdentity::GoogleCodeAssist,
            OAuthProviderIdentity::GitHubCopilot,
        ] {
            let alias = identity.canonical_alias();
            assert_eq!(
                OAuthProviderIdentity::from_alias(alias),
                Some(identity),
                "canonical alias {alias} must parse back to its identity"
            );
        }
    }

    #[test]
    fn serde_rename_wire_tags_are_pinned() {
        // The wire tags are part of the persisted/contract format; pin them so a
        // careless variant rename cannot silently break compatibility.
        let cases = [
            (
                OAuthProviderIdentity::AnthropicClaudeAi,
                "anthropic_claude_ai",
            ),
            (
                OAuthProviderIdentity::AnthropicConsoleApiKey,
                "anthropic_console_api_key",
            ),
            (OAuthProviderIdentity::OpenAiChatGpt, "openai_chatgpt"),
            (
                OAuthProviderIdentity::GoogleCodeAssist,
                "google_code_assist",
            ),
            (OAuthProviderIdentity::GitHubCopilot, "github_copilot"),
        ];
        for (identity, tag) in cases {
            let json = serde_json::to_string(&identity).expect("identity serializes");
            assert_eq!(json, format!("\"{tag}\""));
            let parsed: OAuthProviderIdentity =
                serde_json::from_str(&json).expect("identity round-trips");
            assert_eq!(parsed, identity);
        }
    }

    #[test]
    fn copilot_is_shared_and_device_only() {
        assert_eq!(OAuthProviderIdentity::GitHubCopilot.provider(), None);
        assert!(!OAuthProviderIdentity::GitHubCopilot.supports_browser_flow());
    }
}

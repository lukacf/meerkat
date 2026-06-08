//! OpenAI backend kinds (typed, provider-owned).

use super::openai_auth::OpenAiAuthMethod;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenAiBackendKind {
    OpenAiApi,
    ChatGptBackend,
    AzureOpenAi,
}

pub const CHATGPT_CODEX_DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Legacy ChatGPT backend base URLs that predate the canonical
/// [`CHATGPT_CODEX_DEFAULT_BASE_URL`] and must be healed to it. Single owner
/// consulted by both the CLI config-seed/heal path and the provider-internal
/// `chatgpt_backend_base_url` normalizer.
const LEGACY_CHATGPT_BASE_URLS: &[&str] = &["https://chatgpt.com/backend-api"];

impl OpenAiBackendKind {
    pub const ALL: &'static [Self] = &[Self::OpenAiApi, Self::ChatGptBackend, Self::AzureOpenAi];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "openai_api" => Some(Self::OpenAiApi),
            "chatgpt_backend" => Some(Self::ChatGptBackend),
            "azure_openai" => Some(Self::AzureOpenAi),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiApi => "openai_api",
            Self::ChatGptBackend => "chatgpt_backend",
            Self::AzureOpenAi => "azure_openai",
        }
    }

    /// The OpenAI auth methods this backend supports — the provider-owned
    /// (backend, auth) compatibility policy. Adding or removing a supported
    /// pairing is a single edit here; the cross-provider `supports()` seam in
    /// the provider-runtime catalog delegates to this declaration instead of
    /// hand-maintaining a global match table (dogma rows #122/#178).
    pub fn supported_auth_methods(self) -> &'static [OpenAiAuthMethod] {
        match self {
            Self::OpenAiApi => &[
                OpenAiAuthMethod::ApiKey,
                OpenAiAuthMethod::StaticBearer,
                OpenAiAuthMethod::ExternalAuthorizer,
            ],
            Self::ChatGptBackend => &[
                OpenAiAuthMethod::ManagedChatGptOauth,
                OpenAiAuthMethod::ExternalChatGptTokens,
            ],
            Self::AzureOpenAi => &[OpenAiAuthMethod::AzureApiKey],
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAiApi => "https://api.openai.com",
            Self::ChatGptBackend => CHATGPT_CODEX_DEFAULT_BASE_URL,
            Self::AzureOpenAi => "",
        }
    }

    /// Whether `url` is a legacy ChatGPT backend base URL that should be healed
    /// to [`CHATGPT_CODEX_DEFAULT_BASE_URL`]. Trailing slashes are ignored.
    /// Single owner of the legacy-base-url set (replaces the duplicated magic
    /// literal compared in the CLI and the OpenAI runtime).
    pub fn is_legacy_chatgpt_base_url(url: &str) -> bool {
        let trimmed = url.trim_end_matches('/');
        LEGACY_CHATGPT_BASE_URLS.contains(&trimmed)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        for v in OpenAiBackendKind::ALL {
            let v = *v;
            assert_eq!(OpenAiBackendKind::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert_eq!(OpenAiBackendKind::parse("unknown"), None);
        assert_eq!(OpenAiBackendKind::parse(""), None);
    }

    #[test]
    fn default_base_url_stable() {
        assert_eq!(
            OpenAiBackendKind::OpenAiApi.default_base_url(),
            "https://api.openai.com",
        );
        assert_eq!(
            OpenAiBackendKind::ChatGptBackend.default_base_url(),
            "https://chatgpt.com/backend-api/codex",
        );
        assert_eq!(OpenAiBackendKind::AzureOpenAi.default_base_url(), "");
    }
}

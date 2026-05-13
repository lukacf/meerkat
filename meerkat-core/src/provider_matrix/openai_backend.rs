//! OpenAI backend kinds (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenAiBackendKind {
    OpenAiApi,
    ChatGptBackend,
    AzureOpenAi,
}

pub const CHATGPT_CODEX_DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

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

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAiApi => "https://api.openai.com",
            Self::ChatGptBackend => CHATGPT_CODEX_DEFAULT_BASE_URL,
            Self::AzureOpenAi => "",
        }
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

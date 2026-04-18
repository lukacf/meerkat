//! OpenAI backend kinds (typed, provider-owned).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenAiBackendKind {
    OpenAiApi,
    ChatGptBackend,
}

impl OpenAiBackendKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "openai_api" => Some(Self::OpenAiApi),
            "chatgpt_backend" => Some(Self::ChatGptBackend),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiApi => "openai_api",
            Self::ChatGptBackend => "chatgpt_backend",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::OpenAiApi => "https://api.openai.com",
            Self::ChatGptBackend => "https://chatgpt.com/backend-api",
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
            OpenAiBackendKind::OpenAiApi,
            OpenAiBackendKind::ChatGptBackend,
        ] {
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
            "https://chatgpt.com/backend-api",
        );
    }
}

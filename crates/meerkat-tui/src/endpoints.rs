//! LLM endpoints: the wire [`Dialect`]s and named [`Endpoint`]s.
//!
//! **Minimal shim (P0).** Only the types the ported list-models client
//! ([`crate::models`]), the config overlay ([`crate::overlay`]) and the UI
//! need are here, copied from the old crate. Endpoint discovery, the
//! `endpoint:model` reference parser, startup planning and the per-session
//! choice sidecars are rewritten in P4 on upstream's native model catalog,
//! `auth_binding` and `self_hosted` servers.

use std::fmt;

use meerkat_core::Provider;

use crate::bootstrap::credentials::ProviderCredentials;

/// Names of the built-in endpoints, in display order.
pub const BUILTIN_NAMES: [&str; 3] = ["anthropic", "openai", "gemini"];

/// The wire protocol an endpoint speaks (one of Meerkat's LLM clients).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// Anthropic Messages API.
    Anthropic,
    /// OpenAI Responses API (`/v1/responses`); also LiteLLM and other proxies.
    OpenAi,
    /// Gemini `generateContent`.
    Gemini,
}

impl Dialect {
    /// Every dialect, in display order.
    pub const ALL: [Self; 3] = [Self::Anthropic, Self::OpenAi, Self::Gemini];

    /// Canonical name (`anthropic`, `openai`, `gemini`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
        }
    }

    /// Parse a canonical name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }

    /// The dialect of a concrete provider.
    #[must_use]
    pub fn from_provider(provider: Provider) -> Option<Self> {
        match provider {
            Provider::Anthropic => Some(Self::Anthropic),
            Provider::OpenAI => Some(Self::OpenAi),
            Provider::Gemini => Some(Self::Gemini),
            Provider::SelfHosted | Provider::Other => None,
        }
    }

    /// Infer the dialect from a model id prefix: `claude-*` → Anthropic,
    /// `gpt-*`, `chatgpt-*`, `o<digit>*` → OpenAI, `gemini-*` → Gemini.
    #[must_use]
    pub fn infer_from_model(model: &str) -> Option<Self> {
        let lower = model.to_ascii_lowercase();
        if lower.starts_with("claude-") {
            Some(Self::Anthropic)
        } else if lower.starts_with("gpt-")
            || lower.starts_with("chatgpt-")
            || (lower.starts_with('o') && lower[1..].starts_with(|c: char| c.is_ascii_digit()))
        {
            Some(Self::OpenAi)
        } else if lower.starts_with("gemini-") {
            Some(Self::Gemini)
        } else {
            None
        }
    }
}

impl From<Dialect> for Provider {
    fn from(dialect: Dialect) -> Self {
        match dialect {
            Dialect::Anthropic => Self::Anthropic,
            Dialect::OpenAi => Self::OpenAI,
            Dialect::Gemini => Self::Gemini,
        }
    }
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A secret that never shows up in `Debug` output and has no `Display`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a secret.
    #[must_use]
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// The secret itself; only for request headers and client construction.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// One place to send LLM requests.
#[derive(Debug, Clone)]
pub struct Endpoint {
    /// Unique name (`anthropic`, `litellm`, …); the `endpoint` of `endpoint:model`.
    pub name: String,
    /// Protocol the endpoint speaks.
    pub dialect: Dialect,
    /// Base URL override; `None` uses the dialect's public API.
    pub base_url: Option<String>,
    /// API key.
    pub api_key: SecretString,
    /// Whether this is one of the built-in provider endpoints.
    pub builtin: bool,
    /// Tie-break when several endpoints list the same bare model id
    /// (higher wins; built-ins are 0).
    pub priority: i64,
}

impl Endpoint {
    /// Credentials for the list-models request.
    #[must_use]
    pub fn credentials(&self) -> ProviderCredentials {
        ProviderCredentials {
            provider: self.dialect.into(),
            api_key: self.api_key.expose().to_string(),
            base_url: self.base_url.clone(),
        }
    }

    /// `anthropic dialect` or `openai dialect, http://localhost:4000`.
    #[must_use]
    pub fn describe(&self) -> String {
        describe_endpoint(self.dialect, self.base_url.as_deref())
    }
}

/// `<dialect> dialect[, <base url>]`.
#[must_use]
pub fn describe_endpoint(dialect: Dialect, base_url: Option<&str>) -> String {
    match base_url {
        Some(url) => format!("{dialect} dialect, {url}"),
        None => format!("{dialect} dialect"),
    }
}

/// Whether `name` is usable as an endpoint name: ASCII letters, digits, `_`, `-`.
#[must_use]
pub fn is_valid_endpoint_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(name: &str, dialect: Dialect, builtin: bool) -> Endpoint {
        Endpoint {
            name: name.to_string(),
            dialect,
            base_url: None,
            api_key: SecretString::new("k"),
            builtin,
            priority: 0,
        }
    }

    #[test]
    fn dialect_inference() {
        let cases = [
            ("claude-opus-4-6", Some(Dialect::Anthropic)),
            ("gpt-5.4", Some(Dialect::OpenAi)),
            ("chatgpt-4o-latest", Some(Dialect::OpenAi)),
            ("o3", Some(Dialect::OpenAi)),
            ("o4-mini", Some(Dialect::OpenAi)),
            ("gemini-3.1-pro-preview", Some(Dialect::Gemini)),
            ("ollama", None),
            ("o", None),
            ("llama3:8b", None),
        ];
        for (model, expected) in cases {
            assert_eq!(Dialect::infer_from_model(model), expected, "{model}");
        }
    }

    #[test]
    fn secret_never_debugs() {
        let mut e = ep("x", Dialect::OpenAi, false);
        e.api_key = SecretString::new("sk-very-secret");
        assert!(!format!("{e:?}").contains("sk-very-secret"));
    }

    #[test]
    fn endpoint_names() {
        assert!(is_valid_endpoint_name("lite-llm_2"));
        assert!(!is_valid_endpoint_name(""));
        assert!(!is_valid_endpoint_name("a:b"));
    }

    #[test]
    fn describe_and_credentials() {
        let mut e = ep("litellm", Dialect::OpenAi, false);
        e.base_url = Some("http://localhost:4000".to_string());
        assert_eq!(e.describe(), "openai dialect, http://localhost:4000");
        let creds = e.credentials();
        assert_eq!(creds.provider, Provider::OpenAI);
        assert_eq!(creds.base_url.as_deref(), Some("http://localhost:4000"));
        assert_eq!(Provider::from(Dialect::Gemini), Provider::Gemini);
    }
}

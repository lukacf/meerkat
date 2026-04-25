//! Provider enumeration shared across interfaces.

use serde::{Deserialize, Serialize};

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    OpenAI,
    Gemini,
    SelfHosted,
    Other,
}

impl Provider {
    /// Map a provider name to a Provider enum.
    pub fn from_name(name: &str) -> Self {
        match name {
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAI,
            "gemini" => Self::Gemini,
            "self_hosted" => Self::SelfHosted,
            _ => Self::Other,
        }
    }

    /// Parse a provider name strictly (only canonical lowercase names).
    /// Returns `None` for unrecognized strings instead of falling back to `Other`.
    pub fn parse_strict(name: &str) -> Option<Self> {
        match name {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAI),
            "gemini" => Some(Self::Gemini),
            "self_hosted" => Some(Self::SelfHosted),
            _ => None,
        }
    }

    /// Infer provider from the built-in model catalog.
    /// Returns `None` for uncatalogued models; callers that admit custom
    /// models must resolve them through `ModelRegistry`, not name prefixes.
    pub fn infer_from_model(model: &str) -> Option<Self> {
        crate::model_profile::catalog::catalog()
            .iter()
            .find(|entry| entry.id == model)
            .and_then(|entry| Self::parse_strict(entry.provider))
    }

    /// Return the canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::Gemini => "gemini",
            Self::SelfHosted => "self_hosted",
            Self::Other => "other",
        }
    }

    /// All concrete (non-Other) providers.
    pub const ALL_CONCRETE: &'static [Provider] = &[
        Provider::Anthropic,
        Provider::OpenAI,
        Provider::Gemini,
        Provider::SelfHosted,
    ];
}

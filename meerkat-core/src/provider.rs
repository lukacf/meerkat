//! Provider enumeration shared across interfaces.

use serde::{Deserialize, Serialize};

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    OpenAI,
    Gemini,
    Other,
}

impl Provider {
    /// Map a provider name to a Provider enum.
    pub fn from_name(name: &str) -> Self {
        match name {
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAI,
            "gemini" => Self::Gemini,
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
            _ => None,
        }
    }

    /// Infer provider from a model name using well-known prefixes.
    /// Returns `None` when no prefix matches.
    pub fn infer_from_model(model: &str) -> Option<Self> {
        let lower = model.to_lowercase();

        // Anthropic: claude-*
        if lower.starts_with("claude-") {
            return Some(Self::Anthropic);
        }

        // OpenAI: gpt-*, o1-*, o3-*, chatgpt-*
        if lower.starts_with("gpt-")
            || lower.starts_with("o1-")
            || lower.starts_with("o3-")
            || lower.starts_with("chatgpt-")
        {
            return Some(Self::OpenAI);
        }

        // Gemini: gemini-*
        if lower.starts_with("gemini-") {
            return Some(Self::Gemini);
        }

        None
    }

    /// Return the canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::Gemini => "gemini",
            Self::Other => "other",
        }
    }

    /// All concrete (non-Other) providers.
    pub const ALL_CONCRETE: &'static [Provider] =
        &[Provider::Anthropic, Provider::OpenAI, Provider::Gemini];
}

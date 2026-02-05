//! Provider enumeration shared across interfaces.

use serde::{Deserialize, Serialize};

/// Supported LLM providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
}

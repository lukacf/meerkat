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

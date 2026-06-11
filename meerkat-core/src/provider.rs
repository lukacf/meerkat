//! Provider enumeration shared across interfaces.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Supported LLM providers.
///
/// `JsonSchema` is derived unconditionally (schemars is a non-optional
/// meerkat-core dependency): config-owned types such as
/// [`crate::config::CustomModelConfig`] embed the typed provider directly and
/// derive their schemas without the `schema` feature.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    // `rename_all = "snake_case"` mangles `OpenAI` into `"open_a_i"`, which
    // diverges from the canonical `as_str()` name `"openai"` that every other
    // seam (and durable data) uses. Pin the canonical wire/schema name on the
    // variant so the derived `Serialize`/`Deserialize` and the generated
    // `schemars` schema all agree on `"openai"` — one representation, no shim.
    #[serde(rename = "openai")]
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

/// Serde helper for seams that carry the provider as a plain `String` on the
/// wire (e.g. `LiveProjectionSnapshot.provider_id`, whose JSON schema is
/// `String`) but hold a typed [`Provider`] in memory.
///
/// Serialization matches the canonical [`Provider::as_str`] names — identical
/// to the enum's own derived output now that [`Provider::OpenAI`] is pinned to
/// `"openai"`. Deserialization is intentionally lenient (`Provider::from_name`,
/// unknown → [`Provider::Other`]) so an opaque provider string carried by such
/// a seam round-trips into the catch-all variant rather than failing closed —
/// the leniency the plain-`String` carrier had before it was retyped.
pub mod provider_canonical_str {
    use super::Provider;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(value: &Provider, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.as_str().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Provider, D::Error>
    where
        D: Deserializer<'de>,
    {
        let name = String::deserialize(deserializer)?;
        Ok(Provider::from_name(&name))
    }
}

#[cfg(test)]
mod tests {
    use super::Provider;

    #[test]
    fn parse_strict_fails_closed_where_from_name_coerces_to_other() {
        // Pins the two distinct provider-name boundaries the codebase relies on:
        // `from_name` maps an unrecognized label to the typed `Other` variant
        // (correct where a non-catalog provider is legitimate, e.g. a
        // caller-supplied custom AgentLlmClient), whereas `parse_strict` returns
        // None so fail-closed seams (e.g. catalog-default / session-create
        // provider resolution) can surface a typed error instead of minting a
        // catalog identity from an arbitrary string.
        assert_eq!(
            Provider::from_name("totally-unknown-provider"),
            Provider::Other
        );
        assert_eq!(Provider::parse_strict("totally-unknown-provider"), None);
        // Canonical names still resolve through the strict path.
        assert_eq!(
            Provider::parse_strict("anthropic"),
            Some(Provider::Anthropic)
        );
        assert_eq!(Provider::parse_strict("openai"), Some(Provider::OpenAI));
    }
}

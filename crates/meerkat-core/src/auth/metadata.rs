//! Auth metadata types — generic, provider-neutral.
//!
//! Per-provider marker structs (`OpenAiRouteHints`, `AnthropicAuthMetadata`,
//! etc.) are intentionally empty `#[non_exhaustive]` placeholders in Phase 1.
//! They pin the route-hint shape so the public `AuthRouteHints` enum is
//! stable, but `meerkat-core` does not own provider-specific semantics —
//! those land in `meerkat-client` provider runtimes when the fields are
//! actually needed.

use serde::{Deserialize, Serialize};

/// Generic auth metadata attached to a resolved lease. Provider-specific
/// metadata goes under [`AuthMetadata::provider_metadata`] / [`AuthMetadata::route_hints`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default)]
    pub route_hints: AuthRouteHints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderAuthMetadata>,
}

/// Non-secret defaults merged during auth profile resolution.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthMetadataDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub route_hints: AuthRouteHints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderAuthMetadata>,
}

/// Provider-specific route hints (boxed to keep the enum small).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum AuthRouteHints {
    #[default]
    None,
    OpenAi(Box<OpenAiRouteHints>),
    Anthropic(Box<AnthropicRouteHints>),
    Google(Box<GoogleRouteHints>),
}

/// Provider-tagged auth metadata. Content is opaque to `meerkat-core`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderAuthMetadata {
    OpenAi(OpenAiAuthMetadata),
    Anthropic(AnthropicAuthMetadata),
    Google(GoogleAuthMetadata),
}

// Per-provider marker structs. Empty + non_exhaustive means `meerkat-core`
// declares the shape without owning the contents. Provider runtimes fill
// them in as real fields become necessary.

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct OpenAiRouteHints {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct AnthropicRouteHints {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct GoogleRouteHints {}

/// ChatGPT-specific claims lifted from the ID token per Codex
/// `token_data.rs:71-160`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OpenAiAuthMetadata {
    /// `chatgpt_plan_type` — e.g. "free", "plus", "pro", "enterprise".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    /// `chatgpt_user_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// `chatgpt_account_id` — used as the `ChatGPT-Account-ID` wire header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// `chatgpt_account_is_fedramp` — when true, emit `X-OpenAI-Fedramp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_fedramp: Option<bool>,
    /// Primary email address on the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Anthropic-specific metadata (subscription tier for Claude.ai OAuth,
/// Bedrock/Vertex/Foundry region/project hints).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AnthropicAuthMetadata {
    /// Claude.ai subscription tier ("free" / "pro" / "max" / "team").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_tier: Option<String>,
    /// AWS region for Bedrock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aws_region: Option<String>,
    /// GCP project ID for Vertex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertex_project_id: Option<String>,
    /// GCP region for Vertex / Vertex model endpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertex_region: Option<String>,
    /// Azure deployment URL prefix for Foundry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foundry_deployment: Option<String>,
}

/// Google-specific metadata (ADC account, Vertex project/region hints).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoogleAuthMetadata {
    /// Identifier for the Google account (email address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    /// GCP project ID for Vertex / Code Assist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Preferred region for Vertex model endpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Code Assist user tier ("free" / "standard" / "enterprise").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_assist_tier: Option<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn auth_metadata_default_is_empty() {
        let m = AuthMetadata::default();
        assert!(m.account_id.is_none());
        assert_eq!(m.route_hints, AuthRouteHints::None);
        assert!(m.provider_metadata.is_none());
    }

    #[test]
    fn route_hints_serde_roundtrip() {
        for hints in [
            AuthRouteHints::None,
            AuthRouteHints::OpenAi(Box::default()),
            AuthRouteHints::Anthropic(Box::default()),
            AuthRouteHints::Google(Box::default()),
        ] {
            let s = serde_json::to_string(&hints).unwrap();
            let back: AuthRouteHints = serde_json::from_str(&s).unwrap();
            assert_eq!(back, hints);
        }
    }

    #[test]
    fn provider_auth_metadata_roundtrip() {
        let m = ProviderAuthMetadata::OpenAi(OpenAiAuthMetadata::default());
        let s = serde_json::to_string(&m).unwrap();
        let back: ProviderAuthMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(back, m);
    }
}

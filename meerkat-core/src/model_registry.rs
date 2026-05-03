//! Effective model registry merged from built-in catalog and configured self-hosted aliases.

use crate::Provider;
use crate::config::{
    Config, ConfigError, SelfHostedApiStyle, SelfHostedConfig, SelfHostedTransport,
};
use crate::model_profile::{ModelProfile, catalog::ModelTier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfHostedServerRef {
    pub server_id: String,
    pub remote_model: String,
    pub transport: SelfHostedTransport,
    pub api_style: SelfHostedApiStyle,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct ModelRegistryEntry {
    pub id: String,
    pub display_name: String,
    pub provider: Provider,
    pub tier: ModelTier,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub self_hosted: Option<SelfHostedServerRef>,
}

#[derive(Debug, Clone)]
pub struct ModelRegistry {
    entries: BTreeMap<String, ModelRegistryEntry>,
    profiles: BTreeMap<(Provider, String), ModelProfile>,
    defaults: BTreeMap<Provider, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCapability {
    InlineVideo,
}

impl ModelCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InlineVideo => "inline_video",
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::InlineVideo => "inline video",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedModelCapabilityReason {
    CapabilityDisabled,
    ProviderModelProfileMissing,
    CapabilityRegistryUnavailable,
}

impl UnsupportedModelCapabilityReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CapabilityDisabled => "capability_disabled",
            Self::ProviderModelProfileMissing => "provider_model_profile_missing",
            Self::CapabilityRegistryUnavailable => "capability_registry_unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedModelCapabilityEvidence {
    pub capability: ModelCapability,
    pub provider: Provider,
    pub model: String,
    pub reason: UnsupportedModelCapabilityReason,
}

impl UnsupportedModelCapabilityEvidence {
    pub fn inline_video(
        provider: Provider,
        model: impl Into<String>,
        reason: UnsupportedModelCapabilityReason,
    ) -> Self {
        Self {
            capability: ModelCapability::InlineVideo,
            provider,
            model: model.into(),
            reason,
        }
    }

    pub fn details(&self) -> serde_json::Value {
        serde_json::json!({
            "unsupported_capability": {
                "capability": self.capability.as_str(),
                "provider": self.provider.as_str(),
                "model": self.model.as_str(),
                "reason": self.reason.as_str(),
            },
        })
    }
}

impl fmt::Display for UnsupportedModelCapabilityEvidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} input is not supported by model '{}' on provider '{}' (capability: {}, reason: {})",
            self.capability.display_name(),
            self.model,
            self.provider.as_str(),
            self.capability.as_str(),
            self.reason.as_str()
        )
    }
}

impl ModelRegistry {
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let mut entries = BTreeMap::new();
        let mut profiles = BTreeMap::new();
        let mut defaults = BTreeMap::new();

        for provider_name in crate::model_profile::catalog::provider_names() {
            let provider = Provider::parse_strict(provider_name).ok_or_else(|| {
                ConfigError::InternalError(format!("unknown built-in provider '{provider_name}'"))
            })?;
            let default_model = crate::model_profile::catalog::default_model(provider_name)
                .ok_or_else(|| {
                    ConfigError::InternalError(format!(
                        "missing built-in default for '{provider_name}'"
                    ))
                })?;
            defaults.insert(provider, default_model.to_string());

            for entry in crate::model_profile::catalog::catalog()
                .iter()
                .filter(|entry| entry.provider == *provider_name)
            {
                let profile =
                    crate::model_profile::profile_for(provider, entry.id).ok_or_else(|| {
                        ConfigError::InternalError(format!(
                            "missing built-in profile for {}:{}",
                            entry.provider, entry.id
                        ))
                    })?;
                insert_unique(
                    &mut entries,
                    &mut profiles,
                    ModelRegistryEntry {
                        id: entry.id.to_string(),
                        display_name: entry.display_name.to_string(),
                        provider,
                        tier: entry.tier,
                        context_window: entry.context_window,
                        max_output_tokens: entry.max_output_tokens,
                        self_hosted: None,
                    },
                    profile,
                )?;
            }
        }

        append_self_hosted(
            &mut entries,
            &mut profiles,
            &mut defaults,
            &config.self_hosted,
        )?;

        Ok(Self {
            entries,
            profiles,
            defaults,
        })
    }

    /// Returns model projection metadata by id.
    ///
    /// This model-only lookup intentionally does not expose `ModelProfile` or
    /// capability fields. Capability decisions must use typed provider-aware
    /// lookup through [`ModelRegistry::profile_for_provider`].
    ///
    /// ```compile_fail
    /// let registry = meerkat_core::Config::default().model_registry().unwrap();
    /// let entry = registry.entry("gemini-3-flash-preview").unwrap();
    /// let _inline_video = entry.profile.inline_video;
    /// ```
    ///
    /// ```
    /// let registry = meerkat_core::Config::default().model_registry().unwrap();
    /// let profile = registry
    ///     .profile_for_provider(meerkat_core::Provider::Gemini, "gemini-3-flash-preview")
    ///     .unwrap();
    /// assert!(profile.inline_video);
    /// ```
    pub fn entry(&self, model_id: &str) -> Option<&ModelRegistryEntry> {
        self.entries.get(model_id)
    }

    pub fn entry_for_provider(
        &self,
        provider: Provider,
        model_id: &str,
    ) -> Option<&ModelRegistryEntry> {
        self.entry(model_id)
            .filter(|entry| entry.provider == provider)
    }

    pub fn provider_override_mismatch_reason(
        &self,
        provider: Provider,
        model_id: &str,
    ) -> Option<String> {
        let registered_provider = self.entry(model_id)?.provider;
        if registered_provider == provider {
            return None;
        }

        Some(format!(
            "model '{model_id}' is registered for provider '{}', not provider '{}'; explicit provider overrides must match catalog ownership",
            registered_provider.as_str(),
            provider.as_str()
        ))
    }

    pub fn profile_for_provider(&self, provider: Provider, model_id: &str) -> Option<ModelProfile> {
        self.entry_for_provider(provider, model_id)?;
        self.profiles
            .get(&(provider, model_id.to_string()))
            .cloned()
    }

    pub fn require_inline_video_for_provider(
        &self,
        provider: Provider,
        model_id: &str,
    ) -> Result<(), UnsupportedModelCapabilityEvidence> {
        let Some(profile) = self.profile_for_provider(provider, model_id) else {
            return Err(UnsupportedModelCapabilityEvidence::inline_video(
                provider,
                model_id,
                UnsupportedModelCapabilityReason::ProviderModelProfileMissing,
            ));
        };

        if profile.inline_video {
            Ok(())
        } else {
            Err(UnsupportedModelCapabilityEvidence::inline_video(
                provider,
                model_id,
                UnsupportedModelCapabilityReason::CapabilityDisabled,
            ))
        }
    }

    pub fn default_model(&self, provider: Provider) -> Option<&str> {
        self.defaults.get(&provider).map(String::as_str)
    }

    pub fn entries_for_provider(
        &self,
        provider: Provider,
    ) -> impl Iterator<Item = &ModelRegistryEntry> {
        self.entries
            .values()
            .filter(move |entry| entry.provider == provider)
    }

    pub fn provider_defaults(&self) -> impl Iterator<Item = (Provider, &str)> {
        self.defaults
            .iter()
            .map(|(provider, default_model)| (*provider, default_model.as_str()))
    }
}

fn append_self_hosted(
    entries: &mut BTreeMap<String, ModelRegistryEntry>,
    profiles: &mut BTreeMap<(Provider, String), ModelProfile>,
    defaults: &mut BTreeMap<Provider, String>,
    config: &SelfHostedConfig,
) -> Result<(), ConfigError> {
    if config.models.is_empty() {
        return Ok(());
    }

    let default_model = config.models.keys().min().cloned().ok_or_else(|| {
        ConfigError::InternalError("self-hosted models unexpectedly empty".to_string())
    })?;
    defaults.insert(Provider::SelfHosted, default_model);

    for (model_id, model) in &config.models {
        let server = config.servers.get(&model.server).ok_or_else(|| {
            ConfigError::Validation(format!(
                "self_hosted.models.{model_id} references unknown server '{}'",
                model.server
            ))
        })?;
        if server.bearer_token.is_some() {
            tracing::warn!(
                server_id = %model.server,
                "self-hosted server uses a literal bearer_token; bearer_token_env is recommended to avoid storing secrets in config files"
            );
        }

        let self_hosted = SelfHostedServerRef {
            server_id: model.server.clone(),
            remote_model: model.remote_model.clone(),
            transport: server.transport,
            api_style: server.api_style,
            base_url: normalize_base_url(&server.base_url),
        };
        let profile = ModelProfile {
            provider: Provider::SelfHosted.as_str().to_string(),
            model_family: model.family.clone(),
            supports_temperature: model.supports_temperature,
            supports_thinking: model.supports_thinking,
            supports_reasoning: model.supports_reasoning,
            supports_web_search: model.supports_web_search,
            inline_video: model.inline_video,
            vision: model.vision,
            image_tool_results: model.image_tool_results,
            realtime: false,
            params_schema: serde_json::json!({}),
            beta_headers: Vec::new(),
            call_timeout_secs: model.call_timeout_secs,
        };

        insert_unique(
            entries,
            profiles,
            ModelRegistryEntry {
                id: model_id.clone(),
                display_name: model.display_name.clone(),
                provider: Provider::SelfHosted,
                tier: model.tier,
                context_window: model.context_window,
                max_output_tokens: model.max_output_tokens,
                self_hosted: Some(self_hosted),
            },
            profile,
        )?;
    }

    Ok(())
}

fn insert_unique(
    entries: &mut BTreeMap<String, ModelRegistryEntry>,
    profiles: &mut BTreeMap<(Provider, String), ModelProfile>,
    entry: ModelRegistryEntry,
    profile: ModelProfile,
) -> Result<(), ConfigError> {
    let model_id = entry.id.clone();
    let provider = entry.provider;
    if entries.insert(model_id.clone(), entry).is_some() {
        return Err(ConfigError::Validation(
            "model id must be unique across built-in and self-hosted entries".to_string(),
        ));
    }
    profiles.insert((provider, model_id), profile);
    Ok(())
}

pub fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]

    use super::*;
    use crate::config::{
        SelfHostedApiStyle, SelfHostedModelConfig, SelfHostedServerConfig, SelfHostedTransport,
    };

    fn config_with_self_hosted() -> Config {
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            "local".to_string(),
            SelfHostedServerConfig {
                transport: SelfHostedTransport::OpenAiCompatible,
                base_url: "http://127.0.0.1:11434".to_string(),
                api_style: SelfHostedApiStyle::Responses,
                bearer_token: None,
                bearer_token_env: Some("LOCAL_TOKEN".to_string()),
            },
        );
        config.self_hosted.models.insert(
            "gemma-4-31b".to_string(),
            SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "gemma4:31b".to_string(),
                display_name: "Gemma 4 31B".to_string(),
                family: "gemma-4".to_string(),
                tier: ModelTier::Supported,
                context_window: Some(256_000),
                max_output_tokens: Some(8_192),
                vision: true,
                image_tool_results: true,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: Some(600),
            },
        );
        config
    }

    #[test]
    fn merges_self_hosted_models_into_registry() {
        let config = config_with_self_hosted();
        let registry = match ModelRegistry::from_config(&config) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        let entry = match registry.entry("gemma-4-31b") {
            Some(entry) => entry,
            None => panic!("missing self-hosted entry for gemma-4-31b"),
        };
        assert_eq!(entry.provider, Provider::SelfHosted);
        assert_eq!(entry.display_name, "Gemma 4 31B");
        assert_eq!(
            entry
                .self_hosted
                .as_ref()
                .map(|server| server.server_id.as_str()),
            Some("local")
        );
        assert_eq!(
            entry
                .self_hosted
                .as_ref()
                .map(|server| server.remote_model.as_str()),
            Some("gemma4:31b")
        );
        assert_eq!(
            entry
                .self_hosted
                .as_ref()
                .map(|server| server.base_url.as_str()),
            Some("http://127.0.0.1:11434/v1")
        );
        assert_eq!(
            registry.default_model(Provider::SelfHosted),
            Some("gemma-4-31b")
        );
    }

    #[test]
    fn rejects_unknown_server_reference() {
        let mut config = Config::default();
        config.self_hosted.models.insert(
            "gemma-4-31b".to_string(),
            SelfHostedModelConfig {
                server: "missing".to_string(),
                remote_model: "gemma4:31b".to_string(),
                display_name: "Gemma 4 31B".to_string(),
                family: "gemma-4".to_string(),
                tier: ModelTier::Supported,
                context_window: None,
                max_output_tokens: None,
                vision: true,
                image_tool_results: true,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: None,
            },
        );
        let err = match ModelRegistry::from_config(&config) {
            Ok(_) => panic!("unknown server should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("references unknown server"));
    }

    #[test]
    fn rejects_duplicate_model_ids() {
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            "local".to_string(),
            SelfHostedServerConfig {
                transport: SelfHostedTransport::OpenAiCompatible,
                base_url: "http://127.0.0.1:11434".to_string(),
                api_style: SelfHostedApiStyle::Responses,
                bearer_token: None,
                bearer_token_env: None,
            },
        );
        config.self_hosted.models.insert(
            "gpt-5.4".to_string(),
            SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "override".to_string(),
                display_name: "Override".to_string(),
                family: "override".to_string(),
                tier: ModelTier::Supported,
                context_window: None,
                max_output_tokens: None,
                vision: false,
                image_tool_results: false,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: None,
            },
        );
        let err = match ModelRegistry::from_config(&config) {
            Ok(_) => panic!("duplicate model id should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("model id must be unique"));
    }

    #[test]
    fn uncatalogued_models_do_not_use_provider_prefix_inference() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, "gpt-unknown-preview")
                .is_none()
        );
        assert!(
            registry
                .profile_for_provider(Provider::Anthropic, "claude-unknown-preview")
                .is_none()
        );
        assert!(
            registry
                .profile_for_provider(Provider::Gemini, "gemini-unknown-preview")
                .is_none()
        );
    }

    #[test]
    fn provider_aware_profile_lookup_requires_matching_provider() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let profile = registry.profile_for_provider(Provider::OpenAI, "gpt-5.4");
        assert_eq!(
            profile.and_then(|profile| profile.call_timeout_secs),
            Some(600)
        );
        assert!(
            registry
                .profile_for_provider(Provider::Anthropic, "gpt-5.4")
                .is_none(),
            "provider-aware lookup must not share OpenAI defaults with Anthropic"
        );
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, "gemini-3-flash-preview")
                .is_none(),
            "provider-aware lookup must not let provider strings select another provider's capabilities"
        );
    }

    #[test]
    fn model_only_entries_are_projection_metadata_not_capability_authority() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let entry = registry
            .entry("gemini-3-flash-preview")
            .expect("catalog entry must exist");
        assert_eq!(entry.provider, Provider::Gemini);
        assert_eq!(entry.id, "gemini-3-flash-preview");
        let rendered = format!("{entry:?}");
        assert!(
            !rendered.contains("inline_video") && !rendered.contains("supports_temperature"),
            "model-only projection entry must not expose capability fields: {rendered}"
        );

        let profile = registry
            .profile_for_provider(Provider::Gemini, "gemini-3-flash-preview")
            .expect("typed provider-aware capability lookup should resolve");
        assert!(profile.inline_video);
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, "gemini-3-flash-preview")
                .is_none(),
            "display/catalog lookup must not let another typed provider read capability truth"
        );
    }

    #[test]
    fn provider_aware_profile_lookup_fails_closed_for_unknown_pairs() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        assert!(
            registry
                .profile_for_provider(Provider::Other, "gpt-5.4")
                .is_none(),
            "unknown typed provider must not receive known model defaults"
        );
        assert!(
            registry
                .profile_for_provider(Provider::Other, "uncatalogued-gpt-compatible")
                .is_none(),
            "unknown provider/model pairs must fail closed"
        );
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, "uncatalogued-gpt-compatible")
                .is_none(),
            "known provider plus uncatalogued model must fail closed"
        );
    }

    #[test]
    fn inline_video_capability_requires_typed_provider_owner() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        registry
            .require_inline_video_for_provider(Provider::Gemini, "gemini-3-flash-preview")
            .expect("Gemini catalog owner should authorize inline video");

        let err = registry
            .require_inline_video_for_provider(Provider::OpenAI, "gemini-3-flash-preview")
            .expect_err("same model name under another provider must fail closed");
        assert_eq!(err.capability, ModelCapability::InlineVideo);
        assert_eq!(err.provider, Provider::OpenAI);
        assert_eq!(err.model, "gemini-3-flash-preview");
        assert_eq!(
            err.reason,
            UnsupportedModelCapabilityReason::ProviderModelProfileMissing
        );
    }

    #[test]
    fn inline_video_capability_evidence_distinguishes_disabled_and_unknown() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let disabled = registry
            .require_inline_video_for_provider(Provider::OpenAI, "gpt-5.4")
            .expect_err("known OpenAI model has catalog-owned inline video disabled");
        assert_eq!(
            disabled.reason,
            UnsupportedModelCapabilityReason::CapabilityDisabled
        );

        let unknown = registry
            .require_inline_video_for_provider(Provider::Other, "uncatalogued-video-model")
            .expect_err("unknown provider/model pair must fail closed");
        assert_eq!(
            unknown.reason,
            UnsupportedModelCapabilityReason::ProviderModelProfileMissing
        );
        let details = unknown.details();
        assert_eq!(
            details["unsupported_capability"]["capability"],
            serde_json::json!("inline_video")
        );
        assert_eq!(
            details["unsupported_capability"]["reason"],
            serde_json::json!("provider_model_profile_missing")
        );
    }

    #[test]
    fn provider_override_mismatch_reason_reports_catalog_owner_contradictions() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let reason = registry
            .provider_override_mismatch_reason(Provider::Anthropic, "gpt-5.4")
            .expect("wrong-provider override for a catalog model should be rejected");
        assert!(reason.contains("model 'gpt-5.4'"));
        assert!(reason.contains("registered for provider 'openai'"));
        assert!(reason.contains("not provider 'anthropic'"));
        assert!(reason.contains("explicit provider overrides"));

        assert!(
            registry
                .provider_override_mismatch_reason(Provider::OpenAI, "gpt-5.4")
                .is_none(),
            "matching provider override should remain valid"
        );
        assert!(
            registry
                .provider_override_mismatch_reason(Provider::OpenAI, "uncatalogued-gpt-compatible")
                .is_none(),
            "uncatalogued models have no catalog owner to contradict"
        );
    }
}

//! Effective model registry merged from built-in catalog and configured self-hosted aliases.

use crate::Provider;
use crate::config::{
    Config, ConfigError, SelfHostedApiStyle, SelfHostedConfig, SelfHostedTransport,
};
use meerkat_models::{ModelProfile, ModelTier};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfHostedServerRef {
    pub server_id: String,
    pub remote_model: String,
    pub transport: SelfHostedTransport,
    pub api_style: SelfHostedApiStyle,
    pub base_url: String,
    pub bearer_token: Option<String>,
    pub bearer_token_env: Option<String>,
}

impl SelfHostedServerRef {
    pub fn resolve_bearer_token(&self) -> Option<String> {
        self.bearer_token.clone().or_else(|| {
            self.bearer_token_env
                .as_deref()
                .and_then(|env_key| std::env::var(env_key).ok())
        })
    }
}

#[derive(Debug, Clone)]
pub struct ModelRegistryEntry {
    pub id: String,
    pub display_name: String,
    pub provider: Provider,
    pub tier: ModelTier,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub profile: ModelProfile,
    pub self_hosted: Option<SelfHostedServerRef>,
}

#[derive(Debug, Clone)]
pub struct ModelRegistry {
    entries: BTreeMap<String, ModelRegistryEntry>,
    defaults: BTreeMap<Provider, String>,
}

impl ModelRegistry {
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let mut entries = BTreeMap::new();
        let mut defaults = BTreeMap::new();

        for provider_name in meerkat_models::provider_names() {
            let provider = Provider::parse_strict(provider_name).ok_or_else(|| {
                ConfigError::InternalError(format!("unknown built-in provider '{provider_name}'"))
            })?;
            let default_model = meerkat_models::default_model(provider_name).ok_or_else(|| {
                ConfigError::InternalError(format!(
                    "missing built-in default for '{provider_name}'"
                ))
            })?;
            defaults.insert(provider, default_model.to_string());

            for entry in meerkat_models::catalog()
                .iter()
                .filter(|entry| entry.provider == *provider_name)
            {
                let profile =
                    meerkat_models::profile_for(entry.provider, entry.id).ok_or_else(|| {
                        ConfigError::InternalError(format!(
                            "missing built-in profile for {}:{}",
                            entry.provider, entry.id
                        ))
                    })?;
                insert_unique(
                    &mut entries,
                    ModelRegistryEntry {
                        id: entry.id.to_string(),
                        display_name: entry.display_name.to_string(),
                        provider,
                        tier: entry.tier,
                        context_window: entry.context_window,
                        max_output_tokens: entry.max_output_tokens,
                        profile,
                        self_hosted: None,
                    },
                )?;
            }
        }

        append_self_hosted(&mut entries, &mut defaults, &config.self_hosted)?;

        Ok(Self { entries, defaults })
    }

    pub fn entry(&self, model_id: &str) -> Option<&ModelRegistryEntry> {
        self.entries.get(model_id)
    }

    pub fn profile_for(&self, model_id: &str) -> Option<ModelProfile> {
        self.entry(model_id)
            .map(|entry| entry.profile.clone())
            .or_else(|| inferred_builtin_profile(model_id))
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

fn inferred_builtin_profile(model_id: &str) -> Option<ModelProfile> {
    let provider = Provider::infer_from_model(model_id)?;
    meerkat_models::profile_for(provider.as_str(), model_id)
}

fn append_self_hosted(
    entries: &mut BTreeMap<String, ModelRegistryEntry>,
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
            bearer_token: server.bearer_token.clone(),
            bearer_token_env: server.bearer_token_env.clone(),
        };
        let profile = ModelProfile {
            provider: Provider::SelfHosted.as_str().to_string(),
            model_family: model.family.clone(),
            supports_temperature: model.supports_temperature,
            supports_thinking: model.supports_thinking,
            supports_reasoning: model.supports_reasoning,
            inline_video: model.inline_video,
            vision: model.vision,
            image_tool_results: model.image_tool_results,
            params_schema: serde_json::json!({}),
            call_timeout_secs: model.call_timeout_secs,
        };

        insert_unique(
            entries,
            ModelRegistryEntry {
                id: model_id.clone(),
                display_name: model.display_name.clone(),
                provider: Provider::SelfHosted,
                tier: model.tier,
                context_window: model.context_window,
                max_output_tokens: model.max_output_tokens,
                profile,
                self_hosted: Some(self_hosted),
            },
        )?;
    }

    Ok(())
}

fn insert_unique(
    entries: &mut BTreeMap<String, ModelRegistryEntry>,
    entry: ModelRegistryEntry,
) -> Result<(), ConfigError> {
    if entries.insert(entry.id.clone(), entry).is_some() {
        return Err(ConfigError::Validation(
            "model id must be unique across built-in and self-hosted entries".to_string(),
        ));
    }
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
    fn falls_back_to_inferred_builtin_profiles_for_non_catalog_models() {
        let registry = match ModelRegistry::from_config(&Config::default()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        let profile = match registry.profile_for("gpt-5.2") {
            Some(profile) => profile,
            None => panic!("gpt-5.2 should resolve via built-in provider inference"),
        };
        assert_eq!(profile.provider, "openai");
        assert!(profile.vision);
        assert!(!profile.image_tool_results);
    }
}

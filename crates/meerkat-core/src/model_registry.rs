//! Effective model registry merged from built-in catalog and configured self-hosted aliases.

use crate::Provider;
use crate::config::{
    Config, ConfigError, CustomModelConfig, SelfHostedApiStyle, SelfHostedConfig,
    SelfHostedTransport,
};
use crate::model_profile::{
    ModelCatalog, ModelProfile,
    catalog::{ModelReleaseStage, ModelTier},
};
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
    pub release_stage: ModelReleaseStage,
    pub context_window: Option<u32>,
    pub max_input_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub self_hosted: Option<SelfHostedServerRef>,
}

/// Registry-minted provenance for one exact provider/model capability profile.
///
/// The fields are deliberately private: callers can only obtain this witness
/// through [`ModelRegistry::profile_witness_for_provider`] or
/// [`ModelRegistry::profile_witness_for_identity`], which validate the exact
/// typed provider/model pair against one effective-registry authority. This
/// keeps a [`ModelProfile`] from being detached from either the catalog
/// identity, the binding it was judged through, or the registry instance that
/// minted it when it crosses a model-fallback commit boundary.
#[derive(Clone)]
pub struct ModelProfileWitness {
    registry_authority: ModelRegistryAuthority,
    provider: Provider,
    model: String,
    /// The binding the profile was judged through: the identity's own
    /// binding for [`ModelRegistry::profile_witness_for_identity`], `None`
    /// for [`ModelRegistry::profile_witness_for_provider`] (the model row
    /// alone).
    judged_binding: Option<crate::AuthBindingRef>,
    /// Whether the wire of the backend `judged_binding` names carries more
    /// than one leading System message
    /// ([`crate::BackendProfile::admits_additional_leading_system_messages`]);
    /// the model row alone is the provider's API-key backend, which does.
    admits_additional_leading_system_messages: bool,
    profile: ModelProfile,
    context_window: Option<u32>,
    max_input_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
    /// The catalog row behind this profile; `None` for custom and
    /// self-hosted models, which have no catalog-owned capability facts.
    catalog_capabilities: Option<&'static crate::model_profile::capabilities::ModelCapabilities>,
}

impl fmt::Debug for ModelProfileWitness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModelProfileWitness")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("judged_binding", &self.judged_binding)
            .field(
                "admits_additional_leading_system_messages",
                &self.admits_additional_leading_system_messages,
            )
            .field("profile", &self.profile)
            .field("context_window", &self.context_window)
            .field("max_input_tokens", &self.max_input_tokens)
            .field("max_output_tokens", &self.max_output_tokens)
            .finish_non_exhaustive()
    }
}

impl ModelProfileWitness {
    /// Typed provider identity validated by the effective model registry.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// Exact model identifier validated by the effective model registry.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Capability profile projected for this exact provider/model identity.
    pub fn profile(&self) -> &ModelProfile {
        &self.profile
    }

    /// Context-window limit owned by the same effective-registry entry.
    pub fn context_window(&self) -> Option<u32> {
        self.context_window
    }

    /// Separate input-token ceiling owned by the same effective-registry entry.
    pub fn max_input_tokens(&self) -> Option<u32> {
        self.max_input_tokens
    }

    /// Maximum output-token limit owned by the same effective-registry entry.
    pub fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    /// The catalog-owned capability row of this model, when it has one
    /// (custom and self-hosted models do not).
    pub fn catalog_capabilities(
        &self,
    ) -> Option<&'static crate::model_profile::capabilities::ModelCapabilities> {
        self.catalog_capabilities
    }

    /// Whether this witness was minted for the supplied session identity.
    pub fn matches_identity(&self, identity: &crate::SessionLlmIdentity) -> bool {
        self.provider == identity.provider && self.model == identity.model
    }

    /// Whether this witness states `identity`'s capabilities as its own
    /// binding serves them: minted for the same provider and model, and
    /// judged through the binding `identity` names (an identity that names
    /// none is judged by the model row). See
    /// [`ModelRegistry::profile_witness_for_identity`].
    pub fn judged_for_identity(&self, identity: &crate::SessionLlmIdentity) -> bool {
        self.matches_identity(identity) && self.judged_binding == identity.auth_binding
    }

    /// How operator-instruction entries lower for this model as the binding
    /// it was judged through serves it
    /// ([`ModelRegistry::instruction_lowering_for_identity`]).
    pub(crate) fn instruction_lowering(&self) -> crate::InstructionLowering {
        instruction_lowering_as_served(
            Some(&self.profile),
            self.admits_additional_leading_system_messages,
        )
    }

    pub(crate) fn was_minted_by(&self, authority: ModelRegistryAuthority) -> bool {
        self.registry_authority == authority
    }
}

/// The operator-instruction lowering of a model with `profile` on a wire
/// that does or does not carry more than one leading System message. Only
/// one leading System row (the session's prompt takes it) leaves no
/// position for an entry; any other wire lowers what the profile carries.
fn instruction_lowering_as_served(
    profile: Option<&ModelProfile>,
    admits_additional_leading_system_messages: bool,
) -> crate::InstructionLowering {
    if admits_additional_leading_system_messages {
        crate::InstructionLowering::for_resolved_profile(profile)
    } else {
        crate::InstructionLowering::Unavailable
    }
}

/// The weaker of two operator-instruction lowerings: the one that carries
/// fewer entry placements.
fn weaker_instruction_lowering(
    a: crate::InstructionLowering,
    b: crate::InstructionLowering,
) -> crate::InstructionLowering {
    use crate::InstructionLowering::{LeadingPrefixOnly, MidConversation, Unavailable};
    match (a, b) {
        (Unavailable, _) | (_, Unavailable) => Unavailable,
        (LeadingPrefixOnly, _) | (_, LeadingPrefixOnly) => LeadingPrefixOnly,
        (MidConversation, MidConversation) => MidConversation,
    }
}

/// Opaque authority of one constructed effective model registry.
///
/// This identity is construction-scoped, not a hash of registry contents:
/// separately constructed registries are distinct authorities even when they
/// contain byte-identical profiles. Cloning a registry preserves its authority
/// so all consumers of that captured effective registry can validate the same
/// witnesses. Witnesses are in-memory fallback proposals and are not serialized
/// across restart; a cold build reconstructs the registry and its candidates
/// together under a fresh authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ModelRegistryAuthority(uuid::Uuid);

impl ModelRegistryAuthority {
    fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

#[derive(Debug, Clone)]
pub struct ModelRegistry {
    authority: ModelRegistryAuthority,
    entries: BTreeMap<String, ModelRegistryEntry>,
    profiles: BTreeMap<(Provider, String), ModelProfile>,
    defaults: BTreeMap<Provider, String>,
    /// Catalog capability rows of the catalog-owned entries.
    catalog_capabilities: BTreeMap<
        (Provider, String),
        &'static crate::model_profile::capabilities::ModelCapabilities,
    >,
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
    /// Build the effective registry from a config snapshot and an explicitly
    /// injected model catalog (canonically `meerkat_models::canonical()`).
    pub fn from_config(config: &Config, catalog: ModelCatalog) -> Result<Self, ConfigError> {
        Self::from_config_with_models(config, &BTreeMap::new(), catalog)
    }

    /// Build the effective registry from a config snapshot plus caller-scoped
    /// custom model definitions (e.g. mob-definition `[models.<id>]` entries).
    ///
    /// Config-owned `[models.<id>]` entries merge first, then `extra_models`.
    /// Model ids must stay unique across catalog, config, self-hosted, and
    /// caller-scoped entries (fail closed on conflict).
    pub fn from_config_with_models(
        config: &Config,
        extra_models: &BTreeMap<String, CustomModelConfig>,
        catalog: ModelCatalog,
    ) -> Result<Self, ConfigError> {
        let mut entries = BTreeMap::new();
        let mut profiles = BTreeMap::new();
        let mut defaults = BTreeMap::new();
        let mut catalog_capabilities = BTreeMap::new();

        for &provider in catalog.providers {
            let default_model = catalog.default_model(provider).ok_or_else(|| {
                ConfigError::InternalError(format!(
                    "missing catalog default for '{}'",
                    provider.as_str()
                ))
            })?;
            defaults.insert(provider, default_model.to_string());

            for entry in catalog
                .entries
                .iter()
                .filter(|entry| entry.provider == provider.as_str())
            {
                if let Some(capabilities) = catalog.capabilities_for(provider, entry.id) {
                    catalog_capabilities.insert((provider, entry.id.to_string()), capabilities);
                }
                let profile = catalog.profile_for(provider, entry.id).ok_or_else(|| {
                    ConfigError::InternalError(format!(
                        "missing catalog profile for {}:{}",
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
                        release_stage: entry.release_stage,
                        context_window: entry.context_window,
                        max_input_tokens: entry.max_input_tokens,
                        max_output_tokens: entry.max_output_tokens,
                        self_hosted: None,
                    },
                    profile,
                )?;
            }
        }

        append_custom_models(&mut entries, &mut profiles, &config.models.custom)?;
        append_custom_models(&mut entries, &mut profiles, extra_models)?;

        append_self_hosted(
            &mut entries,
            &mut profiles,
            &mut defaults,
            &config.self_hosted,
        )?;

        Ok(Self {
            authority: ModelRegistryAuthority::new(),
            entries,
            profiles,
            defaults,
            catalog_capabilities,
        })
    }

    /// Authority shared by this effective registry and all of its clones.
    pub(crate) fn authority(&self) -> ModelRegistryAuthority {
        self.authority
    }

    /// Returns model projection metadata by id.
    ///
    /// This model-only lookup intentionally does not expose `ModelProfile` or
    /// capability fields (`ModelRegistryEntry` has no profile accessor).
    /// Capability decisions must use typed provider-aware lookup through
    /// [`ModelRegistry::profile_for_provider`].
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

    /// The capability profile of `identity` as its backend serves it: the
    /// model's profile, less what the backend its configured binding names in
    /// `config` cannot carry
    /// ([`crate::BackendProfile::admits_mid_conversation_system_messages`]).
    ///
    /// An identity that names no binding keeps the model's profile, and so
    /// does one naming the synthetic env-var default: that binding is never
    /// resolved from `config`, and it only ever names a provider's API-key
    /// backend. A named binding that `config` does not resolve fails closed:
    /// its backend is unknown, so the profile does not claim
    /// `supports_mid_conversation_system_messages`, nor does it for a binding
    /// to an OpenAI backend kind this version does not know.
    pub fn profile_for_identity(
        &self,
        config: &crate::Config,
        identity: &crate::SessionLlmIdentity,
    ) -> Option<ModelProfile> {
        let mut profile = self.profile_for_provider(identity.provider, &identity.model)?;
        if profile.supports_mid_conversation_system_messages
            && let Some(auth_binding) = identity.auth_binding.as_ref()
        {
            let backend_admits =
                match crate::resolve_explicit_auth_binding_target(config, auth_binding) {
                    Ok(target) => target.backend.admits_mid_conversation_system_messages(),
                    Err(_) => auth_binding.is_env_default(),
                };
            if !backend_admits {
                profile.supports_mid_conversation_system_messages = false;
            }
        }
        Some(profile)
    }

    /// How operator-instruction entries lower for `identity` as its backend
    /// serves it. A backend whose wire carries only one leading System row
    /// (the session's prompt takes it) lowers none: the lowering is
    /// unavailable whatever the model row says
    /// ([`crate::BackendProfile::admits_additional_leading_system_messages`]).
    /// Any other backend lowers what [`Self::profile_for_identity`] carries:
    /// anywhere when it carries ordered System messages after conversation
    /// rows, else only inside the leading System prefix, as does a model
    /// without a profile. The binding is judged as there: no binding, or the
    /// synthetic env-var default, is the API-key backend, and a named
    /// binding that `config` does not resolve has an unknown backend, which
    /// carries more than one leading System row only when every backend of
    /// the identity's provider does.
    pub fn instruction_lowering_for_identity(
        &self,
        config: &crate::Config,
        identity: &crate::SessionLlmIdentity,
    ) -> crate::InstructionLowering {
        instruction_lowering_as_served(
            self.profile_for_identity(config, identity).as_ref(),
            crate::connection::binding_admits_additional_leading_system_messages(
                config,
                identity.provider,
                identity.auth_binding.as_ref(),
            ),
        )
    }

    /// The weakest operator-instruction lowering `identity` may be served
    /// with, known before any client for it is built: its binding's
    /// ([`Self::instruction_lowering_for_identity`]), or, when it names
    /// none, the weakest of every binding its client resolution may select
    /// ([`crate::resolve_auth_binding_candidates_for_provider`] from
    /// `preferred_realm`). That resolution takes the first candidate that
    /// resolves, which only resolving it tells, so a model change judged by
    /// this lowering never builds a client for a target it refuses. Naming
    /// the binding decides exactly.
    pub fn weakest_instruction_lowering_for_target(
        &self,
        config: &crate::Config,
        identity: &crate::SessionLlmIdentity,
        preferred_realm: Option<&crate::RealmId>,
    ) -> crate::InstructionLowering {
        let lowering = self.instruction_lowering_for_identity(config, identity);
        if identity.auth_binding.is_some() {
            return lowering;
        }
        let Ok(candidates) = crate::resolve_auth_binding_candidates_for_provider(
            config,
            identity.provider,
            None,
            preferred_realm,
            true,
        ) else {
            return lowering;
        };
        candidates.into_iter().fold(lowering, |weakest, candidate| {
            let mut bound = identity.clone();
            bound.auth_binding = Some(candidate.auth_binding);
            weaker_instruction_lowering(
                weakest,
                self.instruction_lowering_for_identity(config, &bound),
            )
        })
    }

    /// Mint provenance for the exact provider/model profile in this registry.
    ///
    /// Unknown pairs and provider/model mismatches fail closed. The returned
    /// witness cannot be re-labelled by callers, so downstream machine inputs
    /// can bind the capability projection to the identity that was actually
    /// validated here.
    pub fn profile_witness_for_provider(
        &self,
        provider: Provider,
        model_id: &str,
    ) -> Option<ModelProfileWitness> {
        let profile = self.profile_for_provider(provider, model_id)?;
        self.mint_profile_witness(provider, model_id, None, true, profile)
    }

    /// Mint provenance for `identity`'s profile as its backend serves it
    /// ([`Self::profile_for_identity`]), recording the binding it was judged
    /// through and whether that backend's wire carries more than one
    /// leading System message.
    ///
    /// A backend can narrow the model row (the ChatGPT backend refuses the
    /// mid-conversation System messages the GPT rows declare), so this is the
    /// witness a model-fallback target is judged by: the agent keeps it when
    /// it activates the target and refuses one judged for another binding
    /// ([`ModelProfileWitness::judged_for_identity`]). Unknown pairs and
    /// provider/model mismatches fail closed.
    pub fn profile_witness_for_identity(
        &self,
        config: &crate::Config,
        identity: &crate::SessionLlmIdentity,
    ) -> Option<ModelProfileWitness> {
        let profile = self.profile_for_identity(config, identity)?;
        self.mint_profile_witness(
            identity.provider,
            &identity.model,
            identity.auth_binding.clone(),
            crate::connection::binding_admits_additional_leading_system_messages(
                config,
                identity.provider,
                identity.auth_binding.as_ref(),
            ),
            profile,
        )
    }

    fn mint_profile_witness(
        &self,
        provider: Provider,
        model_id: &str,
        judged_binding: Option<crate::AuthBindingRef>,
        admits_additional_leading_system_messages: bool,
        profile: ModelProfile,
    ) -> Option<ModelProfileWitness> {
        let entry = self.entry_for_provider(provider, model_id)?;
        Some(ModelProfileWitness {
            registry_authority: self.authority,
            provider,
            model: model_id.to_string(),
            judged_binding,
            admits_additional_leading_system_messages,
            profile,
            context_window: entry.context_window,
            max_input_tokens: entry.max_input_tokens,
            max_output_tokens: entry.max_output_tokens,
            catalog_capabilities: self
                .catalog_capabilities
                .get(&(provider, model_id.to_string()))
                .copied(),
        })
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

/// Merge user-defined `[models.<id>]` entries into the registry.
///
/// One definition is the single owner for provider inference, compaction
/// scaling (via `context_window`), capability gates, and call timeouts.
/// Capability flags default conservatively (absent unless declared).
fn append_custom_models(
    entries: &mut BTreeMap<String, ModelRegistryEntry>,
    profiles: &mut BTreeMap<(Provider, String), ModelProfile>,
    models: &BTreeMap<String, CustomModelConfig>,
) -> Result<(), ConfigError> {
    for (model_id, model) in models {
        match model.provider {
            Provider::Anthropic | Provider::OpenAI | Provider::Gemini => {}
            Provider::SelfHosted => {
                return Err(ConfigError::Validation(format!(
                    "models.{model_id}: self-hosted models must be declared under \
                     [self_hosted.models], not [models]"
                )));
            }
            Provider::Other => {
                return Err(ConfigError::Validation(format!(
                    "models.{model_id}: provider must be a concrete API provider \
                     (anthropic, openai, gemini)"
                )));
            }
        }

        let vision = model.vision.unwrap_or(false);
        let profile = ModelProfile {
            provider: model.provider,
            release_stage: ModelReleaseStage::OperatorDefined,
            model_family: model_id.clone(),
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_web_search: model.web_search.unwrap_or(false),
            supports_mid_conversation_system_messages: false,
            inline_video: false,
            vision,
            image_input: vision,
            image_tool_results: vision,
            realtime: false,
            image_generation: false,
            params_schema: serde_json::json!({}),
            beta_headers: Vec::new(),
            call_timeout_secs: model.call_timeout_secs,
            stream_inactivity_timeout_secs: None,
        };

        insert_unique(
            entries,
            profiles,
            ModelRegistryEntry {
                id: model_id.clone(),
                display_name: model
                    .display_name
                    .clone()
                    .unwrap_or_else(|| model_id.clone()),
                provider: model.provider,
                tier: ModelTier::Supported,
                release_stage: ModelReleaseStage::OperatorDefined,
                context_window: model.context_window,
                max_input_tokens: model.max_input_tokens,
                max_output_tokens: model.max_output_tokens,
                self_hosted: None,
            },
            profile,
        )?;
    }
    Ok(())
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

    // The self-hosted default is a declared choice owned by the config, never a
    // `BTreeMap` key-order artifact. An explicit `default_model` must reference
    // a configured model; absent that, a single configured model is the
    // unambiguous default, while multiple models require an explicit
    // declaration (fail closed).
    let default_model = match &config.default_model {
        Some(declared) => {
            if !config.models.contains_key(declared) {
                return Err(ConfigError::Validation(format!(
                    "self_hosted.default_model '{declared}' does not reference a configured \
                     self_hosted.models entry"
                )));
            }
            declared.clone()
        }
        None => {
            let mut model_ids = config.models.keys();
            match (model_ids.next(), model_ids.next()) {
                (Some(only), None) => only.clone(),
                (Some(_), Some(_)) => {
                    return Err(ConfigError::MissingField(
                        "self_hosted.default_model (required when more than one self_hosted.models \
                         entry is configured)"
                            .to_string(),
                    ));
                }
                _ => {
                    return Err(ConfigError::InternalError(
                        "self-hosted models unexpectedly empty".to_string(),
                    ));
                }
            }
        }
    };
    defaults.insert(Provider::SelfHosted, default_model);

    for (model_id, model) in &config.models {
        let server = config.servers.get(&model.server).ok_or_else(|| {
            ConfigError::Validation(format!(
                "self_hosted.models.{model_id} references unknown server '{}'",
                model.server
            ))
        })?;
        let min_window = crate::retry::MIN_MODEL_STREAM_INACTIVITY_TIMEOUT.as_secs();
        if let Some(window) = model.stream_inactivity_timeout_secs
            && window < min_window
        {
            return Err(ConfigError::Validation(format!(
                "self_hosted.models.{model_id}.stream_inactivity_timeout_secs is {window}; it must \
                 be at least {min_window}, or the stream-inactivity watchdog races the post-finish \
                 trailer wait of the model's stream"
            )));
        }
        let self_hosted = SelfHostedServerRef {
            server_id: model.server.clone(),
            remote_model: model.remote_model.clone(),
            transport: server.transport,
            api_style: server.api_style,
            base_url: normalize_base_url(&server.base_url),
        };
        let image_tool_results = model.image_tool_results
            && matches!(
                (server.transport, server.api_style),
                (
                    SelfHostedTransport::OpenAiCompatible,
                    SelfHostedApiStyle::Responses
                )
            );
        let profile = ModelProfile {
            provider: Provider::SelfHosted,
            release_stage: ModelReleaseStage::OperatorDefined,
            model_family: model.family.clone(),
            supports_temperature: model.supports_temperature,
            supports_thinking: model.supports_thinking,
            supports_reasoning: model.supports_reasoning,
            supports_web_search: model.supports_web_search,
            supports_mid_conversation_system_messages: false,
            inline_video: model.inline_video,
            vision: model.vision,
            image_input: model.vision,
            image_tool_results,
            realtime: false,
            image_generation: false,
            params_schema: serde_json::json!({}),
            beta_headers: Vec::new(),
            call_timeout_secs: model.call_timeout_secs,
            stream_inactivity_timeout_secs: model.stream_inactivity_timeout_secs,
        };

        insert_unique(
            entries,
            profiles,
            ModelRegistryEntry {
                id: model_id.clone(),
                display_name: model.display_name.clone(),
                provider: Provider::SelfHosted,
                tier: model.tier,
                release_stage: ModelReleaseStage::OperatorDefined,
                context_window: model.context_window,
                max_input_tokens: model.max_input_tokens,
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
        return Err(ConfigError::Validation(format!(
            "model id '{model_id}' must be unique across built-in, custom, and self-hosted entries"
        )));
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
    use crate::model_profile::test_catalog::{
        ANTHROPIC_MODEL, OPENAI_MODEL, TEST_CATALOG, VIDEO_MODEL,
    };

    fn test_catalog() -> ModelCatalog {
        *TEST_CATALOG
    }

    fn config_with_self_hosted() -> Config {
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            "local".to_string(),
            SelfHostedServerConfig {
                transport: SelfHostedTransport::OpenAiCompatible,
                base_url: "http://127.0.0.1:11434".to_string(),
                api_style: SelfHostedApiStyle::Responses,
            },
        );
        config.self_hosted.models.insert(
            "test-self-hosted-large".to_string(),
            SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "test-remote-large".to_string(),
                display_name: "Test Large Model".to_string(),
                family: "test-self-hosted-family".to_string(),
                tier: ModelTier::Supported,
                context_window: Some(256_000),
                max_input_tokens: None,
                max_output_tokens: Some(8_192),
                vision: true,
                image_tool_results: true,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: Some(600),
                stream_inactivity_timeout_secs: None,
            },
        );
        config
    }

    #[test]
    fn merges_self_hosted_models_into_registry() {
        let config = config_with_self_hosted();
        let registry = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        let entry = match registry.entry("test-self-hosted-large") {
            Some(entry) => entry,
            None => panic!("missing self-hosted entry for test-self-hosted-large"),
        };
        assert_eq!(entry.provider, Provider::SelfHosted);
        assert_eq!(entry.display_name, "Test Large Model");
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
            Some("test-remote-large")
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
            Some("test-self-hosted-large")
        );
        let profile =
            match registry.profile_for_provider(Provider::SelfHosted, "test-self-hosted-large") {
                Some(profile) => profile,
                None => panic!("missing self-hosted profile"),
            };
        assert!(
            profile.image_tool_results,
            "Responses-mode self-hosted models should retain configured image tool-result support"
        );
    }

    #[test]
    fn profile_witness_is_minted_only_for_the_exact_provider_model_pair() {
        let registry = ModelRegistry::from_config(&Config::default(), test_catalog())
            .unwrap_or_else(|error| panic!("registry construction failed: {error}"));
        let witness = registry
            .profile_witness_for_provider(Provider::OpenAI, OPENAI_MODEL)
            .unwrap_or_else(|| panic!("missing exact OpenAI profile witness"));
        assert_eq!(witness.provider(), Provider::OpenAI);
        assert_eq!(witness.model(), OPENAI_MODEL);
        assert_eq!(witness.profile().provider, Provider::OpenAI);
        assert!(
            registry
                .profile_witness_for_provider(Provider::Anthropic, OPENAI_MODEL)
                .is_none(),
            "a profile witness cannot be relabelled onto another provider"
        );
        assert!(
            registry
                .profile_witness_for_provider(Provider::OpenAI, ANTHROPIC_MODEL)
                .is_none(),
            "a profile witness cannot be relabelled onto another model"
        );
    }

    #[test]
    fn profile_witness_is_scoped_to_one_registry_authority_not_registry_contents() {
        let config = Config::default();
        let registry = ModelRegistry::from_config(&config, test_catalog())
            .unwrap_or_else(|error| panic!("registry construction failed: {error}"));
        let cloned_registry = registry.clone();
        let independently_constructed = ModelRegistry::from_config(&config, test_catalog())
            .unwrap_or_else(|error| panic!("second registry construction failed: {error}"));
        let witness = cloned_registry
            .profile_witness_for_provider(Provider::OpenAI, OPENAI_MODEL)
            .unwrap_or_else(|| panic!("missing cloned-registry profile witness"));

        assert!(witness.was_minted_by(registry.authority()));
        assert!(
            !format!("{witness:?}").contains(&format!("{:?}", registry.authority())),
            "public witness diagnostics must not reveal the raw registry authority"
        );
        assert_eq!(cloned_registry.authority(), registry.authority());
        assert_ne!(independently_constructed.authority(), registry.authority());
        assert!(
            !witness.was_minted_by(independently_constructed.authority()),
            "identical registry contents must not make independent authorities interchangeable"
        );

        let mut changed_config = config;
        changed_config.models.custom.insert(
            "authority-rotation-probe".to_string(),
            CustomModelConfig {
                provider: Provider::OpenAI,
                display_name: None,
                context_window: Some(32_000),
                max_input_tokens: None,
                max_output_tokens: Some(1024),
                vision: Some(false),
                web_search: Some(false),
                call_timeout_secs: None,
            },
        );
        let rebuilt_after_config_change =
            ModelRegistry::from_config(&changed_config, test_catalog())
                .unwrap_or_else(|error| panic!("changed registry construction failed: {error}"));
        assert!(
            !witness.was_minted_by(rebuilt_after_config_change.authority()),
            "rebuilding after config mutation must rotate registry authority"
        );
    }

    #[test]
    fn self_hosted_chat_completions_downgrades_image_tool_result_support() {
        let mut config = config_with_self_hosted();
        match config.self_hosted.servers.get_mut("local") {
            Some(server) => server.api_style = SelfHostedApiStyle::ChatCompletions,
            None => panic!("missing local self-hosted server"),
        }
        let registry = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        let profile =
            match registry.profile_for_provider(Provider::SelfHosted, "test-self-hosted-large") {
                Some(profile) => profile,
                None => panic!("missing self-hosted profile"),
            };
        assert!(
            !profile.image_tool_results,
            "Chat Completions self-hosted models text-project tool results even when raw config requests image support"
        );
    }

    /// A self-hosted model's own stream-inactivity window must be at least
    /// the minimum (0 included), or it races the post-finish trailer wait;
    /// the minimum itself and longer windows reach the profile.
    #[test]
    fn self_hosted_stream_inactivity_window_below_the_minimum_is_refused() {
        let minimum = crate::retry::MIN_MODEL_STREAM_INACTIVITY_TIMEOUT.as_secs();
        let with_window = |window| {
            let mut config = config_with_self_hosted();
            match config.self_hosted.models.get_mut("test-self-hosted-large") {
                Some(model) => model.stream_inactivity_timeout_secs = Some(window),
                None => panic!("missing self-hosted model"),
            }
            config
        };
        for window in [0, 1, minimum - 1] {
            match ModelRegistry::from_config(&with_window(window), test_catalog()) {
                Err(ConfigError::Validation(message)) => assert!(
                    message.contains("stream_inactivity_timeout_secs")
                        && message.contains(&format!("at least {minimum}")),
                    "{message}"
                ),
                Err(other) => panic!("window {window}: expected a validation error, got {other}"),
                Ok(_) => panic!("window {window} must be refused"),
            }
            match with_window(window).validate(test_catalog()) {
                Err(ConfigError::Validation(message)) => assert!(
                    message.contains("stream_inactivity_timeout_secs"),
                    "{message}"
                ),
                other => panic!("window {window}: config validation must refuse it, got {other:?}"),
            }
        }
        for window in [minimum, 900] {
            if let Err(err) = with_window(window).validate(test_catalog()) {
                panic!("window {window}: config validation refused it: {err}");
            }
            let registry = match ModelRegistry::from_config(&with_window(window), test_catalog()) {
                Ok(registry) => registry,
                Err(err) => panic!("window {window}: {err}"),
            };
            let profile = match registry
                .profile_for_provider(Provider::SelfHosted, "test-self-hosted-large")
            {
                Some(profile) => profile,
                None => panic!("missing self-hosted profile"),
            };
            assert_eq!(profile.stream_inactivity_timeout_secs, Some(window));
        }
    }

    fn insert_second_self_hosted_model(config: &mut Config) {
        config.self_hosted.models.insert(
            "test-self-hosted-small".to_string(),
            SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "test-remote-small".to_string(),
                display_name: "Test Small Model".to_string(),
                family: "test-self-hosted-family".to_string(),
                tier: ModelTier::Supported,
                context_window: Some(128_000),
                max_input_tokens: None,
                max_output_tokens: Some(8_192),
                vision: false,
                image_tool_results: false,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: Some(600),
                stream_inactivity_timeout_secs: None,
            },
        );
    }

    #[test]
    fn multiple_self_hosted_models_require_explicit_default() {
        let mut config = config_with_self_hosted();
        insert_second_self_hosted_model(&mut config);
        // No `default_model` declared: the default must not be silently chosen
        // by `BTreeMap` key order. Fail closed.
        let err = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(_) => panic!("multi-model self-hosted config without explicit default should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("self_hosted.default_model"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn explicit_self_hosted_default_is_honored_not_lexicographic() {
        let mut config = config_with_self_hosted();
        insert_second_self_hosted_model(&mut config);
        // Declare the lexicographically-larger id as the default to prove the
        // choice is declared, not a `.min()` key-order artifact.
        config.self_hosted.default_model = Some("test-self-hosted-small".to_string());
        let registry = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        assert_eq!(
            registry.default_model(Provider::SelfHosted),
            Some("test-self-hosted-small")
        );
    }

    #[test]
    fn explicit_self_hosted_default_must_reference_configured_model() {
        let mut config = config_with_self_hosted();
        insert_second_self_hosted_model(&mut config);
        config.self_hosted.default_model = Some("does-not-exist".to_string());
        let err = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(_) => panic!("default_model referencing an absent model should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("does not reference a configured self_hosted.models entry"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn rejects_unknown_server_reference() {
        let mut config = Config::default();
        config.self_hosted.models.insert(
            "test-self-hosted-large".to_string(),
            SelfHostedModelConfig {
                server: "missing".to_string(),
                remote_model: "test-remote-large".to_string(),
                display_name: "Test Large Model".to_string(),
                family: "test-self-hosted-family".to_string(),
                tier: ModelTier::Supported,
                context_window: None,
                max_input_tokens: None,
                max_output_tokens: None,
                vision: true,
                image_tool_results: true,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: None,
                stream_inactivity_timeout_secs: None,
            },
        );
        let err = match ModelRegistry::from_config(&config, test_catalog()) {
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
            },
        );
        config.self_hosted.models.insert(
            OPENAI_MODEL.to_string(),
            SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "override".to_string(),
                display_name: "Override".to_string(),
                family: "override".to_string(),
                tier: ModelTier::Supported,
                context_window: None,
                max_input_tokens: None,
                max_output_tokens: None,
                vision: false,
                image_tool_results: false,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: false,
                supports_reasoning: false,
                supports_web_search: false,
                call_timeout_secs: None,
                stream_inactivity_timeout_secs: None,
            },
        );
        let err = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(_) => panic!("duplicate model id should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("must be unique"));
    }

    fn custom_model(provider: Provider) -> CustomModelConfig {
        CustomModelConfig {
            provider,
            display_name: Some("Claude Custom".to_string()),
            context_window: Some(500_000),
            max_input_tokens: None,
            max_output_tokens: Some(16_384),
            vision: Some(true),
            web_search: None,
            call_timeout_secs: Some(900),
        }
    }

    #[test]
    fn custom_models_merge_into_registry_with_provider_inference() {
        let mut config = Config::default();
        config.models.custom.insert(
            "custom-internal-preview".to_string(),
            custom_model(Provider::Anthropic),
        );
        let registry = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        // One definition feeds provider inference...
        let entry = registry
            .entry("custom-internal-preview")
            .unwrap_or_else(|| panic!("custom entry must be registered"));
        assert_eq!(entry.provider, Provider::Anthropic);
        assert_eq!(entry.display_name, "Claude Custom");
        // ...compaction scaling inputs...
        assert_eq!(entry.context_window, Some(500_000));
        assert_eq!(entry.max_output_tokens, Some(16_384));
        // ...capability gates and call timeouts via the typed profile owner.
        let profile = registry
            .profile_for_provider(Provider::Anthropic, "custom-internal-preview")
            .unwrap_or_else(|| panic!("custom profile must resolve for declared provider"));
        assert!(profile.vision);
        assert!(profile.image_input);
        assert!(
            !profile.supports_web_search,
            "undeclared capability flags must default conservatively"
        );
        assert_eq!(profile.call_timeout_secs, Some(900));
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, "custom-internal-preview")
                .is_none(),
            "custom capability truth must stay scoped to the declared provider"
        );
    }

    #[test]
    fn extra_custom_models_merge_via_from_config_with_models() {
        let mut extra = BTreeMap::new();
        extra.insert(
            "mob-defined-model".to_string(),
            custom_model(Provider::Gemini),
        );
        let registry = match ModelRegistry::from_config_with_models(
            &Config::default(),
            &extra,
            test_catalog(),
        ) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        assert_eq!(
            registry
                .entry("mob-defined-model")
                .map(|entry| entry.provider),
            Some(Provider::Gemini)
        );
    }

    #[test]
    fn custom_models_reject_catalog_id_conflicts() {
        let mut config = Config::default();
        config
            .models
            .custom
            .insert(OPENAI_MODEL.to_string(), custom_model(Provider::OpenAI));
        let err = match ModelRegistry::from_config(&config, test_catalog()) {
            Ok(_) => panic!("custom entry shadowing a catalog id must fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("must be unique"));
    }

    #[test]
    fn custom_models_reject_self_hosted_and_other_providers() {
        for provider in [Provider::SelfHosted, Provider::Other] {
            let mut config = Config::default();
            config
                .models
                .custom
                .insert("custom-x".to_string(), custom_model(provider));
            let err = match ModelRegistry::from_config(&config, test_catalog()) {
                Ok(_) => panic!("non-concrete custom model provider must fail closed"),
                Err(err) => err,
            };
            assert!(
                err.to_string().contains("models.custom-x"),
                "unexpected error: {err}"
            );
        }
    }

    #[test]
    fn custom_model_toml_ingress_is_fail_closed_on_provider() {
        // `[models.<id>]` parses the provider into the typed closed vocabulary.
        let parsed: Result<crate::config::ModelDefaults, _> = toml::from_str(
            r#"
anthropic = "custom-anthropic-default"

[custom-internal-preview]
provider = "anthropic"
context_window = 500000
"#,
        );
        let defaults = match parsed {
            Ok(defaults) => defaults,
            Err(err) => panic!("custom model table must parse: {err}"),
        };
        assert_eq!(
            defaults
                .custom
                .get("custom-internal-preview")
                .map(|model| model.provider),
            Some(Provider::Anthropic)
        );

        let rejected: Result<crate::config::ModelDefaults, _> = toml::from_str(
            r#"
[custom-internal-preview]
provider = "not-a-provider"
"#,
        );
        assert!(
            rejected.is_err(),
            "unknown provider names must fail closed at config ingress"
        );
    }

    #[test]
    fn uncatalogued_models_do_not_use_provider_prefix_inference() {
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, "test-openai-unknown-preview")
                .is_none()
        );
        assert!(
            registry
                .profile_for_provider(Provider::Anthropic, "test-anthropic-unknown-preview")
                .is_none()
        );
        assert!(
            registry
                .profile_for_provider(Provider::Gemini, "test-gemini-unknown-preview")
                .is_none()
        );
    }

    #[test]
    fn provider_aware_profile_lookup_requires_matching_provider() {
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let profile = registry.profile_for_provider(Provider::OpenAI, OPENAI_MODEL);
        assert_eq!(
            profile.and_then(|profile| profile.call_timeout_secs),
            Some(600)
        );
        assert!(
            registry
                .profile_for_provider(Provider::Anthropic, OPENAI_MODEL)
                .is_none(),
            "provider-aware lookup must not share OpenAI defaults with Anthropic"
        );
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, VIDEO_MODEL)
                .is_none(),
            "provider-aware lookup must not let provider strings select another provider's capabilities"
        );
    }

    #[test]
    fn model_only_entries_are_projection_metadata_not_capability_authority() {
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let entry = match registry.entry(VIDEO_MODEL) {
            Some(entry) => entry,
            None => panic!("catalog entry must exist"),
        };
        assert_eq!(entry.provider, Provider::Gemini);
        assert_eq!(entry.id, VIDEO_MODEL);
        let rendered = format!("{entry:?}");
        assert!(
            !rendered.contains("inline_video") && !rendered.contains("supports_temperature"),
            "model-only projection entry must not expose capability fields: {rendered}"
        );

        let profile = match registry.profile_for_provider(Provider::Gemini, VIDEO_MODEL) {
            Some(profile) => profile,
            None => panic!("typed provider-aware capability lookup should resolve"),
        };
        assert!(profile.inline_video);
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, VIDEO_MODEL)
                .is_none(),
            "display/catalog lookup must not let another typed provider read capability truth"
        );
    }

    #[test]
    fn provider_aware_profile_lookup_fails_closed_for_unknown_pairs() {
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        assert!(
            registry
                .profile_for_provider(Provider::Other, OPENAI_MODEL)
                .is_none(),
            "unknown typed provider must not receive known model defaults"
        );
        assert!(
            registry
                .profile_for_provider(Provider::Other, "uncatalogued-compatible")
                .is_none(),
            "unknown provider/model pairs must fail closed"
        );
        assert!(
            registry
                .profile_for_provider(Provider::OpenAI, "uncatalogued-compatible")
                .is_none(),
            "known provider plus uncatalogued model must fail closed"
        );
    }

    #[test]
    fn inline_video_capability_requires_typed_provider_owner() {
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        if let Err(err) = registry.require_inline_video_for_provider(Provider::Gemini, VIDEO_MODEL)
        {
            panic!("Gemini catalog owner should authorize inline video: {err}");
        }

        let err = match registry.require_inline_video_for_provider(Provider::OpenAI, VIDEO_MODEL) {
            Ok(()) => panic!("same model name under another provider must fail closed"),
            Err(err) => err,
        };
        assert_eq!(err.capability, ModelCapability::InlineVideo);
        assert_eq!(err.provider, Provider::OpenAI);
        assert_eq!(err.model, VIDEO_MODEL);
        assert_eq!(
            err.reason,
            UnsupportedModelCapabilityReason::ProviderModelProfileMissing
        );
    }

    #[test]
    fn inline_video_capability_evidence_distinguishes_disabled_and_unknown() {
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let disabled =
            match registry.require_inline_video_for_provider(Provider::OpenAI, OPENAI_MODEL) {
                Ok(()) => panic!("known OpenAI model has catalog-owned inline video disabled"),
                Err(err) => err,
            };
        assert_eq!(
            disabled.reason,
            UnsupportedModelCapabilityReason::CapabilityDisabled
        );

        let unknown = match registry
            .require_inline_video_for_provider(Provider::Other, "uncatalogued-video-model")
        {
            Ok(()) => panic!("unknown provider/model pair must fail closed"),
            Err(err) => err,
        };
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
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };

        let reason =
            match registry.provider_override_mismatch_reason(Provider::Anthropic, OPENAI_MODEL) {
                Some(reason) => reason,
                None => panic!("wrong-provider override for a catalog model should be rejected"),
            };
        assert!(reason.contains(&format!("model '{OPENAI_MODEL}'")));
        assert!(reason.contains("registered for provider 'openai'"));
        assert!(reason.contains("not provider 'anthropic'"));
        assert!(reason.contains("explicit provider overrides"));

        assert!(
            registry
                .provider_override_mismatch_reason(Provider::OpenAI, OPENAI_MODEL)
                .is_none(),
            "matching provider override should remain valid"
        );
        assert!(
            registry
                .provider_override_mismatch_reason(Provider::OpenAI, "uncatalogued-compatible")
                .is_none(),
            "uncatalogued models have no catalog owner to contradict"
        );
    }

    #[test]
    fn registry_defaults_come_from_injected_catalog() {
        let registry = match ModelRegistry::from_config(&Config::default(), test_catalog()) {
            Ok(registry) => registry,
            Err(err) => panic!("registry construction failed: {err}"),
        };
        assert_eq!(
            registry.default_model(Provider::Anthropic),
            Some(ANTHROPIC_MODEL)
        );
        assert_eq!(registry.default_model(Provider::OpenAI), Some(OPENAI_MODEL));
        assert_eq!(registry.default_model(Provider::Gemini), Some(VIDEO_MODEL));
    }

    /// The test catalog with every row claiming ordered System messages after
    /// the leading prefix, so what a backend takes away from a row shows.
    fn mid_conversation_catalog() -> ModelCatalog {
        use crate::model_profile::capabilities::ModelCapabilities;
        use std::sync::LazyLock;

        static CAPABILITIES: LazyLock<Vec<ModelCapabilities>> = LazyLock::new(|| {
            TEST_CATALOG
                .capabilities
                .iter()
                .map(|capabilities| ModelCapabilities {
                    supports_mid_conversation_system_messages: true,
                    ..*capabilities
                })
                .collect()
        });
        ModelCatalog {
            capabilities: CAPABILITIES.as_slice(),
            ..test_catalog()
        }
    }

    /// A named OpenAI binding whose backend kind this version does not know
    /// does not claim ordered System messages after the leading prefix, as a
    /// named binding the config does not resolve does not: either may be a
    /// backend whose wire refuses them. The public API keeps the row's claim.
    #[test]
    fn an_unknown_openai_backend_kind_claims_no_mid_conversation_system_messages() {
        let mut unknown =
            crate::RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]);
        let Some(backend) = unknown.backend.get_mut("default_openai") else {
            panic!("the inline realm has an OpenAI backend");
        };
        backend.backend_kind = "a_backend_this_version_does_not_know".to_string();
        let mut config = Config::default();
        config.realm.insert("unknown_realm".to_string(), unknown);
        config.realm.insert(
            "public_realm".to_string(),
            crate::RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]),
        );
        let registry = ModelRegistry::from_config(&config, mid_conversation_catalog())
            .unwrap_or_else(|error| panic!("registry construction failed: {error}"));
        let claims = |realm: &'static str| {
            let identity = crate::SessionLlmIdentity {
                model: OPENAI_MODEL.to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: Some(crate::AuthBindingRef {
                    realm: crate::RealmId::from_known_valid(realm),
                    binding: crate::BindingId::from_known_valid("default_openai"),
                    profile: None,
                    origin: crate::BindingOrigin::Configured,
                }),
            };
            registry
                .profile_for_identity(&config, &identity)
                .unwrap_or_else(|| panic!("the test catalog has an OpenAI row"))
                .supports_mid_conversation_system_messages
        };
        assert!(claims("public_realm"), "the public API carries them");
        assert!(
            !claims("missing_realm"),
            "a binding the config does not resolve has an unknown backend"
        );
        assert!(
            !claims("unknown_realm"),
            "an OpenAI backend kind this version does not know fails closed"
        );
    }
}

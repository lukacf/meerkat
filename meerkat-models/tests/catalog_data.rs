//! Data-backed catalog/capability/profile tests.
//!
//! These tests pin the canonical model data (real model IDs, defaults,
//! timeouts, schemas). They moved here from `meerkat-core` when the catalog
//! data was extracted: core owns the vocabulary and mechanics, this crate
//! owns the data.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_core::Provider;
use meerkat_core::model_profile::capabilities::{EffortLevel, ThinkingSupport};
use meerkat_core::model_profile::catalog::ImageGenerationModelRoute;
use meerkat_models::capabilities::{all_capabilities, capabilities_for};
use meerkat_models::catalog::{
    allowed_models, canonical, catalog, catalog_providers, default_image_generation_model,
    default_model, entry_for, global_default_model, image_generation_model,
    image_generation_provider_defaults, image_generation_provider_for_model, infer_provider,
    provider_defaults, provider_names, provider_priority,
};
use meerkat_models::profile::{inline_video_support_for, profile_for};
use std::collections::HashSet;

const IMAGE_DEFAULT_OPENAI: &str = "gpt-image-2";
const IMAGE_DEFAULT_GEMINI: &str = "gemini-3.1-flash-image-preview";

fn provider_from_catalog(provider: &str) -> Provider {
    Provider::parse_strict(provider)
        .unwrap_or_else(|| panic!("catalog provider '{provider}' must parse"))
}

// ---------------------------------------------------------------------------
// Canonical instance coherence
// ---------------------------------------------------------------------------

#[test]
fn canonical_instance_wires_the_same_data_as_the_free_functions() {
    let c = canonical();
    assert_eq!(c.entries.len(), catalog().len());
    assert_eq!(c.providers, catalog_providers());
    assert_eq!(c.provider_priority, provider_priority());
    assert_eq!(c.global_default_model, global_default_model());
    assert_eq!(c.provider_defaults.len(), provider_defaults().len());
    for &provider in catalog_providers() {
        assert_eq!(c.default_model(provider), default_model(provider));
    }
    assert_eq!(c.capabilities.len(), all_capabilities().count());
}

// ---------------------------------------------------------------------------
// Catalog tests (moved from meerkat-core/src/model_profile/catalog.rs)
// ---------------------------------------------------------------------------

#[test]
fn exactly_one_default_per_provider() {
    for &provider in catalog_providers() {
        assert!(
            default_model(provider).is_some(),
            "provider '{provider:?}' must have a default model"
        );
    }
}

#[test]
fn provider_names_is_projection_of_typed_catalog_providers() {
    let projected: Vec<&str> = catalog_providers().iter().map(|p| p.as_str()).collect();
    assert_eq!(
        provider_names(),
        projected.as_slice(),
        "provider_names() must be a divergence-free projection of the typed providers"
    );
}

#[test]
fn defaults_exist_in_catalog() {
    for &provider in catalog_providers() {
        let default = default_model(provider);
        assert!(
            default.is_some(),
            "provider '{provider:?}' must have a default model"
        );
        if let Some(default) = default {
            assert!(
                entry_for(provider, default).is_some(),
                "default model '{default}' for provider '{provider:?}' must exist in catalog"
            );
        }
    }
}

#[test]
fn untyped_providers_fail_closed_at_typed_selection_boundary() {
    assert!(default_model(Provider::Other).is_none());
    assert!(default_model(Provider::SelfHosted).is_none());
    assert!(entry_for(Provider::Other, global_default_model()).is_none());
    assert_eq!(allowed_models(Provider::Other).count(), 0);
}

#[test]
fn all_provider_strings_canonical() {
    let canonical_names: HashSet<&str> = provider_names().iter().copied().collect();
    for entry in catalog() {
        assert!(
            canonical_names.contains(entry.provider),
            "catalog entry '{}' has non-canonical provider '{}'",
            entry.id,
            entry.provider
        );
    }
}

#[test]
fn no_duplicate_model_ids_within_provider() {
    for &provider in provider_names() {
        let ids: Vec<&str> = catalog()
            .iter()
            .filter(|e| e.provider == provider)
            .map(|e| e.id)
            .collect();
        let unique: HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(
            ids.len(),
            unique.len(),
            "provider '{provider}' has duplicate model IDs"
        );
    }
}

#[test]
fn provider_defaults_complete() {
    let defaults = provider_defaults();
    assert_eq!(
        defaults.len(),
        provider_names().len(),
        "provider_defaults() must cover all providers"
    );
    for pd in defaults {
        assert!(
            !pd.models.is_empty(),
            "provider '{}' must have at least one model",
            pd.provider
        );
        let has_default = pd.models.iter().any(|m| m.id == pd.default_model_id);
        assert!(
            has_default,
            "default model '{}' for provider '{}' must be in the models list",
            pd.default_model_id, pd.provider
        );
    }
}

#[test]
fn allowed_models_matches_catalog() {
    for &provider in catalog_providers() {
        let allowed: Vec<&str> = allowed_models(provider).collect();
        let from_catalog: Vec<&str> = catalog()
            .iter()
            .filter(|e| e.provider == provider.as_str())
            .map(|e| e.id)
            .collect();
        assert_eq!(
            allowed, from_catalog,
            "allowed_models({provider:?}) must match catalog entries"
        );
    }
}

#[test]
fn catalog_matches_capability_table() {
    for entry in catalog() {
        let provider = provider_from_catalog(entry.provider);
        let caps =
            capabilities_for(provider, entry.id).expect("catalog entry must have a capability row");
        assert_eq!(entry.id, caps.id);
        assert_eq!(entry.provider, caps.provider.as_str());
        assert_eq!(entry.display_name, caps.display_name);
        assert_eq!(entry.tier, caps.tier);
        assert_eq!(entry.context_window, Some(caps.context_window));
        assert_eq!(entry.max_output_tokens, Some(caps.max_output_tokens));
    }
}

#[test]
fn claude_fable_5_in_catalog_without_changing_default_ladder() {
    let entry = entry_for(Provider::Anthropic, "claude-fable-5")
        .expect("claude-fable-5 must be in the catalog");
    assert_eq!(entry.provider, "anthropic");
    assert_eq!(entry.display_name, "Claude Fable 5");
    assert_eq!(entry.context_window, Some(1_000_000));
    assert_eq!(entry.max_output_tokens, Some(128_000));
    assert!(
        allowed_models(Provider::Anthropic).any(|id| id == "claude-fable-5"),
        "claude-fable-5 must be in the Anthropic allowlist"
    );
    // The Anthropic default remains Opus 4.8 even though Fable 5 is the more
    // capable (and more expensive) model. The cross-provider global default
    // is independently owned by the OpenAI recommendation.
    assert_eq!(default_model(Provider::Anthropic), Some("claude-opus-4-8"));
    assert_eq!(global_default_model(), "gpt-5.6-sol");
}

#[test]
fn gpt_56_family_is_cataloged_and_sol_owns_the_openai_default() {
    for (id, display_name) in [
        ("gpt-5.6-sol", "GPT-5.6 Sol"),
        ("gpt-5.6-terra", "GPT-5.6 Terra"),
        ("gpt-5.6-luna", "GPT-5.6 Luna"),
        ("gpt-5.6", "GPT-5.6 Sol"),
    ] {
        let entry = entry_for(Provider::OpenAI, id)
            .unwrap_or_else(|| panic!("{id} must be in the catalog"));
        assert_eq!(entry.provider, "openai");
        assert_eq!(entry.display_name, display_name);
        assert_eq!(
            entry.tier,
            meerkat_core::model_profile::catalog::ModelTier::Supported
        );
        assert_eq!(entry.context_window, Some(1_050_000));
        assert_eq!(entry.max_output_tokens, Some(128_000));
        assert!(allowed_models(Provider::OpenAI).any(|model| model == id));
        assert_eq!(infer_provider(id), Some(Provider::OpenAI));

        let profile = profile_for(Provider::OpenAI, id)
            .unwrap_or_else(|| panic!("{id} must have a model profile"));
        assert_eq!(profile.model_family, "gpt-5");
        assert!(profile.vision);
        assert!(profile.image_input);
        assert!(profile.image_tool_results);
        assert!(profile.image_generation);
        assert!(profile.supports_reasoning);
        assert!(profile.supports_web_search);
        assert!(!profile.inline_video);
        assert!(!profile.realtime);
        assert!(!profile.supports_temperature);
    }

    assert_eq!(default_model(Provider::OpenAI), Some("gpt-5.6-sol"));
    assert_eq!(global_default_model(), "gpt-5.6-sol");
}

#[test]
fn gpt_55_rows_remain_broadly_available_production_recommendations() {
    for id in ["gpt-5.5", "gpt-5.5-pro"] {
        let entry = entry_for(Provider::OpenAI, id)
            .unwrap_or_else(|| panic!("{id} must remain in the catalog"));
        assert_eq!(
            entry.tier,
            meerkat_core::model_profile::catalog::ModelTier::Recommended
        );
    }
}

#[test]
fn image_generation_defaults_are_catalog_owned_and_typed() {
    let openai = default_image_generation_model(Provider::OpenAI)
        .expect("OpenAI must have an image-generation default");
    assert_eq!(openai.model_id, IMAGE_DEFAULT_OPENAI);
    assert_eq!(openai.provider, Provider::OpenAI);
    assert!(matches!(
        openai.route,
        ImageGenerationModelRoute::OpenAiHostedResponsesTool { .. }
    ));

    let gemini = default_image_generation_model(Provider::Gemini)
        .expect("Gemini must have an image-generation default");
    assert_eq!(gemini.model_id, IMAGE_DEFAULT_GEMINI);
    assert_eq!(gemini.provider, Provider::Gemini);
    assert!(matches!(
        gemini.route,
        ImageGenerationModelRoute::GeminiNativeModel { .. }
    ));

    assert!(default_image_generation_model(Provider::Anthropic).is_none());
    assert!(default_image_generation_model(Provider::Other).is_none());
}

#[test]
fn image_generation_lookup_fails_closed_for_unknown_or_mismatched_models() {
    assert!(image_generation_model(Provider::Gemini, "gemini-unknown-image-preview").is_none());
    assert!(image_generation_model(Provider::OpenAI, "gpt-image-unknown").is_none());
    assert!(image_generation_model(Provider::Gemini, IMAGE_DEFAULT_OPENAI).is_none());
    assert!(image_generation_model(Provider::OpenAI, IMAGE_DEFAULT_GEMINI).is_none());
    assert!(image_generation_model(Provider::Other, IMAGE_DEFAULT_OPENAI).is_none());
}

#[test]
fn image_generation_provider_inference_is_catalog_only() {
    assert_eq!(
        image_generation_provider_for_model(IMAGE_DEFAULT_OPENAI),
        Some(Provider::OpenAI)
    );
    assert_eq!(
        image_generation_provider_for_model(IMAGE_DEFAULT_GEMINI),
        Some(Provider::Gemini)
    );
    assert_eq!(
        image_generation_provider_for_model("gpt-5.4"),
        Some(Provider::OpenAI)
    );
    assert_eq!(
        image_generation_provider_for_model("gpt-image-future"),
        None
    );
    assert_eq!(
        image_generation_provider_for_model("gemini-3.5-flash"),
        None,
        "Gemini text catalog rows are not image-generation model authority"
    );
}

#[test]
fn openai_text_models_route_through_hosted_image_tool() {
    let via_text = image_generation_model(Provider::OpenAI, "gpt-5.4")
        .expect("OpenAI text catalog model admitted through hosted tool");
    assert!(matches!(
        via_text.route,
        ImageGenerationModelRoute::OpenAiHostedResponsesTool {
            provider_call_model_id: None,
            tool_model_id: IMAGE_DEFAULT_OPENAI,
        }
    ));
}

#[test]
fn global_default_and_provider_priority_are_catalog_owned() {
    assert_eq!(global_default_model(), "gpt-5.6-sol");

    let priority = provider_priority();
    assert_eq!(
        priority.len(),
        3,
        "provider priority must cover three providers"
    );
    assert_eq!(
        priority.first(),
        Some(&Provider::Anthropic),
        "Anthropic must be the highest-priority provider"
    );
    assert!(priority.contains(&Provider::OpenAI));
    assert!(priority.contains(&Provider::Gemini));
}

#[test]
fn image_generation_provider_defaults_are_complete() {
    let defaults = image_generation_provider_defaults();
    assert_eq!(defaults.len(), 2);
    for defaults in defaults {
        let default_profile = default_image_generation_model(defaults.provider)
            .expect("provider default must resolve");
        assert_eq!(defaults.default_model_id, default_profile.model_id);
        assert!(
            defaults
                .models
                .iter()
                .any(|model| model.model_id == defaults.default_model_id),
            "image-generation default must be present in provider models"
        );
    }
}

#[test]
fn infer_provider_is_exact_catalog_match_only() {
    assert_eq!(infer_provider("claude-opus-4-8"), Some(Provider::Anthropic));
    assert_eq!(
        infer_provider("claude-haiku-4-5-20251001"),
        Some(Provider::Anthropic)
    );
    assert_eq!(infer_provider("gpt-5.4"), Some(Provider::OpenAI));
    assert_eq!(infer_provider("gpt-5.4-mini"), Some(Provider::OpenAI));
    assert_eq!(infer_provider("gemini-3.5-flash"), Some(Provider::Gemini));
    assert_eq!(infer_provider("gpt-unknown-preview"), None);
    assert_eq!(infer_provider("claude-unknown-preview"), None);
    assert_eq!(infer_provider("gemini-unknown-preview"), None);
    assert_eq!(infer_provider("llama-3"), None);
    assert_eq!(infer_provider(""), None);
}

// ---------------------------------------------------------------------------
// Capability tests (moved from meerkat-core/src/model_profile/capabilities.rs)
// ---------------------------------------------------------------------------

#[test]
fn every_capability_matches_a_catalog_entry() {
    for caps in all_capabilities() {
        assert!(
            entry_for(caps.provider, caps.id).is_some(),
            "capability row '{}' (provider '{}') has no catalog entry",
            caps.id,
            caps.provider.as_str(),
        );
    }
}

#[test]
fn no_duplicate_capability_ids_within_provider() {
    for provider_name in provider_names() {
        let provider = provider_from_catalog(provider_name);
        let ids: Vec<&str> = all_capabilities()
            .filter(|c| c.provider == provider)
            .map(|c| c.id)
            .collect();
        let mut unique: Vec<&str> = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "duplicate ids in {provider_name}");
    }
}

#[test]
fn every_catalog_entry_has_capabilities() {
    for entry in catalog() {
        let provider = provider_from_catalog(entry.provider);
        assert!(
            capabilities_for(provider, entry.id).is_some(),
            "catalog model '{}' (provider '{}') has no capability row",
            entry.id,
            entry.provider,
        );
    }
}

#[test]
fn tier_matches_catalog_entry() {
    for caps in all_capabilities() {
        let entry = entry_for(caps.provider, caps.id)
            .unwrap_or_else(|| panic!("missing catalog entry for {}", caps.id));
        assert_eq!(caps.tier, entry.tier, "tier mismatch for {}", caps.id);
    }
}

#[test]
fn claude_haiku_45_is_cataloged_with_official_limits() {
    for model in ["claude-haiku-4-5-20251001", "claude-haiku-4-5"] {
        let caps = capabilities_for(Provider::Anthropic, model)
            .unwrap_or_else(|| panic!("{model} must be in the Anthropic catalog"));
        assert_eq!(caps.model_family, "claude-haiku-4");
        assert_eq!(caps.context_window, 200_000);
        assert_eq!(caps.max_output_tokens, 64_000);
        assert_eq!(caps.thinking, ThinkingSupport::AnthropicEnabledOnly);
        assert!(!caps.supports_compaction);
    }
}

#[test]
fn claude_fable_5_is_cataloged_with_official_limits() {
    let caps = capabilities_for(Provider::Anthropic, "claude-fable-5")
        .expect("claude-fable-5 must be in the Anthropic catalog");
    assert_eq!(caps.provider, Provider::Anthropic);
    assert_eq!(caps.model_family, "claude-fable-5");
    assert_eq!(caps.context_window, 1_000_000);
    assert_eq!(caps.max_output_tokens, 128_000);
    assert!(
        caps.max_output_tokens_beta.is_none(),
        "Fable 5 is not listed for the output-300k batch beta"
    );
    assert_eq!(caps.thinking, ThinkingSupport::AnthropicAdaptiveOnly);
    assert!(
        !caps.supports_temperature && !caps.supports_top_p && !caps.supports_top_k,
        "Fable 5 rejects all sampling parameters"
    );
    assert!(
        !caps.supports_thinking_budget_legacy,
        "budget_tokens is fully removed on Fable 5"
    );
    assert!(caps.vision);
    assert!(caps.supports_compaction);
    assert!(caps.supports_structured_output);
    assert!(caps.supports_web_search);
    assert!(caps.effort_levels.contains(&EffortLevel::Xhigh));
    assert!(caps.effort_levels.contains(&EffortLevel::Max));
}

#[test]
fn typed_provider_mismatch_fails_closed() {
    assert!(capabilities_for(Provider::Anthropic, "gpt-5.4").is_none());
    assert!(capabilities_for(Provider::OpenAI, "gemini-3.5-flash").is_none());
    assert!(capabilities_for(Provider::Other, "gpt-5.4").is_none());
}

#[test]
fn display_provider_string_cannot_be_promoted_to_capability_owner() {
    let display_provider = Provider::parse_strict("Gemini").unwrap_or(Provider::Other);
    assert_eq!(display_provider, Provider::Other);
    assert!(
        capabilities_for(display_provider, "gemini-3.5-flash").is_none(),
        "display provider strings must fail closed at the typed capability boundary"
    );
}

// ---------------------------------------------------------------------------
// Profile tests (moved from meerkat-core/src/model_profile/mod.rs)
// ---------------------------------------------------------------------------

#[test]
fn profile_for_all_catalog_models() {
    for entry in catalog() {
        let profile = profile_for(provider_from_catalog(entry.provider), entry.id);
        assert!(
            profile.is_some(),
            "catalog model '{}' (provider '{}') must have a profile",
            entry.id,
            entry.provider
        );
    }
}

#[test]
fn unknown_provider_returns_none() {
    assert!(profile_for(Provider::Other, "some-model").is_none());
}

#[test]
fn uncatalogued_model_returns_none_for_known_provider() {
    assert!(profile_for(Provider::OpenAI, "gpt-5.9-future").is_none());
    assert!(profile_for(Provider::Anthropic, "claude-opus-4-8-20260501-preview").is_none());
    assert!(profile_for(Provider::Gemini, "gemini-4-future").is_none());
}

#[test]
fn wrong_typed_provider_for_known_model_returns_none() {
    assert!(profile_for(Provider::Anthropic, "gpt-5.4").is_none());
    assert!(profile_for(Provider::OpenAI, "gemini-3.5-flash").is_none());
}

#[test]
fn unknown_provider_model_pairs_fail_closed_without_defaults() {
    assert!(profile_for(Provider::Other, "gpt-5.4").is_none());
    assert!(profile_for(Provider::Other, "uncatalogued-gpt-compatible").is_none());
    assert!(inline_video_support_for(Provider::Other, "gemini-3.5-flash").is_none());
}

#[test]
fn claude_profile_vision_and_image_tool_results_true() {
    let profile = profile_for(Provider::Anthropic, "claude-opus-4-8")
        .expect("claude-opus-4-8 must have a profile");
    assert!(profile.vision, "Anthropic models must support vision");
    assert!(
        profile.image_tool_results,
        "Anthropic models must support image tool results"
    );
    assert!(
        !profile.inline_video,
        "Anthropic models must NOT support inline video"
    );

    let profile = profile_for(Provider::Anthropic, "claude-sonnet-4-5")
        .expect("claude-sonnet-4-5 must have a profile");
    assert!(profile.vision);
    assert!(profile.image_tool_results);
}

#[test]
fn gpt_profile_vision_and_image_tool_results_true() {
    let profile = profile_for(Provider::OpenAI, "gpt-5.4").expect("gpt-5.4 must have a profile");
    assert!(profile.vision, "OpenAI models must support vision");
    assert!(
        profile.image_tool_results,
        "OpenAI Responses models must support image tool results"
    );
    assert!(
        !profile.inline_video,
        "OpenAI models must NOT support inline video"
    );

    let pro =
        profile_for(Provider::OpenAI, "gpt-5.5-pro").expect("gpt-5.5-pro must have a profile");
    assert!(pro.image_tool_results);
    let realtime = profile_for(Provider::OpenAI, "gpt-realtime-2")
        .expect("gpt-realtime-2 must have a profile");
    assert!(
        !realtime.image_tool_results,
        "OpenAI realtime uses a separate tool-result wire path"
    );
}

#[test]
fn gemini_profile_vision_and_image_tool_results_true() {
    let profile = profile_for(Provider::Gemini, "gemini-3.5-flash")
        .expect("gemini-3.5-flash must have a profile");
    assert!(profile.vision, "Gemini models must support vision");
    assert!(
        profile.image_tool_results,
        "Gemini models must support image tool results"
    );
    assert!(
        profile.inline_video,
        "Gemini models must support inline video"
    );
}

#[test]
fn all_gemini_profiles_preserve_inline_video_support() {
    for entry in catalog().iter().filter(|entry| entry.provider == "gemini") {
        assert!(
            profile_for(provider_from_catalog(entry.provider), entry.id)
                .as_ref()
                .is_some_and(|profile| profile.inline_video),
            "Gemini model '{}' must support inline video",
            entry.id
        );
    }
}

#[test]
fn inline_video_support_for_reads_capability_truth() {
    assert_eq!(
        inline_video_support_for(Provider::Gemini, "gemini-3.5-flash"),
        Some(true)
    );
    assert_eq!(
        inline_video_support_for(Provider::OpenAI, "gpt-5.4"),
        Some(false)
    );
    assert_eq!(
        inline_video_support_for(Provider::Gemini, "gemini-4-future"),
        None
    );
}

#[test]
fn params_schema_non_empty_for_all_profiles() {
    for entry in catalog() {
        let profile = profile_for(provider_from_catalog(entry.provider), entry.id);
        if let Some(p) = profile {
            assert!(
                p.params_schema.is_object(),
                "params_schema for '{}' must be a JSON object, got {:?}",
                entry.id,
                p.params_schema
            );
        }
    }
}

#[test]
fn call_timeout_secs_populated_for_known_models() {
    for entry in catalog() {
        let profile = profile_for(provider_from_catalog(entry.provider), entry.id);
        if let Some(p) = profile {
            assert!(
                p.call_timeout_secs.is_some(),
                "catalog model '{}' (provider '{}', family '{}') must have call_timeout_secs",
                entry.id,
                entry.provider,
                p.model_family
            );
        }
    }
}

#[test]
fn anthropic_opus_has_longer_timeout_than_haiku() {
    let opus = profile_for(Provider::Anthropic, "claude-opus-4-8").unwrap();
    let haiku = profile_for(Provider::Anthropic, "claude-haiku-4-5-20251001").unwrap();
    assert!(
        opus.call_timeout_secs.unwrap() > haiku.call_timeout_secs.unwrap(),
        "Opus should have a longer default timeout than Haiku"
    );
}

#[test]
fn openai_pro_has_longer_timeout_than_standard_gpt5() {
    let pro = profile_for(Provider::OpenAI, "gpt-5.5-pro").unwrap();
    let standard = profile_for(Provider::OpenAI, "gpt-5.5").unwrap();
    assert!(
        pro.call_timeout_secs.unwrap() > standard.call_timeout_secs.unwrap(),
        "gpt-5.5-pro ({}) should have a much longer timeout than gpt-5.5 ({})",
        pro.call_timeout_secs.unwrap(),
        standard.call_timeout_secs.unwrap(),
    );
}

#[test]
fn gemini_flash_has_shorter_timeout_than_pro() {
    let flash = profile_for(Provider::Gemini, "gemini-3.1-flash-lite-preview").unwrap();
    let pro = profile_for(Provider::Gemini, "gemini-3.1-pro-preview").unwrap();
    assert!(
        flash.call_timeout_secs.unwrap() < pro.call_timeout_secs.unwrap(),
        "gemini flash ({}) should have shorter timeout than gemini pro ({})",
        flash.call_timeout_secs.unwrap(),
        pro.call_timeout_secs.unwrap(),
    );
}

/// Gate (row #92): `image_generation` is a model-owned catalog fact, not a
/// per-provider derivation. A Gemini text model's provider (Gemini) HAS an
/// image-generation default model, yet the text row itself declares
/// `image_generation = false` and the projected profile must report false —
/// proving the fact comes from the model row, not a provider lookup.
#[test]
fn image_generation_is_model_owned_not_provider_derived() {
    assert!(
        default_image_generation_model(Provider::Gemini).is_some(),
        "Gemini provider must have an image-generation default model"
    );
    let gemini_text = profile_for(Provider::Gemini, "gemini-3.5-flash")
        .expect("gemini-3.5-flash must have a profile");
    assert!(
        !gemini_text.image_generation,
        "Gemini text model must report image_generation=false despite the \
         provider having an image-gen default"
    );

    // OpenAI text rows route through the hosted image tool: model-owned true.
    let gpt = profile_for(Provider::OpenAI, "gpt-5.4").expect("gpt-5.4 must have a profile");
    assert!(gpt.image_generation);

    // Anthropic has no image-gen route: model-owned false.
    let claude = profile_for(Provider::Anthropic, "claude-opus-4-8")
        .expect("claude-opus-4-8 must have a profile");
    assert!(!claude.image_generation);
}

#[test]
fn web_search_supported_across_default_models() {
    for (provider, model) in [
        (Provider::Anthropic, "claude-opus-4-8"),
        (Provider::OpenAI, "gpt-5.4"),
        (Provider::Gemini, "gemini-3.5-flash"),
    ] {
        let profile = profile_for(provider, model).unwrap();
        assert!(
            profile.supports_web_search,
            "{model} must support web search"
        );
    }
}

// ---------------------------------------------------------------------------
// Schema tests over real rows (moved from
// meerkat-core/src/model_profile/schema_builder.rs)
// ---------------------------------------------------------------------------

mod schema {
    use super::*;
    use meerkat_core::model_profile::schema_builder::build_params_schema;
    use serde_json::Value;

    fn property_keys(schema: &Value) -> std::collections::BTreeSet<String> {
        schema
            .get("properties")
            .and_then(|p| p.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn enum_values_for(schema: &Value, prop: &str) -> Option<std::collections::BTreeSet<String>> {
        let val = schema
            .get("properties")
            .and_then(|p| p.get(prop))
            .and_then(|v| v.get("enum"))
            .and_then(|e| e.as_array())?;
        Some(
            val.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
        )
    }

    #[test]
    fn builder_emits_properties_for_every_catalog_model() {
        for caps in all_capabilities() {
            let schema = build_params_schema(caps);
            assert!(
                schema.is_object(),
                "schema for {} must be a JSON object",
                caps.id
            );
            assert_eq!(
                schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "schema for {} must be type=object",
                caps.id
            );
            assert!(
                schema.get("properties").is_some(),
                "schema for {} must have a properties map",
                caps.id
            );
        }
    }

    /// The profile's advertised params schema is exactly the builder output
    /// for the model's capability row.
    #[test]
    fn profile_params_schema_matches_builder_output() {
        for caps in all_capabilities() {
            let built = build_params_schema(caps);
            let profile_schema = profile_for(caps.provider, caps.id)
                .unwrap_or_else(|| panic!("no profile for {}", caps.id))
                .params_schema;
            assert_eq!(built, profile_schema, "schema mismatch for {}", caps.id);
        }
    }

    /// Gate (row #30): the advertised effort/reasoning_effort enum values are
    /// projected from the typed `EffortLevel` vocabulary, never from string
    /// literals. Every emitted value must round-trip back to a typed level.
    #[test]
    fn effort_enum_values_derive_from_typed_effort_levels() {
        let typed_wire = |s: &str| -> bool {
            [
                EffortLevel::None,
                EffortLevel::Minimal,
                EffortLevel::Low,
                EffortLevel::Medium,
                EffortLevel::High,
                EffortLevel::Xhigh,
                EffortLevel::Max,
            ]
            .iter()
            .any(|level| level.as_wire_str() == s)
        };

        for caps in all_capabilities() {
            let schema = build_params_schema(caps);
            let declared: std::collections::BTreeSet<String> = caps
                .effort_levels
                .iter()
                .map(|level| level.as_wire_str().to_string())
                .collect();

            for prop in ["effort", "reasoning_effort"] {
                if let Some(values) = enum_values_for(&schema, prop) {
                    for v in &values {
                        assert!(
                            typed_wire(v),
                            "{}.{} emitted '{}' which is not a typed EffortLevel wire value",
                            caps.id,
                            prop,
                            v
                        );
                    }
                    assert_eq!(
                        values, declared,
                        "{}.{} enum must equal the catalog's typed effort_levels",
                        caps.id, prop
                    );
                }
            }
        }
    }

    #[test]
    fn opus_48_effort_includes_xhigh() {
        let caps = capabilities_for(Provider::Anthropic, "claude-opus-4-8").expect("opus 4.8 row");
        let schema = build_params_schema(caps);
        let values = enum_values_for(&schema, "effort").expect("effort enum");
        assert!(values.contains("xhigh"), "opus 4.8 must advertise xhigh");
        assert!(values.contains("low"));
        assert!(values.contains("max"));
    }

    #[test]
    fn gpt_56_family_advertises_api_max_effort_not_codex_ultra_mode() {
        for id in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.6"] {
            let caps = capabilities_for(Provider::OpenAI, id)
                .unwrap_or_else(|| panic!("{id} capability row"));
            let schema = build_params_schema(caps);
            let values = enum_values_for(&schema, "reasoning_effort")
                .unwrap_or_else(|| panic!("{id} reasoning_effort enum"));

            assert_eq!(
                values,
                ["none", "low", "medium", "high", "xhigh", "max"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                "{id} must advertise the model API effort ladder"
            );
            assert!(
                !values.contains("ultra"),
                "Codex ultra is multi-agent orchestration, not reasoning.effort"
            );

            let expected = |values: &[&str]| {
                values
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<std::collections::BTreeSet<_>>()
            };
            assert_eq!(
                enum_values_for(&schema, "reasoning_mode").expect("reasoning_mode enum"),
                expected(&["standard", "pro"])
            );
            assert_eq!(
                enum_values_for(&schema, "reasoning_context").expect("reasoning_context enum"),
                expected(&["auto", "current_turn", "all_turns"])
            );
            assert_eq!(
                enum_values_for(&schema, "text_verbosity").expect("text_verbosity enum"),
                expected(&["low", "medium", "high"])
            );

            let cache = schema
                .get("properties")
                .and_then(|properties| properties.get("prompt_cache_options"))
                .unwrap_or_else(|| panic!("{id} prompt_cache_options schema"));
            let cache_enum = |name: &str| {
                cache
                    .get("properties")
                    .and_then(|properties| properties.get(name))
                    .and_then(|property| property.get("enum"))
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| panic!("{id} prompt_cache_options.{name} enum"))
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<std::collections::BTreeSet<_>>()
            };
            assert_eq!(cache_enum("mode"), expected(&["implicit", "explicit"]));
            assert_eq!(cache_enum("ttl"), expected(&["30m"]));
            assert!(cache.get("minProperties").is_none());

            let keys = property_keys(&schema);
            for unsupported in ["seed", "frequency_penalty", "presence_penalty"] {
                assert!(
                    !keys.contains(unsupported),
                    "{id} Responses metadata must not advertise {unsupported}"
                );
            }
        }
    }

    #[test]
    fn fable_5_schema_is_adaptive_only_with_full_effort_ladder() {
        let caps = capabilities_for(Provider::Anthropic, "claude-fable-5").expect("fable 5 row");
        let schema = build_params_schema(caps);

        let keys = property_keys(&schema);
        assert!(
            !keys.contains("thinking_budget"),
            "fable 5 must not advertise the legacy thinking budget"
        );

        let values = enum_values_for(&schema, "effort").expect("effort enum");
        assert!(values.contains("xhigh"), "fable 5 must advertise xhigh");
        assert!(values.contains("max"), "fable 5 must advertise max");

        // The thinking schema admits only {"type": "adaptive"} — Fable 5
        // rejects both enabled and explicit disabled modes.
        let thinking_type = schema
            .get("properties")
            .and_then(|p| p.get("thinking"))
            .and_then(|t| t.get("properties"))
            .and_then(|p| p.get("type"))
            .and_then(|t| t.get("enum"))
            .expect("fable 5 thinking type enum");
        assert_eq!(thinking_type, &serde_json::json!(["adaptive"]));
    }

    #[test]
    fn sonnet_45_has_no_effort_property() {
        let caps =
            capabilities_for(Provider::Anthropic, "claude-sonnet-4-5").expect("sonnet 4.5 row");
        let schema = build_params_schema(caps);
        assert!(
            !property_keys(&schema).contains("effort"),
            "sonnet 4.5 must not advertise effort"
        );
    }

    #[test]
    fn gemini_3_schema_exposes_thinking_level() {
        let caps =
            capabilities_for(Provider::Gemini, "gemini-3.5-flash").expect("gemini 3 flash row");
        let schema = build_params_schema(caps);
        assert!(
            property_keys(&schema).contains("thinking_level"),
            "gemini 3 must advertise thinking_level"
        );

        let values = enum_values_for(&schema, "thinking_level").expect("thinking_level enum");
        let expected: std::collections::BTreeSet<String> = ["high", "low", "medium", "minimal"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn gemini_schema_has_no_include_thoughts() {
        for caps in all_capabilities().filter(|c| c.provider == Provider::Gemini) {
            let schema = build_params_schema(caps);
            assert!(
                !property_keys(&schema).contains("include_thoughts"),
                "gemini {} must not advertise include_thoughts (client ignores it)",
                caps.id,
            );
            let thinking = schema.get("properties").and_then(|p| p.get("thinking"));
            if let Some(inner_props) = thinking.and_then(|t| t.get("properties")) {
                let obj = inner_props.as_object().expect("inner properties");
                assert!(
                    !obj.contains_key("include_thoughts"),
                    "gemini {} must not advertise thinking.include_thoughts",
                    caps.id,
                );
            }
        }
    }
}

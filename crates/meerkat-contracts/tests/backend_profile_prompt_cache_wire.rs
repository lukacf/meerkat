//! `WireBackendProfile.prompt_cache_applicable` tells a host whether Meerkat's
//! OpenAI prompt-cache fields apply on a backend (#1781). It is read from the
//! same owner the OpenAI client applies on the wire
//! (`OpenAiBackendKind::admits_prompt_cache_fields`), so a host never infers
//! it from a backend-kind string.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_contracts::WireBackendProfile;
use meerkat_core::{BackendProfile, Provider};

fn backend(provider: Provider, backend_kind: &str) -> BackendProfile {
    BackendProfile {
        id: format!("{}-{backend_kind}", provider.as_str()),
        provider,
        backend_kind: backend_kind.to_string(),
        base_url: None,
        options: serde_json::Value::Null,
        server: None,
    }
}

#[test]
fn the_public_openai_api_backend_reports_prompt_cache_applicable() {
    let wire = WireBackendProfile::from(&backend(Provider::OpenAI, "openai_api"));
    assert_eq!(wire.prompt_cache_applicable, Some(true));
}

#[test]
fn chatgpt_and_azure_openai_backends_report_prompt_cache_not_applicable() {
    for kind in ["chatgpt_backend", "azure_openai"] {
        let wire = WireBackendProfile::from(&backend(Provider::OpenAI, kind));
        assert_eq!(wire.prompt_cache_applicable, Some(false), "{kind}");
    }
}

#[test]
fn other_providers_omit_prompt_cache_applicable() {
    for (provider, kind) in [
        (Provider::Anthropic, "anthropic_api"),
        (Provider::Gemini, "gemini_api"),
    ] {
        let wire = WireBackendProfile::from(&backend(provider, kind));
        assert_eq!(wire.prompt_cache_applicable, None, "{}", provider.as_str());
    }
}

#[test]
fn the_field_is_omitted_on_the_wire_for_other_providers_and_round_trips() {
    let anthropic = WireBackendProfile::from(&backend(Provider::Anthropic, "anthropic_api"));
    let value = serde_json::to_value(&anthropic).unwrap();
    assert!(
        value.get("prompt_cache_applicable").is_none(),
        "a non-OpenAI backend serializes no prompt_cache_applicable: {value}"
    );
    let back: WireBackendProfile = serde_json::from_value(value).unwrap();
    assert_eq!(back, anthropic);

    let openai = WireBackendProfile::from(&backend(Provider::OpenAI, "chatgpt_backend"));
    let value = serde_json::to_value(&openai).unwrap();
    assert_eq!(value["prompt_cache_applicable"], serde_json::json!(false));
    let back: WireBackendProfile = serde_json::from_value(value).unwrap();
    assert_eq!(back, openai);

    // A payload from a server that predates the field still decodes.
    let legacy = serde_json::json!({
        "id": "openai-openai_api",
        "provider": "openai",
        "backend_kind": "openai_api"
    });
    let decoded: WireBackendProfile = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.prompt_cache_applicable, None);
}

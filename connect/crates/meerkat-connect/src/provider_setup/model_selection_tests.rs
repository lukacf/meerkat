//! Resolver-level controls for the request reused by standalone continuation.
//! No runtime, session, credential store, provider or service is opened here.

use super::*;
use meerkat_core::{Config, Provider, RealmId};

fn model_config(model: &str) -> Config {
    Config::from_persisted_toml(&format!("[agent]\nmodel = {model:?}\n"))
        .unwrap().0
}

fn binding_config(model: &str, binding: &str) -> Config {
    Config::from_persisted_toml(&format!(r#"
[agent]
model = "gpt-6-astra"
[realm.owner]
default_binding = "{binding}"
[realm.owner.backend.{binding}]
provider = "openai"
backend_kind = "openai_api"
[realm.owner.auth.{binding}]
provider = "openai"
auth_method = "api_key"
source = {{kind = "managed_store"}}
[realm.owner.binding.{binding}]
backend_profile = "{binding}"
auth_profile = "{binding}"
default_model = "{model}"
"#)).unwrap().0
}

fn binding(name: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("owner").unwrap(),
        binding: BindingId::parse(name).unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

#[test]
fn continuation_request_keeps_initial_model_provider_after_defaults_change() {
    let initial = model_config("gpt-6-astra");
    let refreshed = model_config("claude-sonnet-4-5");
    let unresolved = CreateSessionModelResolutionRequest::default();
    let initial_choice = resolve_create_session_model(&initial, unresolved.clone()).unwrap();
    let retained = retain_model_selection(&initial, unresolved.clone()).unwrap();

    // This is the old main.rs behavior: resolve the original default request
    // again after finalization has refreshed the realm's configuration.
    let drifted = resolve_create_session_model(&refreshed, unresolved).unwrap();
    assert_eq!(initial_choice.model, "gpt-6-astra");
    assert_eq!(initial_choice.provider, Provider::OpenAI);
    assert_eq!(drifted.model, "claude-sonnet-4-5");
    assert_eq!(drifted.provider, Provider::Anthropic);
    assert!(retained.auth_binding.is_none(), "no native binding was selected by model resolution");

    // Main retains this same request for initial open and later continuation;
    // native resolution still reads the fresh configuration in each call.
    for config in [&initial, &refreshed] {
        let actual = resolve_create_session_model(config, retained.clone()).unwrap();
        assert_eq!(actual.model, initial_choice.model);
        assert_eq!(actual.provider, initial_choice.provider);
        assert_eq!(actual.auth_binding, initial_choice.auth_binding);
    }
}

#[test]
fn continuation_request_preserves_explicit_target_and_model_choices() {
    let initial = binding_config("gpt-6-astra", "configured");
    let refreshed = binding_config("gpt-5.4", "configured");
    for explicit_model in [None, Some("gpt-5.4".to_owned())] {
        let request = CreateSessionModelResolutionRequest {
            model: explicit_model.clone(), auth_binding: Some(binding("configured")),
            ..Default::default()
        };
        let initial_choice = resolve_create_session_model(&initial, request.clone()).unwrap();
        let retained = retain_model_selection(&initial, request.clone()).unwrap();
        let actual = resolve_create_session_model(&refreshed, retained.clone()).unwrap();
        assert_eq!(retained.auth_binding, Some(binding("configured")));
        assert_eq!(actual.auth_binding, initial_choice.auth_binding);
        assert_eq!(actual.provider, initial_choice.provider);
        assert_eq!(actual.model, initial_choice.model);
        if let Some(model) = explicit_model {
            assert_eq!(actual.model, model, "an explicit model is not replaced by a binding default");
        } else {
            let drifted = resolve_create_session_model(&refreshed, request).unwrap();
            assert_ne!(drifted.model, initial_choice.model,
                "the original unresolved binding-default request must demonstrate the defect");
        }
    }
}

#[test]
fn absent_binding_keeps_native_current_candidate_resolution() {
    let initial = binding_config("gpt-6-astra", "original");
    let refreshed = binding_config("gpt-5.4", "current");
    let retained = retain_model_selection(&initial, CreateSessionModelResolutionRequest::default()).unwrap();
    assert!(retained.auth_binding.is_none(), "Toolkit must not invent a credential candidate pin");
    let realm = RealmId::parse("owner").unwrap();
    for (config, expected) in [(&initial, "original"), (&refreshed, "current")] {
        let model = resolve_create_session_model(config, retained.clone()).unwrap();
        assert_eq!(model.model, "gpt-6-astra");
        let candidates = resolve_auth_binding_candidates_for_provider(
            config, model.provider, model.auth_binding.as_ref(), Some(&realm), true,
        ).unwrap();
        assert!(candidates.iter().any(|candidate| candidate.auth_binding == binding(expected)));
        assert!(!candidates.iter().any(|candidate|
            candidate.auth_binding == binding(if expected == "original" { "current" } else { "original" })));
    }
}

#[test]
fn retained_explicit_binding_cannot_fall_back_after_owner_removes_it() {
    let initial = binding_config("gpt-6-astra", "configured");
    let refreshed = binding_config("gpt-5.4", "replacement");
    let retained = retain_model_selection(&initial, CreateSessionModelResolutionRequest {
        auth_binding: Some(binding("configured")), ..Default::default()
    }).unwrap();
    assert!(resolve_create_session_model(&refreshed, retained).is_err(),
        "native current binding validation must refuse the missing selection, without substitution");
    assert!(retain_model_selection(&initial, CreateSessionModelResolutionRequest {
        model: Some("gpt-6-astra".into()), provider: Some(Provider::Anthropic),
        ..Default::default()
    }).is_err(), "native model/provider conflict must not become a default choice");
}

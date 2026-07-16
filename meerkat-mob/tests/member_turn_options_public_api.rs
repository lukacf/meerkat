use meerkat_core::lifecycle::run_primitive::{ModelId, TurnMetadataOverride};
use meerkat_core::{Provider, TurnToolOverlay};
use meerkat_mob::MemberTurnOptions;
use std::collections::BTreeMap;

#[test]
fn downstream_host_can_build_exact_per_turn_llm_identity_options() {
    let options = MemberTurnOptions::new()
        .with_model(ModelId::new("shared-local-model"))
        .with_provider(Provider::SelfHosted)
        .with_self_hosted_server_id("local-b")
        .with_provider_params(TurnMetadataOverride::Clear)
        .with_auth_binding(TurnMetadataOverride::Clear);

    assert_eq!(
        options.model.as_ref().map(ModelId::as_str),
        Some("shared-local-model")
    );
    assert_eq!(options.provider, Some(Provider::SelfHosted));
    assert_eq!(options.self_hosted_server_id.as_deref(), Some("local-b"));
    assert!(matches!(
        options.provider_params,
        Some(TurnMetadataOverride::Clear)
    ));
    assert!(matches!(
        options.auth_binding,
        Some(TurnMetadataOverride::Clear)
    ));
}

#[test]
fn downstream_host_can_attach_dispatch_context_to_member_turn_options() {
    let dispatch_context = BTreeMap::from([(
        "host.routing_hint".to_string(),
        serde_json::json!({ "shard": "blue" }),
    )]);
    let options = MemberTurnOptions::new().with_turn_tool_overlay(TurnToolOverlay {
        allowed_tools: Some(vec!["search".into()]),
        dispatch_context: dispatch_context.clone(),
        ..Default::default()
    });

    assert_eq!(
        options
            .turn_tool_overlay
            .as_ref()
            .map(|overlay| &overlay.dispatch_context),
        Some(&dispatch_context)
    );
}

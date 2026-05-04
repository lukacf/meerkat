#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::TypeRef;
use meerkat_machine_schema::catalog::dsl::dsl_auth_machine;

#[test]
fn auth_machine_declares_oauth_flow_lifecycle_state() {
    let schema = dsl_auth_machine();
    for field in [
        "oauth_browser_flow_ids",
        "oauth_device_flow_ids",
        "oauth_device_poll_ids",
    ] {
        let state_field = schema
            .state
            .fields
            .iter()
            .find(|candidate| candidate.name.as_str() == field)
            .unwrap_or_else(|| panic!("AuthMachine must declare state field `{field}`"));
        assert_eq!(
            state_field.ty,
            TypeRef::Set(Box::new(TypeRef::String)),
            "`{field}` should be a Set<String> owned by AuthMachine"
        );
    }
}

#[test]
fn auth_machine_declares_oauth_flow_lifecycle_inputs() {
    let schema = dsl_auth_machine();
    schema
        .inputs
        .variant_named("ClearCredentialLifecycle")
        .expect("AuthMachine must declare credential-only lifecycle clear input");
    for input in [
        "AdmitOAuthBrowserFlow",
        "VerifyOAuthBrowserFlow",
        "ConsumeOAuthBrowserFlow",
        "ExpireOAuthBrowserFlow",
        "AdmitOAuthDeviceFlow",
        "VerifyOAuthDeviceFlow",
        "BeginOAuthDevicePoll",
        "FinishOAuthDevicePoll",
        "ConsumeOAuthDeviceFlow",
        "ExpireOAuthDeviceFlow",
    ] {
        let variant = schema
            .inputs
            .variant_named(input)
            .unwrap_or_else(|_| panic!("AuthMachine must declare input `{input}`"));
        variant
            .field_named("flow_id")
            .unwrap_or_else(|_| panic!("`{input}` must carry flow_id"));
    }
}

#[test]
fn auth_machine_routes_oauth_flow_lifecycle_transitions() {
    let schema = dsl_auth_machine();
    let transition_prefixes = [
        "AdmitOAuthBrowserFlow",
        "VerifyOAuthBrowserFlow",
        "ConsumeOAuthBrowserFlow",
        "ExpireOAuthBrowserFlow",
        "AdmitOAuthDeviceFlow",
        "VerifyOAuthDeviceFlow",
        "BeginOAuthDevicePoll",
        "FinishOAuthDevicePoll",
        "ConsumeOAuthDeviceFlow",
        "ExpireOAuthDeviceFlow",
    ];
    for transition_prefix in transition_prefixes {
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str().starts_with(transition_prefix)),
            "AuthMachine must route OAuth lifecycle transition `{transition_prefix}`"
        );
    }
}

#[test]
fn auth_machine_oauth_flow_lifecycle_transitions_preserve_credential_phase() {
    let schema = dsl_auth_machine();
    for transition in schema.transitions.iter().filter(|transition| {
        [
            "AdmitOAuthBrowserFlow",
            "VerifyOAuthBrowserFlow",
            "ConsumeOAuthBrowserFlow",
            "ExpireOAuthBrowserFlow",
            "AdmitOAuthDeviceFlow",
            "VerifyOAuthDeviceFlow",
            "BeginOAuthDevicePoll",
            "FinishOAuthDevicePoll",
            "ConsumeOAuthDeviceFlow",
            "ExpireOAuthDeviceFlow",
        ]
        .iter()
        .any(|prefix| transition.name.as_str().starts_with(prefix))
    }) {
        assert_eq!(
            transition.from.len(),
            1,
            "{} should be expanded to one source phase",
            transition.name
        );
        assert_eq!(
            &transition.to, &transition.from[0],
            "{} must preserve credential lifecycle phase",
            transition.name
        );
    }
}

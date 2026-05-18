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
    for input in ["AdmitOAuthBrowserFlow", "AdmitOAuthDeviceFlow"] {
        let variant = schema
            .inputs
            .variant_named(input)
            .unwrap_or_else(|_| panic!("AuthMachine must declare input `{input}`"));
        assert_eq!(
            variant
                .field_named("observed_global_outstanding_flows")
                .unwrap_or_else(|_| {
                    panic!("`{input}` must carry typed global OAuth capacity observation")
                })
                .ty,
            TypeRef::U64,
            "`{input}` should carry global capacity observation as u64"
        );
    }
    let durable_confirmation = schema
        .inputs
        .variant_named("ConfirmOAuthDurableAdmission")
        .expect("AuthMachine must declare durable OAuth admission confirmation input");
    assert_eq!(
        durable_confirmation
            .field_named("observed_global_outstanding_flows")
            .expect("durable confirmation must carry typed global OAuth capacity observation")
            .ty,
        TypeRef::U64
    );
    assert_eq!(
        durable_confirmation
            .field_named("max_outstanding_flows")
            .expect("durable confirmation must carry OAuth capacity limit")
            .ty,
        TypeRef::U64
    );
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
fn auth_machine_oauth_admission_has_generated_global_capacity_guard() {
    let schema = dsl_auth_machine();
    for transition in schema.transitions.iter().filter(|transition| {
        ["AdmitOAuthBrowserFlow", "AdmitOAuthDeviceFlow"]
            .iter()
            .any(|prefix| transition.name.as_str().starts_with(prefix))
    }) {
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "oauth_global_capacity_available"),
            "{} must guard global OAuth capacity in generated authority",
            transition.name
        );
    }
}

#[test]
fn auth_machine_oauth_durable_confirmation_has_generated_global_capacity_guard() {
    let schema = dsl_auth_machine();
    let transitions = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("ConfirmOAuthDurableAdmission")
        })
        .collect::<Vec<_>>();
    assert!(
        !transitions.is_empty(),
        "AuthMachine must route durable OAuth admission confirmation transitions"
    );
    for transition in transitions {
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "oauth_global_capacity_available"),
            "{} must confirm durable OAuth admission in generated authority",
            transition.name
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
            "ConfirmOAuthDurableAdmission",
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

#![allow(clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::catalog::dsl::dsl_auth_machine;
use meerkat_machine_schema::{TriggerMatch, TypeRef};

fn is_optional_auth_lifecycle_phase(ty: &TypeRef) -> bool {
    matches!(
        ty,
        TypeRef::Option(inner)
            if matches!(inner.as_ref(), TypeRef::Enum(enum_id) if enum_id.as_str() == "AuthLifecyclePhase")
    )
}

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
        "RestoreOAuthBrowserFlow",
        "RestoreOAuthDeviceFlow",
        "RestoreOAuthDevicePoll",
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
fn auth_machine_restore_snapshot_routes_oauth_membership_per_flow() {
    let schema = dsl_auth_machine();
    let restore = schema
        .inputs
        .variant_named("RestoreAuthoritySnapshot")
        .expect("AuthMachine must declare lifecycle restore input");
    assert!(
        restore
            .fields
            .iter()
            .all(|field| !field.name.as_str().starts_with("oauth_")),
        "bulk lifecycle restore must not carry OAuth membership maps"
    );

    let credential_restore = schema
        .inputs
        .variant_named("RestoreCredentialLifecycleSnapshot")
        .expect("AuthMachine must declare generated credential lifecycle restore input");
    assert!(
        is_optional_auth_lifecycle_phase(
            &credential_restore
                .field_named("lifecycle_phase")
                .expect("credential lifecycle restore must carry optional phase observation")
                .ty
        ),
        "credential lifecycle restore must carry lifecycle_phase as Option<AuthLifecyclePhase>"
    );
    assert_eq!(
        credential_restore
            .field_named("restored_oauth_membership_observed")
            .expect("credential lifecycle restore must carry typed OAuth restore observation")
            .ty,
        TypeRef::Bool
    );

    let browser = schema
        .inputs
        .variant_named("RestoreOAuthBrowserFlow")
        .expect("AuthMachine must declare browser flow restore input");
    assert_eq!(
        browser
            .field_named("provider")
            .expect("browser flow restore must carry provider")
            .ty,
        TypeRef::Option(Box::new(TypeRef::String))
    );
    assert_eq!(
        browser
            .field_named("redirect_uri")
            .expect("browser flow restore must carry redirect URI")
            .ty,
        TypeRef::Option(Box::new(TypeRef::String))
    );
    assert_eq!(
        browser
            .field_named("expires_at_millis")
            .expect("browser flow restore must carry expiry")
            .ty,
        TypeRef::Option(Box::new(TypeRef::U64))
    );

    let device = schema
        .inputs
        .variant_named("RestoreOAuthDeviceFlow")
        .expect("AuthMachine must declare device flow restore input");
    assert_eq!(
        device
            .field_named("provider")
            .expect("device flow restore must carry provider")
            .ty,
        TypeRef::Option(Box::new(TypeRef::String))
    );
    assert_eq!(
        device
            .field_named("expires_at_millis")
            .expect("device flow restore must carry expiry")
            .ty,
        TypeRef::Option(Box::new(TypeRef::U64))
    );

    let device_poll = schema
        .inputs
        .variant_named("RestoreOAuthDevicePoll")
        .expect("AuthMachine must declare device poll restore input");
    assert_eq!(
        device_poll
            .field_named("flow_id")
            .expect("device poll restore must carry flow_id")
            .ty,
        TypeRef::String
    );

    for input in [
        "RestoreAuthoritySnapshot",
        "RestoreCredentialLifecycleSnapshot",
        "RestoreOAuthBrowserFlow",
        "RestoreOAuthDeviceFlow",
        "RestoreOAuthDevicePoll",
    ] {
        assert!(
            schema
                .runtime_internal_inputs
                .iter()
                .any(|candidate| candidate.as_str() == input),
            "`{input}` must be runtime-internal generated authority"
        );
    }

    assert!(
        schema
            .invariants
            .iter()
            .any(|invariant| invariant.name == "oauth_flow_membership_consistent"),
        "AuthMachine must keep OAuth membership maps/count consistent"
    );
}

#[test]
fn auth_machine_routes_oauth_flow_lifecycle_transitions() {
    let schema = dsl_auth_machine();
    let transition_prefixes = [
        "RestoreOAuthBrowserFlow",
        "RestoreOAuthDeviceFlow",
        "RestoreOAuthDevicePoll",
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
        // 0.7.2 D2a: the post-release arrival is a deliberate total no-op
        // (the admitted flow was already terminally cancelled by the release
        // drain); the capacity guard stays the authoritative fail-closed
        // check on every live-phase confirmation.
        if transition.name.as_str() == "ConfirmOAuthDurableAdmissionReleased" {
            assert!(
                transition.updates.is_empty(),
                "post-release durable confirmation must not mutate state"
            );
            continue;
        }
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
fn auth_machine_reopens_released_for_oauth_admission_through_generated_authority() {
    let schema = dsl_auth_machine();
    for (transition_name, input_name) in [
        (
            "ReopenReleasedForOAuthBrowserFlowAdmission",
            "AdmitOAuthBrowserFlow",
        ),
        (
            "ReopenReleasedForOAuthDeviceFlowAdmission",
            "AdmitOAuthDeviceFlow",
        ),
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == transition_name)
            .unwrap_or_else(|| panic!("AuthMachine must declare transition `{transition_name}`"));
        match &transition.on {
            TriggerMatch::Input { variant, .. } => assert_eq!(variant.as_str(), input_name),
            other => panic!("`{transition_name}` must be input-triggered, got {other:?}"),
        }
        assert_eq!(
            transition.to.as_str(),
            "ReauthRequired",
            "`{transition_name}` must reopen the generated auth lifecycle before OAuth admission"
        );
        for guard in [
            "released_without_credential",
            "released_without_oauth_membership",
            "oauth_capacity_available",
            "oauth_global_capacity_available",
        ] {
            assert!(
                transition
                    .guards
                    .iter()
                    .any(|candidate| candidate.name == guard),
                "`{transition_name}` must carry generated guard `{guard}`"
            );
        }
    }
}

#[test]
fn auth_machine_oauth_flow_lifecycle_transitions_preserve_credential_phase() {
    let schema = dsl_auth_machine();
    for transition in schema.transitions.iter().filter(|transition| {
        [
            "RestoreOAuthBrowserFlow",
            "RestoreOAuthDeviceFlow",
            "RestoreOAuthDevicePoll",
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

#[test]
fn auth_machine_restore_rejects_orphan_device_poll_in_generated_authority() {
    let schema = dsl_auth_machine();
    let transitions = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("RestoreOAuthDevicePoll")
        })
        .collect::<Vec<_>>();
    assert!(
        !transitions.is_empty(),
        "AuthMachine must declare generated device poll restore transitions"
    );
    for transition in transitions {
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "device_flow_present_for_poll_restore"),
            "{} must reject orphan restored device polls in generated authority",
            transition.name
        );
    }
}

// --- 0.7.2 disciplined shell inputs (lane L3 auth-release) ---

/// D1: release teardown is two-phase and machine-owned. `BeginRelease`
/// records the draining intent and emits the in-flight flow membership as a
/// typed `CancelOAuthFlowsForRelease` obligation; the final `Release`
/// transition is guarded on the drain being complete.
#[test]
fn auth_machine_release_is_two_phase_with_machine_owned_oauth_drain() {
    let schema = dsl_auth_machine();

    let draining = schema
        .state
        .fields
        .iter()
        .find(|field| field.name.as_str() == "release_draining")
        .expect("AuthMachine must declare release_draining drain sub-state");
    assert_eq!(
        draining.ty,
        TypeRef::Bool,
        "release_draining must be a machine-owned bool sub-state"
    );

    schema
        .inputs
        .variant_named("BeginRelease")
        .expect("AuthMachine must declare BeginRelease drain input");

    let cancel = schema
        .effects
        .variant_named("CancelOAuthFlowsForRelease")
        .expect("AuthMachine must declare typed release-drain cancellation effect");
    for field in ["browser_flow_ids", "device_flow_ids"] {
        assert_eq!(
            cancel
                .field_named(field)
                .unwrap_or_else(|_| panic!(
                    "CancelOAuthFlowsForRelease must carry `{field}` membership"
                ))
                .ty,
            TypeRef::Set(Box::new(TypeRef::String)),
            "`{field}` must carry the machine-owned flow membership set"
        );
    }

    // Each live phase routes a draining BeginRelease that emits the
    // cancellation obligation while membership is still intact.
    let draining_transitions = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition
                .name
                .as_str()
                .starts_with("BeginReleaseDrainingOAuthFlows")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        draining_transitions.len(),
        5,
        "BeginRelease must drain from every live lifecycle phase"
    );
    for transition in draining_transitions {
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "oauth_membership_present"),
            "{} must only emit a drain obligation when flows are in flight",
            transition.name
        );
        assert!(
            transition
                .emit
                .iter()
                .any(|effect| effect.variant.as_str() == "CancelOAuthFlowsForRelease"),
            "{} must emit the typed cancellation obligation",
            transition.name
        );
        assert_eq!(
            transition.from.len(),
            1,
            "{} must be a per-phase expansion",
            transition.name
        );
        assert_eq!(
            &transition.to, &transition.from[0],
            "{} must not change lifecycle phase while draining",
            transition.name
        );
    }

    // The final Release transition is guarded on the drain being complete:
    // a Released machine structurally cannot hold OAuth flow membership.
    let release = schema
        .transitions
        .iter()
        .find(|transition| transition.name.as_str() == "Release")
        .expect("AuthMachine must declare the Release transition");
    assert!(
        release
            .guards
            .iter()
            .any(|guard| guard.name == "oauth_release_drained"),
        "Release must be guarded on all drain obligations closed"
    );

    for invariant in [
        "released_oauth_membership_drained",
        "released_not_release_draining",
    ] {
        assert!(
            schema
                .invariants
                .iter()
                .any(|candidate| candidate.name == invariant),
            "AuthMachine must declare release-drain invariant `{invariant}`"
        );
    }
}

/// D2a: observation/cleanup-shaped OAuth inputs (Expire*, durable-admission
/// Confirm, poll Finish, BeginRelease) are total in `Released` — late
/// poll/prune/compensation arrivals after teardown are benign no-ops, never
/// guard rejections (worklist entries 24-30).
#[test]
fn auth_machine_post_release_oauth_observations_are_total_noops() {
    let schema = dsl_auth_machine();
    for (transition_name, input_name) in [
        ("ExpireOAuthBrowserFlowReleased", "ExpireOAuthBrowserFlow"),
        ("ExpireOAuthDeviceFlowReleased", "ExpireOAuthDeviceFlow"),
        (
            "ConfirmOAuthDurableAdmissionReleased",
            "ConfirmOAuthDurableAdmission",
        ),
        ("FinishOAuthDevicePollReleased", "FinishOAuthDevicePoll"),
        ("BeginReleaseReleased", "BeginRelease"),
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == transition_name)
            .unwrap_or_else(|| {
                panic!("AuthMachine must accept `{input_name}` in Released as `{transition_name}`")
            });
        match &transition.on {
            TriggerMatch::Input { variant, .. } => assert_eq!(variant.as_str(), input_name),
            other => panic!("`{transition_name}` must be input-triggered, got {other:?}"),
        }
        assert_eq!(
            transition
                .from
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["Released"],
            "`{transition_name}` must fire only in Released"
        );
        assert_eq!(
            transition.to.as_str(),
            "Released",
            "`{transition_name}` must stay in Released"
        );
        assert!(
            transition.updates.is_empty(),
            "`{transition_name}` must not mutate state"
        );
        assert!(
            transition.emit.is_empty(),
            "`{transition_name}` must not emit effects"
        );
    }
}

/// D2a: stale expiry/finish observations for flows that are no longer
/// members are total no-ops in every live phase too — a consume or release
/// drain can legitimately remove the flow between a producer's membership
/// observation and its input delivery.
#[test]
fn auth_machine_stale_oauth_cleanup_observations_are_total_noops_in_live_phases() {
    let schema = dsl_auth_machine();
    for (prefix, guard_name) in [
        ("ExpireOAuthBrowserFlowAbsent", "browser_flow_absent"),
        ("ExpireOAuthDeviceFlowAbsent", "device_flow_absent"),
        ("FinishOAuthDevicePollAbsent", "device_poll_absent"),
    ] {
        let transitions = schema
            .transitions
            .iter()
            .filter(|transition| transition.name.as_str().starts_with(prefix))
            .collect::<Vec<_>>();
        assert_eq!(
            transitions.len(),
            5,
            "`{prefix}` must be total across every live lifecycle phase"
        );
        for transition in transitions {
            assert!(
                transition
                    .guards
                    .iter()
                    .any(|guard| guard.name == guard_name),
                "{} must be guarded on absent membership (`{guard_name}`)",
                transition.name
            );
            assert!(
                transition.updates.is_empty(),
                "{} must not mutate state",
                transition.name
            );
            assert!(
                transition.emit.is_empty(),
                "{} must not emit effects",
                transition.name
            );
        }
    }
}

/// D1: while the release drain is in flight, new OAuth flow admissions and
/// membership restores are refused so the drain obligation set cannot grow
/// behind the shell's back.
#[test]
fn auth_machine_oauth_admissions_refuse_release_draining() {
    let schema = dsl_auth_machine();
    for prefix in [
        "AdmitOAuthBrowserFlow",
        "AdmitOAuthDeviceFlow",
        "RestoreOAuthBrowserFlow",
        "RestoreOAuthDeviceFlow",
        "RestoreOAuthDevicePoll",
    ] {
        let transitions = schema
            .transitions
            .iter()
            .filter(|transition| transition.name.as_str().starts_with(prefix))
            .collect::<Vec<_>>();
        assert_eq!(
            transitions.len(),
            5,
            "`{prefix}` must expand across exactly the five live lifecycle phases"
        );
        for transition in transitions {
            assert!(
                transition
                    .guards
                    .iter()
                    .any(|guard| guard.name == "not_release_draining"),
                "{} must refuse admission while the release drain is in flight",
                transition.name
            );
        }
    }
}

//! Kernel runtime typed round-trip tests (Wave-b B-3).
//!
//! Exercises the retyped [`KernelState`], [`KernelInput`], [`KernelSignal`],
//! [`KernelEffect`], [`TransitionOutcome`], and [`TransitionRefusal`] surface
//! against real catalog machines. Every assertion is against a typed identity
//! newtype (`PhaseId`, `InputVariantId`, `SignalVariantId`, `EffectVariantId`,
//! `FieldId`, `TransitionId`, `EnumTypeId`, `EnumVariantId`) — no bare strings.

#![cfg(feature = "test-oracle")]
#![allow(clippy::expect_used, clippy::panic, clippy::redundant_clone)]

use std::collections::BTreeMap;

use meerkat_machine_kernels::test_oracle::{
    GeneratedMachineKernel, KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue,
    TransitionOutcome, TransitionRefusal,
};
use meerkat_machine_schema::RouteVariantId;
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_machine_schema::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId, NamedTypeId,
    PhaseId, SignalVariantId, TransitionId,
};

fn input(slug: &str) -> InputVariantId {
    InputVariantId::parse(slug).expect("valid input variant slug")
}

fn signal(slug: &str) -> SignalVariantId {
    SignalVariantId::parse(slug).expect("valid signal variant slug")
}

fn field(slug: &str) -> FieldId {
    FieldId::parse(slug).expect("valid field slug")
}

fn phase(slug: &str) -> PhaseId {
    PhaseId::parse(slug).expect("valid phase slug")
}

fn effect(slug: &str) -> EffectVariantId {
    EffectVariantId::parse(slug).expect("valid effect slug")
}

fn transition(slug: &str) -> TransitionId {
    TransitionId::parse(slug).expect("valid transition slug")
}

fn enum_type(slug: &str) -> EnumTypeId {
    EnumTypeId::parse(slug).expect("valid enum type slug")
}

fn enum_variant(slug: &str) -> EnumVariantId {
    EnumVariantId::parse(slug).expect("valid enum variant slug")
}

fn named_string(type_name: &str, value: &str) -> KernelValue {
    KernelValue::Named {
        type_name: NamedTypeId::parse(type_name).expect("valid named type slug"),
        value: Box::new(KernelValue::String(value.into())),
    }
}

fn named_u64(type_name: &str, value: u64) -> KernelValue {
    KernelValue::Named {
        type_name: NamedTypeId::parse(type_name).expect("valid named type slug"),
        value: Box::new(KernelValue::U64(value)),
    }
}

fn named_variant(enum_name: &str, variant: &str) -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: enum_type(enum_name),
        variant: enum_variant(variant),
    }
}

fn option_some(value: KernelValue) -> KernelValue {
    KernelValue::Map(BTreeMap::from([(
        KernelValue::String("value".into()),
        value,
    )]))
}

fn identity_reconciliation_fields(
    intent: &str,
    session_creation_receipt: &str,
    retirement_receipt: &str,
    session: &str,
    wiring: &str,
) -> BTreeMap<FieldId, KernelValue> {
    BTreeMap::from([
        (
            field("intent"),
            named_variant("IdentityAuthorityCondition", intent),
        ),
        (
            field("lease"),
            named_variant("IdentityLeaseCondition", "HeldByCurrentIncarnation"),
        ),
        (field("external_binding_required"), KernelValue::Bool(false)),
        (field("initial_delivery_required"), KernelValue::Bool(false)),
        (
            field("session_creation_receipt"),
            named_variant("IdentityReceiptCondition", session_creation_receipt),
        ),
        (
            field("retirement_receipt"),
            named_variant("IdentityReceiptCondition", retirement_receipt),
        ),
        (
            field("session"),
            named_variant("IdentitySessionCondition", session),
        ),
        (
            field("runtime"),
            named_variant("IdentityResourceCondition", "Matching"),
        ),
        (
            field("member"),
            named_variant("IdentityResourceCondition", "Matching"),
        ),
        (
            field("external_binding_receipt"),
            named_variant("IdentityReceiptCondition", "NotRequired"),
        ),
        (
            field("external_trust"),
            named_variant("IdentityExternalTrustCondition", "NotRequired"),
        ),
        (
            field("external_ceremony"),
            named_variant("IdentityExternalCeremonyCondition", "NotRequired"),
        ),
        (
            field("initial_delivery_receipt"),
            named_variant("IdentityReceiptCondition", "NotRequired"),
        ),
        (
            field("initial_delivery"),
            named_variant("IdentityInitialDeliveryCondition", "NotRequired"),
        ),
        (
            field("wiring"),
            named_variant("IdentityResourceCondition", wiring),
        ),
    ])
}

#[test]
fn applying_typed_input_yields_typed_transition_outcome() {
    let kernel = GeneratedMachineKernel::new(meerkat_machine());
    let mut state: KernelState = kernel.initial_state().expect("initial state");
    // Typed phase identity at the boundary.
    let _: &PhaseId = &state.phase;

    // Typed signal in, typed phase transition out.
    let init = kernel
        .transition_signal(
            &state,
            &KernelSignal {
                variant: signal("Initialize"),
                fields: BTreeMap::new(),
            },
        )
        .expect("initialize");
    let _: &TransitionId = &init.transition;
    assert_eq!(init.transition, transition("Initialize"));
    state = init.next_state;
    assert_eq!(state.phase, phase("Idle"));

    // Typed input in, typed field-keyed fields in and out.
    let register = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("RegisterSession"),
                fields: BTreeMap::from([(
                    field("session_id"),
                    named_string("SessionId", "sess-1"),
                )]),
            },
        )
        .expect("register session");
    assert_eq!(register.transition, transition("RegisterSessionIdle"));
    assert_eq!(
        register.next_state.fields.get(&field("session_id")),
        Some(&KernelValue::Map(BTreeMap::from([(
            KernelValue::String("value".into()),
            named_string("SessionId", "sess-1"),
        )])))
    );
    for key in register.next_state.fields.keys() {
        let _: &FieldId = key;
    }
}

/// Phase-1 multi-host BeginSpawnExec observation fields: a local spawn with no
/// placement and no non-portable/secret-bearing observations.
fn begin_spawn_multi_host_fields() -> Vec<(FieldId, KernelValue)> {
    [
        "workgraph_required",
        "rust_bundles_present",
        "per_spawn_external_tools_present",
        "mob_default_external_tools_present",
        "default_llm_client_override_present",
        "host_surface_mcp_allowlist_present",
        "inherited_tool_filter_present",
        "shell_env_present",
        "mcp_stdio_env_present",
        "mcp_http_headers_present",
        "memory_required",
        "mcp_required",
    ]
    .into_iter()
    .map(|name| (field(name), KernelValue::Bool(false)))
    .chain([
        (field("placement"), KernelValue::None),
        (field("resume_session_id"), KernelValue::None),
        (field("placed_spawn_id"), KernelValue::None),
        (field("placed_provision_operation_id"), KernelValue::None),
        (
            field("placed_operation_owner_session_id"),
            KernelValue::None,
        ),
        (
            field("effective_profile_override_present"),
            KernelValue::Bool(false),
        ),
        (
            field("effective_model_override_present"),
            KernelValue::Bool(false),
        ),
    ])
    .collect()
}

/// Phase-1 CommitSpawnMembership local-arm ack fields (all absent).
fn commit_membership_ack_fields() -> Vec<(FieldId, KernelValue)> {
    vec![
        (field("member_peer_endpoint"), KernelValue::None),
        (field("spec_digest_echo"), KernelValue::None),
        (field("ack_engine_version"), KernelValue::None),
        (field("placed_spawn_id"), KernelValue::None),
        (field("provision_operation_id"), KernelValue::None),
    ]
}

#[test]
fn mob_spawn_produces_typed_effect_variants() {
    let kernel = GeneratedMachineKernel::new(mob_machine());
    let state = kernel.initial_state().expect("initial state");
    assert_eq!(state.phase, phase("Running"));
    let profile_material_digest = "profile.worker.1";

    let authorized: TransitionOutcome = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("AuthorizeSpawnProfile"),
                fields: BTreeMap::from([
                    (
                        field("agent_identity"),
                        named_string("AgentIdentity", "agent.worker"),
                    ),
                    (
                        field("profile_name"),
                        KernelValue::String("worker".to_owned()),
                    ),
                    (field("model"), KernelValue::String("test-model".to_owned())),
                    (
                        field("profile_material_digest"),
                        KernelValue::String(profile_material_digest.to_owned()),
                    ),
                    (
                        field("tool_config_digest"),
                        KernelValue::String("tool-config.worker.1".to_owned()),
                    ),
                    (
                        field("skills_digest"),
                        KernelValue::String("skills.worker.1".to_owned()),
                    ),
                    (field("provider_params_digest"), KernelValue::None),
                    (field("output_schema_digest"), KernelValue::None),
                    (field("external_addressable"), KernelValue::Bool(false)),
                    (field("resolved_spec_digest"), KernelValue::None),
                ]),
            },
        )
        .expect("authorize spawn profile");
    assert_eq!(
        authorized.transition,
        transition("AuthorizeSpawnProfileRunning")
    );

    // Spawn-exec ladder: `BeginSpawnExec` opens the phase, then
    // `CommitSpawnMembership` (renamed from `Spawn`) establishes membership and
    // emits `RequestRuntimeBinding`.
    let spawn_fields = || {
        BTreeMap::from([
            (
                field("agent_identity"),
                named_string("AgentIdentity", "agent.worker"),
            ),
            (
                field("agent_runtime_id"),
                named_string("AgentRuntimeId", "runtime.worker.1"),
            ),
            (field("fence_token"), named_u64("FenceToken", 1)),
            (field("generation"), named_u64("Generation", 0)),
            (
                field("profile_material_digest"),
                KernelValue::String(profile_material_digest.to_owned()),
            ),
            (field("external_addressable"), KernelValue::Bool(false)),
            (
                field("runtime_mode"),
                named_variant("SpawnPolicyRuntimeMode", "AutonomousHost"),
            ),
            (
                field("bridge_session_id"),
                option_some(named_string("SessionId", "bridge.worker.1")),
            ),
            (field("replacing"), KernelValue::None),
        ])
    };
    let opened: TransitionOutcome = kernel
        .transition(
            &authorized.next_state,
            &KernelInput {
                variant: input("BeginSpawnExec"),
                fields: {
                    let mut fields = spawn_fields();
                    fields.extend(begin_spawn_multi_host_fields());
                    fields
                },
            },
        )
        .expect("begin spawn exec");
    assert_eq!(opened.transition, transition("BeginSpawnExecFresh"));

    let outcome: TransitionOutcome = kernel
        .transition(
            &opened.next_state,
            &KernelInput {
                variant: input("CommitSpawnMembership"),
                fields: {
                    let mut fields = spawn_fields();
                    fields.extend(commit_membership_ack_fields());
                    fields
                },
            },
        )
        .expect("commit spawn membership");

    assert_eq!(outcome.transition, transition("CommitSpawnMembershipFresh"));
    for emitted in &outcome.effects {
        let _: &EffectVariantId = &emitted.variant;
        let _: &BTreeMap<FieldId, KernelValue> = &emitted.fields;
    }
    assert!(
        outcome
            .effects
            .iter()
            .any(|emitted| emitted.variant == effect("RequestRuntimeBinding"))
    );
}

#[test]
fn mob_identity_reconciliation_uses_the_schema_helper_without_state() {
    let kernel = GeneratedMachineKernel::new(mob_machine());
    let state = kernel.initial_state().expect("initial state");
    let outcome = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("ClassifyIdentityReconciliation"),
                // A later unavailable resource must not stall the independent
                // session obligation.
                fields: identity_reconciliation_fields(
                    "PresentCreateIfAbsent",
                    "Missing",
                    "NotRequired",
                    "Missing",
                    "Unavailable",
                ),
            },
        )
        .expect("identity reconciliation classification");

    assert_eq!(
        outcome.transition,
        transition("ClassifyIdentityReconciliationRunning")
    );
    assert_eq!(outcome.next_state, state);
    assert_eq!(outcome.effects.len(), 1);
    assert_eq!(
        outcome.effects[0].variant,
        effect("IdentityReconciliationClassified")
    );
    assert_eq!(
        outcome.effects[0].fields.get(&field("decision")),
        Some(&named_variant(
            "IdentityReconcileDecision",
            "EnsureSessionAuthority"
        ))
    );
}

#[test]
fn mob_identity_reconciliation_uses_one_wiring_obligation_for_ensure_and_cleanup() {
    let kernel = GeneratedMachineKernel::new(mob_machine());
    let state = kernel.initial_state().expect("initial state");
    let cases = [
        (
            "PresentRequireExisting",
            "NotRequired",
            "NotRequired",
            "Matching",
            "Divergent",
        ),
        (
            "PresentRequireExisting",
            "NotRequired",
            "NotRequired",
            "Malformed",
            "Matching",
        ),
        ("Absent", "NotRequired", "Missing", "Malformed", "Matching"),
    ];

    for (intent, session_creation_receipt, retirement_receipt, session, wiring) in cases {
        let outcome = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: input("ClassifyIdentityReconciliation"),
                    fields: identity_reconciliation_fields(
                        intent,
                        session_creation_receipt,
                        retirement_receipt,
                        session,
                        wiring,
                    ),
                },
            )
            .expect("identity reconciliation classification");

        assert_eq!(outcome.next_state, state);
        assert_eq!(
            outcome.effects[0].fields.get(&field("decision")),
            Some(&named_variant(
                "IdentityReconcileDecision",
                "ReconcileWiring"
            ))
        );
    }
}

#[test]
fn mob_spawn_rejects_unauthorized_addressability() {
    let kernel = GeneratedMachineKernel::new(mob_machine());
    let state = kernel.initial_state().expect("initial state");
    let profile_material_digest = "profile.worker.1";

    let authorized = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("AuthorizeSpawnProfile"),
                fields: BTreeMap::from([
                    (
                        field("agent_identity"),
                        named_string("AgentIdentity", "agent.worker"),
                    ),
                    (
                        field("profile_name"),
                        KernelValue::String("worker".to_owned()),
                    ),
                    (field("model"), KernelValue::String("test-model".to_owned())),
                    (
                        field("profile_material_digest"),
                        KernelValue::String(profile_material_digest.to_owned()),
                    ),
                    (
                        field("tool_config_digest"),
                        KernelValue::String("tool-config.worker.1".to_owned()),
                    ),
                    (
                        field("skills_digest"),
                        KernelValue::String("skills.worker.1".to_owned()),
                    ),
                    (field("provider_params_digest"), KernelValue::None),
                    (field("output_schema_digest"), KernelValue::None),
                    (field("external_addressable"), KernelValue::Bool(false)),
                    (field("resolved_spec_digest"), KernelValue::None),
                ]),
            },
        )
        .expect("authorize spawn profile");

    // The addressability admission guard lives on the `BeginSpawnExec` opener
    // (the ladder entry that carries the original `Spawn` admission guards), so
    // a mismatched `external_addressable` is rejected there.
    let refusal = kernel
        .transition(
            &authorized.next_state,
            &KernelInput {
                variant: input("BeginSpawnExec"),
                fields: BTreeMap::from([
                    (
                        field("agent_identity"),
                        named_string("AgentIdentity", "agent.worker"),
                    ),
                    (
                        field("agent_runtime_id"),
                        named_string("AgentRuntimeId", "runtime.worker.1"),
                    ),
                    (field("fence_token"), named_u64("FenceToken", 1)),
                    (field("generation"), named_u64("Generation", 0)),
                    (
                        field("profile_material_digest"),
                        KernelValue::String(profile_material_digest.to_owned()),
                    ),
                    (field("external_addressable"), KernelValue::Bool(true)),
                    (
                        field("runtime_mode"),
                        named_variant("SpawnPolicyRuntimeMode", "AutonomousHost"),
                    ),
                    (
                        field("bridge_session_id"),
                        option_some(named_string("SessionId", "bridge.worker.1")),
                    ),
                    (field("replacing"), KernelValue::None),
                ])
                .into_iter()
                .chain(begin_spawn_multi_host_fields())
                .collect(),
            },
        )
        .expect_err("addressability must match spawn profile authorization");

    match refusal {
        TransitionRefusal::NoMatchingTransition {
            phase: refused_phase,
            trigger: RouteVariantId::Input(variant),
            ..
        } => {
            assert_eq!(refused_phase, phase("Running"));
            assert_eq!(variant, input("BeginSpawnExec"));
        }
        other => panic!("expected BeginSpawnExec NoMatchingTransition, got {other:?}"),
    }
}

#[test]
fn unknown_input_refusal_carries_typed_identities() {
    let kernel = GeneratedMachineKernel::new(meerkat_machine());
    let state = kernel.initial_state().expect("initial state");
    let refusal = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("DoesNotExist"),
                fields: BTreeMap::new(),
            },
        )
        .expect_err("unknown input");
    match refusal {
        TransitionRefusal::UnknownInputVariant { machine, variant } => {
            // Typed identities, not strings.
            let _: MachineId = machine;
            let _: InputVariantId = variant.clone();
            assert_eq!(variant, input("DoesNotExist"));
        }
        other => panic!("expected UnknownInputVariant, got {other:?}"),
    }
}

#[test]
fn unknown_signal_refusal_carries_typed_identities() {
    let kernel = GeneratedMachineKernel::new(meerkat_machine());
    let state = kernel.initial_state().expect("initial state");
    let refusal = kernel
        .transition_signal(
            &state,
            &KernelSignal {
                variant: signal("DoesNotExist"),
                fields: BTreeMap::new(),
            },
        )
        .expect_err("unknown signal");
    match refusal {
        TransitionRefusal::UnknownSignalVariant { machine, variant } => {
            let _: MachineId = machine;
            let _: SignalVariantId = variant.clone();
            assert_eq!(variant, signal("DoesNotExist"));
        }
        other => panic!("expected UnknownSignalVariant, got {other:?}"),
    }
}

#[test]
fn no_matching_transition_refusal_carries_typed_route_variant() {
    // Send a well-formed input that has no matching transition from the initial phase.
    let kernel = GeneratedMachineKernel::new(meerkat_machine());
    let state = kernel.initial_state().expect("initial state");
    let refusal = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("RegisterSession"),
                fields: BTreeMap::from([(
                    field("session_id"),
                    named_string("SessionId", "sess-early"),
                )]),
            },
        )
        .expect_err("no matching transition from Initializing");
    match refusal {
        TransitionRefusal::NoMatchingTransition {
            machine,
            phase: refused_phase,
            trigger,
        } => {
            let _: MachineId = machine;
            let _: PhaseId = refused_phase.clone();
            assert_eq!(refused_phase, phase("Initializing"));
            match trigger {
                RouteVariantId::Input(variant) => {
                    assert_eq!(variant, input("RegisterSession"));
                }
                RouteVariantId::Signal(_) => {
                    panic!("expected Input route variant, got Signal")
                }
            }
        }
        other => panic!("expected NoMatchingTransition, got {other:?}"),
    }
}

#[test]
fn named_variant_kernel_values_carry_typed_enum_identities() {
    let enum_name = enum_type("WorkOrigin");
    let variant = enum_variant("Internal");
    let value = KernelValue::NamedVariant {
        enum_name: enum_name.clone(),
        variant: variant.clone(),
    };
    match &value {
        KernelValue::NamedVariant {
            enum_name: got_enum,
            variant: got_variant,
        } => {
            let _: &EnumTypeId = got_enum;
            let _: &EnumVariantId = got_variant;
            assert_eq!(got_enum, &enum_name);
            assert_eq!(got_variant, &variant);
        }
        other => panic!("expected NamedVariant, got {other:?}"),
    }
}

#[test]
fn kernel_state_serde_round_trips_with_typed_ids_as_strings() {
    let state = KernelState {
        phase: phase("Attached"),
        fields: BTreeMap::from([
            (field("session_id"), KernelValue::String("sess-1".into())),
            (field("active_runs"), KernelValue::U64(3)),
            (
                field("origin"),
                KernelValue::NamedVariant {
                    enum_name: enum_type("WorkOrigin"),
                    variant: enum_variant("External"),
                },
            ),
        ]),
    };

    let encoded = serde_json::to_string(&state).expect("serialize");
    // Typed ID slugs render as plain strings in the wire form.
    assert!(
        encoded.contains("\"Attached\""),
        "phase should serialize as slug string: {encoded}"
    );
    assert!(
        encoded.contains("\"session_id\""),
        "field keys should serialize as slug strings: {encoded}"
    );
    assert!(
        encoded.contains("\"WorkOrigin\""),
        "enum type slugs should serialize as strings: {encoded}"
    );
    assert!(
        encoded.contains("\"External\""),
        "enum variant slugs should serialize as strings: {encoded}"
    );

    let decoded: KernelState = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, state);

    // The decoded identities are still typed newtypes.
    let _: PhaseId = decoded.phase.clone();
    for key in decoded.fields.keys() {
        let _: &FieldId = key;
    }
}

#[test]
fn kernel_input_signal_effect_serde_round_trip_with_typed_ids() {
    let input_msg = KernelInput {
        variant: input("RegisterSession"),
        fields: BTreeMap::from([(field("session_id"), KernelValue::String("sess-1".into()))]),
    };
    // KernelInput/Signal/Effect don't derive Serialize themselves, so round-trip
    // through the KernelValue surface which is the serde-visible layer.
    let field_map_json =
        serde_json::to_string(&input_msg.fields).expect("serialize typed field map");
    let decoded: BTreeMap<FieldId, KernelValue> =
        serde_json::from_str(&field_map_json).expect("deserialize typed field map");
    assert_eq!(decoded, input_msg.fields);
    for key in decoded.keys() {
        let _: &FieldId = key;
    }

    // Effects and signals use the same typed field shape; cover the effect
    // explicitly so regressions in its typed surface fail this test.
    let emitted = KernelEffect {
        variant: effect("RequestRuntimeBinding"),
        fields: BTreeMap::from([(
            field("bridge_session_id"),
            KernelValue::String("bridge.worker.1".into()),
        )]),
    };
    let effect_fields_json =
        serde_json::to_string(&emitted.fields).expect("serialize typed effect fields");
    let decoded_effect_fields: BTreeMap<FieldId, KernelValue> =
        serde_json::from_str(&effect_fields_json).expect("deserialize typed effect fields");
    assert_eq!(decoded_effect_fields, emitted.fields);
    let _: EffectVariantId = emitted.variant.clone();

    let sig = KernelSignal {
        variant: signal("Initialize"),
        fields: BTreeMap::new(),
    };
    let _: SignalVariantId = sig.variant.clone();
    let _: InputVariantId = input_msg.variant.clone();
}

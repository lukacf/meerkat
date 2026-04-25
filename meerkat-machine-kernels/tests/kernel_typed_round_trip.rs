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
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId, PhaseId,
    SignalVariantId, TransitionId,
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
                    KernelValue::String("sess-1".into()),
                )]),
            },
        )
        .expect("register session");
    assert_eq!(register.transition, transition("RegisterSessionIdle"));
    assert_eq!(
        register.next_state.fields.get(&field("session_id")),
        Some(&KernelValue::Map(BTreeMap::from([(
            KernelValue::String("value".into()),
            KernelValue::String("sess-1".into()),
        )])))
    );
    for key in register.next_state.fields.keys() {
        let _: &FieldId = key;
    }
}

#[test]
fn mob_spawn_produces_typed_effect_variants() {
    let kernel = GeneratedMachineKernel::new(mob_machine());
    let state = kernel.initial_state().expect("initial state");
    assert_eq!(state.phase, phase("Running"));

    let outcome: TransitionOutcome = kernel
        .transition(
            &state,
            &KernelInput {
                variant: input("Spawn"),
                fields: BTreeMap::from([
                    (
                        field("agent_identity"),
                        KernelValue::String("agent.worker".into()),
                    ),
                    (
                        field("agent_runtime_id"),
                        KernelValue::String("runtime.worker.1".into()),
                    ),
                    (field("fence_token"), KernelValue::U64(1)),
                    (field("generation"), KernelValue::U64(1)),
                    (field("external_addressable"), KernelValue::Bool(false)),
                    (
                        field("bridge_session_id"),
                        KernelValue::String("bridge.worker.1".into()),
                    ),
                    (field("replacing"), KernelValue::None),
                ]),
            },
        )
        .expect("spawn");

    assert_eq!(outcome.transition, transition("SpawnRunningFresh"));
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
                    KernelValue::String("sess-early".into()),
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

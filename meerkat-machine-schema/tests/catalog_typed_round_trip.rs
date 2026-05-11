#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    unused_imports
)]

//! B-2 deliverable: typed round-trip assertion for the canonical catalog.
//!
//! Every catalog composition reconstructs through the DSL macro and the
//! hand-authored `catalog::compositions` module. This test asserts that the
//! reconstructed `MachineSchema` / `CompositionSchema` values carry the
//! typed identity newtypes on every kernel-level identity field (never
//! `String` again for: machine names, instance ids, phase/input/signal/
//! effect variant ids, field ids, transition ids, route ids, protocol ids,
//! actor ids, enum-type/enum-variant/named-type ids, composition ids).
//!
//! The typed-shape check is implicit in the type system — the fields _are_
//! newtypes — so the test's job is to walk every catalog entry and call the
//! typed accessors. A compilation failure means the retype regressed; a
//! runtime failure means the accessor returned something nonsensical.
//!
//! We also snapshot the typed kernel shape (machine slug set, composition
//! slug set, per-machine transition kind-count shape) as a thin regression
//! signal; bumps land when the schema intentionally grows.

use meerkat_machine_schema::identity::{
    ActorId, CompositionDriverId, CompositionId, CompositionWitnessId, EffectVariantId,
    EntryInputId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId,
    PhaseId, ProtocolId, RouteId, SignalVariantId, StorePrimitiveId, TransactionPlanId,
    TransactionTriggerId, TransitionId,
};
use meerkat_machine_schema::{
    CompositionSchema, MachineSchema, RouteVariantId, TriggerMatch, TypeRef,
    canonical_composition_schemas, canonical_machine_schemas,
};

#[test]
fn every_canonical_machine_uses_typed_identities() {
    let schemas = canonical_machine_schemas();
    assert!(
        !schemas.is_empty(),
        "canonical machine schemas must not be empty"
    );

    for schema in &schemas {
        assert_typed_machine_schema(schema);
    }
}

#[test]
fn every_canonical_composition_uses_typed_identities() {
    let compositions = canonical_composition_schemas();
    assert!(
        !compositions.is_empty(),
        "canonical composition schemas must not be empty"
    );

    for composition in &compositions {
        assert_typed_composition_schema(composition);
    }
}

#[test]
fn canonical_machine_slug_snapshot() {
    let mut slugs: Vec<String> = canonical_machine_schemas()
        .iter()
        .map(|schema| schema.machine.as_str().to_owned())
        .collect();
    slugs.sort();

    assert_eq!(
        slugs,
        vec![
            "AuthMachine".to_string(),
            "MeerkatMachine".to_string(),
            "MobMachine".to_string(),
            "OccurrenceLifecycleMachine".to_string(),
            "ScheduleLifecycleMachine".to_string(),
            "WorkGraphLifecycleMachine".to_string(),
        ],
        "canonical machine slug set — bump when intentionally adding/removing a kernel"
    );
}

#[test]
fn canonical_composition_slug_snapshot() {
    let mut slugs: Vec<String> = canonical_composition_schemas()
        .iter()
        .map(|composition| composition.name.as_str().to_owned())
        .collect();
    slugs.sort();

    assert_eq!(
        slugs,
        vec![
            "auth_lease_bundle".to_string(),
            "meerkat_mob_seam".to_string(),
            "schedule_bundle".to_string(),
            "schedule_mob_bundle".to_string(),
            "schedule_runtime_bundle".to_string(),
        ],
        "canonical composition slug set — bump when intentionally adding/removing a bundle"
    );
}

fn assert_typed_machine_schema(schema: &MachineSchema) {
    // MachineId round-trip — `parse` of `as_str` must yield the same id.
    let typed: MachineId = MachineId::parse(schema.machine.as_str())
        .expect("machine name already validated at schema construction");
    assert_eq!(
        typed, schema.machine,
        "MachineId round-trip must be idempotent"
    );

    for terminal in &schema.state.terminal_phases {
        assert_eq!(*terminal, PhaseId::parse(terminal.as_str()).unwrap());
    }
    for surface_only in &schema.surface_only_inputs {
        assert_eq!(
            *surface_only,
            InputVariantId::parse(surface_only.as_str()).unwrap()
        );
    }
    assert_phase_id_roundtrip(&schema.state.init.phase);
    for init in &schema.state.init.fields {
        assert_field_id_roundtrip(&init.field);
    }
    for field in &schema.state.fields {
        assert_field_id_roundtrip(&field.name);
        assert_type_ref(&field.ty);
    }
    for variant in &schema.state.phase.variants {
        assert_enum_variant_id_roundtrip(&variant.name);
        for field in &variant.fields {
            assert_field_id_roundtrip(&field.name);
            assert_type_ref(&field.ty);
        }
    }
    for variant in schema
        .inputs
        .variants
        .iter()
        .chain(schema.signals.variants.iter())
        .chain(schema.effects.variants.iter())
    {
        assert_enum_variant_id_roundtrip(&variant.name);
        for field in &variant.fields {
            assert_field_id_roundtrip(&field.name);
            assert_type_ref(&field.ty);
        }
    }
    for transition in &schema.transitions {
        assert_transition_id_roundtrip(&transition.name);
        for from in &transition.from {
            assert_phase_id_roundtrip(from);
        }
        assert_phase_id_roundtrip(&transition.to);
        match &transition.on {
            TriggerMatch::Input { variant, bindings } => {
                let rt: InputVariantId = InputVariantId::parse(variant.as_str()).unwrap();
                assert_eq!(rt, *variant, "InputVariantId round-trip");
                for binding in bindings {
                    assert_field_id_roundtrip(binding);
                }
            }
            TriggerMatch::Signal { variant, bindings } => {
                let rt: SignalVariantId = SignalVariantId::parse(variant.as_str()).unwrap();
                assert_eq!(rt, *variant, "SignalVariantId round-trip");
                for binding in bindings {
                    assert_field_id_roundtrip(binding);
                }
            }
        }
        for emit in &transition.emit {
            let rt: EffectVariantId = EffectVariantId::parse(emit.variant.as_str()).unwrap();
            assert_eq!(rt, emit.variant, "EffectVariantId round-trip");
            for key in emit.fields.keys() {
                assert_field_id_roundtrip(key);
            }
        }
    }
    for rule in &schema.effect_dispositions {
        let rt: EffectVariantId = EffectVariantId::parse(rule.effect_variant.as_str()).unwrap();
        assert_eq!(rt, rule.effect_variant, "EffectVariantId round-trip");
        if let Some(protocol) = rule.handoff_protocol.as_ref() {
            let rt: ProtocolId = ProtocolId::parse(protocol.as_str()).unwrap();
            assert_eq!(rt, *protocol, "ProtocolId round-trip");
        }
        if let meerkat_machine_schema::EffectDisposition::Routed { consumer_machines } =
            &rule.disposition
        {
            for consumer in consumer_machines {
                let rt: MachineId = MachineId::parse(consumer.as_str()).unwrap();
                assert_eq!(rt, *consumer, "MachineId round-trip on consumer");
            }
        }
    }
}

fn assert_typed_composition_schema(composition: &CompositionSchema) {
    let rt: CompositionId = CompositionId::parse(composition.name.as_str()).unwrap();
    assert_eq!(rt, composition.name, "CompositionId round-trip");

    for machine in &composition.machines {
        assert_machine_instance_id_roundtrip(&machine.instance_id);
        assert_eq!(
            machine.machine_name,
            MachineId::parse(machine.machine_name.as_str()).unwrap()
        );
        assert_eq!(
            machine.actor,
            ActorId::parse(machine.actor.as_str()).unwrap()
        );
    }
    for actor in &composition.actors {
        assert_eq!(actor.name, ActorId::parse(actor.name.as_str()).unwrap());
    }
    for protocol in &composition.handoff_protocols {
        assert_eq!(
            protocol.name,
            ProtocolId::parse(protocol.name.as_str()).unwrap()
        );
        assert_machine_instance_id_roundtrip(&protocol.producer_instance);
        assert_eq!(
            protocol.effect_variant,
            EffectVariantId::parse(protocol.effect_variant.as_str()).unwrap()
        );
        assert_eq!(
            protocol.realizing_actor,
            ActorId::parse(protocol.realizing_actor.as_str()).unwrap()
        );
        for field in &protocol.correlation_fields {
            assert_field_id_roundtrip(field);
        }
        for field in &protocol.obligation_fields {
            assert_field_id_roundtrip(field);
        }
        for feedback in &protocol.allowed_feedback_inputs {
            assert_machine_instance_id_roundtrip(&feedback.machine_instance);
            assert_eq!(
                feedback.input_variant,
                InputVariantId::parse(feedback.input_variant.as_str()).unwrap()
            );
            for binding in &feedback.field_bindings {
                assert_field_id_roundtrip(&binding.input_field);
            }
        }
    }
    for route in &composition.routes {
        assert_eq!(route.name, RouteId::parse(route.name.as_str()).unwrap());
        assert_machine_instance_id_roundtrip(&route.from_machine);
        assert_eq!(
            route.effect_variant,
            EffectVariantId::parse(route.effect_variant.as_str()).unwrap()
        );
        assert_machine_instance_id_roundtrip(&route.to.machine);
        assert_route_variant_id_roundtrip(&route.to.input_variant);
        assert_eq!(
            route.to.kind,
            route.to.input_variant.kind(),
            "RouteTarget.kind must mirror the arm of its RouteVariantId",
        );
    }
    for entry_input in &composition.entry_inputs {
        assert_eq!(
            entry_input.name,
            EntryInputId::parse(entry_input.name.as_str()).unwrap()
        );
        assert_machine_instance_id_roundtrip(&entry_input.machine);
        assert_eq!(
            entry_input.input_variant,
            InputVariantId::parse(entry_input.input_variant.as_str()).unwrap()
        );
    }
    for invariant in &composition.invariants {
        for machine in &invariant.references_machines {
            assert_machine_instance_id_roundtrip(machine);
        }
        for actor in &invariant.references_actors {
            assert_eq!(*actor, ActorId::parse(actor.as_str()).unwrap());
        }
    }
    for plan in &composition.transaction_plans {
        assert_eq!(
            plan.name,
            TransactionPlanId::parse(plan.name.as_str()).unwrap()
        );
        assert_eq!(
            plan.trigger,
            TransactionTriggerId::parse(plan.trigger.as_str()).unwrap()
        );
        assert_eq!(
            plan.store_primitive,
            StorePrimitiveId::parse(plan.store_primitive.as_str()).unwrap()
        );
        for route in &plan.route_names {
            assert_eq!(*route, RouteId::parse(route.as_str()).unwrap());
        }
        for protocol in &plan.protocol_names {
            assert_eq!(*protocol, ProtocolId::parse(protocol.as_str()).unwrap());
        }
    }
    for witness in &composition.witnesses {
        assert_eq!(
            witness.name,
            CompositionWitnessId::parse(witness.name.as_str()).unwrap()
        );
    }
    if let Some(driver) = &composition.driver {
        assert_eq!(
            driver.name,
            CompositionDriverId::parse(driver.name.as_str()).unwrap()
        );
    }
}

fn assert_type_ref(ty: &TypeRef) {
    match ty {
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 | TypeRef::String => {}
        TypeRef::Named(id) => {
            let _ = meerkat_machine_schema::identity::NamedTypeId::parse(id.as_str()).unwrap();
        }
        TypeRef::Enum(id) => {
            let _ = EnumTypeId::parse(id.as_str()).unwrap();
        }
        TypeRef::Option(inner) | TypeRef::Set(inner) | TypeRef::Seq(inner) => {
            assert_type_ref(inner);
        }
        TypeRef::Map(key, value) => {
            assert_type_ref(key);
            assert_type_ref(value);
        }
    }
}

fn assert_phase_id_roundtrip(id: &PhaseId) {
    let rt: PhaseId = PhaseId::parse(id.as_str()).unwrap();
    assert_eq!(rt, *id, "PhaseId round-trip");
}

fn assert_field_id_roundtrip(id: &FieldId) {
    let rt: FieldId = FieldId::parse(id.as_str()).unwrap();
    assert_eq!(rt, *id, "FieldId round-trip");
}

fn assert_transition_id_roundtrip(id: &TransitionId) {
    let rt: TransitionId = TransitionId::parse(id.as_str()).unwrap();
    assert_eq!(rt, *id, "TransitionId round-trip");
}

fn assert_enum_variant_id_roundtrip(id: &EnumVariantId) {
    let rt: EnumVariantId = EnumVariantId::parse(id.as_str()).unwrap();
    assert_eq!(rt, *id, "EnumVariantId round-trip");
}

fn assert_machine_instance_id_roundtrip(id: &MachineInstanceId) {
    let rt: MachineInstanceId = MachineInstanceId::parse(id.as_str()).unwrap();
    assert_eq!(rt, *id, "MachineInstanceId round-trip");
}

fn assert_route_variant_id_roundtrip(variant: &RouteVariantId) {
    // Round-trip through the typed arm, not just the slug — asserting that
    // a signal-arm variant never silently migrates into the input arm.
    match variant {
        RouteVariantId::Input(id) => {
            let rt: InputVariantId = InputVariantId::parse(id.as_str()).unwrap();
            assert_eq!(
                rt, *id,
                "InputVariantId round-trip on RouteVariantId::Input"
            );
        }
        RouteVariantId::Signal(id) => {
            let rt: SignalVariantId = SignalVariantId::parse(id.as_str()).unwrap();
            assert_eq!(
                rt, *id,
                "SignalVariantId round-trip on RouteVariantId::Signal"
            );
        }
    }
}

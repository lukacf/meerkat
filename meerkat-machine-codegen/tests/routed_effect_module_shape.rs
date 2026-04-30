#![allow(clippy::expect_used)]

//! Track-B wave-b B-4b — emitted `meerkat_mob_seam` module shape.
//!
//! The per-composition emission must expose:
//!
//! * a `TypedRoutedInput` descriptor carrying typed `MachineInstanceId`,
//!   `InputVariantId`, `RouteId`, and `(FieldId, FieldId)` binding pairs,
//! * generated producer/effect/input/signal/field identity functions, and
//! * `route_to_input(instance, variant)` / `route_to_signal(instance, variant)`
//!   fact resolvers for Input-kind and Signal-kind routes.
//!
//! The shape is asserted via source-text matching. A full dispatcher
//! integration test that actually invokes the emitted `route_to_input`
//! belongs to Track-B B-5 (the runtime composition dispatcher), which
//! compiles the emitted module into the runtime crate.

use meerkat_machine_codegen::render_composition_driver;
use meerkat_machine_schema::catalog::meerkat_mob_seam_composition;
use meerkat_machine_schema::identity::{
    CompositionDriverId, EffectVariantId, InputVariantId, MachineInstanceId, RouteId,
};
use meerkat_machine_schema::{
    CompositionDriver, CompositionDriverRustBinding, DriverDispatchRoute, RouteTargetKind,
    RouteVariantId, WatchedEffect,
};

/// The canonical seam composition has `driver: None` on disk; the emitter
/// is driver-gated (xtask derives the output path from the driver's Rust
/// binding). Supply a trivial driver so the emitter produces the typed
/// module.
fn attach_stub_driver(
    mut schema: meerkat_machine_schema::CompositionSchema,
) -> meerkat_machine_schema::CompositionSchema {
    schema.driver = Some(CompositionDriver {
        name: CompositionDriverId::parse("meerkat_mob_seam_driver").expect("driver slug"),
        rust: CompositionDriverRustBinding {
            module_path: "meerkat-runtime/src/generated/meerkat_mob_seam.rs".into(),
            driver_type: "MeerkatMobSeamDriver".into(),
            store_plan_type: "MeerkatMobSeamStorePlan".into(),
            work_type: "MeerkatMobSeamWork".into(),
            decision_type: "MeerkatMobSeamDecision".into(),
            required_imports: vec![],
        },
        watched_effects: vec![WatchedEffect {
            producer_instance: MachineInstanceId::parse("mob").expect("instance slug"),
            effect_variant: EffectVariantId::parse("RequestRuntimeBinding").expect("effect slug"),
        }],
        dispatch_routes: vec![DriverDispatchRoute {
            name: RouteId::parse("binding_request_reaches_meerkat").expect("route slug"),
            target_instance: MachineInstanceId::parse("meerkat").expect("instance slug"),
            target_kind: RouteTargetKind::Input,
            input_variant: RouteVariantId::Input(
                InputVariantId::parse("PrepareBindings").expect("input slug"),
            ),
        }],
    });
    schema
}

#[test]
fn emitted_seam_module_declares_typed_routed_facts() {
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    // Typed identity imports are present — without these the emitted
    // module cannot compile against typed newtypes.
    assert!(
        rendered.contains(
            "use meerkat_machine_schema::identity::{CompositionId, EffectVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId, RouteId, SignalVariantId};"
        ),
        "missing typed-identity imports:\n{rendered}"
    );

    // TypedRoutedInput carries the three typed fields required by the
    // composition dispatcher: instance, variant, field bindings.
    assert!(
        rendered.contains("pub struct TypedRoutedInput"),
        "missing TypedRoutedInput struct:\n{rendered}"
    );
    for field in [
        "pub route_id: RouteId,",
        "pub instance_id: MachineInstanceId,",
        "pub variant: InputVariantId,",
        "pub bindings: Vec<(FieldId, FieldId)>,",
    ] {
        assert!(
            rendered.contains(field),
            "TypedRoutedInput must declare `{field}`:\n{rendered}"
        );
    }

    assert!(
        rendered.contains("pub struct TypedRoutedSignal"),
        "missing TypedRoutedSignal struct:\n{rendered}"
    );
    for field in [
        "pub route_id: RouteId,",
        "pub instance_id: MachineInstanceId,",
        "pub variant: SignalVariantId,",
        "pub bindings: Vec<(FieldId, FieldId)>,",
    ] {
        assert!(
            rendered.contains(field),
            "TypedRoutedSignal must declare `{field}`:\n{rendered}"
        );
    }
}

#[test]
fn emitted_seam_module_declares_generated_identity_modules() {
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    assert!(
        rendered.contains("pub struct ProducerFacts"),
        "missing ProducerFacts struct:\n{rendered}"
    );
    assert!(
        rendered.contains("pub mod producers"),
        "missing producer facts module:\n{rendered}"
    );
    assert!(
        rendered.contains("pub mod effects"),
        "missing effect facts module:\n{rendered}"
    );
    assert!(
        rendered.contains("pub mod fields"),
        "missing field facts module:\n{rendered}"
    );
    assert!(
        !rendered.contains("pub enum MeerkatMobSeamEffect"),
        "generated-shape seam effect mirror must not be emitted:\n{rendered}"
    );
    assert!(
        !rendered.contains("crate::generated::mob::Effect"),
        "emitted facts must not depend on generated-shape mob effects:\n{rendered}"
    );
}

#[test]
fn emitted_seam_module_declares_route_to_input_with_expected_signature() {
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    assert!(
        rendered.contains("pub fn route_to_input("),
        "missing route_to_input signature:\n{rendered}"
    );
    assert!(
        rendered.contains("producer_instance: &MachineInstanceId,"),
        "route_to_input must accept a producer instance id:\n{rendered}"
    );
    assert!(
        rendered.contains("effect_variant: &EffectVariantId,"),
        "route_to_input must accept an effect variant id:\n{rendered}"
    );
}

#[test]
fn route_to_input_arm_for_request_runtime_binding_targets_prepare_bindings() {
    // A Mob-producer `RequestRuntimeBinding` variant must route to
    // `meerkat.PrepareBindings`. The generated module exposes this as
    // facts, without matching against a generated-shape payload enum.
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    let arm = "pub fn route_binding_request_reaches_meerkat() -> TypedRoutedInput";
    assert!(
        rendered.contains(arm),
        "route fact must be emitted for Mob RequestRuntimeBinding:\n\
         expected substring: `{arm}`\n\
         rendered:\n{rendered}"
    );

    assert!(
        rendered.contains("MachineInstanceId::parse(\"meerkat\")"),
        "route arm must target the meerkat instance:\n{rendered}"
    );
    assert!(
        rendered.contains("InputVariantId::parse(\"PrepareBindings\")"),
        "route arm must target the PrepareBindings input variant:\n{rendered}"
    );
    assert!(
        rendered.contains("effects::mob::request_runtime_binding()"),
        "route_to_input must resolve through the generated effect variant fact:\n{rendered}"
    );
    for (producer_field, consumer_field) in [
        ("agent_runtime_id", "agent_runtime_id"),
        ("fence_token", "fence_token"),
        ("generation", "generation"),
    ] {
        let binding_line = format!(
            "(FieldId::parse(\"{producer_field}\").expect(\"route producer field slug\"), FieldId::parse(\"{consumer_field}\").expect(\"route consumer field slug\")),"
        );
        assert!(
            rendered.contains(&binding_line),
            "route arm must carry producer→consumer field binding:\n\
             expected: `{binding_line}`\n\
             rendered:\n{rendered}"
        );
    }
}

#[test]
fn signal_kind_routes_are_emitted_through_route_to_signal() {
    // `meerkat_mob_seam` declares Signal-kind routes like
    // `runtime_bound_reaches_mob` → `mob.ObserveRuntimeReady`. These are
    // resolved by the generated signal surface, not the input resolver.
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    assert!(
        rendered.contains("pub fn route_to_signal("),
        "missing route_to_signal signature:\n{rendered}"
    );
    for signal_variant in [
        "ObserveRuntimeReady",
        "ObserveRuntimeRetired",
        "ObserveRuntimeDestroyed",
    ] {
        assert!(
            !rendered.contains(&format!("InputVariantId::parse(\"{signal_variant}\")")),
            "signal-kind target `{signal_variant}` must not appear in route_to_input:\n{rendered}"
        );
        assert!(
            rendered.contains(&format!("SignalVariantId::parse(\"{signal_variant}\")")),
            "signal-kind target `{signal_variant}` must appear in route_to_signal:\n{rendered}"
        );
    }
    assert!(
        rendered.contains("pub fn route_runtime_bound_reaches_mob() -> TypedRoutedSignal"),
        "runtime-bound signal route fact must be emitted:\n{rendered}"
    );
}

#[test]
fn emitted_seam_module_carries_generated_marker_and_source_pointer() {
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    assert!(
        rendered.contains("@generated — composition module for `meerkat_mob_seam`"),
        "missing @generated marker:\n{rendered}"
    );
    assert!(
        rendered.contains("#![allow(clippy::expect_used)]"),
        "generated seam facts should be lint-clean under workspace clippy denies:\n{rendered}"
    );
    assert!(
        rendered.contains("Source of truth: catalog::compositions::meerkat_mob_seam"),
        "missing source-of-truth pointer:\n{rendered}"
    );
}

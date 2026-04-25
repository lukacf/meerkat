#![allow(clippy::expect_used)]

//! Track-B wave-b B-4b — emitted `meerkat_mob_seam` module shape.
//!
//! The per-composition emission must expose:
//!
//! * a top-level `MeerkatMobSeamEffect` enum with one variant per
//!   participant machine instance, each wrapping the producer machine's
//!   generated `Effect` enum,
//! * a `TypedRoutedInput` descriptor carrying typed `MachineInstanceId`,
//!   `InputVariantId`, and `(FieldId, FieldId)` binding pairs, and
//! * a `route_to_input(&MeerkatMobSeamEffect) -> Option<TypedRoutedInput>`
//!   function whose match arms resolve each declared Input-kind route to
//!   its typed consumer input,
//! * a `TypedRoutedSignal` descriptor plus `route_to_signal(...)` for
//!   Signal-kind routes.
//!
//! The shape is asserted via source-text matching. A full dispatcher
//! integration test that actually invokes the emitted `route_to_input`
//! belongs to Track-B B-5 (the runtime composition dispatcher), which
//! compiles the emitted module into the runtime crate.

use meerkat_machine_codegen::render_composition_driver;
use meerkat_machine_schema::catalog::meerkat_mob_seam_composition;
use meerkat_machine_schema::identity::{
    EffectVariantId, InputVariantId, MachineInstanceId, RouteId,
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
        name: "meerkat_mob_seam_driver".into(),
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
fn emitted_seam_module_declares_typed_routed_input_and_seam_effect_enum() {
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    // Typed identity imports are present — without these the emitted
    // module cannot compile against typed newtypes.
    assert!(
        rendered.contains(
            "use meerkat_machine_schema::identity::{FieldId, InputVariantId, MachineInstanceId, SignalVariantId};"
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
fn emitted_seam_module_declares_seam_effect_enum_with_producer_variants() {
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    assert!(
        rendered.contains("pub enum MeerkatMobSeamEffect"),
        "missing seam-effect enum:\n{rendered}"
    );
    // One variant per participant machine instance, wrapping the
    // producer's generated `Effect` enum.
    assert!(
        rendered.contains("Meerkat(crate::generated::meerkat::Effect),"),
        "missing Meerkat producer variant:\n{rendered}"
    );
    assert!(
        rendered.contains("Mob(crate::generated::mob::Effect),"),
        "missing Mob producer variant:\n{rendered}"
    );
}

#[test]
fn emitted_seam_module_declares_route_to_input_with_expected_signature() {
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    assert!(
        rendered.contains(
            "pub fn route_to_input(effect: &MeerkatMobSeamEffect) -> Option<TypedRoutedInput>"
        ),
        "missing route_to_input signature:\n{rendered}"
    );
}

#[test]
fn route_to_input_arm_for_request_runtime_binding_targets_prepare_bindings() {
    // A hand-crafted Mob-producer `RequestRuntimeBinding` effect must
    // route to `meerkat.PrepareBindings`. We can't invoke the emitted
    // function without compiling it, so we assert the match arm's
    // structure: the producer variant matches on
    // `crate::generated::mob::Effect::RequestRuntimeBinding(_)` and the
    // arm constructs a `TypedRoutedInput` targeting the
    // `meerkat` instance + `PrepareBindings` variant, with the three
    // field bindings declared on the route.
    let schema = attach_stub_driver(meerkat_mob_seam_composition());
    let rendered = render_composition_driver(&schema).expect("seam composition emits");

    let arm = "crate::generated::mob::Effect::RequestRuntimeBinding(_) => Some(TypedRoutedInput {";
    assert!(
        rendered.contains(arm),
        "route_to_input must match on the Mob RequestRuntimeBinding variant:\n\
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
        rendered.contains(
            "pub fn route_to_signal(effect: &MeerkatMobSeamEffect) -> Option<TypedRoutedSignal>"
        ),
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
        rendered.contains("Source of truth: catalog::compositions::meerkat_mob_seam"),
        "missing source-of-truth pointer:\n{rendered}"
    );
}

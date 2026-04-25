// @generated — composition module for `meerkat_mob_seam`
// DO NOT EDIT. Emitted by meerkat_machine_codegen::render_composition_driver.
// Source of truth: catalog::compositions::meerkat_mob_seam
// Driver: `meerkat_mob_seam_driver` (rust path: `meerkat-runtime/src/generated/meerkat_mob_seam.rs`).

use meerkat_machine_schema::identity::{
    FieldId, InputVariantId, MachineInstanceId, SignalVariantId,
};

/// Typed route descriptor resolved for a producer effect.
///
/// `bindings` lists producer-field → consumer-field pairs in the
/// order declared by the composition schema. The composition
/// dispatcher uses these to construct the typed consumer input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedRoutedInput {
    pub instance_id: MachineInstanceId,
    pub variant: InputVariantId,
    pub bindings: Vec<(FieldId, FieldId)>,
}

/// Typed signal-route descriptor resolved for a producer effect.
///
/// `bindings` lists producer-field → consumer-field pairs in the
/// order declared by the composition schema. The signal dispatcher
/// uses these to construct the typed consumer signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedRoutedSignal {
    pub instance_id: MachineInstanceId,
    pub variant: SignalVariantId,
    pub bindings: Vec<(FieldId, FieldId)>,
}

/// Sum of every participant-machine effect type that can be routed
/// through this composition. One variant per producer instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeerkatMobSeamEffect {
    Meerkat(crate::generated::meerkat::Effect),
    Mob(crate::generated::mob::Effect),
}

/// Resolve a routed producer effect to its typed consumer input.
///
/// Returns `None` when the effect variant has no declared input
/// route in this composition (including signal-kind routes, which
/// are handled by `route_to_signal`).
pub fn route_to_input(effect: &MeerkatMobSeamEffect) -> Option<TypedRoutedInput> {
    match effect {
        MeerkatMobSeamEffect::Meerkat(_) => None,
        MeerkatMobSeamEffect::Mob(inner) => match inner {
            crate::generated::mob::Effect::RequestRuntimeBinding(_) => Some(TypedRoutedInput {
                instance_id: MachineInstanceId::parse("meerkat")
                    .expect("composition instance slug"),
                variant: InputVariantId::parse("PrepareBindings").expect("composition input slug"),
                bindings: vec![
                    (
                        FieldId::parse("agent_runtime_id").expect("route producer field slug"),
                        FieldId::parse("agent_runtime_id").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("fence_token").expect("route producer field slug"),
                        FieldId::parse("fence_token").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("generation").expect("route producer field slug"),
                        FieldId::parse("generation").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("session_id").expect("route producer field slug"),
                        FieldId::parse("session_id").expect("route consumer field slug"),
                    ),
                ],
            }),
            crate::generated::mob::Effect::RequestRuntimeIngress(_) => Some(TypedRoutedInput {
                instance_id: MachineInstanceId::parse("meerkat")
                    .expect("composition instance slug"),
                variant: InputVariantId::parse("Ingest").expect("composition input slug"),
                bindings: vec![
                    (
                        FieldId::parse("agent_runtime_id").expect("route producer field slug"),
                        FieldId::parse("runtime_id").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("work_id").expect("route producer field slug"),
                        FieldId::parse("work_id").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("origin").expect("route producer field slug"),
                        FieldId::parse("origin").expect("route consumer field slug"),
                    ),
                ],
            }),
            crate::generated::mob::Effect::RequestRuntimeRetire(_) => Some(TypedRoutedInput {
                instance_id: MachineInstanceId::parse("meerkat")
                    .expect("composition instance slug"),
                variant: InputVariantId::parse("Retire").expect("composition input slug"),
                bindings: vec![(
                    FieldId::parse("session_id").expect("route producer field slug"),
                    FieldId::parse("session_id").expect("route consumer field slug"),
                )],
            }),
            crate::generated::mob::Effect::RequestRuntimeDestroy(_) => Some(TypedRoutedInput {
                instance_id: MachineInstanceId::parse("meerkat")
                    .expect("composition instance slug"),
                variant: InputVariantId::parse("Destroy").expect("composition input slug"),
                bindings: vec![(
                    FieldId::parse("session_id").expect("route producer field slug"),
                    FieldId::parse("session_id").expect("route consumer field slug"),
                )],
            }),
            _ => None,
        },
    }
}

/// Resolve a routed producer effect to its typed consumer signal.
///
/// Returns `None` when the effect variant has no declared signal
/// route in this composition.
pub fn route_to_signal(effect: &MeerkatMobSeamEffect) -> Option<TypedRoutedSignal> {
    match effect {
        MeerkatMobSeamEffect::Meerkat(inner) => match inner {
            crate::generated::meerkat::Effect::RuntimeBound(_) => Some(TypedRoutedSignal {
                instance_id: MachineInstanceId::parse("mob").expect("composition instance slug"),
                variant: SignalVariantId::parse("ObserveRuntimeReady")
                    .expect("composition signal slug"),
                bindings: vec![
                    (
                        FieldId::parse("agent_runtime_id").expect("route producer field slug"),
                        FieldId::parse("agent_runtime_id").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("fence_token").expect("route producer field slug"),
                        FieldId::parse("fence_token").expect("route consumer field slug"),
                    ),
                ],
            }),
            crate::generated::meerkat::Effect::RuntimeRetired(_) => Some(TypedRoutedSignal {
                instance_id: MachineInstanceId::parse("mob").expect("composition instance slug"),
                variant: SignalVariantId::parse("ObserveRuntimeRetired")
                    .expect("composition signal slug"),
                bindings: vec![
                    (
                        FieldId::parse("agent_runtime_id").expect("route producer field slug"),
                        FieldId::parse("agent_runtime_id").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("fence_token").expect("route producer field slug"),
                        FieldId::parse("fence_token").expect("route consumer field slug"),
                    ),
                ],
            }),
            crate::generated::meerkat::Effect::RuntimeDestroyed(_) => Some(TypedRoutedSignal {
                instance_id: MachineInstanceId::parse("mob").expect("composition instance slug"),
                variant: SignalVariantId::parse("ObserveRuntimeDestroyed")
                    .expect("composition signal slug"),
                bindings: vec![
                    (
                        FieldId::parse("agent_runtime_id").expect("route producer field slug"),
                        FieldId::parse("agent_runtime_id").expect("route consumer field slug"),
                    ),
                    (
                        FieldId::parse("fence_token").expect("route producer field slug"),
                        FieldId::parse("fence_token").expect("route consumer field slug"),
                    ),
                ],
            }),
            _ => None,
        },
        MeerkatMobSeamEffect::Mob(_) => None,
    }
}

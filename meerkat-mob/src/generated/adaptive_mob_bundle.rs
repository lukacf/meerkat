// @generated — composition module for `adaptive_mob_bundle`
// DO NOT EDIT. Emitted by meerkat_machine_codegen::render_composition_driver.
// Source of truth: catalog::compositions::adaptive_mob_bundle
// Driver: `adaptive_mob_bundle_driver` (rust path: `meerkat-mob/src/generated/adaptive_mob_bundle.rs`).
#![allow(clippy::expect_used)]

use meerkat_machine_schema::identity::{
    CompositionId, EffectVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId, RouteId,
    SignalVariantId,
};

/// Typed route descriptor resolved from generated composition facts.
///
/// `bindings` lists producer-field → consumer-field pairs in the
/// order declared by the composition schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedRoutedInput {
    pub route_id: RouteId,
    pub instance_id: MachineInstanceId,
    pub variant: InputVariantId,
    pub bindings: Vec<(FieldId, FieldId)>,
}

/// Typed signal-route descriptor resolved from generated composition facts.
///
/// `bindings` lists producer-field → consumer-field pairs in the
/// order declared by the composition schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedRoutedSignal {
    pub route_id: RouteId,
    pub instance_id: MachineInstanceId,
    pub variant: SignalVariantId,
    pub bindings: Vec<(FieldId, FieldId)>,
}

/// Generated producer identity declared by this composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducerFacts {
    pub instance_id: MachineInstanceId,
    pub machine: MachineId,
}

/// Composition id for this generated fact set.
pub fn composition_id() -> CompositionId {
    CompositionId::parse("adaptive_mob_bundle").expect("composition slug")
}

/// Generated producer instance facts.
pub mod producers {
    use super::*;

    pub fn control_mob() -> ProducerFacts {
        ProducerFacts {
            instance_id: control_mob_instance_id(),
            machine: control_mob_machine_id(),
        }
    }

    pub fn control_mob_instance_id() -> MachineInstanceId {
        MachineInstanceId::parse("control_mob").expect("producer instance slug")
    }

    pub fn control_mob_machine_id() -> MachineId {
        MachineId::parse("MobMachine").expect("producer machine slug")
    }

    pub fn layer_mob() -> ProducerFacts {
        ProducerFacts {
            instance_id: layer_mob_instance_id(),
            machine: layer_mob_machine_id(),
        }
    }

    pub fn layer_mob_instance_id() -> MachineInstanceId {
        MachineInstanceId::parse("layer_mob").expect("producer instance slug")
    }

    pub fn layer_mob_machine_id() -> MachineId {
        MachineId::parse("MobMachine").expect("producer machine slug")
    }
}

/// Generated producer-side effect variant ids.
pub mod effects {

    pub mod control_mob {
        use super::super::*;
    }

    pub mod layer_mob {
        use super::super::*;
    }
}

/// Generated consumer input variant ids.
pub mod inputs {
    use super::*;
}

/// Generated consumer signal variant ids.
pub mod signals {
    use super::*;
}

/// Generated field ids referenced by route bindings.
pub mod fields {
    use super::*;
}

/// Resolve a producer `(instance, effect_variant)` to generated input-route facts.
pub fn route_to_input(
    producer_instance: &MachineInstanceId,
    effect_variant: &EffectVariantId,
) -> Option<TypedRoutedInput> {
    None
}

/// Resolve a producer `(instance, effect_variant)` to generated signal-route facts.
pub fn route_to_signal(
    producer_instance: &MachineInstanceId,
    effect_variant: &EffectVariantId,
) -> Option<TypedRoutedSignal> {
    None
}

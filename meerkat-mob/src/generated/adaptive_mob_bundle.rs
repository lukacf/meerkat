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

        pub fn flow_run_public_result_classified() -> EffectVariantId {
            EffectVariantId::parse("FlowRunPublicResultClassified").expect("effect variant slug")
        }
    }
}

/// Generated consumer input variant ids.
pub mod inputs {
    use super::*;

    pub fn ingest_layer_terminal() -> InputVariantId {
        InputVariantId::parse("IngestLayerTerminal").expect("input variant slug")
    }
}

/// Generated consumer signal variant ids.
pub mod signals {
    use super::*;
}

/// Generated field ids referenced by route bindings.
pub mod fields {
    use super::*;

    pub fn actual_tokens() -> FieldId {
        FieldId::parse("actual_tokens").expect("field slug")
    }

    pub fn actual_tool_calls() -> FieldId {
        FieldId::parse("actual_tool_calls").expect("field slug")
    }

    pub fn adaptive_run_id() -> FieldId {
        FieldId::parse("adaptive_run_id").expect("field slug")
    }

    pub fn attempt() -> FieldId {
        FieldId::parse("attempt").expect("field slug")
    }

    pub fn layer_id() -> FieldId {
        FieldId::parse("layer_id").expect("field slug")
    }

    pub fn result() -> FieldId {
        FieldId::parse("result").expect("field slug")
    }

    pub fn result_class() -> FieldId {
        FieldId::parse("result_class").expect("field slug")
    }
}

/// Generated facts for input route `layer_terminal_reaches_adaptive_kernel`.
pub fn route_layer_terminal_reaches_adaptive_kernel() -> TypedRoutedInput {
    TypedRoutedInput {
        route_id: RouteId::parse("layer_terminal_reaches_adaptive_kernel").expect("route slug"),
        instance_id: MachineInstanceId::parse("control_mob").expect("composition instance slug"),
        variant: InputVariantId::parse("IngestLayerTerminal").expect("composition input slug"),
        bindings: vec![(
            FieldId::parse("result").expect("route producer field slug"),
            FieldId::parse("result_class").expect("route consumer field slug"),
        )],
    }
}

/// Resolve a producer `(instance, effect_variant)` to generated input-route facts.
pub fn route_to_input(
    producer_instance: &MachineInstanceId,
    effect_variant: &EffectVariantId,
) -> Option<TypedRoutedInput> {
    if producer_instance == &producers::layer_mob_instance_id()
        && effect_variant == &effects::layer_mob::flow_run_public_result_classified()
    {
        return Some(route_layer_terminal_reaches_adaptive_kernel());
    }
    None
}

/// Resolve a producer `(instance, effect_variant)` to generated signal-route facts.
pub fn route_to_signal(
    producer_instance: &MachineInstanceId,
    effect_variant: &EffectVariantId,
) -> Option<TypedRoutedSignal> {
    None
}

/// Generated route target selected by `AdaptiveMobBundleDriver`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratedRouteTarget {
    Input(TypedRoutedInput),
    Signal(TypedRoutedSignal),
}

/// Generated store plan emitted by `AdaptiveMobBundleDriver`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveMobBundleStorePlan {
    pub target: GeneratedRouteTarget,
}

impl AdaptiveMobBundleStorePlan {
    pub fn input(route: TypedRoutedInput) -> Self {
        Self {
            target: GeneratedRouteTarget::Input(route),
        }
    }

    pub fn signal(route: TypedRoutedSignal) -> Self {
        Self {
            target: GeneratedRouteTarget::Signal(route),
        }
    }

    pub fn route_id(&self) -> &RouteId {
        match &self.target {
            GeneratedRouteTarget::Input(route) => &route.route_id,
            GeneratedRouteTarget::Signal(route) => &route.route_id,
        }
    }
}

/// Generated work packet consumed by `AdaptiveMobBundleDriver`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdaptiveMobBundleWork {
    pub producer_instance: MachineInstanceId,
    pub effect_variant: EffectVariantId,
}

impl AdaptiveMobBundleWork {
    pub fn new(producer_instance: MachineInstanceId, effect_variant: EffectVariantId) -> Self {
        Self {
            producer_instance,
            effect_variant,
        }
    }
}

/// Generated routing decision emitted by `AdaptiveMobBundleDriver`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdaptiveMobBundleDecision {
    DispatchInput(TypedRoutedInput),
    DispatchSignal(TypedRoutedSignal),
    NoRoute,
}

/// Generated composition driver constrained by the catalog driver descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveMobBundleDriver;

impl AdaptiveMobBundleDriver {
    pub fn decide(work: &AdaptiveMobBundleWork) -> AdaptiveMobBundleDecision {
        if let Some(route) = route_to_input(&work.producer_instance, &work.effect_variant) {
            return AdaptiveMobBundleDecision::DispatchInput(route);
        }
        if let Some(route) = route_to_signal(&work.producer_instance, &work.effect_variant) {
            return AdaptiveMobBundleDecision::DispatchSignal(route);
        }
        AdaptiveMobBundleDecision::NoRoute
    }

    pub fn store_plan(decision: AdaptiveMobBundleDecision) -> Option<AdaptiveMobBundleStorePlan> {
        match decision {
            AdaptiveMobBundleDecision::DispatchInput(route) => {
                Some(AdaptiveMobBundleStorePlan::input(route))
            }
            AdaptiveMobBundleDecision::DispatchSignal(route) => {
                Some(AdaptiveMobBundleStorePlan::signal(route))
            }
            AdaptiveMobBundleDecision::NoRoute => None,
        }
    }
}

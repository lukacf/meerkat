// @generated — composition module for `meerkat_mob_seam`
// DO NOT EDIT. Emitted by meerkat_machine_codegen::render_composition_driver.
// Source of truth: catalog::compositions::meerkat_mob_seam
// Driver: `meerkat_mob_seam_driver` (rust path: `meerkat-runtime/src/generated/meerkat_mob_seam.rs`).
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
    CompositionId::parse("meerkat_mob_seam").expect("composition slug")
}

/// Generated producer instance facts.
pub mod producers {
    use super::*;

    pub fn meerkat() -> ProducerFacts {
        ProducerFacts {
            instance_id: meerkat_instance_id(),
            machine: meerkat_machine_id(),
        }
    }

    pub fn meerkat_instance_id() -> MachineInstanceId {
        MachineInstanceId::parse("meerkat").expect("producer instance slug")
    }

    pub fn meerkat_machine_id() -> MachineId {
        MachineId::parse("MeerkatMachine").expect("producer machine slug")
    }

    pub fn mob() -> ProducerFacts {
        ProducerFacts {
            instance_id: mob_instance_id(),
            machine: mob_machine_id(),
        }
    }

    pub fn mob_instance_id() -> MachineInstanceId {
        MachineInstanceId::parse("mob").expect("producer instance slug")
    }

    pub fn mob_machine_id() -> MachineId {
        MachineId::parse("MobMachine").expect("producer machine slug")
    }
}

/// Generated producer-side effect variant ids.
pub mod effects {

    pub mod meerkat {
        use super::super::*;

        pub fn runtime_bound() -> EffectVariantId {
            EffectVariantId::parse("RuntimeBound").expect("effect variant slug")
        }

        pub fn runtime_destroyed() -> EffectVariantId {
            EffectVariantId::parse("RuntimeDestroyed").expect("effect variant slug")
        }

        pub fn runtime_retired() -> EffectVariantId {
            EffectVariantId::parse("RuntimeRetired").expect("effect variant slug")
        }
    }

    pub mod mob {
        use super::super::*;

        pub fn request_runtime_binding() -> EffectVariantId {
            EffectVariantId::parse("RequestRuntimeBinding").expect("effect variant slug")
        }

        pub fn request_runtime_destroy() -> EffectVariantId {
            EffectVariantId::parse("RequestRuntimeDestroy").expect("effect variant slug")
        }

        pub fn request_runtime_ingress() -> EffectVariantId {
            EffectVariantId::parse("RequestRuntimeIngress").expect("effect variant slug")
        }

        pub fn request_runtime_retire() -> EffectVariantId {
            EffectVariantId::parse("RequestRuntimeRetire").expect("effect variant slug")
        }
    }
}

/// Generated consumer input variant ids.
pub mod inputs {
    use super::*;

    pub fn destroy() -> InputVariantId {
        InputVariantId::parse("Destroy").expect("input variant slug")
    }

    pub fn ingest() -> InputVariantId {
        InputVariantId::parse("Ingest").expect("input variant slug")
    }

    pub fn prepare_bindings() -> InputVariantId {
        InputVariantId::parse("PrepareBindings").expect("input variant slug")
    }

    pub fn retire() -> InputVariantId {
        InputVariantId::parse("Retire").expect("input variant slug")
    }
}

/// Generated consumer signal variant ids.
pub mod signals {
    use super::*;

    pub fn observe_runtime_destroyed() -> SignalVariantId {
        SignalVariantId::parse("ObserveRuntimeDestroyed").expect("signal variant slug")
    }

    pub fn observe_runtime_ready() -> SignalVariantId {
        SignalVariantId::parse("ObserveRuntimeReady").expect("signal variant slug")
    }

    pub fn observe_runtime_retired() -> SignalVariantId {
        SignalVariantId::parse("ObserveRuntimeRetired").expect("signal variant slug")
    }
}

/// Generated field ids referenced by route bindings.
pub mod fields {
    use super::*;

    pub fn agent_runtime_id() -> FieldId {
        FieldId::parse("agent_runtime_id").expect("field slug")
    }

    pub fn fence_token() -> FieldId {
        FieldId::parse("fence_token").expect("field slug")
    }

    pub fn generation() -> FieldId {
        FieldId::parse("generation").expect("field slug")
    }

    pub fn origin() -> FieldId {
        FieldId::parse("origin").expect("field slug")
    }

    pub fn runtime_id() -> FieldId {
        FieldId::parse("runtime_id").expect("field slug")
    }

    pub fn session_id() -> FieldId {
        FieldId::parse("session_id").expect("field slug")
    }

    pub fn work_id() -> FieldId {
        FieldId::parse("work_id").expect("field slug")
    }
}

/// Generated adapters for production seam code.
/// These enums are the typed bridge between domain effect/input/signal
/// values and schema-owned route, variant, and field facts.
pub mod adapters {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Field {
        AgentRuntimeId,
        FenceToken,
        Generation,
        Origin,
        RuntimeId,
        SessionId,
        WorkId,
    }

    impl Field {
        pub fn id(self) -> FieldId {
            match self {
                Self::AgentRuntimeId => fields::agent_runtime_id(),
                Self::FenceToken => fields::fence_token(),
                Self::Generation => fields::generation(),
                Self::Origin => fields::origin(),
                Self::RuntimeId => fields::runtime_id(),
                Self::SessionId => fields::session_id(),
                Self::WorkId => fields::work_id(),
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Input {
        Destroy,
        Ingest,
        PrepareBindings,
        Retire,
    }

    impl Input {
        pub fn from_variant(id: &InputVariantId) -> Option<Self> {
            if id == &inputs::destroy() {
                return Some(Self::Destroy);
            }
            if id == &inputs::ingest() {
                return Some(Self::Ingest);
            }
            if id == &inputs::prepare_bindings() {
                return Some(Self::PrepareBindings);
            }
            if id == &inputs::retire() {
                return Some(Self::Retire);
            }
            None
        }

        pub fn variant_id(self) -> InputVariantId {
            match self {
                Self::Destroy => inputs::destroy(),
                Self::Ingest => inputs::ingest(),
                Self::PrepareBindings => inputs::prepare_bindings(),
                Self::Retire => inputs::retire(),
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Signal {
        ObserveRuntimeDestroyed,
        ObserveRuntimeReady,
        ObserveRuntimeRetired,
    }

    impl Signal {
        pub fn from_variant(id: &SignalVariantId) -> Option<Self> {
            if id == &signals::observe_runtime_destroyed() {
                return Some(Self::ObserveRuntimeDestroyed);
            }
            if id == &signals::observe_runtime_ready() {
                return Some(Self::ObserveRuntimeReady);
            }
            if id == &signals::observe_runtime_retired() {
                return Some(Self::ObserveRuntimeRetired);
            }
            None
        }

        pub fn variant_id(self) -> SignalVariantId {
            match self {
                Self::ObserveRuntimeDestroyed => signals::observe_runtime_destroyed(),
                Self::ObserveRuntimeReady => signals::observe_runtime_ready(),
                Self::ObserveRuntimeRetired => signals::observe_runtime_retired(),
            }
        }
    }

    pub mod meerkat {
        use super::super::*;
        use super::Field;

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Effect {
            RuntimeBound,
            RuntimeDestroyed,
            RuntimeRetired,
        }

        impl Effect {
            pub fn variant_id(self) -> EffectVariantId {
                match self {
                    Self::RuntimeBound => effects::meerkat::runtime_bound(),
                    Self::RuntimeDestroyed => effects::meerkat::runtime_destroyed(),
                    Self::RuntimeRetired => effects::meerkat::runtime_retired(),
                }
            }

            pub fn input_route(self) -> Option<TypedRoutedInput> {
                route_to_input(&producers::meerkat_instance_id(), &self.variant_id())
            }

            pub fn signal_route(self) -> Option<TypedRoutedSignal> {
                route_to_signal(&producers::meerkat_instance_id(), &self.variant_id())
            }

            pub fn field(self, id: &FieldId) -> Option<Field> {
                match self {
                    Self::RuntimeBound => {
                        if id == &Field::AgentRuntimeId.id() {
                            return Some(Field::AgentRuntimeId);
                        }
                        if id == &Field::FenceToken.id() {
                            return Some(Field::FenceToken);
                        }
                        None
                    }
                    Self::RuntimeDestroyed => {
                        if id == &Field::AgentRuntimeId.id() {
                            return Some(Field::AgentRuntimeId);
                        }
                        if id == &Field::FenceToken.id() {
                            return Some(Field::FenceToken);
                        }
                        None
                    }
                    Self::RuntimeRetired => {
                        if id == &Field::AgentRuntimeId.id() {
                            return Some(Field::AgentRuntimeId);
                        }
                        if id == &Field::FenceToken.id() {
                            return Some(Field::FenceToken);
                        }
                        None
                    }
                }
            }
        }
    }

    pub mod mob {
        use super::super::*;
        use super::Field;

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Effect {
            RequestRuntimeBinding,
            RequestRuntimeDestroy,
            RequestRuntimeIngress,
            RequestRuntimeRetire,
        }

        impl Effect {
            pub fn variant_id(self) -> EffectVariantId {
                match self {
                    Self::RequestRuntimeBinding => effects::mob::request_runtime_binding(),
                    Self::RequestRuntimeDestroy => effects::mob::request_runtime_destroy(),
                    Self::RequestRuntimeIngress => effects::mob::request_runtime_ingress(),
                    Self::RequestRuntimeRetire => effects::mob::request_runtime_retire(),
                }
            }

            pub fn input_route(self) -> Option<TypedRoutedInput> {
                route_to_input(&producers::mob_instance_id(), &self.variant_id())
            }

            pub fn signal_route(self) -> Option<TypedRoutedSignal> {
                route_to_signal(&producers::mob_instance_id(), &self.variant_id())
            }

            pub fn field(self, id: &FieldId) -> Option<Field> {
                match self {
                    Self::RequestRuntimeBinding => {
                        if id == &Field::AgentRuntimeId.id() {
                            return Some(Field::AgentRuntimeId);
                        }
                        if id == &Field::FenceToken.id() {
                            return Some(Field::FenceToken);
                        }
                        if id == &Field::Generation.id() {
                            return Some(Field::Generation);
                        }
                        if id == &Field::SessionId.id() {
                            return Some(Field::SessionId);
                        }
                        None
                    }
                    Self::RequestRuntimeDestroy => {
                        if id == &Field::SessionId.id() {
                            return Some(Field::SessionId);
                        }
                        None
                    }
                    Self::RequestRuntimeIngress => {
                        if id == &Field::AgentRuntimeId.id() {
                            return Some(Field::AgentRuntimeId);
                        }
                        if id == &Field::Origin.id() {
                            return Some(Field::Origin);
                        }
                        if id == &Field::WorkId.id() {
                            return Some(Field::WorkId);
                        }
                        None
                    }
                    Self::RequestRuntimeRetire => {
                        if id == &Field::SessionId.id() {
                            return Some(Field::SessionId);
                        }
                        None
                    }
                }
            }
        }
    }
}

/// Generated facts for input route `binding_request_reaches_meerkat`.
pub fn route_binding_request_reaches_meerkat() -> TypedRoutedInput {
    TypedRoutedInput {
        route_id: RouteId::parse("binding_request_reaches_meerkat").expect("route slug"),
        instance_id: MachineInstanceId::parse("meerkat").expect("composition instance slug"),
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
    }
}

/// Generated facts for input route `work_request_reaches_meerkat`.
pub fn route_work_request_reaches_meerkat() -> TypedRoutedInput {
    TypedRoutedInput {
        route_id: RouteId::parse("work_request_reaches_meerkat").expect("route slug"),
        instance_id: MachineInstanceId::parse("meerkat").expect("composition instance slug"),
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
    }
}

/// Generated facts for input route `retire_request_reaches_meerkat`.
pub fn route_retire_request_reaches_meerkat() -> TypedRoutedInput {
    TypedRoutedInput {
        route_id: RouteId::parse("retire_request_reaches_meerkat").expect("route slug"),
        instance_id: MachineInstanceId::parse("meerkat").expect("composition instance slug"),
        variant: InputVariantId::parse("Retire").expect("composition input slug"),
        bindings: vec![(
            FieldId::parse("session_id").expect("route producer field slug"),
            FieldId::parse("session_id").expect("route consumer field slug"),
        )],
    }
}

/// Generated facts for input route `destroy_request_reaches_meerkat`.
pub fn route_destroy_request_reaches_meerkat() -> TypedRoutedInput {
    TypedRoutedInput {
        route_id: RouteId::parse("destroy_request_reaches_meerkat").expect("route slug"),
        instance_id: MachineInstanceId::parse("meerkat").expect("composition instance slug"),
        variant: InputVariantId::parse("Destroy").expect("composition input slug"),
        bindings: vec![(
            FieldId::parse("session_id").expect("route producer field slug"),
            FieldId::parse("session_id").expect("route consumer field slug"),
        )],
    }
}

/// Generated facts for signal route `runtime_bound_reaches_mob`.
pub fn route_runtime_bound_reaches_mob() -> TypedRoutedSignal {
    TypedRoutedSignal {
        route_id: RouteId::parse("runtime_bound_reaches_mob").expect("route slug"),
        instance_id: MachineInstanceId::parse("mob").expect("composition instance slug"),
        variant: SignalVariantId::parse("ObserveRuntimeReady").expect("composition signal slug"),
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
    }
}

/// Generated facts for signal route `runtime_retired_reaches_mob`.
pub fn route_runtime_retired_reaches_mob() -> TypedRoutedSignal {
    TypedRoutedSignal {
        route_id: RouteId::parse("runtime_retired_reaches_mob").expect("route slug"),
        instance_id: MachineInstanceId::parse("mob").expect("composition instance slug"),
        variant: SignalVariantId::parse("ObserveRuntimeRetired").expect("composition signal slug"),
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
    }
}

/// Generated facts for signal route `runtime_destroyed_reaches_mob`.
pub fn route_runtime_destroyed_reaches_mob() -> TypedRoutedSignal {
    TypedRoutedSignal {
        route_id: RouteId::parse("runtime_destroyed_reaches_mob").expect("route slug"),
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
    }
}

/// Resolve a producer `(instance, effect_variant)` to generated input-route facts.
pub fn route_to_input(
    producer_instance: &MachineInstanceId,
    effect_variant: &EffectVariantId,
) -> Option<TypedRoutedInput> {
    if producer_instance == &producers::mob_instance_id()
        && effect_variant == &effects::mob::request_runtime_binding()
    {
        return Some(route_binding_request_reaches_meerkat());
    }
    if producer_instance == &producers::mob_instance_id()
        && effect_variant == &effects::mob::request_runtime_ingress()
    {
        return Some(route_work_request_reaches_meerkat());
    }
    if producer_instance == &producers::mob_instance_id()
        && effect_variant == &effects::mob::request_runtime_retire()
    {
        return Some(route_retire_request_reaches_meerkat());
    }
    if producer_instance == &producers::mob_instance_id()
        && effect_variant == &effects::mob::request_runtime_destroy()
    {
        return Some(route_destroy_request_reaches_meerkat());
    }
    None
}

/// Resolve a producer `(instance, effect_variant)` to generated signal-route facts.
pub fn route_to_signal(
    producer_instance: &MachineInstanceId,
    effect_variant: &EffectVariantId,
) -> Option<TypedRoutedSignal> {
    if producer_instance == &producers::meerkat_instance_id()
        && effect_variant == &effects::meerkat::runtime_bound()
    {
        return Some(route_runtime_bound_reaches_mob());
    }
    if producer_instance == &producers::meerkat_instance_id()
        && effect_variant == &effects::meerkat::runtime_retired()
    {
        return Some(route_runtime_retired_reaches_mob());
    }
    if producer_instance == &producers::meerkat_instance_id()
        && effect_variant == &effects::meerkat::runtime_destroyed()
    {
        return Some(route_runtime_destroyed_reaches_mob());
    }
    None
}

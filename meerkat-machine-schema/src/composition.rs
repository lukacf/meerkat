use crate::identity::{
    ActorId, CompositionId, EffectVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId,
    PhaseId, ProtocolId, RouteId, SignalVariantId, TransitionId,
};
use crate::{Expr, MachineSchema, TypeRef, machine::MachineSchemaError};
use indexmap::IndexSet;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionSchema {
    pub name: CompositionId,
    pub machines: Vec<MachineInstance>,
    pub actors: Vec<ActorSchema>,
    pub handoff_protocols: Vec<EffectHandoffProtocol>,
    pub entry_inputs: Vec<EntryInput>,
    pub routes: Vec<Route>,
    pub route_target_selectors: Vec<RouteTargetSelector>,
    pub driver: Option<CompositionDriver>,
    pub transaction_plans: Vec<CompositionTransactionPlan>,
    pub actor_priorities: Vec<ActorPriority>,
    pub scheduler_rules: Vec<SchedulerRule>,
    pub invariants: Vec<CompositionInvariant>,
    pub witnesses: Vec<CompositionWitness>,
    pub deep_domain_cardinality: usize,
    pub deep_domain_overrides: BTreeMap<String, usize>,
    pub witness_domain_cardinality: usize,
    pub ci_limits: Option<CompositionStateLimits>,
    pub closed_world: bool,
}

/// Declares a named actor participating in a composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorSchema {
    pub name: ActorId,
    pub kind: ActorKind,
}

/// Distinguishes machine actors from owner actors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorKind {
    /// Driven by machine transitions — deterministic given inputs.
    Machine,
    /// Represents a host/runtime that realizes effects and provides
    /// protocol-constrained feedback.
    Owner,
}

/// Declares the contract between a machine that emits an effect and an
/// owner actor that realizes it and provides feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectHandoffProtocol {
    /// Protocol name — must match `handoff_protocol` on the producing
    /// machine's `EffectDispositionRule`.
    pub name: ProtocolId,
    /// The machine instance that produces the effect.
    pub producer_instance: MachineInstanceId,
    /// The effect variant that triggers this protocol.
    pub effect_variant: EffectVariantId,
    /// The owner actor that realizes the effect.
    pub realizing_actor: ActorId,
    /// Fields from the effect variant that correlate the obligation to feedback.
    pub correlation_fields: Vec<FieldId>,
    /// Fields from the effect variant captured in the outstanding obligation record.
    ///
    /// `correlation_fields` must be a subset of these fields.
    pub obligation_fields: Vec<FieldId>,
    /// Machine inputs the owner may submit as feedback.
    pub allowed_feedback_inputs: Vec<FeedbackInputRef>,
    /// When and how the obligation must be closed.
    pub closure_policy: ClosurePolicy,
    /// Optional fairness annotation for TLA+ liveness claims.
    pub liveness_annotation: Option<String>,
    /// Explicit Rust code generation metadata for the checked-in helper module.
    pub rust: ProtocolRustBinding,
}

/// Determines the shape of generated protocol helper code.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProtocolGenerationMode {
    /// Calls `authority.apply()` with the triggering input, returns effects + obligation.
    /// Helper lives in the same crate as the authority.
    #[default]
    Executor,
    /// Scans already-emitted effects for the handoff-annotated variant, extracts obligation.
    /// Helper lives in the authority's crate.
    EffectExtractor,
    /// Wraps authority-derived data into an obligation token for cross-machine handoff.
    /// Helper lives in the consuming machine's crate.
    ShellBridge,
    /// Forwards obligation data through a typed handle trait (e.g.
    /// `ExternalToolSurfaceHandle`) rather than invoking `authority.apply`
    /// directly. Used when the consuming actor speaks to the authority
    /// through a trait object to support both standalone and runtime-backed
    /// deployments.
    ///
    /// Stackable alongside `EffectExtractor` via
    /// `ProtocolRustBinding::additional_modes` — a protocol that needs both
    /// effect extraction and handle-driven submitters declares the primary
    /// mode + `HandleBridge` as additional.
    HandleBridge,
}

/// References a specific machine input that the owner may submit as feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackInputRef {
    /// The machine instance receiving the feedback.
    pub machine_instance: MachineInstanceId,
    /// The input variant on that machine.
    pub input_variant: InputVariantId,
    /// Exhaustive field bindings used to construct the feedback input.
    pub field_bindings: Vec<FeedbackFieldBinding>,
}

/// Binds one feedback input field to an obligation-carried value or owner context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackFieldBinding {
    /// Target field on the feedback input variant.
    pub input_field: FieldId,
    /// Source of the value used to populate the target field.
    pub source: FeedbackFieldSource,
}

/// Source of a feedback field value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackFieldSource {
    /// Value must come from the outstanding obligation record.
    ObligationField(FieldId),
    /// Value is supplied by the realizing owner at feedback time.
    /// Free-form string key into owner context — not a kernel identity.
    OwnerContext(String),
}

/// Rust-side HandleBridge metadata for one feedback input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleBridgeFeedbackBinding {
    pub input_variant: InputVariantId,
    pub method_name: String,
    /// Per-call suffix applied to `obligation.<field>` references when
    /// constructing handle-method arguments. Keys are typed obligation
    /// field ids; values are suffixes like `.0`, `.clone()`, `.into()`.
    /// Absent entries emit bare `obligation.<field>`.
    pub arg_accessors: BTreeMap<FieldId, String>,
    /// Positional list of obligation fields forwarded to the handle
    /// method. `None` falls back to every obligation-sourced feedback
    /// binding in declaration order. Use `Some(vec![...])` when the
    /// feedback input carries correlation fields the handle method does
    /// not accept.
    pub forwarded_fields: Option<Vec<FieldId>>,
}

/// Explicit Rust binding metadata for generated protocol helper modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolRustBinding {
    /// Output file path relative to the repo root.
    pub module_path: String,
    /// How the helper is generated.
    pub generation_mode: ProtocolGenerationMode,
    /// `use ...;` lines inserted at the top of the helper module.
    pub required_imports: Vec<String>,
    /// Concrete authority type used by generated helpers.
    pub authority_type_path: Option<String>,
    /// Sealed mutator trait path used to call `apply`.
    pub mutator_trait_path: Option<String>,
    /// Typed input enum path for feedback/executor helpers.
    pub input_enum_path: Option<String>,
    /// Typed effect enum path for executor/effect-extractor helpers.
    pub effect_enum_path: Option<String>,
    /// Concrete transition type returned by `authority.apply(...)`.
    pub transition_type_path: Option<String>,
    /// Concrete error type returned by `authority.apply(...)`.
    pub error_type_path: Option<String>,
    /// Triggering producer input variant for `Executor` helpers.
    pub executor_trigger_input_variant: Option<String>,
    /// Authority-owned source token type for `ShellBridge` helpers.
    pub bridge_source_type_path: Option<String>,
    /// Shape of the primary generated helper return value.
    pub helper_return_shape: ProtocolHelperReturnShape,
    /// Handle trait path used by `HandleBridge` helpers. Required when
    /// `generation_mode` or `additional_modes` contains `HandleBridge`.
    pub handle_trait_path: Option<String>,
    /// HandleBridge metadata per feedback input. Required for each
    /// feedback entry emitted through the `HandleBridge` mode.
    pub handle_feedback_bindings: Vec<HandleBridgeFeedbackBinding>,
    /// Kernel-codegen-emitted input enums wrap each variant in a named
    /// payload struct under an `inputs` submodule
    /// (`Input::VariantName(inputs::VariantName { ... })`). DSL-emitted
    /// input enums use named-field variants directly
    /// (`Input::VariantName { ... }`). When this field is set, the
    /// codegen emits the tuple-wrapping form and qualifies the payload
    /// struct by this module path (e.g. `inputs`). Absent → named-field
    /// form (the canonical DSL-emitted shape).
    pub input_payload_module_path: Option<String>,
    /// Additional generation modes stacked onto the primary mode. Each
    /// listed mode emits its own family of helpers into the same output
    /// file, letting a single protocol expose both (for example)
    /// `extract_obligations` and handle-driven submitters.
    ///
    /// Must not include the primary `generation_mode` — no duplicates.
    pub additional_modes: Vec<ProtocolGenerationMode>,
}

impl ProtocolRustBinding {
    pub fn handle_feedback_binding(
        &self,
        input_variant: &InputVariantId,
    ) -> Option<&HandleBridgeFeedbackBinding> {
        self.handle_feedback_bindings
            .iter()
            .find(|binding| binding.input_variant == *input_variant)
    }
}

/// Declares the primary generated helper return contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolHelperReturnShape {
    Effects,
    Transition,
    EffectsAndObligation,
    Obligations,
}

/// Explicit Rust binding metadata for generated composition drivers.
///
/// Nested inside [`CompositionDriver`]. This carries the Rust-side rendering
/// details (module path, emitted type names, imports) that the codegen
/// consumes when producing the generated driver module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionDriverRustBinding {
    /// Output file path relative to the repo root.
    pub module_path: String,
    /// Primary generated driver type name.
    pub driver_type: String,
    /// Generated store-plan enum type name.
    pub store_plan_type: String,
    /// Generated follow-up work enum type name.
    pub work_type: String,
    /// Generated decision type name.
    pub decision_type: String,
    /// `use ...;` lines inserted at the top of the generated driver module.
    pub required_imports: Vec<String>,
}

/// Declarative description of a composition-level driver.
///
/// Compositions that need cross-machine orchestration — e.g. projecting a
/// wiring graph owned by one machine onto peer endpoints owned by another —
/// declare a driver that **watches** specific effect variants on producer
/// machines and **dispatches** a typed decision as new inputs on target
/// machines. The runtime (`meerkat-runtime::composition_dispatch`) consumes
/// this descriptor to install the driver and route observed effects through
/// its decision function.
///
/// This is the declarative seam that replaces hand-crafted per-composition
/// driver templates: any composition can now declare a driver without the
/// codegen knowing about it by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionDriver {
    /// Stable logical name — used for driver registration at runtime and
    /// referenced in diagnostics.
    pub name: String,
    /// Rust emission/runtime binding metadata.
    pub rust: CompositionDriverRustBinding,
    /// Effects this driver observes. Each entry pairs a producer machine
    /// instance with a specific effect variant on that machine.
    pub watched_effects: Vec<WatchedEffect>,
    /// Inputs this driver may dispatch. Each entry names a target machine
    /// instance + input variant. At runtime, the driver's decision function
    /// returns dispatched inputs that the dispatcher routes via these
    /// declarations.
    pub dispatch_routes: Vec<DriverDispatchRoute>,
}

/// A single effect the driver observes from a producer machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedEffect {
    /// Producer machine instance id within the composition.
    pub producer_instance: MachineInstanceId,
    /// Effect variant name on that producer's effect enum.
    pub effect_variant: EffectVariantId,
}

/// A single dispatch route — the driver may emit inputs on this target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverDispatchRoute {
    /// Stable logical dispatch name — the decision function references
    /// this when emitting an input, and the runtime dispatcher uses it
    /// to route the payload.
    pub name: RouteId,
    /// Target machine instance id within the composition.
    pub target_instance: MachineInstanceId,
    /// Whether the dispatch lands on an input or a signal.
    /// Structural kind tag; mirrors the arm of `input_variant`.
    pub target_kind: RouteTargetKind,
    /// Typed slug for the dispatched variant, sum-tagged by input/signal.
    pub input_variant: RouteVariantId,
}

/// Declares how a routed effect selects its concrete target machine instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTargetSelector {
    /// Route name this selector applies to.
    pub route_name: RouteId,
    /// Logical selector field on the destination side.
    pub selector_field: FieldId,
    /// Source of the selector value.
    pub source: RouteBindingSource,
}

/// Describes an atomic persistence bundle for a composition-owned driver plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionTransactionPlan {
    /// Stable transaction-plan name. Free-form — not a kernel identity.
    pub name: String,
    /// Host/runtime trigger or entrypoint that requests this plan.
    pub trigger: String,
    /// Human-readable explanation of the bundle.
    pub description: String,
    /// Existing store primitive that realizes the plan atomically.
    pub store_primitive: String,
    /// Deterministic routes included in the bundle.
    pub route_names: Vec<RouteId>,
    /// Handoff protocols explicitly closed or emitted by the bundle.
    pub protocol_names: Vec<ProtocolId>,
}

/// Determines when a handoff obligation is considered closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosurePolicy {
    /// Owner must acknowledge (provide at least one feedback input).
    AckRequired,
    /// Owner must acknowledge or machine must reach terminal/abort phase.
    AckOrAbort,
    /// Obligation is closed when machine reaches terminal phase.
    TerminalClosure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitness {
    /// Stable witness name. Free-form; not a kernel identity.
    pub name: String,
    pub preload_inputs: Vec<CompositionWitnessInput>,
    pub expected_routes: Vec<RouteId>,
    pub expected_scheduler_rules: Vec<SchedulerRule>,
    pub expected_states: Vec<CompositionWitnessState>,
    pub expected_transitions: Vec<CompositionWitnessTransition>,
    pub expected_transition_order: Vec<CompositionWitnessTransitionOrder>,
    pub state_limits: CompositionStateLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessInput {
    pub machine: MachineInstanceId,
    pub input_variant: InputVariantId,
    pub fields: Vec<CompositionWitnessField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessField {
    pub field: FieldId,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessState {
    pub machine: MachineInstanceId,
    pub phase: Option<PhaseId>,
    pub fields: Vec<CompositionWitnessField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessTransition {
    pub machine: MachineInstanceId,
    pub transition: TransitionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessTransitionOrder {
    pub earlier: CompositionWitnessTransition,
    pub later: CompositionWitnessTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionStateLimits {
    pub step_limit: u32,
    pub pending_input_limit: u32,
    pub pending_route_limit: u32,
    pub delivered_route_limit: u32,
    pub emitted_effect_limit: u32,
    pub seq_limit: u32,
    pub set_limit: u32,
    pub map_limit: u32,
}

impl CompositionStateLimits {
    pub fn ci_defaults() -> Self {
        Self {
            step_limit: 6,
            pending_input_limit: 1,
            pending_route_limit: 1,
            delivered_route_limit: 1,
            emitted_effect_limit: 1,
            seq_limit: 1,
            set_limit: 1,
            map_limit: 1,
        }
    }

    pub fn deep_defaults() -> Self {
        Self {
            step_limit: 6,
            pending_input_limit: 2,
            pending_route_limit: 2,
            delivered_route_limit: 2,
            emitted_effect_limit: 2,
            seq_limit: 2,
            set_limit: 2,
            map_limit: 2,
        }
    }
}

#[allow(clippy::result_large_err)]
impl CompositionSchema {
    pub fn validate(&self) -> Result<(), CompositionSchemaError> {
        if self.deep_domain_cardinality == 0 {
            return Err(CompositionSchemaError::InvalidDomainCardinality {
                scope: "deep".into(),
            });
        }
        if self.witness_domain_cardinality == 0 {
            return Err(CompositionSchemaError::InvalidDomainCardinality {
                scope: "witness".into(),
            });
        }
        for (domain, cardinality) in &self.deep_domain_overrides {
            if *cardinality == 0 {
                return Err(CompositionSchemaError::InvalidNamedDomainCardinality {
                    scope: "deep".into(),
                    domain: domain.clone(),
                });
            }
        }

        let machine_ids = unique_names(
            self.machines.iter().map(|item| item.instance_id.as_str()),
            "machine instance",
        )?;
        let route_names =
            unique_names(self.routes.iter().map(|route| route.name.as_str()), "route")?;
        let actor_ids = unique_names(self.actors.iter().map(|actor| actor.name.as_str()), "actor")?;
        let _ = unique_names(
            self.route_target_selectors
                .iter()
                .map(|selector| selector.route_name.as_str()),
            "route target selector",
        )?;
        let _ = unique_names(
            self.transaction_plans.iter().map(|plan| plan.name.as_str()),
            "transaction plan",
        )?;

        // Every MachineInstance.actor must reference an ActorSchema with kind Machine.
        for machine in &self.machines {
            if !actor_ids.contains(machine.actor.as_str()) {
                return Err(CompositionSchemaError::UnknownActor {
                    actor: machine.actor.as_str().to_owned(),
                });
            }
            let Some(actor_schema) = self
                .actors
                .iter()
                .find(|a| a.name.as_str() == machine.actor.as_str())
            else {
                return Err(CompositionSchemaError::UnknownActor {
                    actor: machine.actor.as_str().to_owned(),
                });
            };
            if actor_schema.kind != ActorKind::Machine {
                return Err(CompositionSchemaError::ActorKindMismatch {
                    actor: machine.actor.as_str().to_owned(),
                    expected: ActorKind::Machine,
                    actual: actor_schema.kind.clone(),
                });
            }
        }

        // Validate handoff protocols (structural checks only — no schema cross-ref here).
        let _protocol_names = unique_names(
            self.handoff_protocols.iter().map(|p| p.name.as_str()),
            "handoff protocol",
        )?;
        for protocol in &self.handoff_protocols {
            let _ = unique_names(
                protocol.correlation_fields.iter().map(AsRef::as_ref),
                "handoff correlation field",
            )?;
            let _ = unique_names(
                protocol.obligation_fields.iter().map(AsRef::as_ref),
                "handoff obligation field",
            )?;
            for field in &protocol.correlation_fields {
                if !protocol.obligation_fields.contains(field) {
                    return Err(
                        CompositionSchemaError::HandoffCorrelationFieldNotInObligation {
                            protocol: protocol.name.as_str().to_owned(),
                            field: field.as_str().to_owned(),
                        },
                    );
                }
            }
            if !machine_ids.contains(protocol.producer_instance.as_str()) {
                return Err(CompositionSchemaError::UnknownHandoffProducer {
                    protocol: protocol.name.as_str().to_owned(),
                    instance: protocol.producer_instance.as_str().to_owned(),
                });
            }
            if !actor_ids.contains(protocol.realizing_actor.as_str()) {
                return Err(CompositionSchemaError::UnknownHandoffActor {
                    protocol: protocol.name.as_str().to_owned(),
                    actor: protocol.realizing_actor.as_str().to_owned(),
                });
            }
            let Some(realizing) = self
                .actors
                .iter()
                .find(|a| a.name.as_str() == protocol.realizing_actor.as_str())
            else {
                return Err(CompositionSchemaError::UnknownHandoffActor {
                    protocol: protocol.name.as_str().to_owned(),
                    actor: protocol.realizing_actor.as_str().to_owned(),
                });
            };
            if realizing.kind != ActorKind::Owner {
                return Err(CompositionSchemaError::HandoffActorNotOwner {
                    protocol: protocol.name.as_str().to_owned(),
                    actor: protocol.realizing_actor.as_str().to_owned(),
                });
            }
            if protocol.rust.module_path.is_empty() {
                return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                    protocol: protocol.name.as_str().to_owned(),
                    detail: "module_path must not be empty".into(),
                });
            }
            validate_generation_mode_binding(protocol, &protocol.rust.generation_mode)?;
            // Stacked modes must be distinct and not repeat the primary.
            for (idx, extra) in protocol.rust.additional_modes.iter().enumerate() {
                if *extra == protocol.rust.generation_mode {
                    return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.as_str().to_owned(),
                        detail: format!(
                            "additional_modes[{idx}] duplicates the primary generation_mode ({extra:?})"
                        ),
                    });
                }
                if protocol.rust.additional_modes[..idx].contains(extra) {
                    return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.as_str().to_owned(),
                        detail: format!("additional_modes contains {extra:?} more than once"),
                    });
                }
                validate_generation_mode_binding(protocol, extra)?;
            }
            for feedback in &protocol.allowed_feedback_inputs {
                if !machine_ids.contains(feedback.machine_instance.as_str()) {
                    return Err(CompositionSchemaError::UnknownHandoffFeedbackMachine {
                        protocol: protocol.name.as_str().to_owned(),
                        machine: feedback.machine_instance.as_str().to_owned(),
                    });
                }
            }
        }

        let mut witnessed_routes = IndexSet::new();
        let mut witnessed_scheduler_rules = Vec::new();

        for route in &self.routes {
            if !machine_ids.contains(route.from_machine.as_str()) {
                return Err(CompositionSchemaError::UnknownMachine {
                    machine: route.from_machine.as_str().to_owned(),
                });
            }
            if !machine_ids.contains(route.to.machine.as_str()) {
                return Err(CompositionSchemaError::UnknownMachine {
                    machine: route.to.machine.as_str().to_owned(),
                });
            }
            let _ = unique_names(
                route
                    .bindings
                    .iter()
                    .map(|binding| binding.to_field.as_str()),
                "route binding target",
            )?;
        }

        for selector in &self.route_target_selectors {
            if !route_names.contains(selector.route_name.as_str()) {
                return Err(CompositionSchemaError::UnknownRouteTargetSelectorRoute {
                    route: selector.route_name.as_str().to_owned(),
                });
            }
            if selector.selector_field.as_str().is_empty() {
                return Err(CompositionSchemaError::InvalidRouteTargetSelector {
                    route: selector.route_name.as_str().to_owned(),
                    detail: "selector_field must not be empty".into(),
                });
            }
        }

        if let Some(driver) = &self.driver {
            if driver.name.is_empty() {
                return Err(CompositionSchemaError::InvalidCompositionDriverBinding {
                    composition: self.name.as_str().to_owned(),
                    detail: "driver name must not be empty".into(),
                });
            }
            if driver.rust.module_path.is_empty() {
                return Err(CompositionSchemaError::InvalidCompositionDriverBinding {
                    composition: self.name.as_str().to_owned(),
                    detail: "module_path must not be empty".into(),
                });
            }
            if driver.rust.driver_type.is_empty()
                || driver.rust.store_plan_type.is_empty()
                || driver.rust.work_type.is_empty()
                || driver.rust.decision_type.is_empty()
            {
                return Err(CompositionSchemaError::InvalidCompositionDriverBinding {
                    composition: self.name.as_str().to_owned(),
                    detail: "driver_type, store_plan_type, work_type, and decision_type must not be empty"
                        .into(),
                });
            }

            // Each `(producer_instance, effect_variant)` pair must appear at
            // most once — duplicates would cause the dispatcher to deliver
            // the same effect to the same driver twice.
            let watched_keys: Vec<String> = driver
                .watched_effects
                .iter()
                .map(|watched| format!("{}::{}", watched.producer_instance, watched.effect_variant))
                .collect();
            let _ = unique_names(
                watched_keys.iter().map(AsRef::as_ref),
                "composition driver watched effect",
            )?;

            let _ = unique_names(
                driver
                    .dispatch_routes
                    .iter()
                    .map(|route| route.name.as_str()),
                "composition driver dispatch route",
            )?;
        }

        let protocol_names = self
            .handoff_protocols
            .iter()
            .map(|protocol| protocol.name.as_str())
            .collect::<IndexSet<_>>();
        for plan in &self.transaction_plans {
            if plan.trigger.is_empty() {
                return Err(CompositionSchemaError::InvalidTransactionPlan {
                    plan: plan.name.clone(),
                    detail: "trigger must not be empty".into(),
                });
            }
            if plan.store_primitive.is_empty() {
                return Err(CompositionSchemaError::InvalidTransactionPlan {
                    plan: plan.name.clone(),
                    detail: "store_primitive must not be empty".into(),
                });
            }
            for route_name in &plan.route_names {
                if !route_names.contains(route_name.as_str()) {
                    return Err(CompositionSchemaError::UnknownTransactionPlanRoute {
                        plan: plan.name.clone(),
                        route: route_name.as_str().to_owned(),
                    });
                }
            }
            for protocol_name in &plan.protocol_names {
                if !protocol_names.contains(protocol_name.as_str()) {
                    return Err(CompositionSchemaError::UnknownTransactionPlanProtocol {
                        plan: plan.name.clone(),
                        protocol: protocol_name.as_str().to_owned(),
                    });
                }
            }
        }

        for entry_input in &self.entry_inputs {
            if !machine_ids.contains(entry_input.machine.as_str()) {
                return Err(CompositionSchemaError::UnknownMachine {
                    machine: entry_input.machine.as_str().to_owned(),
                });
            }
        }

        let _ = unique_names(
            self.witnesses.iter().map(|witness| witness.name.as_str()),
            "composition witness",
        )?;

        for witness in &self.witnesses {
            for preload in &witness.preload_inputs {
                if !machine_ids.contains(preload.machine.as_str()) {
                    return Err(CompositionSchemaError::UnknownMachine {
                        machine: preload.machine.as_str().to_owned(),
                    });
                }
                let _ = unique_names(
                    preload.fields.iter().map(|field| field.field.as_str()),
                    "witness field",
                )?;
            }
            let _ = unique_names(
                witness.expected_routes.iter().map(AsRef::as_ref),
                "witness expected route",
            )?;
            for route in &witness.expected_routes {
                if !route_names.contains(route.as_str()) {
                    return Err(CompositionSchemaError::UnknownWitnessRoute {
                        witness: witness.name.clone(),
                        route: route.as_str().to_owned(),
                    });
                }
                witnessed_routes.insert(route.as_str().to_owned());
            }
            for rule in &witness.expected_scheduler_rules {
                if !self
                    .scheduler_rules
                    .iter()
                    .any(|candidate| candidate == rule)
                {
                    return Err(CompositionSchemaError::UnknownWitnessSchedulerRule {
                        witness: witness.name.clone(),
                        rule: rule.clone(),
                    });
                }
                if !witnessed_scheduler_rules
                    .iter()
                    .any(|candidate| candidate == rule)
                {
                    witnessed_scheduler_rules.push(rule.clone());
                }
            }
        }

        for route in &self.routes {
            if !witnessed_routes.contains(route.name.as_str()) {
                return Err(CompositionSchemaError::MissingWitnessRouteCoverage {
                    route: route.name.as_str().to_owned(),
                });
            }
        }

        for rule in &self.scheduler_rules {
            if !witnessed_scheduler_rules
                .iter()
                .any(|candidate| candidate == rule)
            {
                return Err(CompositionSchemaError::MissingWitnessSchedulerCoverage {
                    rule: rule.clone(),
                });
            }
        }

        for priority in &self.actor_priorities {
            if !actor_ids.contains(priority.higher.as_str()) {
                return Err(CompositionSchemaError::UnknownActor {
                    actor: priority.higher.as_str().to_owned(),
                });
            }
            if !actor_ids.contains(priority.lower.as_str()) {
                return Err(CompositionSchemaError::UnknownActor {
                    actor: priority.lower.as_str().to_owned(),
                });
            }
        }

        for rule in &self.scheduler_rules {
            match rule {
                SchedulerRule::PreemptWhenReady { higher, lower } => {
                    if !actor_ids.contains(higher.as_str()) {
                        return Err(CompositionSchemaError::UnknownActor {
                            actor: higher.as_str().to_owned(),
                        });
                    }
                    if !actor_ids.contains(lower.as_str()) {
                        return Err(CompositionSchemaError::UnknownActor {
                            actor: lower.as_str().to_owned(),
                        });
                    }
                }
            }
        }

        for invariant in &self.invariants {
            if invariant.name.is_empty() {
                return Err(CompositionSchemaError::EmptyName("composition invariant"));
            }
            for actor in &invariant.references_actors {
                if !actor_ids.contains(actor.as_str()) {
                    return Err(CompositionSchemaError::UnknownActor {
                        actor: actor.as_str().to_owned(),
                    });
                }
            }
            for machine in &invariant.references_machines {
                if !machine_ids.contains(machine.as_str()) {
                    return Err(CompositionSchemaError::UnknownMachine {
                        machine: machine.as_str().to_owned(),
                    });
                }
            }

            match &invariant.kind {
                CompositionInvariantKind::RoutePresent {
                    from_machine,
                    effect_variant,
                    to_machine,
                    input_variant,
                } => {
                    let present = self.routes.iter().any(|route| {
                        route.from_machine == *from_machine
                            && route.effect_variant == *effect_variant
                            && route.to.machine == *to_machine
                            && route.to.input_variant == *input_variant
                    });
                    if !present {
                        return Err(CompositionSchemaError::MissingRequiredRoute {
                            invariant: invariant.name.clone(),
                            from_machine: from_machine.as_str().to_owned(),
                            effect_variant: effect_variant.as_str().to_owned(),
                            to_machine: to_machine.as_str().to_owned(),
                            input_variant: input_variant.as_str().to_owned(),
                        });
                    }
                }
                CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine,
                    input_variant,
                    from_machine,
                    effect_variant,
                } => {
                    let present = self.routes.iter().any(|route| {
                        route.from_machine == *from_machine
                            && route.effect_variant == *effect_variant
                            && route.to.machine == *to_machine
                            && route.to.input_variant == *input_variant
                    });
                    if !present {
                        return Err(CompositionSchemaError::MissingRequiredObservedInputRoute {
                            invariant: invariant.name.clone(),
                            from_machine: from_machine.as_str().to_owned(),
                            effect_variant: effect_variant.as_str().to_owned(),
                            to_machine: to_machine.as_str().to_owned(),
                            input_variant: input_variant.as_str().to_owned(),
                        });
                    }
                }
                CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name,
                    to_machine,
                    input_variant,
                    from_machine,
                    effect_variant,
                } => {
                    let present = self.routes.iter().any(|route| {
                        route.name == *route_name
                            && route.from_machine == *from_machine
                            && route.effect_variant == *effect_variant
                            && route.to.machine == *to_machine
                            && route.to.input_variant == *input_variant
                    });
                    if !present {
                        return Err(CompositionSchemaError::MissingRequiredObservedRoute {
                            invariant: invariant.name.clone(),
                            route_name: route_name.as_str().to_owned(),
                            from_machine: from_machine.as_str().to_owned(),
                            effect_variant: effect_variant.as_str().to_owned(),
                            to_machine: to_machine.as_str().to_owned(),
                            input_variant: input_variant.as_str().to_owned(),
                        });
                    }
                }
                CompositionInvariantKind::ActorPriorityPresent { higher, lower } => {
                    let present = self
                        .actor_priorities
                        .iter()
                        .any(|priority| priority.higher == *higher && priority.lower == *lower);
                    if !present {
                        return Err(CompositionSchemaError::MissingRequiredActorPriority {
                            invariant: invariant.name.clone(),
                            higher: higher.as_str().to_owned(),
                            lower: lower.as_str().to_owned(),
                        });
                    }
                }
                CompositionInvariantKind::SchedulerRulePresent { rule } => {
                    let present = self
                        .scheduler_rules
                        .iter()
                        .any(|candidate| candidate == rule);
                    if !present {
                        return Err(CompositionSchemaError::MissingRequiredSchedulerRule {
                            invariant: invariant.name.clone(),
                            rule: rule.clone(),
                        });
                    }
                }
                CompositionInvariantKind::OutcomeHandled {
                    from_machine,
                    effect_variant,
                    required_targets,
                } => {
                    for target in required_targets {
                        let present = self.routes.iter().any(|route| {
                            route.from_machine == *from_machine
                                && route.effect_variant == *effect_variant
                                && route.to.machine == target.machine
                                && route.to.input_variant == target.input_variant
                        });
                        if !present {
                            return Err(CompositionSchemaError::MissingOutcomeRoute {
                                invariant: invariant.name.clone(),
                                from_machine: from_machine.as_str().to_owned(),
                                effect_variant: effect_variant.as_str().to_owned(),
                                to_machine: target.machine.as_str().to_owned(),
                                input_variant: target.input_variant.as_str().to_owned(),
                            });
                        }
                    }
                }
                CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance,
                    effect_variant,
                    protocol_name,
                } => {
                    let present = self.handoff_protocols.iter().any(|p| {
                        p.name == *protocol_name
                            && p.producer_instance == *producer_instance
                            && p.effect_variant == *effect_variant
                    });
                    if !present {
                        return Err(CompositionSchemaError::MissingHandoffProtocol {
                            from_instance: producer_instance.as_str().to_owned(),
                            effect_variant: effect_variant.as_str().to_owned(),
                            expected_protocol: protocol_name.as_str().to_owned(),
                        });
                    }
                }
            }
        }

        Ok(())
    }

    pub fn validate_against(
        &self,
        schemas: &[&MachineSchema],
    ) -> Result<(), CompositionSchemaError> {
        self.validate()?;

        let schema_names = unique_names(
            schemas.iter().map(|schema| schema.machine.as_str()),
            "machine schema",
        )?;

        for machine in &self.machines {
            if !schema_names.contains(machine.machine_name.as_str()) {
                return Err(CompositionSchemaError::UnknownMachineSchema {
                    schema: machine.machine_name.as_str().to_owned(),
                });
            }
        }

        for route in &self.routes {
            let from_schema = schemas
                .iter()
                .find(|schema| {
                    self.machines.iter().any(|instance| {
                        instance.instance_id == route.from_machine
                            && instance.machine_name.as_str() == schema.machine.as_str()
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: route.from_machine.as_str().to_owned(),
                })?;

            let to_schema = schemas
                .iter()
                .find(|schema| {
                    self.machines.iter().any(|instance| {
                        instance.instance_id == route.to.machine
                            && instance.machine_name.as_str() == schema.machine.as_str()
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: route.to.machine.as_str().to_owned(),
                })?;

            let from_effects = from_schema
                .effects
                .variants_by_name()
                .map_err(CompositionSchemaError::MachineSchema)?;
            if !from_effects.contains(route.effect_variant.as_str()) {
                return Err(CompositionSchemaError::UnknownRouteEffect {
                    machine: route.from_machine.as_str().to_owned(),
                    effect: route.effect_variant.as_str().to_owned(),
                });
            }

            let from_variant = from_schema
                .effects
                .variant_named(route.effect_variant.as_str())
                .map_err(CompositionSchemaError::MachineSchema)?;
            let to_variant = match route.to.kind {
                RouteTargetKind::Input => {
                    let to_inputs = to_schema
                        .inputs
                        .variants_by_name()
                        .map_err(CompositionSchemaError::MachineSchema)?;
                    if !to_inputs.contains(route.to.input_variant.as_str()) {
                        return Err(CompositionSchemaError::UnknownRouteInput {
                            machine: route.to.machine.as_str().to_owned(),
                            input: route.to.input_variant.as_str().to_owned(),
                        });
                    }
                    to_schema
                        .inputs
                        .variant_named(route.to.input_variant.as_str())
                        .map_err(CompositionSchemaError::MachineSchema)?
                }
                RouteTargetKind::Signal => {
                    let to_signals = to_schema
                        .signals
                        .variants_by_name()
                        .map_err(CompositionSchemaError::MachineSchema)?;
                    if !to_signals.contains(route.to.input_variant.as_str()) {
                        return Err(CompositionSchemaError::UnknownRouteSignal {
                            machine: route.to.machine.as_str().to_owned(),
                            signal: route.to.input_variant.as_str().to_owned(),
                        });
                    }
                    to_schema
                        .signals
                        .variant_named(route.to.input_variant.as_str())
                        .map_err(CompositionSchemaError::MachineSchema)?
                }
            };

            for binding in &route.bindings {
                let to_field = to_variant
                    .field_named(binding.to_field.as_str())
                    .map_err(CompositionSchemaError::MachineSchema)?;

                match &binding.source {
                    RouteBindingSource::Field {
                        from_field,
                        allow_named_alias,
                    } => {
                        let from_field_schema = from_variant
                            .field_named(from_field.as_str())
                            .map_err(CompositionSchemaError::MachineSchema)?;

                        let exact_match = from_field_schema.ty == to_field.ty;
                        let named_alias_match = *allow_named_alias
                            && matches!(
                                (&from_field_schema.ty, &to_field.ty),
                                (TypeRef::Named(_), TypeRef::Named(_))
                            );

                        if !exact_match && !named_alias_match {
                            return Err(CompositionSchemaError::RouteFieldTypeMismatch {
                                route: route.name.as_str().to_owned(),
                                from_machine: route.from_machine.as_str().to_owned(),
                                from_field: from_field.as_str().to_owned(),
                                from_ty: from_field_schema.ty.clone(),
                                to_machine: route.to.machine.as_str().to_owned(),
                                to_field: binding.to_field.as_str().to_owned(),
                                to_ty: to_field.ty.clone(),
                            });
                        }
                    }
                    RouteBindingSource::Literal(expr) => {
                        if !route_literal_expr_allowed(expr) {
                            return Err(CompositionSchemaError::UnsupportedRouteLiteral {
                                route: route.name.as_str().to_owned(),
                                to_machine: route.to.machine.as_str().to_owned(),
                                to_field: binding.to_field.as_str().to_owned(),
                            });
                        }

                        if !literal_matches_type(expr, &to_field.ty) {
                            return Err(CompositionSchemaError::RouteLiteralTypeMismatch {
                                route: route.name.as_str().to_owned(),
                                to_machine: route.to.machine.as_str().to_owned(),
                                to_field: binding.to_field.as_str().to_owned(),
                                to_ty: to_field.ty.clone(),
                            });
                        }
                    }
                    RouteBindingSource::OwnerProvided => {
                        // Owner-provided bindings skip type checking — the
                        // realizing owner actor supplies the value at runtime.
                        // The handoff protocol enforces the contract instead.
                    }
                }
            }
        }

        for entry_input in &self.entry_inputs {
            let machine_schema = schemas
                .iter()
                .find(|schema| {
                    self.machines.iter().any(|instance| {
                        instance.instance_id == entry_input.machine
                            && instance.machine_name.as_str() == schema.machine.as_str()
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: entry_input.machine.as_str().to_owned(),
                })?;

            let input_variants = machine_schema
                .inputs
                .variants_by_name()
                .map_err(CompositionSchemaError::MachineSchema)?;
            if !input_variants.contains(entry_input.input_variant.as_str()) {
                return Err(CompositionSchemaError::UnknownRouteInput {
                    machine: entry_input.machine.as_str().to_owned(),
                    input: entry_input.input_variant.as_str().to_owned(),
                });
            }
        }

        if let Some(driver) = &self.driver {
            for watched in &driver.watched_effects {
                let producer_schema = schemas
                    .iter()
                    .find(|schema| {
                        self.machines.iter().any(|instance| {
                            instance.instance_id == watched.producer_instance
                                && instance.machine_name.as_str() == schema.machine.as_str()
                        })
                    })
                    .ok_or_else(|| {
                        CompositionSchemaError::UnknownCompositionDriverWatchedMachine {
                            composition: self.name.as_str().to_owned(),
                            driver: driver.name.clone(),
                            instance: watched.producer_instance.as_str().to_owned(),
                        }
                    })?;

                let effect_variants = producer_schema
                    .effects
                    .variants_by_name()
                    .map_err(CompositionSchemaError::MachineSchema)?;
                if !effect_variants.contains(watched.effect_variant.as_str()) {
                    return Err(
                        CompositionSchemaError::UnknownCompositionDriverWatchedEffect {
                            composition: self.name.as_str().to_owned(),
                            driver: driver.name.clone(),
                            instance: watched.producer_instance.as_str().to_owned(),
                            effect_variant: watched.effect_variant.as_str().to_owned(),
                        },
                    );
                }
            }

            for dispatch in &driver.dispatch_routes {
                let target_schema = schemas
                    .iter()
                    .find(|schema| {
                        self.machines.iter().any(|instance| {
                            instance.instance_id == dispatch.target_instance
                                && instance.machine_name.as_str() == schema.machine.as_str()
                        })
                    })
                    .ok_or_else(|| {
                        CompositionSchemaError::UnknownCompositionDriverDispatchMachine {
                            composition: self.name.as_str().to_owned(),
                            driver: driver.name.clone(),
                            instance: dispatch.target_instance.as_str().to_owned(),
                        }
                    })?;

                let known_variants = match dispatch.target_kind {
                    RouteTargetKind::Input => target_schema
                        .inputs
                        .variants_by_name()
                        .map_err(CompositionSchemaError::MachineSchema)?,
                    RouteTargetKind::Signal => target_schema
                        .signals
                        .variants_by_name()
                        .map_err(CompositionSchemaError::MachineSchema)?,
                };
                if !known_variants.contains(dispatch.input_variant.as_str()) {
                    return Err(
                        CompositionSchemaError::UnknownCompositionDriverDispatchVariant {
                            composition: self.name.as_str().to_owned(),
                            driver: driver.name.clone(),
                            instance: dispatch.target_instance.as_str().to_owned(),
                            target_kind: dispatch.target_kind,
                            variant: dispatch.input_variant.as_str().to_owned(),
                        },
                    );
                }
            }
        }

        for witness in &self.witnesses {
            for preload in &witness.preload_inputs {
                let machine_schema = schemas
                    .iter()
                    .find(|schema| {
                        self.machines.iter().any(|instance| {
                            instance.instance_id == preload.machine
                                && instance.machine_name.as_str() == schema.machine.as_str()
                        })
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                        machine: preload.machine.as_str().to_owned(),
                    })?;

                let input_variant = machine_schema
                    .inputs
                    .variant_named(preload.input_variant.as_str())
                    .map_err(CompositionSchemaError::MachineSchema)?;

                for field in &preload.fields {
                    let target_field = input_variant
                        .field_named(field.field.as_str())
                        .map_err(CompositionSchemaError::MachineSchema)?;
                    if !route_literal_expr_allowed(&field.expr) {
                        return Err(CompositionSchemaError::UnsupportedWitnessLiteral {
                            witness: witness.name.clone(),
                            machine: preload.machine.as_str().to_owned(),
                            field: field.field.as_str().to_owned(),
                        });
                    }
                    if !literal_matches_type(&field.expr, &target_field.ty) {
                        return Err(CompositionSchemaError::WitnessLiteralTypeMismatch {
                            witness: witness.name.clone(),
                            machine: preload.machine.as_str().to_owned(),
                            field: field.field.as_str().to_owned(),
                            ty: target_field.ty.clone(),
                        });
                    }
                }

                for field in &input_variant.fields {
                    let present = preload
                        .fields
                        .iter()
                        .any(|candidate| candidate.field == field.name);
                    if !present {
                        return Err(CompositionSchemaError::MissingWitnessField {
                            witness: witness.name.clone(),
                            machine: preload.machine.as_str().to_owned(),
                            input_variant: preload.input_variant.as_str().to_owned(),
                            field: field.name.as_str().to_owned(),
                        });
                    }
                }
            }

            for state in &witness.expected_states {
                let machine_schema = schemas
                    .iter()
                    .find(|schema| {
                        self.machines.iter().any(|instance| {
                            instance.instance_id == state.machine
                                && instance.machine_name.as_str() == schema.machine.as_str()
                        })
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                        machine: state.machine.as_str().to_owned(),
                    })?;

                if let Some(phase) = &state.phase {
                    let phases = machine_schema
                        .state
                        .phase
                        .variants_by_name()
                        .map_err(CompositionSchemaError::MachineSchema)?;
                    if !phases.contains(phase.as_str()) {
                        return Err(CompositionSchemaError::UnknownWitnessPhase {
                            witness: witness.name.clone(),
                            machine: state.machine.as_str().to_owned(),
                            phase: phase.as_str().to_owned(),
                        });
                    }
                }

                let _ = unique_names(
                    state.fields.iter().map(|field| field.field.as_str()),
                    "witness state field",
                )?;

                for field in &state.fields {
                    let target_field = machine_schema
                        .state
                        .fields
                        .iter()
                        .find(|candidate| candidate.name == field.field)
                        .ok_or_else(|| CompositionSchemaError::UnknownWitnessStateField {
                            witness: witness.name.clone(),
                            machine: state.machine.as_str().to_owned(),
                            field: field.field.as_str().to_owned(),
                        })?;
                    if !route_literal_expr_allowed(&field.expr) {
                        return Err(CompositionSchemaError::UnsupportedWitnessStateLiteral {
                            witness: witness.name.clone(),
                            machine: state.machine.as_str().to_owned(),
                            field: field.field.as_str().to_owned(),
                        });
                    }
                    if !literal_matches_type(&field.expr, &target_field.ty) {
                        return Err(CompositionSchemaError::WitnessStateLiteralTypeMismatch {
                            witness: witness.name.clone(),
                            machine: state.machine.as_str().to_owned(),
                            field: field.field.as_str().to_owned(),
                            ty: target_field.ty.clone(),
                        });
                    }
                }
            }

            for transition in &witness.expected_transitions {
                validate_witness_transition_ref(self, schemas, witness, transition)?;
            }

            for ordering in &witness.expected_transition_order {
                validate_witness_transition_ref(self, schemas, witness, &ordering.earlier)?;
                validate_witness_transition_ref(self, schemas, witness, &ordering.later)?;
            }
        }

        // Validate handoff protocols against machine schemas.
        for protocol in &self.handoff_protocols {
            let producer_schema = schemas
                .iter()
                .find(|schema| {
                    self.machines.iter().any(|instance| {
                        instance.instance_id == protocol.producer_instance
                            && instance.machine_name.as_str() == schema.machine.as_str()
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: protocol.producer_instance.as_str().to_owned(),
                })?;

            // Effect variant must exist on the producer.
            let effect_variants = producer_schema
                .effects
                .variants_by_name()
                .map_err(CompositionSchemaError::MachineSchema)?;
            if !effect_variants.contains(protocol.effect_variant.as_str()) {
                return Err(CompositionSchemaError::UnknownHandoffEffect {
                    protocol: protocol.name.as_str().to_owned(),
                    effect: protocol.effect_variant.as_str().to_owned(),
                });
            }

            // The producer's disposition rule must reference this protocol.
            let disposition_rule = producer_schema
                .effect_dispositions
                .iter()
                .find(|rule| rule.effect_variant == protocol.effect_variant);
            match disposition_rule.and_then(|rule| rule.handoff_protocol.as_ref()) {
                Some(hp) if hp == &protocol.name => {}
                _ => {
                    return Err(CompositionSchemaError::HandoffProtocolMismatch {
                        protocol: protocol.name.as_str().to_owned(),
                        effect_variant: protocol.effect_variant.as_str().to_owned(),
                        expected_protocol: protocol.name.as_str().to_owned(),
                    });
                }
            }

            // Correlation fields must exist on the effect variant.
            let effect_variant_schema = producer_schema
                .effects
                .variant_named(protocol.effect_variant.as_str())
                .map_err(CompositionSchemaError::MachineSchema)?;
            for field in &protocol.correlation_fields {
                effect_variant_schema
                    .field_named(field.as_str())
                    .map_err(|_| CompositionSchemaError::UnknownHandoffCorrelationField {
                        protocol: protocol.name.as_str().to_owned(),
                        field: field.as_str().to_owned(),
                    })?;
            }
            for field in &protocol.obligation_fields {
                effect_variant_schema
                    .field_named(field.as_str())
                    .map_err(|_| CompositionSchemaError::UnknownHandoffObligationField {
                        protocol: protocol.name.as_str().to_owned(),
                        field: field.as_str().to_owned(),
                    })?;
            }

            // Feedback inputs must exist on their target machines.
            for feedback in &protocol.allowed_feedback_inputs {
                let target_schema = schemas
                    .iter()
                    .find(|schema| {
                        self.machines.iter().any(|instance| {
                            instance.instance_id == feedback.machine_instance
                                && instance.machine_name.as_str() == schema.machine.as_str()
                        })
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                        machine: feedback.machine_instance.as_str().to_owned(),
                    })?;
                let input_variants = target_schema
                    .inputs
                    .variants_by_name()
                    .map_err(CompositionSchemaError::MachineSchema)?;
                if !input_variants.contains(feedback.input_variant.as_str()) {
                    return Err(CompositionSchemaError::UnknownHandoffFeedbackInput {
                        protocol: protocol.name.as_str().to_owned(),
                        machine: feedback.machine_instance.as_str().to_owned(),
                        input: feedback.input_variant.as_str().to_owned(),
                    });
                }
                let _ = unique_names(
                    feedback
                        .field_bindings
                        .iter()
                        .map(|binding| binding.input_field.as_str()),
                    "handoff feedback binding target",
                )?;
                let input_variant_schema = target_schema
                    .inputs
                    .variant_named(feedback.input_variant.as_str())
                    .map_err(CompositionSchemaError::MachineSchema)?;
                for field in &input_variant_schema.fields {
                    if !feedback
                        .field_bindings
                        .iter()
                        .any(|binding| binding.input_field == field.name)
                    {
                        return Err(CompositionSchemaError::MissingHandoffFeedbackBinding {
                            protocol: protocol.name.as_str().to_owned(),
                            machine: feedback.machine_instance.as_str().to_owned(),
                            input: feedback.input_variant.as_str().to_owned(),
                            field: field.name.as_str().to_owned(),
                        });
                    }
                }
                for binding in &feedback.field_bindings {
                    input_variant_schema
                        .field_named(binding.input_field.as_str())
                        .map_err(
                            |_| CompositionSchemaError::UnknownHandoffFeedbackInputField {
                                protocol: protocol.name.as_str().to_owned(),
                                machine: feedback.machine_instance.as_str().to_owned(),
                                input: feedback.input_variant.as_str().to_owned(),
                                field: binding.input_field.as_str().to_owned(),
                            },
                        )?;
                    if let FeedbackFieldSource::ObligationField(field) = &binding.source
                        && !protocol.obligation_fields.contains(field)
                    {
                        return Err(
                            CompositionSchemaError::UnknownHandoffBindingObligationField {
                                protocol: protocol.name.as_str().to_owned(),
                                field: field.as_str().to_owned(),
                            },
                        );
                    }
                }
                for correlation_field in &protocol.correlation_fields {
                    if !feedback.field_bindings.iter().any(|binding| {
                        matches!(
                            &binding.source,
                            FeedbackFieldSource::ObligationField(field) if field == correlation_field
                        )
                    }) {
                        return Err(CompositionSchemaError::MissingCorrelationBinding {
                            protocol: protocol.name.as_str().to_owned(),
                            machine: feedback.machine_instance.as_str().to_owned(),
                            input: feedback.input_variant.as_str().to_owned(),
                            obligation_field: correlation_field.as_str().to_owned(),
                        });
                    }
                }
            }

            if matches!(
                protocol.rust.generation_mode,
                ProtocolGenerationMode::Executor
            ) {
                let Some(trigger) = protocol.rust.executor_trigger_input_variant.as_ref() else {
                    return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.as_str().to_owned(),
                        detail: "executor_trigger_input_variant missing after executor validation"
                            .into(),
                    });
                };
                producer_schema.inputs.variant_named(trigger).map_err(|_| {
                    CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.as_str().to_owned(),
                        detail: format!(
                            "executor_trigger_input_variant `{trigger}` does not exist on producer"
                        ),
                    }
                })?;
            }

            for feedback in &protocol.allowed_feedback_inputs {
                if self.routes.iter().any(|route| {
                    route.from_machine == protocol.producer_instance
                        && route.effect_variant == protocol.effect_variant
                        && route.to.machine == feedback.machine_instance
                        && route.to.kind == RouteTargetKind::Input
                        && route.to.input_variant.as_str() == feedback.input_variant.as_str()
                }) {
                    return Err(CompositionSchemaError::DirectRouteBypassesHandoffProtocol {
                        protocol: protocol.name.as_str().to_owned(),
                        machine: feedback.machine_instance.as_str().to_owned(),
                        input: feedback.input_variant.as_str().to_owned(),
                    });
                }
            }

            // TerminalClosure requires the producer machine to have terminal phases.
            if protocol.closure_policy == ClosurePolicy::TerminalClosure
                && producer_schema.state.terminal_phases.is_empty()
            {
                return Err(
                    CompositionSchemaError::TerminalClosureRequiresTerminalPhases {
                        protocol: protocol.name.as_str().to_owned(),
                        producer_instance: protocol.producer_instance.as_str().to_owned(),
                    },
                );
            }
        }

        // Closed-world validation: every Routed effect must have a matching route
        // to every consumer instance in this composition.
        if self.closed_world {
            for machine_instance in &self.machines {
                let machine_schema = schemas
                    .iter()
                    .find(|schema| {
                        schema.machine.as_str() == machine_instance.machine_name.as_str()
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachineSchema {
                        schema: machine_instance.machine_name.as_str().to_owned(),
                    })?;

                for rule in &machine_schema.effect_dispositions {
                    if let crate::machine::EffectDisposition::Routed { consumer_machines } =
                        &rule.disposition
                    {
                        for consumer_machine_name in consumer_machines {
                            let consumer_instances: Vec<_> = self
                                .machines
                                .iter()
                                .filter(|inst| inst.machine_name == *consumer_machine_name)
                                .collect();
                            for consumer_inst in consumer_instances {
                                let route_exists = self.routes.iter().any(|route| {
                                    route.from_machine == machine_instance.instance_id
                                        && route.effect_variant == rule.effect_variant
                                        && route.to.machine == consumer_inst.instance_id
                                });
                                if !route_exists {
                                    return Err(CompositionSchemaError::MissingRoutedEffect {
                                        from_instance: machine_instance
                                            .instance_id
                                            .as_str()
                                            .to_owned(),
                                        effect_variant: rule.effect_variant.as_str().to_owned(),
                                        consumer_machine: consumer_machine_name.as_str().to_owned(),
                                        consumer_instance: consumer_inst
                                            .instance_id
                                            .as_str()
                                            .to_owned(),
                                    });
                                }
                            }
                        }
                    }
                }
            }

            // Closed-world validation: every effect with handoff_protocol set
            // must have a matching EffectHandoffProtocol in the composition.
            for machine_instance in &self.machines {
                let machine_schema = schemas
                    .iter()
                    .find(|schema| {
                        schema.machine.as_str() == machine_instance.machine_name.as_str()
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachineSchema {
                        schema: machine_instance.machine_name.as_str().to_owned(),
                    })?;

                for rule in &machine_schema.effect_dispositions {
                    if let Some(protocol_name) = &rule.handoff_protocol {
                        let protocol_exists = self.handoff_protocols.iter().any(|p| {
                            p.name == *protocol_name
                                && p.producer_instance == machine_instance.instance_id
                                && p.effect_variant == rule.effect_variant
                        });
                        if !protocol_exists {
                            return Err(CompositionSchemaError::MissingHandoffProtocol {
                                from_instance: machine_instance.instance_id.as_str().to_owned(),
                                effect_variant: rule.effect_variant.as_str().to_owned(),
                                expected_protocol: protocol_name.as_str().to_owned(),
                            });
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineInstance {
    pub instance_id: MachineInstanceId,
    pub machine_name: MachineId,
    pub actor: ActorId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInput {
    /// Free-form entry-point name. Not a kernel identity.
    pub name: String,
    pub machine: MachineInstanceId,
    pub input_variant: InputVariantId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub name: RouteId,
    pub from_machine: MachineInstanceId,
    pub effect_variant: EffectVariantId,
    pub to: RouteTarget,
    pub bindings: Vec<RouteFieldBinding>,
    pub delivery: RouteDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTarget {
    pub machine: MachineInstanceId,
    /// Structural kind tag; kept alongside `input_variant` for callers
    /// that dispatch on kind without unwrapping the typed slug. The
    /// invariant `self.kind == self.input_variant.kind()` is enforced by
    /// the `RouteTarget::new` constructor and asserted in validate().
    pub kind: RouteTargetKind,
    /// Typed slug for the target variant, sum-tagged by input/signal.
    pub input_variant: RouteVariantId,
}

impl RouteTarget {
    /// Construct a `RouteTarget` whose `kind` tag is in sync with the
    /// wrapped [`RouteVariantId`] arm.
    pub fn new(machine: MachineInstanceId, input_variant: RouteVariantId) -> Self {
        let kind = input_variant.kind();
        Self {
            machine,
            kind,
            input_variant,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteTargetKind {
    Input,
    Signal,
}

/// Typed sum for the polymorphic `input_variant` slug on [`RouteTarget`],
/// [`DriverDispatchRoute`], and the `input_variant` fields of every
/// [`CompositionInvariantKind`] variant that references one.
///
/// The slug is semantically an [`InputVariantId`] when the target is an
/// input and a [`SignalVariantId`] when the target is a signal. Carrying
/// the typed discriminator alongside the slug eliminates the last
/// stringly-typed identity on the composition surface (parallel to the
/// [`crate::machine::TriggerMatch`] sum on the machine surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteVariantId {
    Input(InputVariantId),
    Signal(SignalVariantId),
}

impl RouteVariantId {
    /// Borrow the slug regardless of the input/signal arm.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Input(id) => id.as_str(),
            Self::Signal(id) => id.as_str(),
        }
    }

    /// Report the structural kind of the wrapped identity.
    pub fn kind(&self) -> RouteTargetKind {
        match self {
            Self::Input(_) => RouteTargetKind::Input,
            Self::Signal(_) => RouteTargetKind::Signal,
        }
    }
}

impl std::fmt::Display for RouteVariantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for RouteVariantId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFieldBinding {
    pub to_field: FieldId,
    pub source: RouteBindingSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteBindingSource {
    Field {
        from_field: FieldId,
        allow_named_alias: bool,
    },
    Literal(Expr),
    /// Value is supplied by the realizing owner actor at runtime, not by the
    /// producing machine's effect. Used when the target input needs data that
    /// the producer does not own (e.g., TurnExecution's `run_id` is not known
    /// to OpsLifecycle).
    OwnerProvided,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDelivery {
    Immediate,
    Enqueue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerRule {
    PreemptWhenReady { higher: ActorId, lower: ActorId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorPriority {
    pub higher: ActorId,
    pub lower: ActorId,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionInvariant {
    /// Free-form invariant name. Not a kernel identity.
    pub name: String,
    pub kind: CompositionInvariantKind,
    pub statement: String,
    pub references_machines: Vec<MachineInstanceId>,
    pub references_actors: Vec<ActorId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionInvariantKind {
    RoutePresent {
        from_machine: MachineInstanceId,
        effect_variant: EffectVariantId,
        to_machine: MachineInstanceId,
        input_variant: RouteVariantId,
    },
    ObservedInputOriginatesFromEffect {
        to_machine: MachineInstanceId,
        input_variant: RouteVariantId,
        from_machine: MachineInstanceId,
        effect_variant: EffectVariantId,
    },
    ObservedRouteInputOriginatesFromEffect {
        route_name: RouteId,
        to_machine: MachineInstanceId,
        input_variant: RouteVariantId,
        from_machine: MachineInstanceId,
        effect_variant: EffectVariantId,
    },
    ActorPriorityPresent {
        higher: ActorId,
        lower: ActorId,
    },
    SchedulerRulePresent {
        rule: SchedulerRule,
    },
    OutcomeHandled {
        from_machine: MachineInstanceId,
        effect_variant: EffectVariantId,
        required_targets: Vec<RouteTarget>,
    },
    HandoffProtocolCovered {
        producer_instance: MachineInstanceId,
        effect_variant: EffectVariantId,
        protocol_name: ProtocolId,
    },
}

impl CompositionInvariantKind {
    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            CompositionInvariantKind::RoutePresent { .. }
                | CompositionInvariantKind::ActorPriorityPresent { .. }
                | CompositionInvariantKind::SchedulerRulePresent { .. }
                | CompositionInvariantKind::HandoffProtocolCovered { .. }
        )
    }

    pub fn is_behavioral(&self) -> bool {
        !self.is_structural()
    }
}

#[allow(clippy::result_large_err)]
fn validate_generation_mode_binding(
    protocol: &EffectHandoffProtocol,
    mode: &ProtocolGenerationMode,
) -> Result<(), CompositionSchemaError> {
    let rust = &protocol.rust;
    match mode {
        ProtocolGenerationMode::Executor => {
            if rust.authority_type_path.is_none()
                || rust.mutator_trait_path.is_none()
                || rust.input_enum_path.is_none()
                || rust.effect_enum_path.is_none()
                || rust.transition_type_path.is_none()
                || rust.error_type_path.is_none()
                || rust.executor_trigger_input_variant.is_none()
            {
                return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                    protocol: protocol.name.as_str().to_owned(),
                    detail:
                        "Executor protocols require authority_type_path, mutator_trait_path, input_enum_path, effect_enum_path, transition_type_path, error_type_path, and executor_trigger_input_variant"
                            .into(),
                });
            }
        }
        ProtocolGenerationMode::EffectExtractor => {
            if rust.effect_enum_path.is_none() {
                return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                    protocol: protocol.name.as_str().to_owned(),
                    detail: "EffectExtractor protocols require effect_enum_path".into(),
                });
            }
            // Authority paths are optional. When present, EffectExtractor
            // emits `submit_*` helpers that call `authority.apply(...)`
            // for each feedback input. When absent, only
            // `extract_obligations` is emitted; feedback flows through a
            // stacked `HandleBridge` mode. Validator only requires full
            // authority plumbing when the codegen is actually going to
            // invoke it — honor partial bindings.
            let authority_present = rust.authority_type_path.is_some();
            if authority_present {
                if rust.mutator_trait_path.is_none()
                    || rust.input_enum_path.is_none()
                    || rust.transition_type_path.is_none()
                    || rust.error_type_path.is_none()
                {
                    return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.as_str().to_owned(),
                        detail:
                            "EffectExtractor with authority_type_path set also requires mutator_trait_path, input_enum_path, transition_type_path, and error_type_path"
                                .into(),
                    });
                }
            } else if !protocol.allowed_feedback_inputs.is_empty() {
                // No authority → feedback must flow through HandleBridge.
                let has_handle_bridge = rust.generation_mode
                    == ProtocolGenerationMode::HandleBridge
                    || rust
                        .additional_modes
                        .contains(&ProtocolGenerationMode::HandleBridge);
                if !has_handle_bridge {
                    return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.as_str().to_owned(),
                        detail:
                            "EffectExtractor without authority_type_path must stack HandleBridge (or declare no feedback inputs)"
                                .into(),
                    });
                }
            }
        }
        ProtocolGenerationMode::ShellBridge => {
            if rust.authority_type_path.is_none()
                || rust.mutator_trait_path.is_none()
                || rust.input_enum_path.is_none()
                || rust.transition_type_path.is_none()
                || rust.error_type_path.is_none()
                || rust.bridge_source_type_path.is_none()
            {
                return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                    protocol: protocol.name.as_str().to_owned(),
                    detail:
                        "ShellBridge protocols require authority_type_path, mutator_trait_path, input_enum_path, transition_type_path, error_type_path, and bridge_source_type_path"
                            .into(),
                });
            }
        }
        ProtocolGenerationMode::HandleBridge => {
            if rust.handle_trait_path.is_none() {
                return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                    protocol: protocol.name.as_str().to_owned(),
                    detail: "HandleBridge protocols require handle_trait_path".into(),
                });
            }
            // Every feedback input must have a handle method mapping.
            for feedback in &protocol.allowed_feedback_inputs {
                if rust
                    .handle_feedback_binding(&feedback.input_variant)
                    .is_none()
                {
                    return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.as_str().to_owned(),
                        detail: format!(
                            "HandleBridge protocol missing handle_feedback_bindings entry for feedback input `{}`",
                            feedback.input_variant
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn unique_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
    kind: &'static str,
) -> Result<IndexSet<&'a str>, CompositionSchemaError> {
    let mut seen = IndexSet::new();
    for name in names {
        if name.is_empty() {
            return Err(CompositionSchemaError::EmptyName(kind));
        }
        if !seen.insert(name) {
            return Err(CompositionSchemaError::DuplicateName {
                kind,
                name: name.to_owned(),
            });
        }
    }
    Ok(seen)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompositionSchemaError {
    DuplicateName {
        kind: &'static str,
        name: String,
    },
    EmptyName(&'static str),
    UnknownMachine {
        machine: String,
    },
    UnknownMachineSchema {
        schema: String,
    },
    UnknownActor {
        actor: String,
    },
    ActorKindMismatch {
        actor: String,
        expected: ActorKind,
        actual: ActorKind,
    },
    UnknownRouteEffect {
        machine: String,
        effect: String,
    },
    UnknownRouteInput {
        machine: String,
        input: String,
    },
    UnknownRouteSignal {
        machine: String,
        signal: String,
    },
    MissingRequiredRoute {
        invariant: String,
        from_machine: String,
        effect_variant: String,
        to_machine: String,
        input_variant: String,
    },
    MissingRequiredObservedInputRoute {
        invariant: String,
        from_machine: String,
        effect_variant: String,
        to_machine: String,
        input_variant: String,
    },
    MissingRequiredObservedRoute {
        invariant: String,
        route_name: String,
        from_machine: String,
        effect_variant: String,
        to_machine: String,
        input_variant: String,
    },
    MissingRequiredActorPriority {
        invariant: String,
        higher: String,
        lower: String,
    },
    MissingRequiredSchedulerRule {
        invariant: String,
        rule: SchedulerRule,
    },
    MissingOutcomeRoute {
        invariant: String,
        from_machine: String,
        effect_variant: String,
        to_machine: String,
        input_variant: String,
    },
    RouteFieldTypeMismatch {
        route: String,
        from_machine: String,
        from_field: String,
        from_ty: TypeRef,
        to_machine: String,
        to_field: String,
        to_ty: TypeRef,
    },
    RouteLiteralTypeMismatch {
        route: String,
        to_machine: String,
        to_field: String,
        to_ty: TypeRef,
    },
    UnsupportedRouteLiteral {
        route: String,
        to_machine: String,
        to_field: String,
    },
    MissingWitnessField {
        witness: String,
        machine: String,
        input_variant: String,
        field: String,
    },
    UnsupportedWitnessLiteral {
        witness: String,
        machine: String,
        field: String,
    },
    WitnessLiteralTypeMismatch {
        witness: String,
        machine: String,
        field: String,
        ty: TypeRef,
    },
    UnknownWitnessRoute {
        witness: String,
        route: String,
    },
    UnknownWitnessSchedulerRule {
        witness: String,
        rule: SchedulerRule,
    },
    UnknownWitnessPhase {
        witness: String,
        machine: String,
        phase: String,
    },
    UnknownWitnessStateField {
        witness: String,
        machine: String,
        field: String,
    },
    UnsupportedWitnessStateLiteral {
        witness: String,
        machine: String,
        field: String,
    },
    WitnessStateLiteralTypeMismatch {
        witness: String,
        machine: String,
        field: String,
        ty: TypeRef,
    },
    UnknownWitnessTransition {
        witness: String,
        machine: String,
        transition: String,
    },
    MissingWitnessRouteCoverage {
        route: String,
    },
    MissingWitnessSchedulerCoverage {
        rule: SchedulerRule,
    },
    InvalidDomainCardinality {
        scope: String,
    },
    InvalidNamedDomainCardinality {
        scope: String,
        domain: String,
    },
    UnknownRouteTargetSelectorRoute {
        route: String,
    },
    InvalidRouteTargetSelector {
        route: String,
        detail: String,
    },
    InvalidCompositionDriverBinding {
        composition: String,
        detail: String,
    },
    UnknownCompositionDriverWatchedMachine {
        composition: String,
        driver: String,
        instance: String,
    },
    UnknownCompositionDriverWatchedEffect {
        composition: String,
        driver: String,
        instance: String,
        effect_variant: String,
    },
    UnknownCompositionDriverDispatchMachine {
        composition: String,
        driver: String,
        instance: String,
    },
    UnknownCompositionDriverDispatchVariant {
        composition: String,
        driver: String,
        instance: String,
        target_kind: RouteTargetKind,
        variant: String,
    },
    InvalidTransactionPlan {
        plan: String,
        detail: String,
    },
    UnknownTransactionPlanRoute {
        plan: String,
        route: String,
    },
    UnknownTransactionPlanProtocol {
        plan: String,
        protocol: String,
    },
    MissingRoutedEffect {
        from_instance: String,
        effect_variant: String,
        consumer_machine: String,
        consumer_instance: String,
    },
    MissingHandoffProtocol {
        from_instance: String,
        effect_variant: String,
        expected_protocol: String,
    },
    UnknownHandoffProducer {
        protocol: String,
        instance: String,
    },
    UnknownHandoffEffect {
        protocol: String,
        effect: String,
    },
    UnknownHandoffActor {
        protocol: String,
        actor: String,
    },
    HandoffActorNotOwner {
        protocol: String,
        actor: String,
    },
    UnknownHandoffFeedbackMachine {
        protocol: String,
        machine: String,
    },
    UnknownHandoffFeedbackInput {
        protocol: String,
        machine: String,
        input: String,
    },
    HandoffProtocolMismatch {
        protocol: String,
        effect_variant: String,
        expected_protocol: String,
    },
    UnknownHandoffCorrelationField {
        protocol: String,
        field: String,
    },
    UnknownHandoffObligationField {
        protocol: String,
        field: String,
    },
    HandoffCorrelationFieldNotInObligation {
        protocol: String,
        field: String,
    },
    UnknownHandoffFeedbackInputField {
        protocol: String,
        machine: String,
        input: String,
        field: String,
    },
    UnknownHandoffBindingObligationField {
        protocol: String,
        field: String,
    },
    MissingHandoffFeedbackBinding {
        protocol: String,
        machine: String,
        input: String,
        field: String,
    },
    MissingCorrelationBinding {
        protocol: String,
        machine: String,
        input: String,
        obligation_field: String,
    },
    InvalidHandoffRustBinding {
        protocol: String,
        detail: String,
    },
    DirectRouteBypassesHandoffProtocol {
        protocol: String,
        machine: String,
        input: String,
    },
    TerminalClosureRequiresTerminalPhases {
        protocol: String,
        producer_instance: String,
    },
    MachineSchema(MachineSchemaError),
}

impl fmt::Display for CompositionSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { kind, name } => write!(f, "duplicate {kind} name `{name}`"),
            Self::EmptyName(kind) => write!(f, "empty {kind} name"),
            Self::UnknownMachine { machine } => write!(f, "unknown machine instance `{machine}`"),
            Self::UnknownMachineSchema { schema } => write!(f, "unknown machine schema `{schema}`"),
            Self::UnknownActor { actor } => write!(f, "unknown actor `{actor}`"),
            Self::ActorKindMismatch {
                actor,
                expected,
                actual,
            } => write!(
                f,
                "actor `{actor}` has kind {actual:?} but expected {expected:?}"
            ),
            Self::UnknownRouteEffect { machine, effect } => {
                write!(
                    f,
                    "route from `{machine}` references unknown effect `{effect}`"
                )
            }
            Self::UnknownRouteInput { machine, input } => {
                write!(f, "route to `{machine}` references unknown input `{input}`")
            }
            Self::UnknownRouteSignal { machine, signal } => {
                write!(
                    f,
                    "route to `{machine}` references unknown signal `{signal}`"
                )
            }
            Self::MissingRequiredRoute {
                invariant,
                from_machine,
                effect_variant,
                to_machine,
                input_variant,
            } => write!(
                f,
                "invariant `{invariant}` requires route {from_machine}.{effect_variant} -> {to_machine}.{input_variant}, but it is missing"
            ),
            Self::MissingRequiredObservedInputRoute {
                invariant,
                from_machine,
                effect_variant,
                to_machine,
                input_variant,
            } => write!(
                f,
                "invariant `{invariant}` requires observed input origin {from_machine}.{effect_variant} -> {to_machine}.{input_variant}, but it is missing"
            ),
            Self::MissingRequiredObservedRoute {
                invariant,
                route_name,
                from_machine,
                effect_variant,
                to_machine,
                input_variant,
            } => write!(
                f,
                "invariant `{invariant}` requires observed route `{route_name}` carrying {from_machine}.{effect_variant} -> {to_machine}.{input_variant}, but it is missing"
            ),
            Self::MissingRequiredActorPriority {
                invariant,
                higher,
                lower,
            } => write!(
                f,
                "invariant `{invariant}` requires actor priority {higher} > {lower}, but it is missing"
            ),
            Self::MissingRequiredSchedulerRule { invariant, rule } => write!(
                f,
                "invariant `{invariant}` requires scheduler rule `{rule:?}`, but it is missing"
            ),
            Self::MissingOutcomeRoute {
                invariant,
                from_machine,
                effect_variant,
                to_machine,
                input_variant,
            } => write!(
                f,
                "invariant `{invariant}` requires outcome route {from_machine}.{effect_variant} -> {to_machine}.{input_variant}, but it is missing"
            ),
            Self::RouteFieldTypeMismatch {
                route,
                from_machine,
                from_field,
                from_ty,
                to_machine,
                to_field,
                to_ty,
            } => write!(
                f,
                "route `{route}` field type mismatch: {from_machine}.{from_field}:{from_ty:?} -> {to_machine}.{to_field}:{to_ty:?}"
            ),
            Self::RouteLiteralTypeMismatch {
                route,
                to_machine,
                to_field,
                to_ty,
            } => write!(
                f,
                "route `{route}` literal does not match destination type {to_machine}.{to_field}:{to_ty:?}"
            ),
            Self::UnsupportedRouteLiteral {
                route,
                to_machine,
                to_field,
            } => write!(
                f,
                "route `{route}` uses unsupported literal binding for {to_machine}.{to_field}"
            ),
            Self::MissingWitnessField {
                witness,
                machine,
                input_variant,
                field,
            } => write!(
                f,
                "witness `{witness}` is missing required field {machine}.{input_variant}.{field}"
            ),
            Self::UnsupportedWitnessLiteral {
                witness,
                machine,
                field,
            } => write!(
                f,
                "witness `{witness}` uses unsupported literal for {machine}.{field}"
            ),
            Self::WitnessLiteralTypeMismatch {
                witness,
                machine,
                field,
                ty,
            } => write!(
                f,
                "witness `{witness}` literal does not match destination type {machine}.{field}:{ty:?}"
            ),
            Self::UnknownWitnessRoute { witness, route } => {
                write!(f, "witness `{witness}` references unknown route `{route}`")
            }
            Self::UnknownWitnessSchedulerRule { witness, rule } => write!(
                f,
                "witness `{witness}` references unknown scheduler rule `{rule:?}`"
            ),
            Self::UnknownWitnessPhase {
                witness,
                machine,
                phase,
            } => write!(
                f,
                "witness `{witness}` references unknown phase `{phase}` on machine `{machine}`"
            ),
            Self::UnknownWitnessStateField {
                witness,
                machine,
                field,
            } => write!(
                f,
                "witness `{witness}` references unknown state field `{field}` on machine `{machine}`"
            ),
            Self::UnsupportedWitnessStateLiteral {
                witness,
                machine,
                field,
            } => write!(
                f,
                "witness `{witness}` uses unsupported state literal for {machine}.{field}"
            ),
            Self::WitnessStateLiteralTypeMismatch {
                witness,
                machine,
                field,
                ty,
            } => write!(
                f,
                "witness `{witness}` state literal does not match destination type {machine}.{field}:{ty:?}"
            ),
            Self::UnknownWitnessTransition {
                witness,
                machine,
                transition,
            } => write!(
                f,
                "witness `{witness}` references unknown transition `{transition}` on machine `{machine}`"
            ),
            Self::MissingWitnessRouteCoverage { route } => write!(
                f,
                "route `{route}` is not covered by any composition witness"
            ),
            Self::MissingWitnessSchedulerCoverage { rule } => write!(
                f,
                "scheduler rule `{rule:?}` is not covered by any composition witness"
            ),
            Self::InvalidDomainCardinality { scope } => write!(
                f,
                "invalid composition {scope} domain cardinality (must be >= 1)"
            ),
            Self::InvalidNamedDomainCardinality { scope, domain } => write!(
                f,
                "invalid composition {scope} domain cardinality for `{domain}` (must be >= 1)"
            ),
            Self::UnknownRouteTargetSelectorRoute { route } => write!(
                f,
                "route target selector references unknown route `{route}`"
            ),
            Self::InvalidRouteTargetSelector { route, detail } => {
                write!(f, "invalid route target selector for `{route}`: {detail}")
            }
            Self::InvalidCompositionDriverBinding {
                composition,
                detail,
            } => write!(
                f,
                "invalid composition driver binding for `{composition}`: {detail}"
            ),
            Self::UnknownCompositionDriverWatchedMachine {
                composition,
                driver,
                instance,
            } => write!(
                f,
                "composition `{composition}` driver `{driver}` watches effect on unknown machine instance `{instance}`"
            ),
            Self::UnknownCompositionDriverWatchedEffect {
                composition,
                driver,
                instance,
                effect_variant,
            } => write!(
                f,
                "composition `{composition}` driver `{driver}` watches unknown effect `{effect_variant}` on machine instance `{instance}`"
            ),
            Self::UnknownCompositionDriverDispatchMachine {
                composition,
                driver,
                instance,
            } => write!(
                f,
                "composition `{composition}` driver `{driver}` dispatches to unknown machine instance `{instance}`"
            ),
            Self::UnknownCompositionDriverDispatchVariant {
                composition,
                driver,
                instance,
                target_kind,
                variant,
            } => write!(
                f,
                "composition `{composition}` driver `{driver}` dispatches unknown {target_kind:?} variant `{variant}` on machine instance `{instance}`"
            ),
            Self::InvalidTransactionPlan { plan, detail } => {
                write!(f, "invalid transaction plan `{plan}`: {detail}")
            }
            Self::UnknownTransactionPlanRoute { plan, route } => write!(
                f,
                "transaction plan `{plan}` references unknown route `{route}`"
            ),
            Self::UnknownTransactionPlanProtocol { plan, protocol } => write!(
                f,
                "transaction plan `{plan}` references unknown protocol `{protocol}`"
            ),
            Self::MissingRoutedEffect {
                from_instance,
                effect_variant,
                consumer_machine,
                consumer_instance,
            } => write!(
                f,
                "closed-world violation: instance `{from_instance}` emits routed effect `{effect_variant}` targeting `{consumer_machine}` but no route exists to instance `{consumer_instance}`"
            ),
            Self::MissingHandoffProtocol {
                from_instance,
                effect_variant,
                expected_protocol,
            } => write!(
                f,
                "closed-world violation: instance `{from_instance}` emits effect `{effect_variant}` with handoff_protocol `{expected_protocol}` but no matching EffectHandoffProtocol exists in the composition"
            ),
            Self::UnknownHandoffProducer { protocol, instance } => write!(
                f,
                "handoff protocol `{protocol}` references unknown producer instance `{instance}`"
            ),
            Self::UnknownHandoffEffect { protocol, effect } => write!(
                f,
                "handoff protocol `{protocol}` references unknown effect `{effect}`"
            ),
            Self::UnknownHandoffActor { protocol, actor } => write!(
                f,
                "handoff protocol `{protocol}` references unknown actor `{actor}`"
            ),
            Self::HandoffActorNotOwner { protocol, actor } => write!(
                f,
                "handoff protocol `{protocol}` requires actor `{actor}` to be Owner, but it is not"
            ),
            Self::UnknownHandoffFeedbackMachine { protocol, machine } => write!(
                f,
                "handoff protocol `{protocol}` references unknown feedback machine `{machine}`"
            ),
            Self::UnknownHandoffFeedbackInput {
                protocol,
                machine,
                input,
            } => write!(
                f,
                "handoff protocol `{protocol}` references unknown feedback input `{input}` on machine `{machine}`"
            ),
            Self::HandoffProtocolMismatch {
                protocol,
                effect_variant,
                expected_protocol,
            } => write!(
                f,
                "handoff protocol `{protocol}` expects effect `{effect_variant}` to declare handoff_protocol `{expected_protocol}`, but it does not"
            ),
            Self::UnknownHandoffCorrelationField { protocol, field } => write!(
                f,
                "handoff protocol `{protocol}` references unknown correlation field `{field}`"
            ),
            Self::UnknownHandoffObligationField { protocol, field } => write!(
                f,
                "handoff protocol `{protocol}` references unknown obligation field `{field}`"
            ),
            Self::HandoffCorrelationFieldNotInObligation { protocol, field } => write!(
                f,
                "handoff protocol `{protocol}` uses correlation field `{field}` that is not present in obligation_fields"
            ),
            Self::UnknownHandoffFeedbackInputField {
                protocol,
                machine,
                input,
                field,
            } => write!(
                f,
                "handoff protocol `{protocol}` references unknown feedback field `{field}` on {machine}.{input}"
            ),
            Self::UnknownHandoffBindingObligationField { protocol, field } => write!(
                f,
                "handoff protocol `{protocol}` binds feedback from unknown obligation field `{field}`"
            ),
            Self::MissingHandoffFeedbackBinding {
                protocol,
                machine,
                input,
                field,
            } => write!(
                f,
                "handoff protocol `{protocol}` is missing a feedback field binding for {machine}.{input}.{field}"
            ),
            Self::MissingCorrelationBinding {
                protocol,
                machine,
                input,
                obligation_field,
            } => write!(
                f,
                "handoff protocol `{protocol}` does not bind correlation obligation field `{obligation_field}` into {machine}.{input}"
            ),
            Self::InvalidHandoffRustBinding { protocol, detail } => write!(
                f,
                "handoff protocol `{protocol}` has invalid Rust binding metadata: {detail}"
            ),
            Self::DirectRouteBypassesHandoffProtocol {
                protocol,
                machine,
                input,
            } => write!(
                f,
                "handoff protocol `{protocol}` is bypassed by a direct route to {machine}.{input}"
            ),
            Self::TerminalClosureRequiresTerminalPhases {
                protocol,
                producer_instance,
            } => write!(
                f,
                "handoff protocol `{protocol}` uses TerminalClosure but producer `{producer_instance}` has no terminal phases"
            ),
            Self::MachineSchema(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for CompositionSchemaError {}

fn route_literal_expr_allowed(expr: &Expr) -> bool {
    match expr {
        Expr::Bool(_)
        | Expr::U64(_)
        | Expr::String(_)
        | Expr::NamedVariant { .. }
        | Expr::None
        | Expr::EmptyMap => true,
        Expr::Some(inner) => route_literal_expr_allowed(inner),
        Expr::SeqLiteral(items) => items.iter().all(route_literal_expr_allowed),
        _ => false,
    }
}

fn literal_matches_type(expr: &Expr, ty: &TypeRef) -> bool {
    match (expr, ty) {
        (Expr::Bool(_), TypeRef::Bool) => true,
        (Expr::U64(_), TypeRef::U32 | TypeRef::U64) => true,
        (Expr::String(_), TypeRef::String | TypeRef::Named(_)) => true,
        (Expr::NamedVariant { enum_name, .. }, TypeRef::Enum(name)) => enum_name == name,
        (Expr::None, TypeRef::Option(_)) => true,
        (Expr::Some(inner), TypeRef::Option(inner_ty)) => literal_matches_type(inner, inner_ty),
        (Expr::EmptyMap, TypeRef::Map(_, _)) => true,
        (Expr::SeqLiteral(items), TypeRef::Seq(inner)) => {
            items.iter().all(|item| literal_matches_type(item, inner))
        }
        _ => false,
    }
}

#[allow(clippy::result_large_err)]
fn validate_witness_transition_ref(
    composition: &CompositionSchema,
    schemas: &[&MachineSchema],
    witness: &CompositionWitness,
    transition: &CompositionWitnessTransition,
) -> Result<(), CompositionSchemaError> {
    let machine_schema = schemas
        .iter()
        .find(|schema| {
            composition.machines.iter().any(|instance| {
                instance.instance_id == transition.machine
                    && instance.machine_name.as_str() == schema.machine.as_str()
            })
        })
        .ok_or_else(|| CompositionSchemaError::UnknownMachine {
            machine: transition.machine.as_str().to_owned(),
        })?;

    let present = machine_schema
        .transitions
        .iter()
        .any(|candidate| candidate.name == transition.transition);
    if !present {
        return Err(CompositionSchemaError::UnknownWitnessTransition {
            witness: witness.name.clone(),
            machine: transition.machine.as_str().to_owned(),
            transition: transition.transition.as_str().to_owned(),
        });
    }

    Ok(())
}

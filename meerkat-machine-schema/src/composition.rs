use crate::{Expr, MachineSchema, TypeRef, machine::MachineSchemaError};
use indexmap::IndexSet;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionSchema {
    pub name: String,
    pub machines: Vec<MachineInstance>,
    pub actors: Vec<ActorSchema>,
    pub handoff_protocols: Vec<EffectHandoffProtocol>,
    pub entry_inputs: Vec<EntryInput>,
    pub routes: Vec<Route>,
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
    pub name: String,
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
    pub name: String,
    /// The machine instance that produces the effect.
    pub producer_instance: String,
    /// The effect variant that triggers this protocol.
    pub effect_variant: String,
    /// The owner actor that realizes the effect.
    pub realizing_actor: String,
    /// Fields from the effect variant that correlate the obligation to feedback.
    pub correlation_fields: Vec<String>,
    /// Fields from the effect variant captured in the outstanding obligation record.
    ///
    /// `correlation_fields` must be a subset of these fields.
    pub obligation_fields: Vec<String>,
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
}

/// References a specific machine input that the owner may submit as feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackInputRef {
    /// The machine instance receiving the feedback.
    pub machine_instance: String,
    /// The input variant on that machine.
    pub input_variant: String,
    /// Exhaustive field bindings used to construct the feedback input.
    pub field_bindings: Vec<FeedbackFieldBinding>,
}

/// Binds one feedback input field to an obligation-carried value or owner context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackFieldBinding {
    /// Target field on the feedback input variant.
    pub input_field: String,
    /// Source of the value used to populate the target field.
    pub source: FeedbackFieldSource,
}

/// Source of a feedback field value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackFieldSource {
    /// Value must come from the outstanding obligation record.
    ObligationField(String),
    /// Value is supplied by the realizing owner at feedback time.
    OwnerContext(String),
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
}

/// Declares the primary generated helper return contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolHelperReturnShape {
    Effects,
    Transition,
    EffectsAndObligation,
    Obligations,
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
    pub name: String,
    pub preload_inputs: Vec<CompositionWitnessInput>,
    pub expected_routes: Vec<String>,
    pub expected_scheduler_rules: Vec<SchedulerRule>,
    pub expected_states: Vec<CompositionWitnessState>,
    pub expected_transitions: Vec<CompositionWitnessTransition>,
    pub expected_transition_order: Vec<CompositionWitnessTransitionOrder>,
    pub state_limits: CompositionStateLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessInput {
    pub machine: String,
    pub input_variant: String,
    pub fields: Vec<CompositionWitnessField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessField {
    pub field: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessState {
    pub machine: String,
    pub phase: Option<String>,
    pub fields: Vec<CompositionWitnessField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionWitnessTransition {
    pub machine: String,
    pub transition: String,
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

        // Every MachineInstance.actor must reference an ActorSchema with kind Machine.
        for machine in &self.machines {
            if !actor_ids.contains(machine.actor.as_str()) {
                return Err(CompositionSchemaError::UnknownActor {
                    actor: machine.actor.clone(),
                });
            }
            let Some(actor_schema) = self.actors.iter().find(|a| a.name == machine.actor) else {
                return Err(CompositionSchemaError::UnknownActor {
                    actor: machine.actor.clone(),
                });
            };
            if actor_schema.kind != ActorKind::Machine {
                return Err(CompositionSchemaError::ActorKindMismatch {
                    actor: machine.actor.clone(),
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
                protocol.correlation_fields.iter().map(String::as_str),
                "handoff correlation field",
            )?;
            let _ = unique_names(
                protocol.obligation_fields.iter().map(String::as_str),
                "handoff obligation field",
            )?;
            for field in &protocol.correlation_fields {
                if !protocol.obligation_fields.contains(field) {
                    return Err(
                        CompositionSchemaError::HandoffCorrelationFieldNotInObligation {
                            protocol: protocol.name.clone(),
                            field: field.clone(),
                        },
                    );
                }
            }
            if !machine_ids.contains(protocol.producer_instance.as_str()) {
                return Err(CompositionSchemaError::UnknownHandoffProducer {
                    protocol: protocol.name.clone(),
                    instance: protocol.producer_instance.clone(),
                });
            }
            if !actor_ids.contains(protocol.realizing_actor.as_str()) {
                return Err(CompositionSchemaError::UnknownHandoffActor {
                    protocol: protocol.name.clone(),
                    actor: protocol.realizing_actor.clone(),
                });
            }
            let Some(realizing) = self
                .actors
                .iter()
                .find(|a| a.name == protocol.realizing_actor)
            else {
                return Err(CompositionSchemaError::UnknownHandoffActor {
                    protocol: protocol.name.clone(),
                    actor: protocol.realizing_actor.clone(),
                });
            };
            if realizing.kind != ActorKind::Owner {
                return Err(CompositionSchemaError::HandoffActorNotOwner {
                    protocol: protocol.name.clone(),
                    actor: protocol.realizing_actor.clone(),
                });
            }
            if protocol.rust.module_path.is_empty() {
                return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                    protocol: protocol.name.clone(),
                    detail: "module_path must not be empty".into(),
                });
            }
            match protocol.rust.generation_mode {
                ProtocolGenerationMode::Executor => {
                    if protocol.rust.authority_type_path.is_none()
                        || protocol.rust.mutator_trait_path.is_none()
                        || protocol.rust.input_enum_path.is_none()
                        || protocol.rust.effect_enum_path.is_none()
                        || protocol.rust.transition_type_path.is_none()
                        || protocol.rust.error_type_path.is_none()
                        || protocol.rust.executor_trigger_input_variant.is_none()
                    {
                        return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                            protocol: protocol.name.clone(),
                            detail:
                                "Executor protocols require authority_type_path, mutator_trait_path, input_enum_path, effect_enum_path, transition_type_path, error_type_path, and executor_trigger_input_variant"
                                    .into(),
                        });
                    }
                }
                ProtocolGenerationMode::EffectExtractor => {
                    if protocol.rust.authority_type_path.is_none()
                        || protocol.rust.mutator_trait_path.is_none()
                        || protocol.rust.input_enum_path.is_none()
                        || protocol.rust.effect_enum_path.is_none()
                        || protocol.rust.transition_type_path.is_none()
                        || protocol.rust.error_type_path.is_none()
                    {
                        return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                            protocol: protocol.name.clone(),
                            detail:
                                "EffectExtractor protocols require authority_type_path, mutator_trait_path, input_enum_path, effect_enum_path, transition_type_path, and error_type_path"
                                    .into(),
                        });
                    }
                }
                ProtocolGenerationMode::ShellBridge => {
                    if protocol.rust.authority_type_path.is_none()
                        || protocol.rust.mutator_trait_path.is_none()
                        || protocol.rust.input_enum_path.is_none()
                        || protocol.rust.transition_type_path.is_none()
                        || protocol.rust.error_type_path.is_none()
                        || protocol.rust.bridge_source_type_path.is_none()
                    {
                        return Err(CompositionSchemaError::InvalidHandoffRustBinding {
                            protocol: protocol.name.clone(),
                            detail:
                                "ShellBridge protocols require authority_type_path, mutator_trait_path, input_enum_path, transition_type_path, error_type_path, and bridge_source_type_path"
                                    .into(),
                        });
                    }
                }
            }
            for feedback in &protocol.allowed_feedback_inputs {
                if !machine_ids.contains(feedback.machine_instance.as_str()) {
                    return Err(CompositionSchemaError::UnknownHandoffFeedbackMachine {
                        protocol: protocol.name.clone(),
                        machine: feedback.machine_instance.clone(),
                    });
                }
            }
        }

        let mut witnessed_routes = IndexSet::new();
        let mut witnessed_scheduler_rules = Vec::new();

        for route in &self.routes {
            if !machine_ids.contains(route.from_machine.as_str()) {
                return Err(CompositionSchemaError::UnknownMachine {
                    machine: route.from_machine.clone(),
                });
            }
            if !machine_ids.contains(route.to.machine.as_str()) {
                return Err(CompositionSchemaError::UnknownMachine {
                    machine: route.to.machine.clone(),
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

        for entry_input in &self.entry_inputs {
            if !machine_ids.contains(entry_input.machine.as_str()) {
                return Err(CompositionSchemaError::UnknownMachine {
                    machine: entry_input.machine.clone(),
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
                        machine: preload.machine.clone(),
                    });
                }
                let _ = unique_names(
                    preload.fields.iter().map(|field| field.field.as_str()),
                    "witness field",
                )?;
            }
            let _ = unique_names(
                witness.expected_routes.iter().map(String::as_str),
                "witness expected route",
            )?;
            for route in &witness.expected_routes {
                if !route_names.contains(route.as_str()) {
                    return Err(CompositionSchemaError::UnknownWitnessRoute {
                        witness: witness.name.clone(),
                        route: route.clone(),
                    });
                }
                witnessed_routes.insert(route.clone());
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
            if !witnessed_routes.contains(&route.name) {
                return Err(CompositionSchemaError::MissingWitnessRouteCoverage {
                    route: route.name.clone(),
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
                    actor: priority.higher.clone(),
                });
            }
            if !actor_ids.contains(priority.lower.as_str()) {
                return Err(CompositionSchemaError::UnknownActor {
                    actor: priority.lower.clone(),
                });
            }
        }

        for rule in &self.scheduler_rules {
            match rule {
                SchedulerRule::PreemptWhenReady { higher, lower } => {
                    if !actor_ids.contains(higher.as_str()) {
                        return Err(CompositionSchemaError::UnknownActor {
                            actor: higher.clone(),
                        });
                    }
                    if !actor_ids.contains(lower.as_str()) {
                        return Err(CompositionSchemaError::UnknownActor {
                            actor: lower.clone(),
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
                        actor: actor.clone(),
                    });
                }
            }
            for machine in &invariant.references_machines {
                if !machine_ids.contains(machine.as_str()) {
                    return Err(CompositionSchemaError::UnknownMachine {
                        machine: machine.clone(),
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
                            from_machine: from_machine.clone(),
                            effect_variant: effect_variant.clone(),
                            to_machine: to_machine.clone(),
                            input_variant: input_variant.clone(),
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
                            from_machine: from_machine.clone(),
                            effect_variant: effect_variant.clone(),
                            to_machine: to_machine.clone(),
                            input_variant: input_variant.clone(),
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
                            route_name: route_name.clone(),
                            from_machine: from_machine.clone(),
                            effect_variant: effect_variant.clone(),
                            to_machine: to_machine.clone(),
                            input_variant: input_variant.clone(),
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
                            higher: higher.clone(),
                            lower: lower.clone(),
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
                                from_machine: from_machine.clone(),
                                effect_variant: effect_variant.clone(),
                                to_machine: target.machine.clone(),
                                input_variant: target.input_variant.clone(),
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
                            from_instance: producer_instance.clone(),
                            effect_variant: effect_variant.clone(),
                            expected_protocol: protocol_name.clone(),
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
                    schema: machine.machine_name.clone(),
                });
            }
        }

        for route in &self.routes {
            let from_schema = schemas
                .iter()
                .find(|schema| {
                    self.machines.iter().any(|instance| {
                        instance.instance_id == route.from_machine
                            && instance.machine_name == schema.machine
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: route.from_machine.clone(),
                })?;

            let to_schema = schemas
                .iter()
                .find(|schema| {
                    self.machines.iter().any(|instance| {
                        instance.instance_id == route.to.machine
                            && instance.machine_name == schema.machine
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: route.to.machine.clone(),
                })?;

            let from_effects = from_schema
                .effects
                .variants_by_name()
                .map_err(CompositionSchemaError::MachineSchema)?;
            if !from_effects.contains(&route.effect_variant) {
                return Err(CompositionSchemaError::UnknownRouteEffect {
                    machine: route.from_machine.clone(),
                    effect: route.effect_variant.clone(),
                });
            }

            let to_inputs = to_schema
                .inputs
                .variants_by_name()
                .map_err(CompositionSchemaError::MachineSchema)?;
            if !to_inputs.contains(&route.to.input_variant) {
                return Err(CompositionSchemaError::UnknownRouteInput {
                    machine: route.to.machine.clone(),
                    input: route.to.input_variant.clone(),
                });
            }

            let from_variant = from_schema
                .effects
                .variant_named(&route.effect_variant)
                .map_err(CompositionSchemaError::MachineSchema)?;
            let to_variant = to_schema
                .inputs
                .variant_named(&route.to.input_variant)
                .map_err(CompositionSchemaError::MachineSchema)?;

            for binding in &route.bindings {
                let to_field = to_variant
                    .field_named(&binding.to_field)
                    .map_err(CompositionSchemaError::MachineSchema)?;

                match &binding.source {
                    RouteBindingSource::Field {
                        from_field,
                        allow_named_alias,
                    } => {
                        let from_field_schema = from_variant
                            .field_named(from_field)
                            .map_err(CompositionSchemaError::MachineSchema)?;

                        let exact_match = from_field_schema.ty == to_field.ty;
                        let named_alias_match = *allow_named_alias
                            && matches!(
                                (&from_field_schema.ty, &to_field.ty),
                                (TypeRef::Named(_), TypeRef::Named(_))
                            );

                        if !exact_match && !named_alias_match {
                            return Err(CompositionSchemaError::RouteFieldTypeMismatch {
                                route: route.name.clone(),
                                from_machine: route.from_machine.clone(),
                                from_field: from_field.clone(),
                                from_ty: from_field_schema.ty.clone(),
                                to_machine: route.to.machine.clone(),
                                to_field: binding.to_field.clone(),
                                to_ty: to_field.ty.clone(),
                            });
                        }
                    }
                    RouteBindingSource::Literal(expr) => {
                        if !route_literal_expr_allowed(expr) {
                            return Err(CompositionSchemaError::UnsupportedRouteLiteral {
                                route: route.name.clone(),
                                to_machine: route.to.machine.clone(),
                                to_field: binding.to_field.clone(),
                            });
                        }

                        if !literal_matches_type(expr, &to_field.ty) {
                            return Err(CompositionSchemaError::RouteLiteralTypeMismatch {
                                route: route.name.clone(),
                                to_machine: route.to.machine.clone(),
                                to_field: binding.to_field.clone(),
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
                            && instance.machine_name == schema.machine
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: entry_input.machine.clone(),
                })?;

            let input_variants = machine_schema
                .inputs
                .variants_by_name()
                .map_err(CompositionSchemaError::MachineSchema)?;
            if !input_variants.contains(&entry_input.input_variant) {
                return Err(CompositionSchemaError::UnknownRouteInput {
                    machine: entry_input.machine.clone(),
                    input: entry_input.input_variant.clone(),
                });
            }
        }

        for witness in &self.witnesses {
            for preload in &witness.preload_inputs {
                let machine_schema = schemas
                    .iter()
                    .find(|schema| {
                        self.machines.iter().any(|instance| {
                            instance.instance_id == preload.machine
                                && instance.machine_name == schema.machine
                        })
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                        machine: preload.machine.clone(),
                    })?;

                let input_variant = machine_schema
                    .inputs
                    .variant_named(&preload.input_variant)
                    .map_err(CompositionSchemaError::MachineSchema)?;

                for field in &preload.fields {
                    let target_field = input_variant
                        .field_named(&field.field)
                        .map_err(CompositionSchemaError::MachineSchema)?;
                    if !route_literal_expr_allowed(&field.expr) {
                        return Err(CompositionSchemaError::UnsupportedWitnessLiteral {
                            witness: witness.name.clone(),
                            machine: preload.machine.clone(),
                            field: field.field.clone(),
                        });
                    }
                    if !literal_matches_type(&field.expr, &target_field.ty) {
                        return Err(CompositionSchemaError::WitnessLiteralTypeMismatch {
                            witness: witness.name.clone(),
                            machine: preload.machine.clone(),
                            field: field.field.clone(),
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
                            machine: preload.machine.clone(),
                            input_variant: preload.input_variant.clone(),
                            field: field.name.clone(),
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
                                && instance.machine_name == schema.machine
                        })
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                        machine: state.machine.clone(),
                    })?;

                if let Some(phase) = &state.phase {
                    let phases = machine_schema
                        .state
                        .phase
                        .variants_by_name()
                        .map_err(CompositionSchemaError::MachineSchema)?;
                    if !phases.contains(phase) {
                        return Err(CompositionSchemaError::UnknownWitnessPhase {
                            witness: witness.name.clone(),
                            machine: state.machine.clone(),
                            phase: phase.clone(),
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
                            machine: state.machine.clone(),
                            field: field.field.clone(),
                        })?;
                    if !route_literal_expr_allowed(&field.expr) {
                        return Err(CompositionSchemaError::UnsupportedWitnessStateLiteral {
                            witness: witness.name.clone(),
                            machine: state.machine.clone(),
                            field: field.field.clone(),
                        });
                    }
                    if !literal_matches_type(&field.expr, &target_field.ty) {
                        return Err(CompositionSchemaError::WitnessStateLiteralTypeMismatch {
                            witness: witness.name.clone(),
                            machine: state.machine.clone(),
                            field: field.field.clone(),
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
                            && instance.machine_name == schema.machine
                    })
                })
                .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                    machine: protocol.producer_instance.clone(),
                })?;

            // Effect variant must exist on the producer.
            let effect_variants = producer_schema
                .effects
                .variants_by_name()
                .map_err(CompositionSchemaError::MachineSchema)?;
            if !effect_variants.contains(&protocol.effect_variant) {
                return Err(CompositionSchemaError::UnknownHandoffEffect {
                    protocol: protocol.name.clone(),
                    effect: protocol.effect_variant.clone(),
                });
            }

            // The producer's disposition rule must reference this protocol.
            let disposition_rule = producer_schema
                .effect_dispositions
                .iter()
                .find(|rule| rule.effect_variant == protocol.effect_variant);
            if let Some(rule) = disposition_rule {
                match &rule.handoff_protocol {
                    Some(hp) if hp == &protocol.name => {}
                    _ => {
                        return Err(CompositionSchemaError::HandoffProtocolMismatch {
                            protocol: protocol.name.clone(),
                            effect_variant: protocol.effect_variant.clone(),
                            expected_protocol: protocol.name.clone(),
                        });
                    }
                }
            }

            // Correlation fields must exist on the effect variant.
            let effect_variant_schema = producer_schema
                .effects
                .variant_named(&protocol.effect_variant)
                .map_err(CompositionSchemaError::MachineSchema)?;
            for field in &protocol.correlation_fields {
                effect_variant_schema.field_named(field).map_err(|_| {
                    CompositionSchemaError::UnknownHandoffCorrelationField {
                        protocol: protocol.name.clone(),
                        field: field.clone(),
                    }
                })?;
            }
            for field in &protocol.obligation_fields {
                effect_variant_schema.field_named(field).map_err(|_| {
                    CompositionSchemaError::UnknownHandoffObligationField {
                        protocol: protocol.name.clone(),
                        field: field.clone(),
                    }
                })?;
            }

            // Feedback inputs must exist on their target machines.
            for feedback in &protocol.allowed_feedback_inputs {
                let target_schema = schemas
                    .iter()
                    .find(|schema| {
                        self.machines.iter().any(|instance| {
                            instance.instance_id == feedback.machine_instance
                                && instance.machine_name == schema.machine
                        })
                    })
                    .ok_or_else(|| CompositionSchemaError::UnknownMachine {
                        machine: feedback.machine_instance.clone(),
                    })?;
                let input_variants = target_schema
                    .inputs
                    .variants_by_name()
                    .map_err(CompositionSchemaError::MachineSchema)?;
                if !input_variants.contains(&feedback.input_variant) {
                    return Err(CompositionSchemaError::UnknownHandoffFeedbackInput {
                        protocol: protocol.name.clone(),
                        machine: feedback.machine_instance.clone(),
                        input: feedback.input_variant.clone(),
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
                    .variant_named(&feedback.input_variant)
                    .map_err(CompositionSchemaError::MachineSchema)?;
                for field in &input_variant_schema.fields {
                    if !feedback
                        .field_bindings
                        .iter()
                        .any(|binding| binding.input_field == field.name)
                    {
                        return Err(CompositionSchemaError::MissingHandoffFeedbackBinding {
                            protocol: protocol.name.clone(),
                            machine: feedback.machine_instance.clone(),
                            input: feedback.input_variant.clone(),
                            field: field.name.clone(),
                        });
                    }
                }
                for binding in &feedback.field_bindings {
                    input_variant_schema
                        .field_named(&binding.input_field)
                        .map_err(
                            |_| CompositionSchemaError::UnknownHandoffFeedbackInputField {
                                protocol: protocol.name.clone(),
                                machine: feedback.machine_instance.clone(),
                                input: feedback.input_variant.clone(),
                                field: binding.input_field.clone(),
                            },
                        )?;
                    if let FeedbackFieldSource::ObligationField(field) = &binding.source
                        && !protocol.obligation_fields.contains(field)
                    {
                        return Err(
                            CompositionSchemaError::UnknownHandoffBindingObligationField {
                                protocol: protocol.name.clone(),
                                field: field.clone(),
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
                            protocol: protocol.name.clone(),
                            machine: feedback.machine_instance.clone(),
                            input: feedback.input_variant.clone(),
                            obligation_field: correlation_field.clone(),
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
                        protocol: protocol.name.clone(),
                        detail: "executor_trigger_input_variant missing after executor validation"
                            .into(),
                    });
                };
                producer_schema.inputs.variant_named(trigger).map_err(|_| {
                    CompositionSchemaError::InvalidHandoffRustBinding {
                        protocol: protocol.name.clone(),
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
                        && route.to.input_variant == feedback.input_variant
                }) {
                    return Err(CompositionSchemaError::DirectRouteBypassesHandoffProtocol {
                        protocol: protocol.name.clone(),
                        machine: feedback.machine_instance.clone(),
                        input: feedback.input_variant.clone(),
                    });
                }
            }

            // TerminalClosure requires the producer machine to have terminal phases.
            if protocol.closure_policy == ClosurePolicy::TerminalClosure
                && producer_schema.state.terminal_phases.is_empty()
            {
                return Err(
                    CompositionSchemaError::TerminalClosureRequiresTerminalPhases {
                        protocol: protocol.name.clone(),
                        producer_instance: protocol.producer_instance.clone(),
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
                    .find(|schema| schema.machine == machine_instance.machine_name)
                    .ok_or_else(|| CompositionSchemaError::UnknownMachineSchema {
                        schema: machine_instance.machine_name.clone(),
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
                                        from_instance: machine_instance.instance_id.clone(),
                                        effect_variant: rule.effect_variant.clone(),
                                        consumer_machine: consumer_machine_name.clone(),
                                        consumer_instance: consumer_inst.instance_id.clone(),
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
                    .find(|schema| schema.machine == machine_instance.machine_name)
                    .ok_or_else(|| CompositionSchemaError::UnknownMachineSchema {
                        schema: machine_instance.machine_name.clone(),
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
                                from_instance: machine_instance.instance_id.clone(),
                                effect_variant: rule.effect_variant.clone(),
                                expected_protocol: protocol_name.clone(),
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
    pub instance_id: String,
    pub machine_name: String,
    pub actor: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInput {
    pub name: String,
    pub machine: String,
    pub input_variant: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub name: String,
    pub from_machine: String,
    pub effect_variant: String,
    pub to: RouteTarget,
    pub bindings: Vec<RouteFieldBinding>,
    pub delivery: RouteDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteTarget {
    pub machine: String,
    pub input_variant: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFieldBinding {
    pub to_field: String,
    pub source: RouteBindingSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteBindingSource {
    Field {
        from_field: String,
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
    PreemptWhenReady { higher: String, lower: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorPriority {
    pub higher: String,
    pub lower: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionInvariant {
    pub name: String,
    pub kind: CompositionInvariantKind,
    pub statement: String,
    pub references_machines: Vec<String>,
    pub references_actors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositionInvariantKind {
    RoutePresent {
        from_machine: String,
        effect_variant: String,
        to_machine: String,
        input_variant: String,
    },
    ObservedInputOriginatesFromEffect {
        to_machine: String,
        input_variant: String,
        from_machine: String,
        effect_variant: String,
    },
    ObservedRouteInputOriginatesFromEffect {
        route_name: String,
        to_machine: String,
        input_variant: String,
        from_machine: String,
        effect_variant: String,
    },
    ActorPriorityPresent {
        higher: String,
        lower: String,
    },
    SchedulerRulePresent {
        rule: SchedulerRule,
    },
    OutcomeHandled {
        from_machine: String,
        effect_variant: String,
        required_targets: Vec<RouteTarget>,
    },
    HandoffProtocolCovered {
        producer_instance: String,
        effect_variant: String,
        protocol_name: String,
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
                    && instance.machine_name == schema.machine
            })
        })
        .ok_or_else(|| CompositionSchemaError::UnknownMachine {
            machine: transition.machine.clone(),
        })?;

    let present = machine_schema
        .transitions
        .iter()
        .any(|candidate| candidate.name == transition.transition);
    if !present {
        return Err(CompositionSchemaError::UnknownWitnessTransition {
            witness: witness.name.clone(),
            machine: transition.machine.clone(),
            transition: transition.transition.clone(),
        });
    }

    Ok(())
}

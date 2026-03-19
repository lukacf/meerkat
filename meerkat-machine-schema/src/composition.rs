use crate::{Expr, MachineSchema, TypeRef, machine::MachineSchemaError};
use indexmap::IndexSet;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionSchema {
    pub name: String,
    pub machines: Vec<MachineInstance>,
    pub entry_inputs: Vec<EntryInput>,
    pub routes: Vec<Route>,
    pub actor_priorities: Vec<ActorPriority>,
    pub scheduler_rules: Vec<SchedulerRule>,
    pub invariants: Vec<CompositionInvariant>,
    pub witnesses: Vec<CompositionWitness>,
    pub deep_domain_cardinality: usize,
    pub deep_domain_overrides: BTreeMap<String, usize>,
    pub witness_domain_cardinality: usize,
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
        let actor_ids = unique_names(
            self.machines.iter().map(|item| item.actor.as_str()),
            "actor",
        )?;
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
}

impl CompositionInvariantKind {
    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            CompositionInvariantKind::RoutePresent { .. }
                | CompositionInvariantKind::ActorPriorityPresent { .. }
                | CompositionInvariantKind::SchedulerRulePresent { .. }
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

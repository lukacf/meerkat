use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionInvariantKind, CompositionSchema,
    CompositionStateLimits, CompositionWitness, EntryInput, EnumSchema, Expr, Guard, HelperSchema,
    MachineCoverageManifest, MachineSchema, Quantifier, Route, RouteBindingSource, RouteDelivery,
    SchedulerRule, TransitionSchema, TypeRef, Update, canonical_machine_schemas,
};

pub fn render_machine_contract_markdown(
    schema: &MachineSchema,
    coverage: &MachineCoverageManifest,
) -> String {
    let mut out = String::new();

    writeln!(&mut out, "# {}", schema.machine).expect("write to string");
    writeln!(&mut out).expect("write to string");
    writeln!(
        &mut out,
        "_Generated from the Rust machine catalog. Do not edit by hand._"
    )
    .expect("write to string");
    writeln!(&mut out).expect("write to string");
    writeln!(&mut out, "- Version: `{}`", schema.version).expect("write to string");
    writeln!(
        &mut out,
        "- Rust owner: `{}` / `{}`",
        schema.rust.crate_name, schema.rust.module
    )
    .expect("write to string");
    writeln!(&mut out).expect("write to string");

    render_state_markdown(&mut out, schema);
    render_enum_markdown(&mut out, "Inputs", &schema.inputs);
    render_enum_markdown(&mut out, "Effects", &schema.effects);

    if !schema.helpers.is_empty() {
        writeln!(&mut out, "## Helpers").expect("write to string");
        for helper in &schema.helpers {
            writeln!(
                &mut out,
                "- `{}`({}) -> `{}`",
                helper.name,
                render_field_list(&helper.params),
                render_type_ref(&helper.returns)
            )
            .expect("write to string");
        }
        writeln!(&mut out).expect("write to string");
    }

    if !schema.derived.is_empty() {
        writeln!(&mut out, "## Derived").expect("write to string");
        for derived in &schema.derived {
            writeln!(
                &mut out,
                "- `{}`({}) -> `{}`",
                derived.name,
                render_field_list(&derived.params),
                render_type_ref(&derived.returns)
            )
            .expect("write to string");
        }
        writeln!(&mut out).expect("write to string");
    }

    writeln!(&mut out, "## Invariants").expect("write to string");
    for invariant in &schema.invariants {
        writeln!(&mut out, "- `{}`", invariant.name).expect("write to string");
    }
    writeln!(&mut out).expect("write to string");

    writeln!(&mut out, "## Transitions").expect("write to string");
    for transition in &schema.transitions {
        writeln!(&mut out, "### `{}`", transition.name).expect("write to string");
        writeln!(
            &mut out,
            "- From: {}",
            transition
                .from
                .iter()
                .map(|item| format!("`{item}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "- On: `{}`({})",
            transition.on.variant,
            transition.on.bindings.join(", ")
        )
        .expect("write to string");
        if !transition.guards.is_empty() {
            writeln!(&mut out, "- Guards:").expect("write to string");
            for guard in &transition.guards {
                writeln!(&mut out, "  - `{}`", guard.name).expect("write to string");
            }
        }
        if !transition.emit.is_empty() {
            writeln!(
                &mut out,
                "- Emits: {}",
                transition
                    .emit
                    .iter()
                    .map(|effect| format!("`{}`", effect.variant))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .expect("write to string");
        }
        writeln!(&mut out, "- To: `{}`", transition.to).expect("write to string");
        writeln!(&mut out).expect("write to string");
    }

    writeln!(&mut out, "## Coverage").expect("write to string");
    writeln!(&mut out, "### Code Anchors").expect("write to string");
    for anchor in &coverage.code_anchors {
        writeln!(&mut out, "- `{}` — {}", anchor.path, anchor.note).expect("write to string");
    }
    writeln!(&mut out).expect("write to string");
    writeln!(&mut out, "### Scenarios").expect("write to string");
    for scenario in &coverage.scenarios {
        writeln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary).expect("write to string");
    }

    out
}

pub fn render_composition_contract_markdown(
    schema: &CompositionSchema,
    coverage: &CompositionCoverageManifest,
) -> String {
    let mut out = String::new();

    writeln!(&mut out, "# {}", schema.name).expect("write to string");
    writeln!(&mut out).expect("write to string");
    writeln!(
        &mut out,
        "_Generated from the Rust composition catalog. Do not edit by hand._"
    )
    .expect("write to string");
    writeln!(&mut out).expect("write to string");

    writeln!(&mut out, "## Machines").expect("write to string");
    for machine in &schema.machines {
        writeln!(
            &mut out,
            "- `{}`: `{}` @ actor `{}`",
            machine.instance_id, machine.machine_name, machine.actor
        )
        .expect("write to string");
    }
    writeln!(&mut out).expect("write to string");

    writeln!(&mut out, "## Routes").expect("write to string");
    for route in &schema.routes {
        writeln!(
            &mut out,
            "- `{}`: `{}`.`{}` -> `{}`.`{}` [{}]",
            route.name,
            route.from_machine,
            route.effect_variant,
            route.to.machine,
            route.to.input_variant,
            render_route_delivery(route)
        )
        .expect("write to string");
    }
    writeln!(&mut out).expect("write to string");

    writeln!(&mut out, "## Scheduler Rules").expect("write to string");
    if schema.scheduler_rules.is_empty() {
        writeln!(&mut out, "- `(none)`").expect("write to string");
    } else {
        for rule in &schema.scheduler_rules {
            writeln!(&mut out, "- `{}`", render_scheduler_rule(rule)).expect("write to string");
        }
    }
    writeln!(&mut out).expect("write to string");

    let structural_invariants = schema
        .invariants
        .iter()
        .filter(|invariant| invariant.kind.is_structural())
        .collect::<Vec<_>>();
    let behavioral_invariants = schema
        .invariants
        .iter()
        .filter(|invariant| invariant.kind.is_behavioral())
        .collect::<Vec<_>>();

    writeln!(&mut out, "## Structural Requirements").expect("write to string");
    if structural_invariants.is_empty() {
        writeln!(&mut out, "- `(none)`").expect("write to string");
    } else {
        for invariant in &structural_invariants {
            writeln!(&mut out, "- `{}` — {}", invariant.name, invariant.statement)
                .expect("write to string");
        }
    }
    writeln!(&mut out).expect("write to string");

    writeln!(&mut out, "## Behavioral Invariants").expect("write to string");
    if behavioral_invariants.is_empty() {
        writeln!(&mut out, "- `(none)`").expect("write to string");
    } else {
        for invariant in &behavioral_invariants {
            writeln!(&mut out, "- `{}` — {}", invariant.name, invariant.statement)
                .expect("write to string");
        }
    }
    writeln!(&mut out).expect("write to string");

    writeln!(&mut out, "## Coverage").expect("write to string");
    writeln!(&mut out, "### Code Anchors").expect("write to string");
    for anchor in &coverage.code_anchors {
        writeln!(&mut out, "- `{}` — {}", anchor.path, anchor.note).expect("write to string");
    }
    writeln!(&mut out).expect("write to string");
    writeln!(&mut out, "### Scenarios").expect("write to string");
    for scenario in &coverage.scenarios {
        writeln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary).expect("write to string");
    }

    out
}

pub fn render_machine_ci_cfg(schema: &MachineSchema, deep: bool) -> String {
    let mut out = String::new();
    let domains = collect_binding_domains(schema);
    let named_samples = collect_machine_named_type_samples(schema);

    writeln!(&mut out, "SPECIFICATION Spec").expect("write to string");
    if !domains.is_empty() {
        writeln!(&mut out, "CONSTANTS").expect("write to string");
        for (name, ty) in domains {
            if matches!(ty, TypeRef::Seq(_)) {
                continue;
            }
            writeln!(
                &mut out,
                "  {} = {}",
                name,
                render_default_domain_assignment(
                    &ty,
                    default_sample_cardinality(deep),
                    &named_samples,
                )
            )
            .expect("write to string");
        }
    }
    if !schema.invariants.is_empty() {
        writeln!(&mut out, "INVARIANTS").expect("write to string");
        for invariant in &schema.invariants {
            writeln!(&mut out, "  {}", invariant.name).expect("write to string");
        }
    }
    writeln!(&mut out, "CONSTRAINTS").expect("write to string");
    writeln!(
        &mut out,
        "  {}",
        if deep {
            "DeepStateConstraint"
        } else {
            "CiStateConstraint"
        }
    )
    .expect("write to string");

    out
}

pub fn render_composition_ci_cfg(schema: &CompositionSchema, deep: bool) -> String {
    let mut out = String::new();
    let machine_catalog = canonical_machine_schemas();
    let machine_by_name = machine_catalog
        .iter()
        .map(|machine| (machine.machine.as_str(), machine))
        .collect::<BTreeMap<_, _>>();
    let machine_by_instance = schema
        .machines
        .iter()
        .map(|instance| {
            (
                instance.instance_id.as_str(),
                *machine_by_name
                    .get(instance.machine_name.as_str())
                    .expect("canonical composition machine"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let domains = collect_composition_binding_domains(schema, &machine_by_instance);
    let named_samples = collect_composition_named_type_samples(schema, &machine_by_instance);
    let mut instance_invariants = Vec::new();

    for instance in &schema.machines {
        let machine = machine_by_instance
            .get(instance.instance_id.as_str())
            .copied()
            .expect("canonical composition machine");
        for invariant in &machine.invariants {
            instance_invariants.push(format!(
                "{}_{}",
                tla_ident(&instance.instance_id),
                tla_ident(&invariant.name)
            ));
        }
    }

    writeln!(&mut out, "SPECIFICATION Spec").expect("write to string");
    if !domains.is_empty() {
        writeln!(&mut out, "CONSTANTS").expect("write to string");
        for (name, ty) in domains {
            if matches!(ty, TypeRef::Seq(_)) {
                continue;
            }
            writeln!(
                &mut out,
                "  {} = {}",
                name,
                render_default_domain_assignment(
                    &ty,
                    if deep {
                        schema.deep_domain_cardinality
                    } else {
                        default_sample_cardinality(false)
                    },
                    &named_samples,
                )
            )
            .expect("write to string");
        }
    }
    let behavioral_invariants = schema
        .invariants
        .iter()
        .filter(|invariant| invariant.kind.is_behavioral())
        .collect::<Vec<_>>();
    if !behavioral_invariants.is_empty() || !instance_invariants.is_empty() || deep {
        writeln!(&mut out, "INVARIANTS").expect("write to string");
        for invariant in behavioral_invariants {
            writeln!(&mut out, "  {}", invariant.name).expect("write to string");
        }
        for invariant_name in instance_invariants {
            writeln!(&mut out, "  {}", invariant_name).expect("write to string");
        }
        if deep {
            writeln!(&mut out, "  CoverageInstrumentation").expect("write to string");
        }
    }
    writeln!(&mut out, "CONSTRAINTS").expect("write to string");
    writeln!(
        &mut out,
        "  {}",
        if deep {
            "DeepStateConstraint"
        } else {
            "CiStateConstraint"
        }
    )
    .expect("write to string");

    out
}

pub fn render_composition_witness_cfg(
    schema: &CompositionSchema,
    witness: &CompositionWitness,
) -> String {
    let mut out = String::new();
    let machine_catalog = canonical_machine_schemas();
    let machine_by_name = machine_catalog
        .iter()
        .map(|machine| (machine.machine.as_str(), machine))
        .collect::<BTreeMap<_, _>>();
    let machine_by_instance = schema
        .machines
        .iter()
        .map(|instance| {
            (
                instance.instance_id.as_str(),
                *machine_by_name
                    .get(instance.machine_name.as_str())
                    .expect("canonical composition machine"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let domains = collect_composition_binding_domains(schema, &machine_by_instance);
    let named_samples =
        collect_composition_witness_named_type_samples(schema, witness, &machine_by_instance);
    let mut instance_invariants = Vec::new();

    for instance in &schema.machines {
        let machine = machine_by_instance
            .get(instance.instance_id.as_str())
            .copied()
            .expect("canonical composition machine");
        for invariant in &machine.invariants {
            instance_invariants.push(format!(
                "{}_{}",
                tla_ident(&instance.instance_id),
                tla_ident(&invariant.name)
            ));
        }
    }

    writeln!(
        &mut out,
        "SPECIFICATION {}",
        composition_witness_spec_name(&witness.name)
    )
    .expect("write to string");
    if !domains.is_empty() {
        writeln!(&mut out, "CONSTANTS").expect("write to string");
        for (name, ty) in domains {
            if matches!(ty, TypeRef::Seq(_)) {
                continue;
            }
            writeln!(
                &mut out,
                "  {} = {}",
                name,
                render_default_domain_assignment(
                    &ty,
                    schema.witness_domain_cardinality,
                    &named_samples,
                )
            )
            .expect("write to string");
        }
    }
    if !schema.invariants.is_empty() || !instance_invariants.is_empty() {
        writeln!(&mut out, "INVARIANTS").expect("write to string");
        for invariant in &schema.invariants {
            writeln!(&mut out, "  {}", invariant.name).expect("write to string");
        }
        for invariant_name in instance_invariants {
            writeln!(&mut out, "  {}", invariant_name).expect("write to string");
        }
        writeln!(&mut out, "  CoverageInstrumentation").expect("write to string");
    }
    let witness_properties = witness
        .expected_routes
        .iter()
        .map(|route| composition_witness_route_property_name(&witness.name, route))
        .chain(
            witness
                .expected_scheduler_rules
                .iter()
                .map(|rule| composition_witness_scheduler_property_name(&witness.name, rule)),
        )
        .chain(
            witness
                .expected_states
                .iter()
                .enumerate()
                .map(|(idx, _)| composition_witness_state_property_name(&witness.name, idx)),
        )
        .chain(witness.expected_transitions.iter().map(|transition| {
            composition_witness_transition_property_name(
                &witness.name,
                &transition.machine,
                &transition.transition,
            )
        }))
        .chain(
            witness
                .expected_transition_order
                .iter()
                .enumerate()
                .map(|(idx, _)| {
                    composition_witness_transition_order_property_name(&witness.name, idx)
                }),
        )
        .collect::<Vec<_>>();
    if !witness_properties.is_empty() {
        writeln!(&mut out, "PROPERTIES").expect("write to string");
        for property in witness_properties {
            writeln!(&mut out, "  {property}").expect("write to string");
        }
    }
    writeln!(&mut out, "CONSTRAINTS").expect("write to string");
    writeln!(
        &mut out,
        "  {}",
        composition_witness_state_constraint_name(&witness.name)
    )
    .expect("write to string");

    out
}

pub fn composition_route_observed_operator_name(route_name: &str) -> String {
    format!("RouteObserved_{}", tla_ident(route_name))
}

pub fn composition_route_coverage_operator_name(route_name: &str) -> String {
    format!("RouteCoverage_{}", tla_ident(route_name))
}

pub fn composition_scheduler_triggered_operator_name(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => format!(
            "SchedulerTriggered_PreemptWhenReady_{}_{}",
            tla_ident(higher),
            tla_ident(lower)
        ),
    }
}

pub fn composition_scheduler_coverage_operator_name(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => format!(
            "SchedulerCoverage_PreemptWhenReady_{}_{}",
            tla_ident(higher),
            tla_ident(lower)
        ),
    }
}

pub fn composition_witness_cfg_name(name: &str) -> String {
    format!("witness-{}.cfg", tla_ident(name))
}

fn composition_witness_route_property_name(witness: &str, route_name: &str) -> String {
    format!(
        "WitnessRouteObserved_{}_{}",
        tla_ident(witness),
        tla_ident(route_name)
    )
}

fn composition_witness_scheduler_property_name(witness: &str, rule: &SchedulerRule) -> String {
    format!(
        "WitnessSchedulerTriggered_{}_{}",
        tla_ident(witness),
        tla_ident(&witness_scheduler_rule_label(rule))
    )
}

fn composition_witness_state_property_name(witness: &str, index: usize) -> String {
    format!("WitnessStateObserved_{}_{}", tla_ident(witness), index + 1)
}

fn composition_witness_transition_property_name(
    witness: &str,
    machine: &str,
    transition: &str,
) -> String {
    format!(
        "WitnessTransitionObserved_{}_{}_{}",
        tla_ident(witness),
        tla_ident(machine),
        tla_ident(transition)
    )
}

fn composition_witness_transition_order_property_name(witness: &str, index: usize) -> String {
    format!(
        "WitnessTransitionOrder_{}_{}",
        tla_ident(witness),
        index + 1
    )
}

fn witness_scheduler_rule_label(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!("PreemptWhenReady_{}_{}", higher, lower)
        }
    }
}

fn composition_witness_init_name(name: &str) -> String {
    format!("WitnessInit_{}", tla_ident(name))
}

fn composition_witness_next_name(name: &str) -> String {
    format!("WitnessNext_{}", tla_ident(name))
}

fn composition_witness_spec_name(name: &str) -> String {
    format!("WitnessSpec_{}", tla_ident(name))
}

fn composition_witness_state_constraint_name(name: &str) -> String {
    format!("WitnessStateConstraint_{}", tla_ident(name))
}

pub fn render_composition_semantic_model(schema: &CompositionSchema) -> String {
    let machine_catalog = canonical_machine_schemas();
    CompositionTlaCompiler::new(schema, &machine_catalog)
        .expect("composition machine lookup")
        .render()
}

pub fn render_machine_semantic_model(schema: &MachineSchema) -> String {
    let mut compiler = MachineTlaCompiler::new(schema);
    compiler.render()
}

fn render_state_markdown(out: &mut String, schema: &MachineSchema) {
    writeln!(out, "## State").expect("write to string");
    writeln!(
        out,
        "- Phase enum: `{}`",
        schema
            .state
            .phase
            .variants
            .iter()
            .map(|variant| variant.name.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    )
    .expect("write to string");
    for field in &schema.state.fields {
        writeln!(out, "- `{}`: `{}`", field.name, render_type_ref(&field.ty))
            .expect("write to string");
    }
    writeln!(out).expect("write to string");
}

fn render_enum_markdown(out: &mut String, label: &str, schema: &EnumSchema) {
    writeln!(out, "## {label}").expect("write to string");
    for variant in &schema.variants {
        if variant.fields.is_empty() {
            writeln!(out, "- `{}`", variant.name).expect("write to string");
        } else {
            writeln!(
                out,
                "- `{}`({})",
                variant.name,
                render_field_list(&variant.fields)
            )
            .expect("write to string");
        }
    }
    writeln!(out).expect("write to string");
}

fn render_field_list(fields: &[meerkat_machine_schema::FieldSchema]) -> String {
    fields
        .iter()
        .map(|field| format!("{}: {}", field.name, render_type_ref(&field.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn collect_binding_domains(schema: &MachineSchema) -> BTreeMap<String, TypeRef> {
    let mut domains = BTreeMap::new();

    for transition in &schema.transitions {
        if let Some(variant) = schema
            .inputs
            .variants
            .iter()
            .find(|item| item.name == transition.on.variant)
        {
            for binding in &transition.on.bindings {
                if let Some(field) = variant.fields.iter().find(|field| field.name == *binding) {
                    let name = domain_constant_name(&field.ty);
                    domains.entry(name).or_insert_with(|| field.ty.clone());
                }
            }
        }
    }

    domains
}

fn collect_composition_binding_domains(
    schema: &CompositionSchema,
    machine_by_instance: &BTreeMap<&str, &MachineSchema>,
) -> BTreeMap<String, TypeRef> {
    let mut domains = BTreeMap::new();

    for machine in machine_by_instance.values() {
        for (name, ty) in collect_binding_domains(machine) {
            domains.entry(name).or_insert(ty);
        }
    }

    for entry_input in &schema.entry_inputs {
        let machine = machine_by_instance
            .get(entry_input.machine.as_str())
            .copied()
            .expect("entry-input machine schema");
        let variant = machine
            .inputs
            .variant_named(&entry_input.input_variant)
            .expect("entry-input variant");
        for field in &variant.fields {
            let name = domain_constant_name(&field.ty);
            domains.entry(name).or_insert_with(|| field.ty.clone());
        }
    }

    domains
}

fn collect_machine_named_type_samples(
    schema: &MachineSchema,
) -> BTreeMap<String, BTreeSet<String>> {
    let field_types = schema
        .state
        .fields
        .iter()
        .map(|field| (field.name.clone(), field.ty.clone()))
        .collect::<BTreeMap<_, _>>();
    let helper_returns = schema
        .helpers
        .iter()
        .chain(schema.derived.iter())
        .map(|helper| (helper.name.clone(), helper.returns.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut samples = BTreeMap::new();

    for init in &schema.state.init.fields {
        if let Some(field_ty) = field_types.get(&init.field) {
            collect_named_literals_from_expr(
                &mut samples,
                &init.expr,
                Some(field_ty),
                &field_types,
                &helper_returns,
                &BTreeMap::new(),
            );
        }
    }

    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        let bindings = helper
            .params
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();
        collect_named_literals_from_expr(
            &mut samples,
            &helper.body,
            Some(&helper.returns),
            &field_types,
            &helper_returns,
            &bindings,
        );
    }

    for invariant in &schema.invariants {
        collect_named_literals_from_expr(
            &mut samples,
            &invariant.expr,
            None,
            &field_types,
            &helper_returns,
            &BTreeMap::new(),
        );
    }

    for transition in &schema.transitions {
        let binding_types = schema
            .inputs
            .variant_named(&transition.on.variant)
            .expect("transition input variant")
            .fields
            .iter()
            .filter(|field| {
                transition
                    .on
                    .bindings
                    .iter()
                    .any(|binding| binding == &field.name)
            })
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();

        for guard in &transition.guards {
            collect_named_literals_from_expr(
                &mut samples,
                &guard.expr,
                None,
                &field_types,
                &helper_returns,
                &binding_types,
            );
        }
        for update in &transition.updates {
            collect_named_literals_from_update(
                &mut samples,
                update,
                &field_types,
                &helper_returns,
                &binding_types,
            );
        }
        for effect in &transition.emit {
            let effect_variant = schema
                .effects
                .variant_named(&effect.variant)
                .expect("effect variant");
            for field in &effect_variant.fields {
                if let Some(expr) = effect.fields.get(&field.name) {
                    collect_named_literals_from_expr(
                        &mut samples,
                        expr,
                        Some(&field.ty),
                        &field_types,
                        &helper_returns,
                        &binding_types,
                    );
                }
            }
        }
    }

    samples
}

fn collect_composition_named_type_samples(
    schema: &CompositionSchema,
    machine_by_instance: &BTreeMap<&str, &MachineSchema>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut samples = BTreeMap::new();
    for machine in machine_by_instance.values() {
        merge_named_type_samples(&mut samples, collect_machine_named_type_samples(machine));
    }

    for entry_input in &schema.entry_inputs {
        let machine = machine_by_instance
            .get(entry_input.machine.as_str())
            .copied()
            .expect("entry-input machine schema");
        merge_named_type_samples(&mut samples, collect_machine_named_type_samples(machine));
    }

    for route in &schema.routes {
        let source_machine = machine_by_instance
            .get(route.from_machine.as_str())
            .copied()
            .expect("route source machine schema");
        let source_variant = source_machine
            .effects
            .variant_named(&route.effect_variant)
            .expect("route source effect variant");
        let source_field_types = source_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();
        let target_machine = machine_by_instance
            .get(route.to.machine.as_str())
            .copied()
            .expect("route target machine schema");
        let target_variant = target_machine
            .inputs
            .variant_named(&route.to.input_variant)
            .expect("route target input variant");
        let field_types = target_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();

        for binding in &route.bindings {
            if let RouteBindingSource::Literal(expr) = &binding.source {
                if let Some(field_ty) = field_types.get(&binding.to_field) {
                    collect_named_literals_from_expr(
                        &mut samples,
                        expr,
                        Some(field_ty),
                        &field_types,
                        &BTreeMap::new(),
                        &BTreeMap::new(),
                    );
                }
            }
            if let RouteBindingSource::Field { from_field, .. } = &binding.source {
                if let (Some(source_ty), Some(target_ty)) = (
                    source_field_types.get(from_field),
                    field_types.get(&binding.to_field),
                ) {
                    propagate_named_samples_between_types(&mut samples, source_ty, target_ty);
                }
            }
        }
    }

    for witness in &schema.witnesses {
        for preload in &witness.preload_inputs {
            let machine = machine_by_instance
                .get(preload.machine.as_str())
                .copied()
                .expect("witness preload machine schema");
            let variant = machine
                .inputs
                .variant_named(&preload.input_variant)
                .expect("witness preload input variant");
            let field_types = variant
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.ty.clone()))
                .collect::<BTreeMap<_, _>>();

            for field in &preload.fields {
                if let Some(field_ty) = field_types.get(&field.field) {
                    collect_named_literals_from_expr(
                        &mut samples,
                        &field.expr,
                        Some(field_ty),
                        &field_types,
                        &BTreeMap::new(),
                        &BTreeMap::new(),
                    );
                }
            }
        }
    }

    samples
}

fn collect_composition_witness_named_type_samples(
    schema: &CompositionSchema,
    witness: &CompositionWitness,
    machine_by_instance: &BTreeMap<&str, &MachineSchema>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut samples = BTreeMap::new();
    let expected_routes = witness.expected_routes.iter().collect::<BTreeSet<_>>();

    for route in &schema.routes {
        if !expected_routes.contains(&route.name) {
            continue;
        }

        let source_machine = machine_by_instance
            .get(route.from_machine.as_str())
            .copied()
            .expect("route source machine schema");
        let source_variant = source_machine
            .effects
            .variant_named(&route.effect_variant)
            .expect("route source effect variant");
        let source_field_types = source_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();
        let target_machine = machine_by_instance
            .get(route.to.machine.as_str())
            .copied()
            .expect("route target machine schema");
        let target_variant = target_machine
            .inputs
            .variant_named(&route.to.input_variant)
            .expect("route target input variant");
        let field_types = target_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();

        for binding in &route.bindings {
            if let RouteBindingSource::Literal(expr) = &binding.source {
                if let Some(field_ty) = field_types.get(&binding.to_field) {
                    collect_named_literals_from_expr(
                        &mut samples,
                        expr,
                        Some(field_ty),
                        &field_types,
                        &BTreeMap::new(),
                        &BTreeMap::new(),
                    );
                }
            }
            if let RouteBindingSource::Field { from_field, .. } = &binding.source {
                if let (Some(source_ty), Some(target_ty)) = (
                    source_field_types.get(from_field),
                    field_types.get(&binding.to_field),
                ) {
                    propagate_named_samples_between_types(&mut samples, source_ty, target_ty);
                }
            }
        }
    }

    for preload in &witness.preload_inputs {
        let machine = machine_by_instance
            .get(preload.machine.as_str())
            .copied()
            .expect("witness preload machine schema");
        let variant = machine
            .inputs
            .variant_named(&preload.input_variant)
            .expect("witness preload input variant");
        let field_types = variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();

        for field in &preload.fields {
            if let Some(field_ty) = field_types.get(&field.field) {
                collect_named_literals_from_expr(
                    &mut samples,
                    &field.expr,
                    Some(field_ty),
                    &field_types,
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                );
            }
        }

        for route in &schema.routes {
            if !expected_routes.contains(&route.name) || route.from_machine != preload.machine {
                continue;
            }
            let source_machine = machine_by_instance
                .get(route.from_machine.as_str())
                .copied()
                .expect("route source machine schema");
            let source_variant = source_machine
                .effects
                .variant_named(&route.effect_variant)
                .expect("route source effect variant");
            let source_field_types = source_variant
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.ty.clone()))
                .collect::<BTreeMap<_, _>>();
            let target_machine = machine_by_instance
                .get(route.to.machine.as_str())
                .copied()
                .expect("route target machine schema");
            let target_variant = target_machine
                .inputs
                .variant_named(&route.to.input_variant)
                .expect("route target input variant");
            let target_field_types = target_variant
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.ty.clone()))
                .collect::<BTreeMap<_, _>>();
            for binding in &route.bindings {
                if let RouteBindingSource::Field { from_field, .. } = &binding.source {
                    if let (Some(source_ty), Some(target_ty)) = (
                        source_field_types.get(from_field),
                        target_field_types.get(&binding.to_field),
                    ) {
                        propagate_named_samples_between_types(&mut samples, source_ty, target_ty);
                    }
                }
            }
        }
    }

    samples
}

fn merge_named_type_samples(
    target: &mut BTreeMap<String, BTreeSet<String>>,
    source: BTreeMap<String, BTreeSet<String>>,
) {
    for (name, values) in source {
        target.entry(name).or_default().extend(values);
    }
}

fn propagate_named_samples_between_types(
    samples: &mut BTreeMap<String, BTreeSet<String>>,
    source_ty: &TypeRef,
    target_ty: &TypeRef,
) {
    match (source_ty, target_ty) {
        (TypeRef::Named(source_name), TypeRef::Named(target_name)) => {
            if let Some(source_samples) = samples.get(source_name).cloned() {
                samples
                    .entry(target_name.clone())
                    .or_default()
                    .extend(source_samples);
            }
        }
        (TypeRef::Option(source_inner), TypeRef::Option(target_inner))
        | (TypeRef::Seq(source_inner), TypeRef::Seq(target_inner))
        | (TypeRef::Set(source_inner), TypeRef::Set(target_inner)) => {
            propagate_named_samples_between_types(samples, source_inner, target_inner);
        }
        _ => {}
    }
}

fn collect_named_literals_from_update(
    samples: &mut BTreeMap<String, BTreeSet<String>>,
    update: &Update,
    field_types: &BTreeMap<String, TypeRef>,
    helper_returns: &BTreeMap<String, TypeRef>,
    binding_types: &BTreeMap<String, TypeRef>,
) {
    match update {
        Update::Assign { field, expr } => {
            collect_named_literals_from_expr(
                samples,
                expr,
                field_types.get(field),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Update::MapInsert { field, key, value } => {
            if let Some(TypeRef::Map(key_ty, value_ty)) = field_types.get(field) {
                collect_named_literals_from_expr(
                    samples,
                    key,
                    Some(key_ty),
                    field_types,
                    helper_returns,
                    binding_types,
                );
                collect_named_literals_from_expr(
                    samples,
                    value,
                    Some(value_ty),
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Update::SetInsert { field, value } | Update::SetRemove { field, value } => {
            if let Some(TypeRef::Set(inner_ty)) = field_types.get(field) {
                collect_named_literals_from_expr(
                    samples,
                    value,
                    Some(inner_ty),
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Update::SeqAppend { field, value } | Update::SeqRemoveValue { field, value } => {
            if let Some(TypeRef::Seq(inner_ty)) = field_types.get(field) {
                collect_named_literals_from_expr(
                    samples,
                    value,
                    Some(inner_ty),
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Update::SeqPrepend { field, values } | Update::SeqRemoveAll { field, values } => {
            if let Some(field_ty) = field_types.get(field) {
                collect_named_literals_from_expr(
                    samples,
                    values,
                    Some(field_ty),
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            collect_named_literals_from_expr(
                samples,
                condition,
                None,
                field_types,
                helper_returns,
                binding_types,
            );
            for nested in then_updates {
                collect_named_literals_from_update(
                    samples,
                    nested,
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
            for nested in else_updates {
                collect_named_literals_from_update(
                    samples,
                    nested,
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Update::ForEach {
            binding,
            over,
            updates,
        } => {
            collect_named_literals_from_expr(
                samples,
                over,
                None,
                field_types,
                helper_returns,
                binding_types,
            );
            let mut nested_bindings = binding_types.clone();
            if let Some(over_ty) = infer_expr_type(over, field_types, helper_returns, binding_types)
            {
                match over_ty {
                    TypeRef::Seq(inner_ty) | TypeRef::Set(inner_ty) => {
                        nested_bindings.insert(binding.clone(), (*inner_ty).clone());
                    }
                    TypeRef::Map(key_ty, _) => {
                        nested_bindings.insert(binding.clone(), (*key_ty).clone());
                    }
                    _ => {}
                }
            }
            for nested in updates {
                collect_named_literals_from_update(
                    samples,
                    nested,
                    field_types,
                    helper_returns,
                    &nested_bindings,
                );
            }
        }
        Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => {}
    }
}

fn collect_named_literals_from_expr(
    samples: &mut BTreeMap<String, BTreeSet<String>>,
    expr: &Expr,
    expected_ty: Option<&TypeRef>,
    field_types: &BTreeMap<String, TypeRef>,
    helper_returns: &BTreeMap<String, TypeRef>,
    binding_types: &BTreeMap<String, TypeRef>,
) {
    if let (Expr::String(value), Some(TypeRef::Named(name))) = (expr, expected_ty) {
        samples
            .entry(name.clone())
            .or_default()
            .insert(value.clone());
    }

    match expr {
        Expr::Eq(left, right) | Expr::Neq(left, right) => {
            let left_ty = infer_expr_type(left, field_types, helper_returns, binding_types);
            let right_ty = infer_expr_type(right, field_types, helper_returns, binding_types);
            if let (Some(TypeRef::Named(name)), Expr::String(value)) = (&left_ty, right.as_ref()) {
                samples
                    .entry(name.clone())
                    .or_default()
                    .insert(value.clone());
            }
            if let (Some(TypeRef::Named(name)), Expr::String(value)) = (&right_ty, left.as_ref()) {
                samples
                    .entry(name.clone())
                    .or_default()
                    .insert(value.clone());
            }
            collect_named_literals_from_expr(
                samples,
                left,
                left_ty.as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                right,
                right_ty.as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_named_literals_from_expr(
                samples,
                condition,
                Some(&TypeRef::Bool),
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                then_expr,
                expected_ty,
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                else_expr,
                expected_ty,
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::Not(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::Some(inner) => {
            let nested_ty = infer_expr_type(inner, field_types, helper_returns, binding_types);
            collect_named_literals_from_expr(
                samples,
                inner,
                nested_ty.as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::And(items) | Expr::Or(items) => {
            for item in items {
                collect_named_literals_from_expr(
                    samples,
                    item,
                    None,
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Expr::SeqLiteral(items) => {
            let item_expected_ty = match expected_ty {
                Some(TypeRef::Seq(inner)) => Some(inner.as_ref()),
                _ => None,
            };
            for item in items {
                collect_named_literals_from_expr(
                    samples,
                    item,
                    item_expected_ty,
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            collect_named_literals_from_expr(
                samples,
                left,
                infer_expr_type(left, field_types, helper_returns, binding_types).as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                right,
                infer_expr_type(right, field_types, helper_returns, binding_types).as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::Contains { collection, value } => {
            let collection_ty =
                infer_expr_type(collection, field_types, helper_returns, binding_types);
            let value_ty = match collection_ty {
                Some(TypeRef::Set(inner_ty)) | Some(TypeRef::Seq(inner_ty)) => Some(*inner_ty),
                Some(TypeRef::Map(key_ty, _)) => Some(*key_ty),
                _ => None,
            };
            collect_named_literals_from_expr(
                samples,
                collection,
                None,
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                value,
                value_ty.as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::SeqStartsWith { seq, prefix } => {
            let seq_ty = infer_expr_type(seq, field_types, helper_returns, binding_types);
            collect_named_literals_from_expr(
                samples,
                seq,
                seq_ty.as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                prefix,
                seq_ty.as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::MapGet { map, key } => {
            let map_ty = infer_expr_type(map, field_types, helper_returns, binding_types);
            let key_ty = match map_ty {
                Some(TypeRef::Map(key_ty, _)) => Some(*key_ty),
                _ => None,
            };
            collect_named_literals_from_expr(
                samples,
                map,
                None,
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                key,
                key_ty.as_ref(),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::Call { helper: _, args } => {
            for arg in args {
                collect_named_literals_from_expr(
                    samples,
                    arg,
                    infer_expr_type(arg, field_types, helper_returns, binding_types).as_ref(),
                    field_types,
                    helper_returns,
                    binding_types,
                );
            }
        }
        Expr::Quantified { over, body, .. } => {
            collect_named_literals_from_expr(
                samples,
                over,
                None,
                field_types,
                helper_returns,
                binding_types,
            );
            collect_named_literals_from_expr(
                samples,
                body,
                Some(&TypeRef::Bool),
                field_types,
                helper_returns,
                binding_types,
            );
        }
        Expr::Bool(_)
        | Expr::U64(_)
        | Expr::String(_)
        | Expr::EmptySet
        | Expr::EmptyMap
        | Expr::CurrentPhase
        | Expr::Phase(_)
        | Expr::Field(_)
        | Expr::Binding(_)
        | Expr::Variant(_)
        | Expr::None => {}
    }
}

fn infer_expr_type(
    expr: &Expr,
    field_types: &BTreeMap<String, TypeRef>,
    helper_returns: &BTreeMap<String, TypeRef>,
    binding_types: &BTreeMap<String, TypeRef>,
) -> Option<TypeRef> {
    match expr {
        Expr::Bool(_) => Some(TypeRef::Bool),
        Expr::U64(_) => Some(TypeRef::U64),
        Expr::String(_) | Expr::CurrentPhase | Expr::Phase(_) | Expr::Variant(_) => {
            Some(TypeRef::String)
        }
        Expr::Field(name) => field_types.get(name).cloned(),
        Expr::Binding(name) => binding_types.get(name).cloned(),
        Expr::None => None,
        Expr::IfElse {
            then_expr,
            else_expr,
            ..
        } => {
            let then_ty = infer_expr_type(then_expr, field_types, helper_returns, binding_types);
            let else_ty = infer_expr_type(else_expr, field_types, helper_returns, binding_types);
            if then_ty == else_ty {
                then_ty
            } else {
                then_ty.or(else_ty)
            }
        }
        Expr::Not(_)
        | Expr::And(_)
        | Expr::Or(_)
        | Expr::Eq(_, _)
        | Expr::Neq(_, _)
        | Expr::Gt(_, _)
        | Expr::Gte(_, _)
        | Expr::Lt(_, _)
        | Expr::Lte(_, _)
        | Expr::Contains { .. }
        | Expr::SeqStartsWith { .. }
        | Expr::Quantified { .. } => Some(TypeRef::Bool),
        Expr::Add(_, _) | Expr::Sub(_, _) | Expr::Len(_) => Some(TypeRef::U64),
        Expr::Head(inner) => {
            match infer_expr_type(inner, field_types, helper_returns, binding_types) {
                Some(TypeRef::Seq(inner_ty)) => Some(*inner_ty),
                _ => None,
            }
        }
        Expr::MapKeys(inner) => {
            match infer_expr_type(inner, field_types, helper_returns, binding_types) {
                Some(TypeRef::Map(key_ty, _)) => Some(TypeRef::Set(key_ty)),
                _ => None,
            }
        }
        Expr::MapGet { map, .. } => {
            match infer_expr_type(map, field_types, helper_returns, binding_types) {
                Some(TypeRef::Map(_, value_ty)) => Some(*value_ty),
                _ => None,
            }
        }
        Expr::Some(inner) => infer_expr_type(inner, field_types, helper_returns, binding_types)
            .map(|inner_ty| TypeRef::Option(Box::new(inner_ty))),
        Expr::Call { helper, .. } => helper_returns.get(helper).cloned(),
        Expr::SeqLiteral(items) => items.first().and_then(|item| {
            infer_expr_type(item, field_types, helper_returns, binding_types)
                .map(|inner_ty| TypeRef::Seq(Box::new(inner_ty)))
        }),
        Expr::EmptySet => None,
        Expr::EmptyMap => None,
    }
}

fn default_sample_cardinality(deep: bool) -> usize {
    if deep { 2 } else { 1 }
}

fn render_default_domain_assignment(
    ty: &TypeRef,
    sample_cardinality: usize,
    named_samples: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    match ty {
        TypeRef::Bool => "{TRUE, FALSE}".into(),
        TypeRef::U32 | TypeRef::U64 => {
            if sample_cardinality > 1 {
                "{0, 1, 2}".into()
            } else {
                "{0, 1}".into()
            }
        }
        TypeRef::String => {
            if sample_cardinality > 1 {
                "{\"alpha\", \"beta\"}".into()
            } else {
                "{\"alpha\"}".into()
            }
        }
        TypeRef::Named(name) => {
            render_named_domain_assignment(name, sample_cardinality, named_samples)
        }
        TypeRef::Seq(inner) => {
            let samples = sample_values(inner, sample_cardinality, named_samples);
            if samples.len() >= 2 {
                format!(
                    "{{<<>>, <<{}>>, <<{}, {}>>}}",
                    samples[0], samples[0], samples[1]
                )
            } else if let Some(sample) = samples.first() {
                format!("{{<<>>, <<{}>>}}", sample)
            } else {
                "{<<>>}".into()
            }
        }
        TypeRef::Set(inner) => {
            let samples = sample_values(inner, sample_cardinality, named_samples);
            if samples.len() >= 2 {
                format!(
                    "{{{{}}, {{{}}}, {{{}, {}}}}}",
                    samples[0], samples[0], samples[1]
                )
            } else if let Some(sample) = samples.first() {
                format!("{{{{}}, {{{}}}}}", sample)
            } else {
                "{{{}}}".into()
            }
        }
        TypeRef::Option(inner) => {
            render_default_domain_assignment(inner, sample_cardinality, named_samples)
        }
        TypeRef::Map(_, _) => "{}".into(),
    }
}

fn render_named_domain_assignment(
    name: &str,
    sample_cardinality: usize,
    named_samples: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    if let Some(samples) = named_samples.get(name) {
        let limit = sample_cardinality.max(1);
        let rendered = samples
            .iter()
            .take(limit)
            .map(|sample| tla_string(sample))
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            return format!("{{{}}}", rendered.join(", "));
        }
    }

    let values = (1..=sample_cardinality.max(1))
        .map(|idx| tla_string(&format!("{}_{}", tla_ident(name).to_lowercase(), idx)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{values}}}")
}

fn sample_values(
    ty: &TypeRef,
    sample_cardinality: usize,
    named_samples: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    match ty {
        TypeRef::Bool => vec!["TRUE".into(), "FALSE".into()],
        TypeRef::U32 | TypeRef::U64 => {
            if sample_cardinality > 1 {
                vec!["1".into(), "2".into()]
            } else {
                vec!["1".into()]
            }
        }
        TypeRef::String => {
            if sample_cardinality > 1 {
                vec![tla_string("alpha"), tla_string("beta")]
            } else {
                vec![tla_string("alpha")]
            }
        }
        TypeRef::Named(name) => {
            if let Some(samples) = named_samples.get(name) {
                let limit = sample_cardinality.max(1);
                let rendered = samples
                    .iter()
                    .take(limit)
                    .map(|sample| tla_string(sample))
                    .collect::<Vec<_>>();
                if !rendered.is_empty() {
                    return rendered;
                }
            }
            (1..=sample_cardinality.max(1))
                .map(|idx| tla_string(&format!("{}_{}", tla_ident(name).to_lowercase(), idx)))
                .collect()
        }
        TypeRef::Option(inner) => sample_values(inner, sample_cardinality, named_samples),
        TypeRef::Seq(inner) => sample_values(inner, sample_cardinality, named_samples),
        TypeRef::Set(inner) => sample_values(inner, sample_cardinality, named_samples),
        TypeRef::Map(_, _) => vec![],
    }
}

fn render_sequence_domain_definition(inner: &TypeRef) -> String {
    let inner_domain = match inner {
        TypeRef::Bool => "BOOLEAN".into(),
        TypeRef::U32 | TypeRef::U64 => "NatValues".into(),
        TypeRef::String => "StringValues".into(),
        TypeRef::Named(_) | TypeRef::Set(_) | TypeRef::Seq(_) => domain_constant_name(inner),
        TypeRef::Option(nested) => render_sequence_domain_definition(nested),
        TypeRef::Map(_, _) => "{}".into(),
    };
    format!(
        "{{<<>>}} \\cup {{<<x>> : x \\in {inner_domain}}} \\cup {{<<x, y>> : x \\in {inner_domain}, y \\in {inner_domain}}}"
    )
}

fn domain_constant_name(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named(name) => format!("{}Values", tla_ident(name)),
        TypeRef::Seq(inner) => format!("SeqOf{}Values", tla_ident(&type_ref_name(inner))),
        TypeRef::Set(inner) => format!("SetOf{}Values", tla_ident(&type_ref_name(inner))),
        TypeRef::String => "StringValues".into(),
        TypeRef::Bool => "BooleanValues".into(),
        TypeRef::U32 | TypeRef::U64 => "NatValues".into(),
        TypeRef::Option(inner) => domain_constant_name(inner),
        TypeRef::Map(_, _) => "MapValues".into(),
    }
}

fn type_ref_name(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "Bool".into(),
        TypeRef::U32 => "U32".into(),
        TypeRef::U64 => "U64".into(),
        TypeRef::String => "String".into(),
        TypeRef::Named(name) => name.clone(),
        TypeRef::Option(inner) => format!("Option{}", type_ref_name(inner)),
        TypeRef::Set(inner) => format!("Set{}", type_ref_name(inner)),
        TypeRef::Seq(inner) => format!("Seq{}", type_ref_name(inner)),
        TypeRef::Map(key, value) => format!("Map{}{}", type_ref_name(key), type_ref_name(value)),
    }
}

struct CompositionTlaCompiler<'a> {
    schema: &'a CompositionSchema,
    machine_by_instance: BTreeMap<&'a str, &'a MachineSchema>,
}

impl<'a> CompositionTlaCompiler<'a> {
    fn new(
        schema: &'a CompositionSchema,
        machine_catalog: &'a [MachineSchema],
    ) -> Result<Self, String> {
        let machine_catalog_by_name = machine_catalog
            .iter()
            .map(|machine| (machine.machine.as_str(), machine))
            .collect::<BTreeMap<_, _>>();

        let mut machine_by_instance = BTreeMap::new();
        for instance in &schema.machines {
            let machine = machine_catalog_by_name
                .get(instance.machine_name.as_str())
                .copied()
                .ok_or_else(|| {
                    format!(
                        "unknown machine schema `{}` for composition `{}`",
                        instance.machine_name, schema.name
                    )
                })?;
            machine_by_instance.insert(instance.instance_id.as_str(), machine);
        }

        Ok(Self {
            schema,
            machine_by_instance,
        })
    }

    fn render(&self) -> String {
        let mut out = String::new();
        let constants = self.collect_binding_domains();
        let machine_vars = self.machine_vars();
        let mut machine_invariant_names = Vec::new();

        writeln!(&mut out, "---- MODULE model ----").expect("write to string");
        writeln!(&mut out, "EXTENDS TLC, Naturals, Sequences, FiniteSets")
            .expect("write to string");
        writeln!(&mut out).expect("write to string");
        writeln!(
            &mut out,
            "\\* Generated composition model for {}.",
            self.schema.name
        )
        .expect("write to string");
        writeln!(&mut out).expect("write to string");

        let model_constants = constants
            .iter()
            .filter(|(_, ty)| !matches!(ty, TypeRef::Seq(_)))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        if !model_constants.is_empty() {
            writeln!(&mut out, "CONSTANTS {}", model_constants.join(", "))
                .expect("write to string");
            writeln!(&mut out).expect("write to string");
        }

        for (name, ty) in &constants {
            if let TypeRef::Seq(inner) = ty {
                writeln!(
                    &mut out,
                    "{} == {}",
                    name,
                    render_sequence_domain_definition(inner)
                )
                .expect("write to string");
            }
        }
        if constants.values().any(|ty| matches!(ty, TypeRef::Seq(_))) {
            writeln!(&mut out).expect("write to string");
        }

        writeln!(&mut out, "None == [tag |-> \"none\", value |-> \"none\"]")
            .expect("write to string");
        writeln!(&mut out, "Some(v) == [tag |-> \"some\", value |-> v]").expect("write to string");
        writeln!(
            &mut out,
            "MapLookup(map, key) == IF key \\in DOMAIN map THEN map[key] ELSE None"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "MapSet(map, key, value) == [x \\in DOMAIN map \\cup {{key}} |-> IF x = key THEN value ELSE map[x]]"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "StartsWith(seq, prefix) == /\\ Len(prefix) <= Len(seq) /\\ SubSeq(seq, 1, Len(prefix)) = prefix"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "SeqElements(seq) == {{seq[i] : i \\in 1..Len(seq)}}"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "RECURSIVE SeqRemove(_, _)\nSeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \\o SeqRemove(Tail(seq), value)"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "RECURSIVE SeqRemoveAll(_, _)\nSeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "AppendIfMissing(seq, value) == IF value \\in SeqElements(seq) THEN seq ELSE Append(seq, value)"
        )
        .expect("write to string");
        self.render_static_sets(&mut out);

        writeln!(
            &mut out,
            "VARIABLES {}, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs",
            machine_vars.join(", ")
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "vars == << {}, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>",
            machine_vars.join(", ")
        )
        .expect("write to string");
        writeln!(&mut out).expect("write to string");

        writeln!(
            &mut out,
            "RoutePackets == SeqElements(pending_routes) \\cup delivered_routes"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "PendingActors == {{ActorOfMachine(packet.machine) : packet \\in SeqElements(pending_inputs)}}"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "HigherPriorityReady(actor) == \\E priority \\in ActorPriorities : /\\ priority[2] = actor /\\ priority[1] \\in PendingActors"
        )
        .expect("write to string");
        writeln!(&mut out).expect("write to string");

        writeln!(&mut out, "BaseInit ==").expect("write to string");
        for instance in &self.schema.machines {
            let machine = self.machine(instance.instance_id.as_str());
            writeln!(
                &mut out,
                "    /\\ {} = {}",
                self.phase_var(instance.instance_id.as_str()),
                tla_string(&machine.state.init.phase)
            )
            .expect("write to string");
            for field in &machine.state.fields {
                let init_expr = machine
                    .state
                    .init
                    .fields
                    .iter()
                    .find(|item| item.field == field.name)
                    .map(|item| {
                        let compiler = MachineTlaCompiler::new_with_helper_prefix(
                            machine,
                            format!("{}__", tla_ident(&instance.instance_id)),
                        )
                        .with_phase_symbol(self.phase_var(instance.instance_id.as_str()))
                        .with_field_env_override(
                            self.machine_field_env(instance.instance_id.as_str()),
                        );
                        compiler.render_expr(&item.expr, &BTreeMap::new(), &BTreeMap::new())
                    })
                    .unwrap_or_else(|| default_state_init_expr(&field.ty));
                writeln!(
                    &mut out,
                    "    /\\ {} = {}",
                    self.field_var(instance.instance_id.as_str(), field.name.as_str()),
                    init_expr
                )
                .expect("write to string");
            }
        }
        writeln!(&mut out, "    /\\ model_step_count = 0").expect("write to string");
        writeln!(&mut out, "    /\\ pending_routes = <<>>").expect("write to string");
        writeln!(&mut out, "    /\\ delivered_routes = {{}}").expect("write to string");
        writeln!(&mut out, "    /\\ emitted_effects = {{}}").expect("write to string");
        writeln!(&mut out, "    /\\ observed_transitions = {{}}").expect("write to string");
        writeln!(&mut out).expect("write to string");

        writeln!(&mut out, "Init ==").expect("write to string");
        writeln!(&mut out, "    /\\ BaseInit").expect("write to string");
        writeln!(&mut out, "    /\\ pending_inputs = <<>>").expect("write to string");
        writeln!(&mut out, "    /\\ observed_inputs = {{}}").expect("write to string");
        writeln!(&mut out, "    /\\ witness_current_script_input = None").expect("write to string");
        writeln!(&mut out, "    /\\ witness_remaining_script_inputs = <<>>")
            .expect("write to string");
        writeln!(&mut out).expect("write to string");

        for witness in &self.schema.witnesses {
            writeln!(
                &mut out,
                "{} ==",
                composition_witness_init_name(&witness.name)
            )
            .expect("write to string");
            writeln!(&mut out, "    /\\ BaseInit").expect("write to string");
            writeln!(
                &mut out,
                "    /\\ pending_inputs = {}",
                self.witness_initial_pending_inputs_expr(witness)
            )
            .expect("write to string");
            writeln!(
                &mut out,
                "    /\\ observed_inputs = {}",
                self.witness_initial_observed_inputs_expr(witness)
            )
            .expect("write to string");
            writeln!(
                &mut out,
                "    /\\ witness_current_script_input = {}",
                self.witness_current_script_input_expr(witness)
            )
            .expect("write to string");
            writeln!(
                &mut out,
                "    /\\ witness_remaining_script_inputs = {}",
                self.witness_remaining_script_inputs_expr(witness)
            )
            .expect("write to string");
            writeln!(&mut out).expect("write to string");
        }

        for instance in &self.schema.machines {
            let machine = self.machine(instance.instance_id.as_str());
            let helper_prefix = format!("{}__", tla_ident(&instance.instance_id));
            let mut compiler = MachineTlaCompiler::new_with_helper_prefix(machine, helper_prefix)
                .with_phase_symbol(self.phase_var(instance.instance_id.as_str()))
                .with_field_env_override(self.machine_field_env(instance.instance_id.as_str()));
            for helper in &machine.helpers {
                compiler.render_helper(&mut out, helper);
                writeln!(&mut out).expect("write to string");
            }
            for derived in &machine.derived {
                compiler.render_helper(&mut out, derived);
                writeln!(&mut out).expect("write to string");
            }

            let mut rendered_actions = Vec::new();
            for transition in &machine.transitions {
                let mut action = String::new();
                self.render_machine_transition_action(
                    &mut compiler,
                    instance.instance_id.as_str(),
                    transition,
                    &mut action,
                );
                rendered_actions.push(action);
            }

            for helper in &compiler.helper_defs {
                writeln!(&mut out, "{helper}").expect("write to string");
                writeln!(&mut out).expect("write to string");
            }

            for action in &rendered_actions {
                writeln!(&mut out, "{action}").expect("write to string");
                writeln!(&mut out).expect("write to string");
            }

            let invariant_field_env = self.machine_field_env(instance.instance_id.as_str());
            let invariant_binding_env = BTreeMap::new();
            let invariant_types = BTreeMap::new();
            for invariant in &machine.invariants {
                let invariant_name =
                    self.machine_invariant_name(instance.instance_id.as_str(), &invariant.name);
                machine_invariant_names.push(invariant_name.clone());
                writeln!(
                    &mut out,
                    "{} == {}",
                    invariant_name,
                    compiler.render_expr_with_types(
                        &invariant.expr,
                        &invariant_field_env,
                        &invariant_binding_env,
                        &invariant_types
                    )
                )
                .expect("write to string");
            }
            if !machine.invariants.is_empty() {
                writeln!(&mut out).expect("write to string");
            }
        }

        self.render_entry_input_actions(&mut out);
        self.render_deliver_queued_route_action(&mut out);
        self.render_quiescent_stutter(&mut out);
        self.render_witness_inject_actions(&mut out);

        let core_next_branches = self.core_next_branches();

        writeln!(&mut out, "CoreNext ==").expect("write to string");
        for branch in &core_next_branches {
            writeln!(&mut out, "    \\/ {branch}").expect("write to string");
        }
        writeln!(&mut out).expect("write to string");

        let inject_branches = self
            .schema
            .entry_inputs
            .iter()
            .map(|entry_input| self.render_entry_input_call(entry_input))
            .collect::<Vec<_>>();

        writeln!(&mut out, "InjectNext ==").expect("write to string");
        if inject_branches.is_empty() {
            writeln!(&mut out, "    FALSE").expect("write to string");
        } else {
            for branch in &inject_branches {
                writeln!(&mut out, "    \\/ {branch}").expect("write to string");
            }
        }
        writeln!(&mut out).expect("write to string");

        writeln!(&mut out, "Next ==").expect("write to string");
        writeln!(&mut out, "    \\/ CoreNext").expect("write to string");
        if !inject_branches.is_empty() {
            writeln!(&mut out, "    \\/ InjectNext").expect("write to string");
        }
        writeln!(&mut out).expect("write to string");

        for witness in &self.schema.witnesses {
            writeln!(
                &mut out,
                "{} ==",
                composition_witness_next_name(&witness.name)
            )
            .expect("write to string");
            writeln!(&mut out, "    \\/ CoreNext").expect("write to string");
            writeln!(
                &mut out,
                "    \\/ {}",
                self.render_witness_inject_next_call(witness)
            )
            .expect("write to string");
            writeln!(&mut out).expect("write to string");
        }
        writeln!(&mut out).expect("write to string");

        for invariant in &self.schema.invariants {
            writeln!(
                &mut out,
                "{} == {}",
                invariant.name,
                render_composition_invariant_formula(self.schema, &invariant.kind)
            )
            .expect("write to string");
        }
        if !self.schema.invariants.is_empty() {
            writeln!(&mut out).expect("write to string");
        }

        self.render_coverage_instrumentation(&mut out);

        writeln!(
            &mut out,
            "CiStateConstraint == {}",
            render_composition_state_constraint(
                self.schema,
                &CompositionStateLimits::ci_defaults(),
                self
            )
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "DeepStateConstraint == {}",
            render_composition_state_constraint(
                self.schema,
                &CompositionStateLimits::deep_defaults(),
                self
            )
        )
        .expect("write to string");
        for witness in &self.schema.witnesses {
            writeln!(
                &mut out,
                "{} == {}",
                composition_witness_state_constraint_name(&witness.name),
                render_composition_state_constraint(
                    self.schema,
                    &effective_composition_state_limits(&witness.state_limits, Some(witness)),
                    self
                )
            )
            .expect("write to string");
        }
        writeln!(&mut out).expect("write to string");

        writeln!(&mut out, "Spec == Init /\\ [][Next]_vars").expect("write to string");
        for witness in &self.schema.witnesses {
            let fairness = self
                .witness_fairness_clauses(witness)
                .into_iter()
                .map(|clause| format!("/\\ WF_vars({clause})"))
                .collect::<Vec<_>>()
                .join(" ");
            writeln!(
                &mut out,
                "{} == {} /\\ [] [{}]_vars {}",
                composition_witness_spec_name(&witness.name),
                composition_witness_init_name(&witness.name),
                composition_witness_next_name(&witness.name),
                fairness
            )
            .expect("write to string");
        }
        writeln!(&mut out).expect("write to string");
        for witness in &self.schema.witnesses {
            for route in &witness.expected_routes {
                writeln!(
                    &mut out,
                    "{} == <> {}",
                    composition_witness_route_property_name(&witness.name, route),
                    composition_route_observed_operator_name(route)
                )
                .expect("write to string");
            }
            for rule in &witness.expected_scheduler_rules {
                writeln!(
                    &mut out,
                    "{} == <> {}",
                    composition_witness_scheduler_property_name(&witness.name, rule),
                    composition_scheduler_triggered_operator_name(rule)
                )
                .expect("write to string");
            }
            for (idx, state) in witness.expected_states.iter().enumerate() {
                writeln!(
                    &mut out,
                    "{} == <> ({})",
                    composition_witness_state_property_name(&witness.name, idx),
                    self.render_witness_state_formula(state)
                )
                .expect("write to string");
            }
            for transition in &witness.expected_transitions {
                writeln!(
                    &mut out,
                    "{} == <> ({})",
                    composition_witness_transition_property_name(
                        &witness.name,
                        &transition.machine,
                        &transition.transition,
                    ),
                    self.render_witness_transition_formula(transition)
                )
                .expect("write to string");
            }
            for (idx, ordering) in witness.expected_transition_order.iter().enumerate() {
                writeln!(
                    &mut out,
                    "{} == <> ({})",
                    composition_witness_transition_order_property_name(&witness.name, idx),
                    self.render_witness_transition_order_formula(ordering)
                )
                .expect("write to string");
            }
        }
        if !self.schema.witnesses.is_empty() {
            writeln!(&mut out).expect("write to string");
        }
        for invariant in &self.schema.invariants {
            writeln!(&mut out, "THEOREM Spec => []{}", invariant.name).expect("write to string");
        }
        for invariant_name in &machine_invariant_names {
            writeln!(&mut out, "THEOREM Spec => []{}", invariant_name).expect("write to string");
        }
        writeln!(&mut out).expect("write to string");
        writeln!(
            &mut out,
            "============================================================================="
        )
        .expect("write to string");

        out
    }

    fn collect_binding_domains(&self) -> BTreeMap<String, TypeRef> {
        collect_composition_binding_domains(self.schema, &self.machine_by_instance)
    }

    fn machine(&self, instance_id: &str) -> &MachineSchema {
        self.machine_by_instance
            .get(instance_id)
            .copied()
            .expect("machine instance schema")
    }

    fn actor(&self, instance_id: &str) -> &str {
        self.schema
            .machines
            .iter()
            .find(|machine| machine.instance_id == instance_id)
            .map(|machine| machine.actor.as_str())
            .expect("machine instance actor")
    }

    fn phase_var(&self, instance_id: &str) -> String {
        format!("{}_phase", tla_ident(instance_id))
    }

    fn field_var(&self, instance_id: &str, field: &str) -> String {
        format!("{}_{}", tla_ident(instance_id), tla_ident(field))
    }

    fn machine_invariant_name(&self, instance_id: &str, invariant: &str) -> String {
        format!("{}_{}", tla_ident(instance_id), tla_ident(invariant))
    }

    fn machine_transition_name(&self, instance_id: &str, transition: &TransitionSchema) -> String {
        format!("{}_{}", tla_ident(instance_id), tla_ident(&transition.name))
    }

    fn machine_transition_call(&self, instance_id: &str, transition: &TransitionSchema) -> String {
        if transition.on.bindings.is_empty() {
            self.machine_transition_name(instance_id, transition)
        } else {
            let binding_types = self
                .machine(instance_id)
                .inputs
                .variant_named(&transition.on.variant)
                .expect("transition input variant")
                .fields
                .iter()
                .filter(|field| {
                    transition
                        .on
                        .bindings
                        .iter()
                        .any(|binding| binding == &field.name)
                })
                .map(|field| (field.name.clone(), field.ty.clone()))
                .collect::<BTreeMap<_, _>>();
            let prefix = transition
                .on
                .bindings
                .iter()
                .map(|binding| {
                    let local = format!("arg_{}", tla_ident(binding));
                    let domain = self
                        .binding_domain_for_type(binding_types.get(binding).expect("binding type"));
                    format!("\\E {local} \\in {domain} : ")
                })
                .collect::<String>();
            let args = transition
                .on
                .bindings
                .iter()
                .map(|binding| format!("arg_{}", tla_ident(binding)))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{prefix}{}({args})",
                self.machine_transition_name(instance_id, transition)
            )
        }
    }

    fn binding_domain_for_type(&self, ty: &TypeRef) -> String {
        match ty {
            TypeRef::Bool => "BOOLEAN".into(),
            TypeRef::U32 | TypeRef::U64 => "0..2".into(),
            TypeRef::String => "{\"alpha\", \"beta\"}".into(),
            TypeRef::Named(_) | TypeRef::Seq(_) | TypeRef::Set(_) => domain_constant_name(ty),
            TypeRef::Option(inner) => self.binding_domain_for_type(inner),
            TypeRef::Map(_, _) => "{}".into(),
        }
    }

    fn machine_vars(&self) -> Vec<String> {
        let mut vars = Vec::new();
        for instance in &self.schema.machines {
            vars.push(self.phase_var(instance.instance_id.as_str()));
            let machine = self.machine(instance.instance_id.as_str());
            for field in &machine.state.fields {
                vars.push(self.field_var(instance.instance_id.as_str(), field.name.as_str()));
            }
        }
        vars
    }

    fn machine_field_env(&self, instance_id: &str) -> BTreeMap<String, String> {
        let machine = self.machine(instance_id);
        machine
            .state
            .fields
            .iter()
            .map(|field| {
                (
                    field.name.clone(),
                    self.field_var(instance_id, field.name.as_str()),
                )
            })
            .collect()
    }

    fn render_coverage_instrumentation(&self, out: &mut String) {
        let route_ops = self
            .schema
            .routes
            .iter()
            .map(|route| composition_route_coverage_operator_name(&route.name))
            .collect::<Vec<_>>();
        let scheduler_ops = self
            .schema
            .scheduler_rules
            .iter()
            .map(composition_scheduler_coverage_operator_name)
            .collect::<Vec<_>>();

        for route in &self.schema.routes {
            writeln!(
                out,
                "{} == \\E packet \\in RoutePackets : packet.route = {}",
                composition_route_observed_operator_name(&route.name),
                tla_string(&route.name)
            )
            .expect("write to string");
            writeln!(
                out,
                "{} == ({} \\/ ~{})",
                composition_route_coverage_operator_name(&route.name),
                composition_route_observed_operator_name(&route.name),
                composition_route_observed_operator_name(&route.name)
            )
            .expect("write to string");
        }
        for rule in &self.schema.scheduler_rules {
            writeln!(
                out,
                "{} == {}",
                composition_scheduler_triggered_operator_name(rule),
                render_scheduler_trigger_formula(rule)
            )
            .expect("write to string");
            writeln!(
                out,
                "{} == ({} \\/ ~{})",
                composition_scheduler_coverage_operator_name(rule),
                composition_scheduler_triggered_operator_name(rule),
                composition_scheduler_triggered_operator_name(rule)
            )
            .expect("write to string");
        }

        let mut instrumentation_terms = Vec::new();
        instrumentation_terms.extend(route_ops.into_iter());
        instrumentation_terms.extend(scheduler_ops.into_iter());

        let instrumentation = if instrumentation_terms.is_empty() {
            "TRUE".to_string()
        } else {
            instrumentation_terms.join(" /\\ ")
        };

        writeln!(out, "CoverageInstrumentation == {instrumentation}").expect("write to string");
        writeln!(out).expect("write to string");
    }

    fn render_static_sets(&self, out: &mut String) {
        writeln!(out, "Machines == {{").expect("write to string");
        for (idx, machine) in self.schema.machines.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.machines.len() {
                ""
            } else {
                ","
            };
            writeln!(
                out,
                "    <<{}, {}, {}>>{}",
                tla_string(&machine.instance_id),
                tla_string(&machine.machine_name),
                tla_string(&machine.actor),
                suffix
            )
            .expect("write to string");
        }
        writeln!(out, "}}").expect("write to string");
        writeln!(out).expect("write to string");

        writeln!(out, "RouteNames == {{").expect("write to string");
        for (idx, route) in self.schema.routes.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.routes.len() {
                ""
            } else {
                ","
            };
            writeln!(out, "    {}{}", tla_string(&route.name), suffix).expect("write to string");
        }
        writeln!(out, "}}").expect("write to string");
        writeln!(out).expect("write to string");

        writeln!(out, "Actors == {{").expect("write to string");
        for (idx, machine) in self.schema.machines.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.machines.len() {
                ""
            } else {
                ","
            };
            writeln!(out, "    {}{}", tla_string(&machine.actor), suffix).expect("write to string");
        }
        writeln!(out, "}}").expect("write to string");
        writeln!(out).expect("write to string");

        writeln!(out, "ActorPriorities == {{").expect("write to string");
        for (idx, priority) in self.schema.actor_priorities.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.actor_priorities.len() {
                ""
            } else {
                ","
            };
            writeln!(
                out,
                "    <<{}, {}>>{}",
                tla_string(&priority.higher),
                tla_string(&priority.lower),
                suffix
            )
            .expect("write to string");
        }
        writeln!(out, "}}").expect("write to string");
        writeln!(out).expect("write to string");

        writeln!(out, "SchedulerRules == {{").expect("write to string");
        for (idx, rule) in self.schema.scheduler_rules.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.scheduler_rules.len() {
                ""
            } else {
                ","
            };
            writeln!(out, "    {}{}", render_scheduler_rule_tuple(rule), suffix)
                .expect("write to string");
        }
        writeln!(out, "}}").expect("write to string");
        writeln!(out).expect("write to string");

        render_composition_case_fn(
            out,
            "ActorOfMachine",
            "machine_id",
            self.schema
                .machines
                .iter()
                .map(|machine| (machine.instance_id.as_str(), tla_string(&machine.actor)))
                .collect(),
            tla_string("unknown_actor"),
        );
        render_composition_case_fn(
            out,
            "RouteSource",
            "route_name",
            self.schema
                .routes
                .iter()
                .map(|route| (route.name.as_str(), tla_string(&route.from_machine)))
                .collect(),
            tla_string("unknown_machine"),
        );
        render_composition_case_fn(
            out,
            "RouteEffect",
            "route_name",
            self.schema
                .routes
                .iter()
                .map(|route| (route.name.as_str(), tla_string(&route.effect_variant)))
                .collect(),
            tla_string("unknown_effect"),
        );
        render_composition_case_fn(
            out,
            "RouteTargetMachine",
            "route_name",
            self.schema
                .routes
                .iter()
                .map(|route| (route.name.as_str(), tla_string(&route.to.machine)))
                .collect(),
            tla_string("unknown_machine"),
        );
        render_composition_case_fn(
            out,
            "RouteTargetInput",
            "route_name",
            self.schema
                .routes
                .iter()
                .map(|route| (route.name.as_str(), tla_string(&route.to.input_variant)))
                .collect(),
            tla_string("unknown_input"),
        );
        render_composition_case_fn(
            out,
            "RouteDeliveryKind",
            "route_name",
            self.schema
                .routes
                .iter()
                .map(|route| {
                    (
                        route.name.as_str(),
                        tla_string(match route.delivery {
                            RouteDelivery::Immediate => "Immediate",
                            RouteDelivery::Enqueue => "Enqueue",
                        }),
                    )
                })
                .collect(),
            tla_string("Unknown"),
        );
        writeln!(
            out,
            "RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))"
        )
        .expect("write to string");
        writeln!(out).expect("write to string");
    }

    fn render_entry_input_actions(&self, out: &mut String) {
        for entry_input in &self.schema.entry_inputs {
            let machine = self.machine(entry_input.machine.as_str());
            let variant = machine
                .inputs
                .variant_named(&entry_input.input_variant)
                .expect("entry input variant");
            let action_name = self.entry_input_action_name(entry_input);
            let params = variant
                .fields
                .iter()
                .map(|field| format!("arg_{}", tla_ident(&field.name)))
                .collect::<Vec<_>>();
            let packet_expr = self.input_packet_expr(
                entry_input.machine.as_str(),
                &entry_input.input_variant,
                &variant
                    .fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.clone(),
                            format!("arg_{}", tla_ident(&field.name)),
                        )
                    })
                    .collect::<BTreeMap<_, _>>(),
                "entry",
                &entry_input.name,
                "external_entry",
                &entry_input.input_variant,
                "0",
            );

            if params.is_empty() {
                writeln!(out, "{} ==", action_name).expect("write to string");
            } else {
                writeln!(out, "{}({}) ==", action_name, params.join(", "))
                    .expect("write to string");
            }
            writeln!(
                out,
                "    /\\ ~({} \\in SeqElements(pending_inputs))",
                packet_expr
            )
            .expect("write to string");
            writeln!(
                out,
                "    /\\ pending_inputs' = Append(pending_inputs, {})",
                packet_expr
            )
            .expect("write to string");
            writeln!(
                out,
                "    /\\ observed_inputs' = observed_inputs \\cup {{{}}}",
                packet_expr
            )
            .expect("write to string");
            writeln!(out, "    /\\ model_step_count' = model_step_count + 1")
                .expect("write to string");
            writeln!(
                out,
                "    /\\ UNCHANGED << {}, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>",
                self.machine_vars().join(", ")
            )
            .expect("write to string");
            writeln!(out).expect("write to string");
        }
    }

    fn render_entry_input_call(&self, entry_input: &EntryInput) -> String {
        let machine = self.machine(entry_input.machine.as_str());
        let variant = machine
            .inputs
            .variant_named(&entry_input.input_variant)
            .expect("entry input variant");
        let action_name = self.entry_input_action_name(entry_input);
        if variant.fields.is_empty() {
            action_name
        } else {
            let prefix = variant
                .fields
                .iter()
                .map(|field| {
                    let local = format!("arg_{}", tla_ident(&field.name));
                    let domain = self.binding_domain_for_type(&field.ty);
                    format!("\\E {local} \\in {domain} : ")
                })
                .collect::<String>();
            let args = variant
                .fields
                .iter()
                .map(|field| format!("arg_{}", tla_ident(&field.name)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{prefix}{action_name}({args})")
        }
    }

    fn render_deliver_queued_route_action(&self, out: &mut String) {
        writeln!(out, "DeliverQueuedRoute ==").expect("write to string");
        writeln!(out, "    /\\ Len(pending_routes) > 0").expect("write to string");
        writeln!(out, "    /\\ LET route == Head(pending_routes) IN").expect("write to string");
        writeln!(out, "       /\\ pending_routes' = Tail(pending_routes)")
            .expect("write to string");
        writeln!(
            out,
            "       /\\ delivered_routes' = delivered_routes \\cup {{route}}"
        )
        .expect("write to string");
        writeln!(out, "       /\\ model_step_count' = model_step_count + 1")
            .expect("write to string");
        writeln!(
            out,
            "       /\\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> \"route\", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])"
        )
        .expect("write to string");
        writeln!(
            out,
            "       /\\ observed_inputs' = observed_inputs \\cup {{[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> \"route\", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}}"
        )
        .expect("write to string");
        writeln!(
            out,
            "       /\\ UNCHANGED << {}, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>",
            self.machine_vars().join(", ")
        )
        .expect("write to string");
        writeln!(out).expect("write to string");
    }

    fn render_quiescent_stutter(&self, out: &mut String) {
        writeln!(out, "QuiescentStutter ==").expect("write to string");
        writeln!(out, "    /\\ Len(pending_routes) = 0").expect("write to string");
        writeln!(out, "    /\\ Len(pending_inputs) = 0").expect("write to string");
        writeln!(out, "    /\\ UNCHANGED vars").expect("write to string");
        writeln!(out).expect("write to string");
    }

    fn render_witness_inject_next_call(&self, witness: &CompositionWitness) -> String {
        format!("WitnessInjectNext_{}", tla_ident(&witness.name))
    }

    fn core_next_branches(&self) -> Vec<String> {
        let mut branches = Vec::new();
        if !self.schema.routes.is_empty() {
            branches.push("DeliverQueuedRoute".into());
        }
        for instance in &self.schema.machines {
            let machine = self.machine(instance.instance_id.as_str());
            for transition in &machine.transitions {
                branches.push(self.machine_transition_call(instance.instance_id.as_str(), transition));
            }
        }
        branches.push("QuiescentStutter".into());
        branches
    }

    fn witness_fairness_clauses(&self, witness: &CompositionWitness) -> Vec<String> {
        let mut clauses = Vec::new();
        if !self.schema.routes.is_empty() {
            clauses.push("DeliverQueuedRoute".into());
        }
        clauses.extend(
            self.schema
                .machines
                .iter()
                .flat_map(|instance| {
                    let machine = self.machine(instance.instance_id.as_str());
                    machine
                        .transitions
                        .iter()
                        .map(|transition| {
                            self.machine_transition_call(instance.instance_id.as_str(), transition)
                        })
                        .collect::<Vec<_>>()
                }),
        );
        if witness.preload_inputs.len() > 1 {
            clauses.push(self.render_witness_inject_next_call(witness));
        }
        clauses
    }

    fn entry_input_action_name(&self, entry_input: &EntryInput) -> String {
        format!("Inject_{}", tla_ident(&entry_input.name))
    }

    fn input_packet_expr(
        &self,
        machine_id: &str,
        input_variant: &str,
        payload_fields: &BTreeMap<String, String>,
        source_kind: &str,
        source_route: &str,
        source_machine: &str,
        source_effect: &str,
        effect_id: &str,
    ) -> String {
        format!(
            "[machine |-> {}, variant |-> {}, payload |-> {}, source_kind |-> {}, source_route |-> {}, source_machine |-> {}, source_effect |-> {}, effect_id |-> {}]",
            tla_string(machine_id),
            tla_string(input_variant),
            self.payload_record_expr(payload_fields),
            tla_string(source_kind),
            tla_string(source_route),
            tla_string(source_machine),
            tla_string(source_effect),
            effect_id
        )
    }

    fn input_packet_expr_from_payload_expr(
        &self,
        machine_id: &str,
        input_variant: &str,
        payload_expr: &str,
        source_kind: &str,
        source_route: &str,
        source_machine: &str,
        source_effect: &str,
        effect_id: &str,
    ) -> String {
        format!(
            "[machine |-> {}, variant |-> {}, payload |-> {}, source_kind |-> {}, source_route |-> {}, source_machine |-> {}, source_effect |-> {}, effect_id |-> {}]",
            tla_string(machine_id),
            tla_string(input_variant),
            payload_expr,
            tla_string(source_kind),
            tla_string(source_route),
            tla_string(source_machine),
            tla_string(source_effect),
            effect_id
        )
    }

    fn payload_record_expr(&self, payload_fields: &BTreeMap<String, String>) -> String {
        if payload_fields.is_empty() {
            "[tag |-> \"unit\"]".into()
        } else {
            format!(
                "[{}]",
                payload_fields
                    .iter()
                    .map(|(field, expr)| format!("{field} |-> {expr}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn witness_packet_exprs(&self, witness: &CompositionWitness) -> Vec<String> {
        witness
            .preload_inputs
            .iter()
            .enumerate()
            .map(|(idx, packet)| {
                let payload_fields = packet
                    .fields
                    .iter()
                    .map(|field| (field.field.clone(), self.render_literal_expr(&field.expr)))
                    .collect::<BTreeMap<_, _>>();
                self.input_packet_expr(
                    packet.machine.as_str(),
                    packet.input_variant.as_str(),
                    &payload_fields,
                    "entry",
                    &format!("witness:{}:{}", witness.name, idx + 1),
                    "external_entry",
                    &packet.input_variant,
                    "0",
                )
            })
            .collect()
    }

    fn witness_initial_batch_size(&self, witness: &CompositionWitness) -> usize {
        if witness.expected_scheduler_rules.is_empty() {
            1
        } else {
            witness.preload_inputs.len().min(2).max(1)
        }
    }

    fn witness_initial_observed_inputs_expr(&self, witness: &CompositionWitness) -> String {
        let packets = self.witness_packet_exprs(witness);
        let batch_size = self.witness_initial_batch_size(witness);
        if packets.is_empty() {
            "{}".into()
        } else {
            format!(
                "{{{}}}",
                packets
                    .into_iter()
                    .take(batch_size)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn witness_initial_pending_inputs_expr(&self, witness: &CompositionWitness) -> String {
        let packets = self.witness_packet_exprs(witness);
        let batch_size = self.witness_initial_batch_size(witness);
        if packets.is_empty() {
            "<<>>".into()
        } else {
            format!(
                "<<{}>>",
                packets
                    .into_iter()
                    .take(batch_size)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }

    fn witness_current_script_input_expr(&self, witness: &CompositionWitness) -> String {
        let packets = self.witness_packet_exprs(witness);
        let batch_size = self.witness_initial_batch_size(witness);
        packets
            .into_iter()
            .take(batch_size)
            .last()
            .unwrap_or_else(|| "None".into())
    }

    fn witness_remaining_script_inputs_expr(&self, witness: &CompositionWitness) -> String {
        let packets = self.witness_packet_exprs(witness);
        let batch_size = self.witness_initial_batch_size(witness);
        if packets.len() <= batch_size {
            return "<<>>".into();
        }

        format!(
            "<<{}>>",
            packets
                .into_iter()
                .skip(batch_size)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_witness_inject_actions(&self, out: &mut String) {
        for witness in &self.schema.witnesses {
            let action_name = self.render_witness_inject_next_call(witness);
            writeln!(out, "{action_name} ==").expect("write to string");
            if witness.preload_inputs.len() <= 1 {
                writeln!(out, "    FALSE").expect("write to string");
                writeln!(out).expect("write to string");
                continue;
            }

            writeln!(out, "    /\\ witness_current_script_input # None").expect("write to string");
            writeln!(
                out,
                "    /\\ ~(witness_current_script_input \\in SeqElements(pending_inputs))"
            )
            .expect("write to string");
            writeln!(out, "    /\\ Len(pending_inputs) = 0").expect("write to string");
            writeln!(out, "    /\\ Len(pending_routes) = 0").expect("write to string");
            writeln!(out, "    /\\ Len(witness_remaining_script_inputs) > 0")
                .expect("write to string");
            writeln!(
                out,
                "    /\\ pending_inputs' = Append(pending_inputs, Head(witness_remaining_script_inputs))"
            )
            .expect("write to string");
            writeln!(
                out,
                "    /\\ observed_inputs' = observed_inputs \\cup {{Head(witness_remaining_script_inputs)}}"
            )
            .expect("write to string");
            writeln!(
                out,
                "    /\\ witness_current_script_input' = Head(witness_remaining_script_inputs)"
            )
            .expect("write to string");
            writeln!(
                out,
                "    /\\ witness_remaining_script_inputs' = Tail(witness_remaining_script_inputs)"
            )
            .expect("write to string");
            writeln!(out, "    /\\ model_step_count' = model_step_count + 1")
                .expect("write to string");
            writeln!(
                out,
                "    /\\ UNCHANGED << {}, pending_routes, delivered_routes, emitted_effects, observed_transitions >>",
                self.machine_vars().join(", ")
            )
            .expect("write to string");
            writeln!(out).expect("write to string");
        }
    }

    fn render_literal_expr(&self, expr: &Expr) -> String {
        match expr {
            Expr::Bool(value) => {
                if *value {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
            Expr::U64(value) => value.to_string(),
            Expr::String(value) => tla_string(value),
            Expr::None => "None".into(),
            Expr::SeqLiteral(items) => {
                let rendered = items
                    .iter()
                    .map(|item| self.render_literal_expr(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("<<{rendered}>>")
            }
            other => panic!("unsupported witness literal expr: {other:?}"),
        }
    }

    fn render_witness_state_formula(
        &self,
        state: &meerkat_machine_schema::CompositionWitnessState,
    ) -> String {
        let mut clauses = Vec::new();
        if let Some(phase) = &state.phase {
            clauses.push(format!(
                "{} = {}",
                self.phase_var(&state.machine),
                tla_string(phase)
            ));
        }
        for field in &state.fields {
            clauses.push(format!(
                "{} = {}",
                self.field_var(&state.machine, &field.field),
                self.render_literal_expr(&field.expr)
            ));
        }
        if clauses.is_empty() {
            "TRUE".into()
        } else {
            clauses.join(" /\\ ")
        }
    }

    fn render_witness_transition_formula(
        &self,
        transition: &meerkat_machine_schema::CompositionWitnessTransition,
    ) -> String {
        format!(
            "\\E packet \\in observed_transitions : /\\ packet.machine = {} /\\ packet.transition = {}",
            tla_string(&transition.machine),
            tla_string(&transition.transition)
        )
    }

    fn render_witness_transition_order_formula(
        &self,
        ordering: &meerkat_machine_schema::CompositionWitnessTransitionOrder,
    ) -> String {
        format!(
            "\\E earlier \\in observed_transitions, later \\in observed_transitions : /\\ earlier.machine = {} /\\ earlier.transition = {} /\\ later.machine = {} /\\ later.transition = {} /\\ earlier.step < later.step",
            tla_string(&ordering.earlier.machine),
            tla_string(&ordering.earlier.transition),
            tla_string(&ordering.later.machine),
            tla_string(&ordering.later.transition),
        )
    }

    fn route_packet_expr(
        &self,
        route_name: &str,
        from_machine: &str,
        effect_variant: &str,
        to_machine: &str,
        to_input_variant: &str,
        payload_expr: &str,
        effect_id_expr: &str,
        source_transition: &str,
    ) -> String {
        format!(
            "[route |-> {}, source_machine |-> {}, effect |-> {}, target_machine |-> {}, target_input |-> {}, payload |-> {}, actor |-> {}, effect_id |-> {}, source_transition |-> {}]",
            tla_string(route_name),
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(to_machine),
            tla_string(to_input_variant),
            payload_expr,
            tla_string(self.actor(to_machine)),
            effect_id_expr,
            tla_string(source_transition)
        )
    }

    fn render_machine_transition_action(
        &self,
        compiler: &mut MachineTlaCompiler<'_>,
        instance_id: &str,
        transition: &TransitionSchema,
        out: &mut String,
    ) {
        let machine = self.machine(instance_id);
        let action_name = self.machine_transition_name(instance_id, transition);
        let binding_types = compiler.binding_type_map(transition);
        let binding_env = transition
            .on
            .bindings
            .iter()
            .map(|binding| {
                (
                    binding.clone(),
                    format!("packet.payload.{}", tla_ident(binding)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let params = transition
            .on
            .bindings
            .iter()
            .map(|binding| format!("arg_{}", tla_ident(binding)))
            .collect::<Vec<_>>();

        if params.is_empty() {
            writeln!(out, "{} ==", action_name).expect("write to string");
        } else {
            writeln!(out, "{}({}) ==", action_name, params.join(", ")).expect("write to string");
        }

        let phase_var = self.phase_var(instance_id);
        let from_guard = if transition.from.len() == 1 {
            format!("{} = {}", phase_var, tla_string(&transition.from[0]))
        } else {
            transition
                .from
                .iter()
                .map(|phase| format!("{} = {}", phase_var, tla_string(phase)))
                .collect::<Vec<_>>()
                .join(" \\/ ")
        };

        writeln!(out, "    /\\ \\E packet \\in SeqElements(pending_inputs) :")
            .expect("write to string");
        writeln!(
            out,
            "       /\\ packet.machine = {}",
            tla_string(instance_id)
        )
        .expect("write to string");
        writeln!(
            out,
            "       /\\ packet.variant = {}",
            tla_string(&transition.on.variant)
        )
        .expect("write to string");
        for binding in &transition.on.bindings {
            let arg_name = format!("arg_{}", tla_ident(binding));
            writeln!(
                out,
                "       /\\ packet.payload.{} = {}",
                tla_ident(binding),
                arg_name
            )
            .expect("write to string");
        }
        writeln!(
            out,
            "       /\\ ~HigherPriorityReady({})",
            tla_string(self.actor(instance_id))
        )
        .expect("write to string");
        writeln!(out, "       /\\ {}", from_guard).expect("write to string");

        let field_env = self.machine_field_env(instance_id);
        for guard in &transition.guards {
            writeln!(
                out,
                "       /\\ {}",
                compiler.render_guard(guard, &field_env, &binding_env, &binding_types)
            )
            .expect("write to string");
        }

        let next_env = compiler.compile_updates(
            &action_name,
            &field_env,
            &binding_env,
            &binding_types,
            &transition.updates,
        );
        let effect_id_expr = "(model_step_count + 1)".to_string();
        let effect_packets = transition
            .emit
            .iter()
            .map(|effect| {
                let payload_fields = effect
                    .fields
                    .iter()
                    .map(|(field, expr)| {
                        (
                            field.clone(),
                            compiler.render_expr_with_types(
                                expr,
                                &next_env,
                                &binding_env,
                                &binding_types,
                            ),
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                (
                    effect.variant.clone(),
                    format!(
                        "[machine |-> {}, variant |-> {}, payload |-> {}, effect_id |-> {}, source_transition |-> {}]",
                        tla_string(instance_id),
                        tla_string(&effect.variant),
                        self.payload_record_expr(&payload_fields),
                        &effect_id_expr,
                        tla_string(&transition.name)
                    ),
                    payload_fields,
                )
            })
            .collect::<Vec<_>>();

        let mut queued_route_packets = Vec::new();
        let mut immediate_route_packets = Vec::new();
        for (effect_variant, _effect_packet, payload_fields) in &effect_packets {
            for route in self.schema.routes.iter().filter(|route| {
                route.from_machine == instance_id && route.effect_variant == *effect_variant
            }) {
                let target_payload = self.target_payload_for_route(
                    machine,
                    effect_variant,
                    payload_fields,
                    route,
                    compiler,
                    &next_env,
                    &binding_env,
                    &binding_types,
                );
                let route_packet = self.route_packet_expr(
                    &route.name,
                    instance_id,
                    effect_variant,
                    &route.to.machine,
                    &route.to.input_variant,
                    &target_payload,
                    &effect_id_expr,
                    &transition.name,
                );
                match route.delivery {
                    RouteDelivery::Enqueue => {
                        queued_route_packets.push((route_packet, target_payload))
                    }
                    RouteDelivery::Immediate => immediate_route_packets.push((
                        route_packet,
                        self.input_packet_expr_from_payload_expr(
                            &route.to.machine,
                            &route.to.input_variant,
                            &target_payload,
                            "route",
                            &route.name,
                            instance_id,
                            effect_variant,
                            &effect_id_expr,
                        ),
                    )),
                }
            }
        }

        writeln!(
            out,
            "       /\\ {}' = {}",
            phase_var,
            tla_string(&transition.to)
        )
        .expect("write to string");
        let touched = machine
            .state
            .fields
            .iter()
            .filter_map(|field| {
                let next = next_env.get(&field.name)?;
                if next != field_env.get(&field.name).unwrap() {
                    Some((field.name.as_str(), next.as_str()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for (field, expr) in &touched {
            writeln!(
                out,
                "       /\\ {}' = {}",
                self.field_var(instance_id, field),
                expr
            )
            .expect("write to string");
        }

        let mut unchanged_machine_vars = Vec::new();
        for instance in &self.schema.machines {
            if instance.instance_id == instance_id {
                for field in &machine.state.fields {
                    if !touched.iter().any(|(name, _)| *name == field.name) {
                        unchanged_machine_vars
                            .push(self.field_var(instance_id, field.name.as_str()));
                    }
                }
            } else {
                unchanged_machine_vars.push(self.phase_var(instance.instance_id.as_str()));
                let other_machine = self.machine(instance.instance_id.as_str());
                for field in &other_machine.state.fields {
                    unchanged_machine_vars
                        .push(self.field_var(instance.instance_id.as_str(), field.name.as_str()));
                }
            }
        }

        if !unchanged_machine_vars.is_empty() {
            unchanged_machine_vars.push("witness_current_script_input".into());
            unchanged_machine_vars.push("witness_remaining_script_inputs".into());
            writeln!(
                out,
                "       /\\ UNCHANGED << {} >>",
                unchanged_machine_vars.join(", ")
            )
            .expect("write to string");
        }

        let pending_inputs_next = self.append_if_missing_chain(
            format!("SeqRemove(pending_inputs, packet)"),
            &immediate_route_packets
                .iter()
                .map(|(_route_packet, input_packet)| input_packet.clone())
                .collect::<Vec<_>>(),
        );
        let pending_routes_next = self.append_if_missing_chain(
            "pending_routes".into(),
            &queued_route_packets
                .iter()
                .map(|(route_packet, _target_payload)| route_packet.clone())
                .collect::<Vec<_>>(),
        );
        let delivered_routes_next = if immediate_route_packets.is_empty() {
            "delivered_routes".into()
        } else {
            format!(
                "delivered_routes \\cup {{ {} }}",
                immediate_route_packets
                    .iter()
                    .map(|(route_packet, _)| route_packet.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let observed_inputs_next = {
            let immediate_inputs = immediate_route_packets
                .iter()
                .map(|(_route_packet, input_packet)| input_packet.clone())
                .collect::<Vec<_>>();
            self.union_set_items("observed_inputs".into(), &immediate_inputs)
        };
        let emitted_effects_next = if effect_packets.is_empty() {
            "emitted_effects".into()
        } else {
            format!(
                "emitted_effects \\cup {{ {} }}",
                effect_packets
                    .iter()
                    .map(|(_variant, effect_packet, _)| effect_packet.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let observed_transitions_next = format!(
            "observed_transitions \\cup {{{}}}",
            self.transition_packet_expr(instance_id, transition)
        );

        writeln!(out, "       /\\ pending_inputs' = {}", pending_inputs_next)
            .expect("write to string");
        writeln!(
            out,
            "       /\\ observed_inputs' = {}",
            observed_inputs_next
        )
        .expect("write to string");
        writeln!(out, "       /\\ pending_routes' = {}", pending_routes_next)
            .expect("write to string");
        writeln!(
            out,
            "       /\\ delivered_routes' = {}",
            delivered_routes_next
        )
        .expect("write to string");
        writeln!(
            out,
            "       /\\ emitted_effects' = {}",
            emitted_effects_next
        )
        .expect("write to string");
        writeln!(
            out,
            "       /\\ observed_transitions' = {}",
            observed_transitions_next
        )
        .expect("write to string");
        writeln!(out, "       /\\ model_step_count' = model_step_count + 1")
            .expect("write to string");
    }

    fn target_payload_for_route(
        &self,
        source_machine: &MachineSchema,
        effect_variant: &str,
        effect_payload_fields: &BTreeMap<String, String>,
        route: &Route,
        compiler: &MachineTlaCompiler<'_>,
        next_env: &BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> String {
        let target_machine = self.machine(route.to.machine.as_str());
        let target_variant = target_machine
            .inputs
            .variant_named(&route.to.input_variant)
            .expect("target input variant");
        let source_effect_variant = source_machine
            .effects
            .variant_named(effect_variant)
            .expect("source effect variant");
        let bindings = route
            .bindings
            .iter()
            .map(|binding| {
                let expr = match &binding.source {
                    RouteBindingSource::Field {
                        from_field,
                        allow_named_alias: _,
                    } => effect_payload_fields
                        .get(from_field)
                        .cloned()
                        .or_else(|| {
                            source_effect_variant
                                .fields
                                .iter()
                                .find(|field| field.name == *from_field)
                                .map(|_| format!("packet.payload.{}", tla_ident(from_field)))
                        })
                        .expect("effect field binding"),
                    RouteBindingSource::Literal(expr) => {
                        compiler.render_expr_with_types(expr, next_env, binding_env, binding_types)
                    }
                };
                (binding.to_field.clone(), expr)
            })
            .collect::<BTreeMap<_, _>>();
        let payload_fields = target_variant
            .fields
            .iter()
            .map(|field| {
                let expr = bindings
                    .get(&field.name)
                    .cloned()
                    .unwrap_or_else(|| default_state_init_expr(&field.ty));
                (field.name.clone(), expr)
            })
            .collect::<BTreeMap<_, _>>();
        self.payload_record_expr(&payload_fields)
    }

    fn append_if_missing_chain(&self, base: String, items: &[String]) -> String {
        items.iter().fold(base, |expr, item| {
            format!("AppendIfMissing({expr}, {item})")
        })
    }

    fn union_set_items(&self, base: String, items: &[String]) -> String {
        if items.is_empty() {
            base
        } else {
            format!("{base} \\cup {{{}}}", items.join(", "))
        }
    }

    fn transition_packet_expr(&self, instance_id: &str, transition: &TransitionSchema) -> String {
        format!(
            "[machine |-> {}, transition |-> {}, actor |-> {}, step |-> (model_step_count + 1), from_phase |-> {}, to_phase |-> {}]",
            tla_string(instance_id),
            tla_string(&transition.name),
            tla_string(self.actor(instance_id)),
            self.phase_var(instance_id),
            tla_string(&transition.to)
        )
    }
}

struct MachineTlaCompiler<'a> {
    schema: &'a MachineSchema,
    helper_defs: Vec<String>,
    helper_counter: usize,
    helper_prefix: Option<String>,
    phase_symbol: Option<String>,
    field_env_override: Option<BTreeMap<String, String>>,
}

impl<'a> MachineTlaCompiler<'a> {
    fn new(schema: &'a MachineSchema) -> Self {
        Self {
            schema,
            helper_defs: Vec::new(),
            helper_counter: 0,
            helper_prefix: None,
            phase_symbol: None,
            field_env_override: None,
        }
    }

    fn new_with_helper_prefix(schema: &'a MachineSchema, helper_prefix: impl Into<String>) -> Self {
        Self {
            schema,
            helper_defs: Vec::new(),
            helper_counter: 0,
            helper_prefix: Some(helper_prefix.into()),
            phase_symbol: None,
            field_env_override: None,
        }
    }

    fn with_phase_symbol(mut self, phase_symbol: impl Into<String>) -> Self {
        self.phase_symbol = Some(phase_symbol.into());
        self
    }

    fn with_field_env_override(mut self, field_env_override: BTreeMap<String, String>) -> Self {
        self.field_env_override = Some(field_env_override);
        self
    }

    fn render(&mut self) -> String {
        let mut out = String::new();
        let constants = collect_binding_domains(self.schema);

        writeln!(&mut out, "---- MODULE model ----").expect("write to string");
        writeln!(&mut out, "EXTENDS TLC, Naturals, Sequences, FiniteSets")
            .expect("write to string");
        writeln!(&mut out).expect("write to string");
        writeln!(
            &mut out,
            "\\* Generated semantic machine model for {}.",
            self.schema.machine
        )
        .expect("write to string");
        writeln!(&mut out).expect("write to string");

        let model_constants = constants
            .iter()
            .filter(|(_, ty)| !matches!(ty, TypeRef::Seq(_)))
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        if !model_constants.is_empty() {
            let constant_list = model_constants.join(", ");
            writeln!(&mut out, "CONSTANTS {constant_list}").expect("write to string");
            writeln!(&mut out).expect("write to string");
        }

        for (name, ty) in &constants {
            if let TypeRef::Seq(inner) = ty {
                writeln!(
                    &mut out,
                    "{} == {}",
                    name,
                    render_sequence_domain_definition(inner)
                )
                .expect("write to string");
            }
        }
        if constants.values().any(|ty| matches!(ty, TypeRef::Seq(_))) {
            writeln!(&mut out).expect("write to string");
        }

        writeln!(&mut out, "None == [tag |-> \"none\", value |-> \"none\"]")
            .expect("write to string");
        writeln!(&mut out, "Some(v) == [tag |-> \"some\", value |-> v]").expect("write to string");
        writeln!(
            &mut out,
            "MapLookup(map, key) == IF key \\in DOMAIN map THEN map[key] ELSE None"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "MapSet(map, key, value) == [x \\in DOMAIN map \\cup {{key}} |-> IF x = key THEN value ELSE map[x]]"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "StartsWith(seq, prefix) == /\\ Len(prefix) <= Len(seq) /\\ SubSeq(seq, 1, Len(prefix)) = prefix"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "SeqElements(seq) == {{seq[i] : i \\in 1..Len(seq)}}"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "RECURSIVE SeqRemove(_, _)\nSeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \\o SeqRemove(Tail(seq), value)"
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "RECURSIVE SeqRemoveAll(_, _)\nSeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))"
        )
        .expect("write to string");
        writeln!(&mut out).expect("write to string");

        let variable_list = std::iter::once("phase".to_string())
            .chain(std::iter::once("model_step_count".to_string()))
            .chain(
                self.schema
                    .state
                    .fields
                    .iter()
                    .map(|field| field.name.clone()),
            )
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(&mut out, "VARIABLES {variable_list}").expect("write to string");
        writeln!(&mut out).expect("write to string");
        let vars = std::iter::once("phase".to_string())
            .chain(std::iter::once("model_step_count".to_string()))
            .chain(
                self.schema
                    .state
                    .fields
                    .iter()
                    .map(|field| field.name.clone()),
            )
            .collect::<Vec<_>>();
        writeln!(&mut out, "vars == << {} >>", vars.join(", ")).expect("write to string");
        writeln!(&mut out).expect("write to string");

        for helper in &self.schema.helpers {
            self.render_helper(&mut out, helper);
        }
        for derived in &self.schema.derived {
            self.render_helper(&mut out, derived);
        }
        if !self.schema.helpers.is_empty() || !self.schema.derived.is_empty() {
            writeln!(&mut out).expect("write to string");
        }

        writeln!(&mut out, "Init ==").expect("write to string");
        writeln!(
            &mut out,
            "    /\\ phase = {}",
            tla_string(&self.schema.state.init.phase)
        )
        .expect("write to string");
        writeln!(&mut out, "    /\\ model_step_count = 0").expect("write to string");
        for field in &self.schema.state.fields {
            let init_expr = self
                .schema
                .state
                .init
                .fields
                .iter()
                .find(|item| item.field == field.name)
                .map(|item| self.render_expr(&item.expr, &BTreeMap::new(), &BTreeMap::new()))
                .unwrap_or_else(|| default_state_init_expr(&field.ty));
            writeln!(&mut out, "    /\\ {} = {}", field.name, init_expr).expect("write to string");
        }
        writeln!(&mut out).expect("write to string");

        let mut rendered_transitions = Vec::new();
        for transition in &self.schema.transitions {
            let mut rendered = String::new();
            self.render_transition(&mut rendered, transition);
            rendered_transitions.push(rendered);
        }

        if !self.schema.state.terminal_phases.is_empty() {
            let terminal_guard = if self.schema.state.terminal_phases.len() == 1 {
                format!(
                    "phase = {}",
                    tla_string(&self.schema.state.terminal_phases[0])
                )
            } else {
                self.schema
                    .state
                    .terminal_phases
                    .iter()
                    .map(|phase| format!("phase = {}", tla_string(phase)))
                    .collect::<Vec<_>>()
                    .join(" \\/ ")
            };
            writeln!(&mut out, "TerminalStutter ==").expect("write to string");
            writeln!(&mut out, "    /\\ {}", terminal_guard).expect("write to string");
            writeln!(&mut out, "    /\\ UNCHANGED vars").expect("write to string");
            writeln!(&mut out).expect("write to string");
        }

        if !self.helper_defs.is_empty() {
            for helper in &self.helper_defs {
                writeln!(&mut out, "{helper}").expect("write to string");
                writeln!(&mut out).expect("write to string");
            }
        }

        for transition in &rendered_transitions {
            writeln!(&mut out, "{transition}").expect("write to string");
            writeln!(&mut out).expect("write to string");
        }

        writeln!(&mut out, "Next ==").expect("write to string");
        for transition in &self.schema.transitions {
            let call = if transition.on.bindings.is_empty() {
                transition.name.clone()
            } else {
                let binding_types = self.binding_type_map(transition);
                let binding_env = transition
                    .on
                    .bindings
                    .iter()
                    .map(|binding| (binding.clone(), self.local_binding_name(binding)))
                    .collect::<BTreeMap<_, _>>();
                let mut prefix = String::new();
                for binding in &transition.on.bindings {
                    let ty = binding_types.get(binding).expect("binding type");
                    let domain = self.binding_domain_for_type(ty);
                    let local = binding_env
                        .get(binding)
                        .cloned()
                        .unwrap_or_else(|| binding.clone());
                    prefix.push_str(&format!("\\E {local} \\in {domain} : "));
                }
                let args = transition
                    .on
                    .bindings
                    .iter()
                    .map(|binding| {
                        binding_env
                            .get(binding)
                            .cloned()
                            .unwrap_or_else(|| binding.clone())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{prefix}{}({})", transition.name, args)
            };
            writeln!(&mut out, "    \\/ {}", call).expect("write to string");
        }
        if !self.schema.state.terminal_phases.is_empty() {
            writeln!(&mut out, "    \\/ TerminalStutter").expect("write to string");
        }
        writeln!(&mut out).expect("write to string");

        for invariant in &self.schema.invariants {
            writeln!(
                &mut out,
                "{} == {}",
                invariant.name,
                self.render_expr(&invariant.expr, &BTreeMap::new(), &BTreeMap::new())
            )
            .expect("write to string");
        }
        writeln!(&mut out).expect("write to string");

        writeln!(
            &mut out,
            "CiStateConstraint == {}",
            render_machine_state_constraint(self.schema, false)
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "DeepStateConstraint == {}",
            render_machine_state_constraint(self.schema, true)
        )
        .expect("write to string");
        writeln!(&mut out).expect("write to string");

        writeln!(&mut out, "Spec == Init /\\ [][Next]_vars").expect("write to string");
        writeln!(&mut out).expect("write to string");
        for invariant in &self.schema.invariants {
            writeln!(&mut out, "THEOREM Spec => []{}", invariant.name).expect("write to string");
        }
        writeln!(&mut out).expect("write to string");
        writeln!(
            &mut out,
            "============================================================================="
        )
        .expect("write to string");

        out
    }

    fn render_helper(&self, out: &mut String, helper: &HelperSchema) {
        let params = helper
            .params
            .iter()
            .map(|field| self.local_binding_name(&field.name))
            .collect::<Vec<_>>()
            .join(", ");
        let binding_env = helper
            .params
            .iter()
            .map(|field| (field.name.clone(), self.local_binding_name(&field.name)))
            .collect::<BTreeMap<_, _>>();
        let field_env = self.field_env_override.clone().unwrap_or_else(|| {
            self.schema
                .state
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.name.clone()))
                .collect::<BTreeMap<_, _>>()
        });
        let binding_types = helper
            .params
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();
        if helper.params.is_empty() {
            writeln!(
                out,
                "{} == {}",
                self.scoped_helper_name(&helper.name),
                self.render_expr_with_types(&helper.body, &field_env, &binding_env, &binding_types)
            )
            .expect("write to string");
        } else {
            writeln!(
                out,
                "{}({}) == {}",
                self.scoped_helper_name(&helper.name),
                params,
                self.render_expr_with_types(&helper.body, &field_env, &binding_env, &binding_types)
            )
            .expect("write to string");
        }
    }

    fn render_transition(&mut self, out: &mut String, transition: &TransitionSchema) {
        let binding_types = self.binding_type_map(transition);
        let binding_env = transition
            .on
            .bindings
            .iter()
            .map(|binding| (binding.clone(), self.local_binding_name(binding)))
            .collect::<BTreeMap<_, _>>();
        let params = transition
            .on
            .bindings
            .iter()
            .map(|binding| {
                binding_env
                    .get(binding)
                    .cloned()
                    .unwrap_or_else(|| binding.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");
        if params.is_empty() {
            writeln!(out, "{} ==", transition.name).expect("write to string");
        } else {
            writeln!(out, "{}({}) ==", transition.name, params).expect("write to string");
        }

        let from_guard = if transition.from.len() == 1 {
            format!("phase = {}", tla_string(&transition.from[0]))
        } else {
            transition
                .from
                .iter()
                .map(|phase| format!("phase = {}", tla_string(phase)))
                .collect::<Vec<_>>()
                .join(" \\/ ")
        };
        writeln!(out, "    /\\ {}", from_guard).expect("write to string");

        let mut env = self
            .schema
            .state
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.name.clone()))
            .collect::<BTreeMap<_, _>>();

        for guard in &transition.guards {
            writeln!(
                out,
                "    /\\ {}",
                self.render_guard(guard, &env, &binding_env, &binding_types)
            )
            .expect("write to string");
        }

        env = self.compile_updates(
            &transition.name,
            &env,
            &binding_env,
            &binding_types,
            &transition.updates,
        );

        writeln!(out, "    /\\ phase' = {}", tla_string(&transition.to)).expect("write to string");
        writeln!(out, "    /\\ model_step_count' = model_step_count + 1").expect("write to string");
        let touched = self
            .schema
            .state
            .fields
            .iter()
            .filter_map(|field| {
                let next = env.get(&field.name)?;
                if next != &field.name {
                    Some((field.name.as_str(), next.as_str()))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        for (field, expr) in &touched {
            writeln!(out, "    /\\ {}' = {}", field, expr).expect("write to string");
        }

        let unchanged = self
            .schema
            .state
            .fields
            .iter()
            .filter(|field| !touched.iter().any(|(name, _)| *name == field.name))
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        if !unchanged.is_empty() {
            writeln!(out, "    /\\ UNCHANGED << {} >>", unchanged.join(", "))
                .expect("write to string");
        }
    }

    fn render_guard(
        &self,
        guard: &Guard,
        env: &BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> String {
        self.render_expr_with_types(&guard.expr, env, binding_env, binding_types)
    }

    fn binding_type_map(&self, transition: &TransitionSchema) -> BTreeMap<String, TypeRef> {
        let mut map = BTreeMap::new();
        if let Some(variant) = self
            .schema
            .inputs
            .variants
            .iter()
            .find(|item| item.name == transition.on.variant)
        {
            for binding in &transition.on.bindings {
                if let Some(field) = variant.fields.iter().find(|field| field.name == *binding) {
                    map.insert(binding.clone(), field.ty.clone());
                }
            }
        }
        map
    }

    fn local_binding_name(&self, name: &str) -> String {
        let candidate = tla_ident(name);
        if candidate == "phase"
            || self
                .schema
                .state
                .fields
                .iter()
                .any(|field| field.name == name)
        {
            format!("arg_{candidate}")
        } else {
            candidate
        }
    }

    fn binding_domain_for_type(&self, ty: &TypeRef) -> String {
        match ty {
            TypeRef::Bool => "BOOLEAN".into(),
            TypeRef::U32 | TypeRef::U64 => "0..2".into(),
            TypeRef::String => "{\"alpha\", \"beta\"}".into(),
            TypeRef::Named(_) | TypeRef::Seq(_) | TypeRef::Set(_) => domain_constant_name(ty),
            TypeRef::Option(inner) => self.binding_domain_for_type(inner),
            TypeRef::Map(_, _) => "{}".into(),
        }
    }

    fn compile_updates(
        &mut self,
        transition_name: &str,
        env: &BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
        updates: &[Update],
    ) -> BTreeMap<String, String> {
        let mut current = env.clone();
        for update in updates {
            self.apply_update(
                transition_name,
                &mut current,
                binding_env,
                binding_types,
                update,
            );
        }
        current
    }

    fn apply_update(
        &mut self,
        transition_name: &str,
        env: &mut BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
        update: &Update,
    ) {
        match update {
            Update::Assign { field, expr } => {
                let value = self.render_expr_with_types(expr, env, binding_env, binding_types);
                env.insert(field.clone(), value);
            }
            Update::Increment { field, amount } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                env.insert(field.clone(), format!("({current}) + {amount}"));
            }
            Update::Decrement { field, amount } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                env.insert(field.clone(), format!("({current}) - {amount}"));
            }
            Update::MapInsert { field, key, value } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                let key_expr = self.render_expr_with_types(key, env, binding_env, binding_types);
                let value_expr =
                    self.render_expr_with_types(value, env, binding_env, binding_types);
                env.insert(
                    field.clone(),
                    format!("MapSet({current}, {key_expr}, {value_expr})"),
                );
            }
            Update::SetInsert { field, value } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                let value_expr =
                    self.render_expr_with_types(value, env, binding_env, binding_types);
                env.insert(field.clone(), format!("({current} \\cup {{{value_expr}}})"));
            }
            Update::SetRemove { field, value } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                let value_expr =
                    self.render_expr_with_types(value, env, binding_env, binding_types);
                env.insert(field.clone(), format!("({current} \\ {{{value_expr}}})"));
            }
            Update::SeqAppend { field, value } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                let value_expr =
                    self.render_expr_with_types(value, env, binding_env, binding_types);
                env.insert(field.clone(), format!("Append({current}, {value_expr})"));
            }
            Update::SeqPrepend { field, values } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                let value_expr =
                    self.render_expr_with_types(values, env, binding_env, binding_types);
                env.insert(field.clone(), format!("({value_expr} \\o {current})"));
            }
            Update::SeqPopFront { field } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                env.insert(field.clone(), format!("Tail({current})"));
            }
            Update::SeqRemoveValue { field, value } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                let value_expr =
                    self.render_expr_with_types(value, env, binding_env, binding_types);
                env.insert(field.clone(), format!("SeqRemove({current}, {value_expr})"));
            }
            Update::SeqRemoveAll { field, values } => {
                let current = env.get(field).cloned().unwrap_or_else(|| field.clone());
                let value_expr =
                    self.render_expr_with_types(values, env, binding_env, binding_types);
                env.insert(
                    field.clone(),
                    format!("SeqRemoveAll({current}, {value_expr})"),
                );
            }
            Update::Conditional {
                condition,
                then_updates,
                else_updates,
            } => {
                let base = env.clone();
                let mut then_env = base.clone();
                for update in then_updates {
                    self.apply_update(
                        transition_name,
                        &mut then_env,
                        binding_env,
                        binding_types,
                        update,
                    );
                }
                let mut else_env = base.clone();
                for update in else_updates {
                    self.apply_update(
                        transition_name,
                        &mut else_env,
                        binding_env,
                        binding_types,
                        update,
                    );
                }
                let condition_expr =
                    self.render_expr_with_types(condition, &base, binding_env, binding_types);
                let touched = union_touched_fields(&base, &then_env, &else_env);
                for field in touched {
                    let then_expr = then_env
                        .get(&field)
                        .cloned()
                        .unwrap_or_else(|| field.clone());
                    let else_expr = else_env
                        .get(&field)
                        .cloned()
                        .unwrap_or_else(|| field.clone());
                    env.insert(
                        field.clone(),
                        format!("IF {condition_expr} THEN {then_expr} ELSE {else_expr}"),
                    );
                }
            }
            Update::ForEach {
                binding,
                over,
                updates,
            } => {
                let helper_index = self.helper_counter;
                self.helper_counter += 1;
                let over_expr = self.render_expr_with_types(over, env, binding_env, binding_types);
                let touched = collect_update_fields(updates);
                let referenced_bindings = collect_update_bindings(updates)
                    .into_iter()
                    .filter(|item| item != binding)
                    .collect::<BTreeSet<_>>();
                let referenced_fields = collect_update_fields_exprs(updates)
                    .into_iter()
                    .filter(|field| {
                        touched.contains(field) || env.get(field.as_str()) != Some(field)
                    })
                    .collect::<BTreeSet<_>>();

                for field in touched {
                    let current = env.get(&field).cloned().unwrap_or_else(|| field.clone());
                    let helper_name = format!(
                        "{}_ForEach{}_{}",
                        tla_ident(transition_name),
                        helper_index,
                        tla_ident(&field)
                    );
                    let collection_kind = infer_collection_kind(over, self.schema, binding_types);

                    let mut param_defs = Vec::new();
                    let mut call_args = vec![current.clone(), over_expr.clone()];
                    let mut helper_binding_env = binding_env.clone();
                    helper_binding_env.insert(binding.clone(), binding.clone());
                    let mut helper_field_env = self
                        .schema
                        .state
                        .fields
                        .iter()
                        .map(|item| (item.name.clone(), item.name.clone()))
                        .collect::<BTreeMap<_, _>>();
                    helper_field_env.insert(field.clone(), "acc".into());

                    for outer_binding in &referenced_bindings {
                        let param = format!("outer_{}", tla_ident(outer_binding));
                        param_defs.push(param.clone());
                        call_args.push(
                            binding_env
                                .get(outer_binding)
                                .cloned()
                                .unwrap_or_else(|| outer_binding.clone()),
                        );
                        helper_binding_env.insert(outer_binding.clone(), param);
                    }

                    for ref_field in &referenced_fields {
                        if *ref_field == field {
                            continue;
                        }
                        let current_expr = env
                            .get(ref_field)
                            .cloned()
                            .unwrap_or_else(|| ref_field.clone());
                        if current_expr != *ref_field {
                            let param = format!("captured_{}", tla_ident(ref_field));
                            param_defs.push(param.clone());
                            call_args.push(current_expr);
                            helper_field_env.insert(ref_field.clone(), param);
                        }
                    }

                    let body_expr = self.compile_for_each_field_body(
                        &field,
                        &helper_field_env,
                        &helper_binding_env,
                        binding_types,
                        updates,
                    );

                    let helper_def = match collection_kind {
                        CollectionKind::Sequence => render_sequence_foreach_helper(
                            &helper_name,
                            binding,
                            &param_defs,
                            &body_expr,
                        ),
                        CollectionKind::Set => render_set_foreach_helper(
                            &helper_name,
                            binding,
                            &param_defs,
                            &body_expr,
                        ),
                    };
                    self.helper_defs.push(helper_def);
                    env.insert(
                        field.clone(),
                        format!("{}({})", helper_name, call_args.join(", ")),
                    );
                }
            }
        }
    }

    fn compile_for_each_field_body(
        &mut self,
        target_field: &str,
        env: &BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
        updates: &[Update],
    ) -> String {
        let mut current = env.clone();
        for update in updates {
            self.apply_update_for_target(
                target_field,
                &mut current,
                binding_env,
                binding_types,
                update,
            );
        }
        current
            .get(target_field)
            .cloned()
            .unwrap_or_else(|| target_field.to_owned())
    }

    fn apply_update_for_target(
        &mut self,
        target_field: &str,
        env: &mut BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
        update: &Update,
    ) {
        match update {
            Update::Conditional {
                condition,
                then_updates,
                else_updates,
            } => {
                let base = env.clone();
                let mut then_env = base.clone();
                for update in then_updates {
                    self.apply_update_for_target(
                        target_field,
                        &mut then_env,
                        binding_env,
                        binding_types,
                        update,
                    );
                }
                let mut else_env = base.clone();
                for update in else_updates {
                    self.apply_update_for_target(
                        target_field,
                        &mut else_env,
                        binding_env,
                        binding_types,
                        update,
                    );
                }
                let base_expr = base
                    .get(target_field)
                    .cloned()
                    .unwrap_or_else(|| target_field.to_owned());
                let then_expr = then_env
                    .get(target_field)
                    .cloned()
                    .unwrap_or_else(|| base_expr.clone());
                let else_expr = else_env
                    .get(target_field)
                    .cloned()
                    .unwrap_or_else(|| base_expr.clone());
                if then_expr != base_expr || else_expr != base_expr {
                    let condition_expr =
                        self.render_expr_with_types(condition, &base, binding_env, binding_types);
                    env.insert(
                        target_field.to_owned(),
                        format!("IF {condition_expr} THEN {then_expr} ELSE {else_expr}"),
                    );
                }
            }
            Update::ForEach {
                binding,
                over,
                updates,
            } => {
                let helper_index = self.helper_counter;
                self.helper_counter += 1;
                let over_expr = self.render_expr_with_types(over, env, binding_env, binding_types);
                let helper_name = format!(
                    "{}_NestedForEach{}_{}",
                    tla_ident(target_field),
                    helper_index,
                    tla_ident(binding)
                );
                let collection_kind = infer_collection_kind(over, self.schema, binding_types);
                let mut nested_binding_env = binding_env.clone();
                nested_binding_env.insert(binding.clone(), binding.clone());
                let mut nested_env = env.clone();
                nested_env.insert(target_field.to_owned(), "acc".into());
                let body_expr = self.compile_for_each_field_body(
                    target_field,
                    &nested_env,
                    &nested_binding_env,
                    binding_types,
                    updates,
                );
                let helper_def = match collection_kind {
                    CollectionKind::Sequence => {
                        render_sequence_foreach_helper(&helper_name, binding, &[], &body_expr)
                    }
                    CollectionKind::Set => {
                        render_set_foreach_helper(&helper_name, binding, &[], &body_expr)
                    }
                };
                self.helper_defs.push(helper_def);
                let current = env
                    .get(target_field)
                    .cloned()
                    .unwrap_or_else(|| target_field.to_owned());
                env.insert(
                    target_field.to_owned(),
                    format!("{}({}, {})", helper_name, current, over_expr),
                );
            }
            _ => {
                let before = env
                    .get(target_field)
                    .cloned()
                    .unwrap_or_else(|| target_field.to_owned());
                self.apply_update(
                    &format!("Target{}", target_field),
                    env,
                    binding_env,
                    binding_types,
                    update,
                );
                let after = env
                    .get(target_field)
                    .cloned()
                    .unwrap_or_else(|| target_field.to_owned());
                if after == before {
                    env.insert(target_field.to_owned(), before);
                }
            }
        }
    }

    fn render_expr(
        &self,
        expr: &Expr,
        env: &BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
    ) -> String {
        self.render_expr_with_types(expr, env, binding_env, &BTreeMap::new())
    }

    fn scoped_helper_name(&self, helper: &str) -> String {
        match &self.helper_prefix {
            Some(prefix) => format!("{prefix}{}", tla_ident(helper)),
            None => helper.to_owned(),
        }
    }

    fn render_expr_with_types(
        &self,
        expr: &Expr,
        env: &BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> String {
        match expr {
            Expr::Bool(value) => {
                if *value {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                }
            }
            Expr::U64(value) => value.to_string(),
            Expr::String(value) => tla_string(value),
            Expr::EmptySet => "{}".into(),
            Expr::EmptyMap => "[x \\in {} |-> None]".into(),
            Expr::SeqLiteral(items) => format!(
                "<<{}>>",
                items
                    .iter()
                    .map(|item| self.render_expr_with_types(item, env, binding_env, binding_types))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Expr::CurrentPhase => self.phase_symbol.clone().unwrap_or_else(|| "phase".into()),
            Expr::Phase(value) => tla_string(value),
            Expr::Field(name) => env.get(name).cloned().unwrap_or_else(|| name.clone()),
            Expr::Binding(name) => binding_env
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
            Expr::Variant(name) => tla_string(name),
            Expr::None => "None".into(),
            Expr::IfElse {
                condition,
                then_expr,
                else_expr,
            } => format!(
                "(IF {} THEN {} ELSE {})",
                self.render_expr_with_types(condition, env, binding_env, binding_types),
                self.render_expr_with_types(then_expr, env, binding_env, binding_types),
                self.render_expr_with_types(else_expr, env, binding_env, binding_types)
            ),
            Expr::Not(inner) => format!(
                "~({})",
                self.render_expr_with_types(inner, env, binding_env, binding_types)
            ),
            Expr::And(items) => join_exprs(
                &items
                    .iter()
                    .map(|item| self.render_expr_with_types(item, env, binding_env, binding_types))
                    .collect::<Vec<_>>(),
                " /\\ ",
            ),
            Expr::Or(items) => join_exprs(
                &items
                    .iter()
                    .map(|item| self.render_expr_with_types(item, env, binding_env, binding_types))
                    .collect::<Vec<_>>(),
                " \\/ ",
            ),
            Expr::Eq(left, right) => format!(
                "({} = {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Neq(left, right) => format!(
                "({} # {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Add(left, right) => format!(
                "({} + {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Sub(left, right) => format!(
                "({} - {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Gt(left, right) => format!(
                "({} > {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Gte(left, right) => format!(
                "({} >= {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Lt(left, right) => format!(
                "({} < {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Lte(left, right) => format!(
                "({} <= {})",
                self.render_expr_with_types(left, env, binding_env, binding_types),
                self.render_expr_with_types(right, env, binding_env, binding_types)
            ),
            Expr::Contains { collection, value } => format!(
                "({} \\in {})",
                self.render_expr_with_types(value, env, binding_env, binding_types),
                self.render_collection_domain(collection, env, binding_env, binding_types)
            ),
            Expr::SeqStartsWith { seq, prefix } => format!(
                "StartsWith({}, {})",
                self.render_expr_with_types(seq, env, binding_env, binding_types),
                self.render_expr_with_types(prefix, env, binding_env, binding_types)
            ),
            Expr::Len(inner) => format!(
                "Len({})",
                self.render_expr_with_types(inner, env, binding_env, binding_types)
            ),
            Expr::Head(inner) => format!(
                "Head({})",
                self.render_expr_with_types(inner, env, binding_env, binding_types)
            ),
            Expr::MapKeys(inner) => format!(
                "DOMAIN {}",
                self.render_expr_with_types(inner, env, binding_env, binding_types)
            ),
            Expr::MapGet { map, key } => format!(
                "(IF {} \\in DOMAIN {} THEN {}[{}] ELSE {})",
                self.render_expr_with_types(key, env, binding_env, binding_types),
                self.render_expr_with_types(map, env, binding_env, binding_types),
                self.render_expr_with_types(map, env, binding_env, binding_types),
                self.render_expr_with_types(key, env, binding_env, binding_types),
                self.map_absent_value_expr(map, binding_types)
            ),
            Expr::Some(inner) => format!(
                "Some({})",
                self.render_expr_with_types(inner, env, binding_env, binding_types)
            ),
            Expr::Call { helper, args } => format!(
                "{}",
                if args.is_empty() {
                    self.scoped_helper_name(helper)
                } else {
                    format!(
                        "{}({})",
                        self.scoped_helper_name(helper),
                        args.iter()
                            .map(|arg| self.render_expr_with_types(
                                arg,
                                env,
                                binding_env,
                                binding_types
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ),
            Expr::Quantified {
                quantifier,
                binding,
                over,
                body,
            } => {
                let keyword = match quantifier {
                    Quantifier::Any => "\\E",
                    Quantifier::All => "\\A",
                };
                let mut nested_bindings = binding_env.clone();
                nested_bindings.insert(binding.clone(), binding.clone());
                let mut nested_binding_types = binding_types.clone();
                if let Some(item_ty) = self.collection_item_type(over, binding_types) {
                    nested_binding_types.insert(binding.clone(), item_ty);
                }
                format!(
                    "{} {} \\in {} : {}",
                    keyword,
                    binding,
                    self.render_collection_domain(over, env, binding_env, binding_types),
                    self.render_expr_with_types(body, env, &nested_bindings, &nested_binding_types)
                )
            }
        }
    }

    fn render_collection_domain(
        &self,
        over: &Expr,
        env: &BTreeMap<String, String>,
        binding_env: &BTreeMap<String, String>,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> String {
        let rendered = self.render_expr_with_types(over, env, binding_env, binding_types);
        match self.collection_item_type(over, binding_types) {
            Some(_) if self.is_sequence_expr(over, binding_types) => {
                format!("SeqElements({rendered})")
            }
            _ => rendered,
        }
    }

    fn collection_item_type(
        &self,
        expr: &Expr,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> Option<TypeRef> {
        match expr {
            Expr::Field(name) => self
                .schema
                .state
                .fields
                .iter()
                .find(|field| field.name == *name)
                .and_then(|field| match &field.ty {
                    TypeRef::Seq(inner) | TypeRef::Set(inner) => Some((**inner).clone()),
                    _ => None,
                }),
            Expr::Binding(name) => binding_types.get(name).and_then(|ty| match ty {
                TypeRef::Seq(inner) | TypeRef::Set(inner) => Some((**inner).clone()),
                _ => None,
            }),
            Expr::SeqLiteral(items) => {
                if let Some(first) = items.first() {
                    self.scalar_expr_type(first, binding_types)
                } else {
                    Some(TypeRef::String)
                }
            }
            Expr::MapKeys(_) | Expr::EmptySet => Some(TypeRef::String),
            _ => None,
        }
    }

    fn scalar_expr_type(
        &self,
        expr: &Expr,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> Option<TypeRef> {
        match expr {
            Expr::Bool(_) => Some(TypeRef::Bool),
            Expr::U64(_) => Some(TypeRef::U64),
            Expr::String(_) | Expr::Phase(_) | Expr::Variant(_) => Some(TypeRef::String),
            Expr::Field(name) => self
                .schema
                .state
                .fields
                .iter()
                .find(|field| field.name == *name)
                .map(|field| field.ty.clone()),
            Expr::Binding(name) => binding_types.get(name).cloned(),
            _ => None,
        }
    }

    fn map_value_type(
        &self,
        expr: &Expr,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> Option<TypeRef> {
        match expr {
            Expr::Field(name) => self
                .schema
                .state
                .fields
                .iter()
                .find(|field| field.name == *name)
                .and_then(|field| match &field.ty {
                    TypeRef::Map(_, value) => Some((**value).clone()),
                    _ => None,
                }),
            Expr::Binding(name) => binding_types.get(name).and_then(|ty| match ty {
                TypeRef::Map(_, value) => Some((**value).clone()),
                _ => None,
            }),
            _ => None,
        }
    }

    fn map_absent_value_expr(
        &self,
        expr: &Expr,
        binding_types: &BTreeMap<String, TypeRef>,
    ) -> String {
        self.map_value_type(expr, binding_types)
            .map(|ty| default_absent_map_value_expr(&ty))
            .unwrap_or_else(|| "None".into())
    }

    fn is_sequence_expr(&self, expr: &Expr, binding_types: &BTreeMap<String, TypeRef>) -> bool {
        match expr {
            Expr::Field(name) => self
                .schema
                .state
                .fields
                .iter()
                .find(|field| field.name == *name)
                .is_some_and(|field| matches!(field.ty, TypeRef::Seq(_))),
            Expr::Binding(name) => binding_types
                .get(name)
                .is_some_and(|ty| matches!(ty, TypeRef::Seq(_))),
            Expr::SeqLiteral(_) => true,
            _ => false,
        }
    }
}

#[derive(Copy, Clone)]
enum CollectionKind {
    Sequence,
    Set,
}

fn infer_collection_kind(
    expr: &Expr,
    schema: &MachineSchema,
    binding_types: &BTreeMap<String, TypeRef>,
) -> CollectionKind {
    match expr {
        Expr::Field(name) => schema
            .state
            .fields
            .iter()
            .find(|field| field.name == *name)
            .map(|field| match field.ty {
                TypeRef::Set(_) => CollectionKind::Set,
                _ => CollectionKind::Sequence,
            })
            .unwrap_or(CollectionKind::Sequence),
        Expr::Binding(name) => binding_types
            .get(name)
            .map(|ty| match ty {
                TypeRef::Set(_) => CollectionKind::Set,
                _ => CollectionKind::Sequence,
            })
            .unwrap_or(CollectionKind::Sequence),
        Expr::MapKeys(_) | Expr::EmptySet => CollectionKind::Set,
        _ => CollectionKind::Sequence,
    }
}

fn render_sequence_foreach_helper(
    name: &str,
    binding: &str,
    extra_params: &[String],
    body_expr: &str,
) -> String {
    let mut params = vec!["acc".to_owned(), "items".to_owned()];
    params.extend(extra_params.iter().cloned());
    let params_str = params.join(", ");
    let recurse_args = std::iter::once("next_acc".to_owned())
        .chain(std::iter::once("Tail(items)".to_owned()))
        .chain(extra_params.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let recursive_args = std::iter::repeat_n("_", params.len())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "RECURSIVE {name}({recursive_args})\n{name}({params_str}) == IF Len(items) = 0 THEN acc ELSE LET {binding} == Head(items) IN LET next_acc == {body_expr} IN {name}({recurse_args})"
    )
}

fn render_set_foreach_helper(
    name: &str,
    binding: &str,
    extra_params: &[String],
    body_expr: &str,
) -> String {
    let mut params = vec!["acc".to_owned(), "remaining".to_owned()];
    params.extend(extra_params.iter().cloned());
    let params_str = params.join(", ");
    let recurse_args = std::iter::once("next_acc".to_owned())
        .chain(std::iter::once("remaining \\ {item}".to_owned()))
        .chain(extra_params.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let recursive_args = std::iter::repeat_n("_", params.len())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "RECURSIVE {name}({recursive_args})\n{name}({params_str}) == IF remaining = {{}} THEN acc ELSE LET item == CHOOSE x \\in remaining : TRUE IN LET {binding} == item IN LET next_acc == {body_expr} IN {name}({recurse_args})"
    )
}

fn collect_update_fields(updates: &[Update]) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    for update in updates {
        match update {
            Update::Assign { field, .. }
            | Update::Increment { field, .. }
            | Update::Decrement { field, .. }
            | Update::MapInsert { field, .. }
            | Update::SetInsert { field, .. }
            | Update::SetRemove { field, .. }
            | Update::SeqAppend { field, .. }
            | Update::SeqPrepend { field, .. }
            | Update::SeqPopFront { field }
            | Update::SeqRemoveValue { field, .. }
            | Update::SeqRemoveAll { field, .. } => {
                fields.insert(field.clone());
            }
            Update::Conditional {
                then_updates,
                else_updates,
                ..
            } => {
                fields.extend(collect_update_fields(then_updates));
                fields.extend(collect_update_fields(else_updates));
            }
            Update::ForEach { updates, .. } => {
                fields.extend(collect_update_fields(updates));
            }
        }
    }
    fields
}

fn collect_update_bindings(updates: &[Update]) -> BTreeSet<String> {
    let mut bindings = BTreeSet::new();
    for update in updates {
        match update {
            Update::Assign { expr, .. } => collect_expr_bindings(expr, &mut bindings),
            Update::MapInsert { key, value, .. } => {
                collect_expr_bindings(key, &mut bindings);
                collect_expr_bindings(value, &mut bindings);
            }
            Update::SetInsert { value, .. }
            | Update::SetRemove { value, .. }
            | Update::SeqAppend { value, .. }
            | Update::SeqRemoveValue { value, .. }
            | Update::SeqPrepend { values: value, .. }
            | Update::SeqRemoveAll { values: value, .. } => {
                collect_expr_bindings(value, &mut bindings);
            }
            Update::Conditional {
                condition,
                then_updates,
                else_updates,
            } => {
                collect_expr_bindings(condition, &mut bindings);
                bindings.extend(collect_update_bindings(then_updates));
                bindings.extend(collect_update_bindings(else_updates));
            }
            Update::ForEach {
                binding,
                over,
                updates,
            } => {
                bindings.insert(binding.clone());
                collect_expr_bindings(over, &mut bindings);
                bindings.extend(collect_update_bindings(updates));
            }
            Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => {}
        }
    }
    bindings
}

fn collect_update_fields_exprs(updates: &[Update]) -> BTreeSet<String> {
    let mut fields = BTreeSet::new();
    for update in updates {
        match update {
            Update::Assign { expr, .. } => collect_expr_fields(expr, &mut fields),
            Update::MapInsert { key, value, .. } => {
                collect_expr_fields(key, &mut fields);
                collect_expr_fields(value, &mut fields);
            }
            Update::SetInsert { value, .. }
            | Update::SetRemove { value, .. }
            | Update::SeqAppend { value, .. }
            | Update::SeqRemoveValue { value, .. }
            | Update::SeqPrepend { values: value, .. }
            | Update::SeqRemoveAll { values: value, .. } => {
                collect_expr_fields(value, &mut fields);
            }
            Update::Conditional {
                condition,
                then_updates,
                else_updates,
            } => {
                collect_expr_fields(condition, &mut fields);
                fields.extend(collect_update_fields_exprs(then_updates));
                fields.extend(collect_update_fields_exprs(else_updates));
            }
            Update::ForEach { over, updates, .. } => {
                collect_expr_fields(over, &mut fields);
                fields.extend(collect_update_fields_exprs(updates));
            }
            Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => {}
        }
    }
    fields
}

fn collect_expr_bindings(expr: &Expr, bindings: &mut BTreeSet<String>) {
    match expr {
        Expr::Binding(name) => {
            bindings.insert(name.clone());
        }
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => {
            for item in items {
                collect_expr_bindings(item, bindings);
            }
        }
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_expr_bindings(condition, bindings);
            collect_expr_bindings(then_expr, bindings);
            collect_expr_bindings(else_expr, bindings);
        }
        Expr::Not(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::Some(inner) => collect_expr_bindings(inner, bindings),
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            collect_expr_bindings(left, bindings);
            collect_expr_bindings(right, bindings);
        }
        Expr::Contains { collection, value } => {
            collect_expr_bindings(collection, bindings);
            collect_expr_bindings(value, bindings);
        }
        Expr::SeqStartsWith { seq, prefix } => {
            collect_expr_bindings(seq, bindings);
            collect_expr_bindings(prefix, bindings);
        }
        Expr::MapGet { map, key } => {
            collect_expr_bindings(map, bindings);
            collect_expr_bindings(key, bindings);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_bindings(arg, bindings);
            }
        }
        Expr::Quantified {
            binding,
            over,
            body,
            ..
        } => {
            bindings.insert(binding.clone());
            collect_expr_bindings(over, bindings);
            collect_expr_bindings(body, bindings);
        }
        Expr::Bool(_)
        | Expr::U64(_)
        | Expr::String(_)
        | Expr::EmptySet
        | Expr::EmptyMap
        | Expr::CurrentPhase
        | Expr::Phase(_)
        | Expr::Field(_)
        | Expr::Variant(_)
        | Expr::None => {}
    }
}

fn collect_expr_fields(expr: &Expr, fields: &mut BTreeSet<String>) {
    match expr {
        Expr::Field(name) => {
            fields.insert(name.clone());
        }
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => {
            for item in items {
                collect_expr_fields(item, fields);
            }
        }
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_expr_fields(condition, fields);
            collect_expr_fields(then_expr, fields);
            collect_expr_fields(else_expr, fields);
        }
        Expr::Not(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::Some(inner) => collect_expr_fields(inner, fields),
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            collect_expr_fields(left, fields);
            collect_expr_fields(right, fields);
        }
        Expr::Contains { collection, value } => {
            collect_expr_fields(collection, fields);
            collect_expr_fields(value, fields);
        }
        Expr::SeqStartsWith { seq, prefix } => {
            collect_expr_fields(seq, fields);
            collect_expr_fields(prefix, fields);
        }
        Expr::MapGet { map, key } => {
            collect_expr_fields(map, fields);
            collect_expr_fields(key, fields);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_fields(arg, fields);
            }
        }
        Expr::Quantified { over, body, .. } => {
            collect_expr_fields(over, fields);
            collect_expr_fields(body, fields);
        }
        Expr::Bool(_)
        | Expr::U64(_)
        | Expr::String(_)
        | Expr::EmptySet
        | Expr::EmptyMap
        | Expr::CurrentPhase
        | Expr::Phase(_)
        | Expr::Binding(_)
        | Expr::Variant(_)
        | Expr::None => {}
    }
}

fn union_touched_fields(
    base: &BTreeMap<String, String>,
    then_env: &BTreeMap<String, String>,
    else_env: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    base.keys()
        .filter(|field| {
            then_env.get(*field) != base.get(*field) || else_env.get(*field) != base.get(*field)
        })
        .cloned()
        .collect()
}

fn join_exprs(items: &[String], separator: &str) -> String {
    if items.is_empty() {
        return "TRUE".into();
    }
    if items.len() == 1 {
        items[0].clone()
    } else {
        format!("({})", items.join(separator))
    }
}

fn default_state_init_expr(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "FALSE".into(),
        TypeRef::U32 | TypeRef::U64 => "0".into(),
        TypeRef::String => tla_string(""),
        TypeRef::Named(name) => tla_string(&format!("{}_default", tla_ident(name).to_lowercase())),
        TypeRef::Option(_) => "None".into(),
        TypeRef::Set(_) => "{}".into(),
        TypeRef::Seq(_) => "<<>>".into(),
        TypeRef::Map(_, _) => "[x \\in {} |-> None]".into(),
    }
}

fn default_absent_map_value_expr(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "FALSE".into(),
        TypeRef::U32 | TypeRef::U64 => "0".into(),
        TypeRef::String | TypeRef::Named(_) => tla_string("None"),
        TypeRef::Option(_) => "None".into(),
        TypeRef::Set(_) => "{}".into(),
        TypeRef::Seq(_) => "<<>>".into(),
        TypeRef::Map(_, _) => "[x \\in {} |-> None]".into(),
    }
}

fn render_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "Bool".into(),
        TypeRef::U32 => "u32".into(),
        TypeRef::U64 => "u64".into(),
        TypeRef::String => "String".into(),
        TypeRef::Named(name) => name.clone(),
        TypeRef::Option(inner) => format!("Option<{}>", render_type_ref(inner)),
        TypeRef::Set(inner) => format!("Set<{}>", render_type_ref(inner)),
        TypeRef::Seq(inner) => format!("Seq<{}>", render_type_ref(inner)),
        TypeRef::Map(key, value) => {
            format!("Map<{}, {}>", render_type_ref(key), render_type_ref(value))
        }
    }
}

fn render_route_delivery(route: &meerkat_machine_schema::Route) -> &'static str {
    match route.delivery {
        RouteDelivery::Immediate => "Immediate",
        RouteDelivery::Enqueue => "Enqueue",
    }
}

fn render_scheduler_rule(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!("PreemptWhenReady({higher}, {lower})")
        }
    }
}

fn render_scheduler_rule_tuple(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!(
                "<<{}, {}, {}>>",
                tla_string("PreemptWhenReady"),
                tla_string(higher),
                tla_string(lower)
            )
        }
    }
}

fn render_scheduler_trigger_formula(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => format!(
            "/\\ {} \\in PendingActors /\\ {} \\in PendingActors",
            tla_string(higher),
            tla_string(lower)
        ),
    }
}

fn render_composition_invariant_formula(
    schema: &CompositionSchema,
    kind: &CompositionInvariantKind,
) -> String {
    match kind {
        CompositionInvariantKind::RoutePresent {
            from_machine,
            effect_variant,
            to_machine,
            input_variant,
        } => format!(
            "\\E route_name \\in RouteNames : /\\ RouteSource(route_name) = {} /\\ RouteEffect(route_name) = {} /\\ RouteTargetMachine(route_name) = {} /\\ RouteTargetInput(route_name) = {}",
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(to_machine),
            tla_string(input_variant)
        ),
        CompositionInvariantKind::ObservedInputOriginatesFromEffect {
            to_machine,
            input_variant,
            from_machine,
            effect_variant,
        } => format!(
            "\\A input_packet \\in observed_inputs : ((input_packet.machine = {} /\\ input_packet.variant = {}) => (/\\ input_packet.source_kind = \"route\" /\\ input_packet.source_machine = {} /\\ input_packet.source_effect = {} /\\ \\E effect_packet \\in emitted_effects : /\\ effect_packet.machine = {} /\\ effect_packet.variant = {} /\\ effect_packet.effect_id = input_packet.effect_id /\\ \\E route_packet \\in RoutePackets : /\\ route_packet.route = input_packet.source_route /\\ route_packet.source_machine = {} /\\ route_packet.effect = {} /\\ route_packet.target_machine = {} /\\ route_packet.target_input = {} /\\ route_packet.effect_id = input_packet.effect_id /\\ route_packet.payload = input_packet.payload))",
            tla_string(to_machine),
            tla_string(input_variant),
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(to_machine),
            tla_string(input_variant)
        ),
        CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
            route_name,
            to_machine,
            input_variant,
            from_machine,
            effect_variant,
        } => format!(
            "\\A input_packet \\in observed_inputs : ((input_packet.machine = {} /\\ input_packet.variant = {} /\\ input_packet.source_route = {}) => (/\\ input_packet.source_kind = \"route\" /\\ input_packet.source_machine = {} /\\ input_packet.source_effect = {} /\\ \\E effect_packet \\in emitted_effects : /\\ effect_packet.machine = {} /\\ effect_packet.variant = {} /\\ effect_packet.effect_id = input_packet.effect_id /\\ \\E route_packet \\in RoutePackets : /\\ route_packet.route = {} /\\ route_packet.source_machine = {} /\\ route_packet.effect = {} /\\ route_packet.target_machine = {} /\\ route_packet.target_input = {} /\\ route_packet.effect_id = input_packet.effect_id /\\ route_packet.payload = input_packet.payload))",
            tla_string(to_machine),
            tla_string(input_variant),
            tla_string(route_name),
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(route_name),
            tla_string(from_machine),
            tla_string(effect_variant),
            tla_string(to_machine),
            tla_string(input_variant)
        ),
        CompositionInvariantKind::ActorPriorityPresent { higher, lower } => format!(
            "<<{}, {}>> \\in ActorPriorities",
            tla_string(higher),
            tla_string(lower)
        ),
        CompositionInvariantKind::SchedulerRulePresent { rule } => {
            format!("{} \\in SchedulerRules", render_scheduler_rule_tuple(rule))
        }
        CompositionInvariantKind::OutcomeHandled {
            from_machine,
            effect_variant,
            required_targets,
        } => {
            let required = required_targets
                .iter()
                .enumerate()
                .map(|(idx, target)| {
                    let binding = format!("route_packet_{idx}");
                    let input_binding = format!("input_packet_{idx}");
                    format!(
                        "\\E {binding} \\in RoutePackets : /\\ {binding}.source_machine = {} /\\ {binding}.effect = {} /\\ {binding}.target_machine = {} /\\ {binding}.target_input = {} /\\ {binding}.effect_id = effect_packet.effect_id /\\ IF RouteDeliveryKind({binding}.route) = \"Immediate\" THEN \\E {input_binding} \\in observed_inputs : /\\ {input_binding}.machine = {} /\\ {input_binding}.variant = {} /\\ {input_binding}.source_kind = \"route\" /\\ {input_binding}.source_route = {binding}.route /\\ {input_binding}.source_machine = {} /\\ {input_binding}.source_effect = {} /\\ {input_binding}.effect_id = effect_packet.effect_id /\\ {input_binding}.payload = {binding}.payload ELSE TRUE",
                        tla_string(from_machine),
                        tla_string(effect_variant),
                        tla_string(&target.machine),
                        tla_string(&target.input_variant)
                        ,
                        tla_string(&target.machine),
                        tla_string(&target.input_variant),
                        tla_string(from_machine),
                        tla_string(effect_variant)
                    )
                })
                .collect::<Vec<_>>()
                .join(" /\\ ");
            let fallback = schema
                .routes
                .iter()
                .filter(|route| {
                    route.from_machine == *from_machine && route.effect_variant == *effect_variant
                })
                .count();
            if fallback == 0 {
                "FALSE".into()
            } else {
                format!(
                    "\\A effect_packet \\in emitted_effects : ((effect_packet.machine = {} /\\ effect_packet.variant = {}) => ({}))",
                    tla_string(from_machine),
                    tla_string(effect_variant),
                    required
                )
            }
        }
    }
}

fn render_composition_state_constraint(
    schema: &CompositionSchema,
    limits: &CompositionStateLimits,
    compiler: &CompositionTlaCompiler<'_>,
) -> String {
    let limits = effective_composition_state_limits(limits, None);
    let mut clauses = vec![
        format!("model_step_count <= {}", limits.step_limit),
        format!("Len(pending_inputs) <= {}", limits.pending_input_limit),
        format!(
            "Cardinality(observed_inputs) <= {}",
            limits.pending_input_limit + limits.delivered_route_limit + 2
        ),
        format!("Len(pending_routes) <= {}", limits.pending_route_limit),
        format!(
            "Cardinality(delivered_routes) <= {}",
            limits.delivered_route_limit
        ),
        format!(
            "Cardinality(emitted_effects) <= {}",
            limits.emitted_effect_limit
        ),
        format!("Cardinality(observed_transitions) <= {}", limits.step_limit),
    ];

    for instance in &schema.machines {
        let machine = compiler.machine(instance.instance_id.as_str());
        for field in &machine.state.fields {
            let field_var = compiler.field_var(instance.instance_id.as_str(), field.name.as_str());
            match &field.ty {
                TypeRef::Seq(_) => {
                    clauses.push(format!("Len({field_var}) <= {}", limits.seq_limit))
                }
                TypeRef::Set(_) => {
                    clauses.push(format!("Cardinality({field_var}) <= {}", limits.set_limit))
                }
                TypeRef::Map(_, _) => clauses.push(format!(
                    "Cardinality(DOMAIN {field_var}) <= {}",
                    limits.map_limit
                )),
                _ => {}
            }
        }
    }

    format!("/\\ {}", clauses.join(" /\\ "))
}

fn effective_composition_state_limits(
    limits: &CompositionStateLimits,
    witness: Option<&CompositionWitness>,
) -> CompositionStateLimits {
    let mut effective = limits.clone();
    if let Some(witness) = witness {
        let minimum_step_limit =
            (witness.preload_inputs.len() + witness.expected_transitions.len()) as u32;
        effective.step_limit = effective.step_limit.max(minimum_step_limit);
        effective.delivered_route_limit = effective
            .delivered_route_limit
            .max(witness.expected_routes.len() as u32);
        effective.emitted_effect_limit = effective
            .emitted_effect_limit
            .max(witness.expected_transitions.len() as u32);
        effective.pending_input_limit = effective.pending_input_limit.max(1);
    }
    effective
}

fn render_machine_state_constraint(schema: &MachineSchema, deep: bool) -> String {
    let step_limit = if deep { 8 } else { 6 };
    let seq_limit = if deep { 2 } else { 1 };
    let set_limit = if deep { 2 } else { 1 };
    let map_limit = if deep { 2 } else { 1 };
    let mut clauses = vec![format!("model_step_count <= {step_limit}")];

    for field in &schema.state.fields {
        match &field.ty {
            TypeRef::Seq(_) => {
                clauses.push(format!("Len({}) <= {seq_limit}", tla_ident(&field.name)))
            }
            TypeRef::Set(_) => clauses.push(format!(
                "Cardinality({}) <= {set_limit}",
                tla_ident(&field.name)
            )),
            TypeRef::Map(_, _) => clauses.push(format!(
                "Cardinality(DOMAIN {}) <= {map_limit}",
                tla_ident(&field.name)
            )),
            _ => {}
        }
    }

    if clauses.is_empty() {
        "TRUE".into()
    } else {
        format!("/\\ {}", clauses.join(" /\\ "))
    }
}

fn render_composition_case_fn(
    out: &mut String,
    fn_name: &str,
    param: &str,
    mappings: Vec<(&str, String)>,
    default_expr: String,
) {
    writeln!(out, "{fn_name}({param}) ==").expect("write to string");
    if mappings.is_empty() {
        writeln!(out, "    {}", default_expr).expect("write to string");
        writeln!(out).expect("write to string");
        return;
    }

    let mut first = true;
    for (key, value) in mappings {
        if first {
            writeln!(out, "    CASE {} = {} -> {}", param, tla_string(key), value)
                .expect("write to string");
            first = false;
        } else {
            writeln!(out, "      [] {} = {} -> {}", param, tla_string(key), value)
                .expect("write to string");
        }
    }
    writeln!(out, "      [] OTHER -> {}", default_expr).expect("write to string");
    writeln!(out).expect("write to string");
}

fn tla_ident(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn tla_string(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

struct InfallibleWrite;

impl InfallibleWrite {
    fn expect(self, _message: &str) {}
}

fn push_fmt(out: &mut String, args: std::fmt::Arguments<'_>) {
    let _ignored = out.write_fmt(args);
}

fn push_line(out: &mut String, args: std::fmt::Arguments<'_>) {
    push_fmt(out, args);
    out.push('\n');
}

macro_rules! pushln {
    ($out:expr) => {{
        $out.push('\n');
    }};
    ($out:expr, $($arg:tt)*) => {{
        push_line($out, format_args!($($arg)*));
    }};
}

macro_rules! writeln {
    ($dst:expr) => {{
        let _ignored = ::std::writeln!($dst);
        InfallibleWrite
    }};
    ($dst:expr, $($arg:tt)*) => {{
        let _ignored = ::std::writeln!($dst, $($arg)*);
        InfallibleWrite
    }};
}

use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionInvariantKind, CompositionSchema,
    CompositionStateLimits, CompositionWitness, EntryInput, EnumSchema, Expr, FeedbackFieldSource,
    FeedbackInputRef, Guard, HelperSchema, MachineCoverageManifest, MachineSchema, Quantifier,
    Route, RouteBindingSource, RouteDelivery, SchedulerRule, TransitionSchema, TypeRef, Update,
    VariantSchema, canonical_machine_schemas,
};

fn collect_helper_calls(expr: &Expr, calls: &mut BTreeSet<String>) {
    match expr {
        Expr::Bool(_)
        | Expr::U64(_)
        | Expr::String(_)
        | Expr::NamedVariant { .. }
        | Expr::EmptySet
        | Expr::EmptyMap
        | Expr::CurrentPhase
        | Expr::Phase(_)
        | Expr::Field(_)
        | Expr::Binding(_)
        | Expr::Variant(_)
        | Expr::None => {}
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => {
            for item in items {
                collect_helper_calls(item, calls);
            }
        }
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_helper_calls(condition, calls);
            collect_helper_calls(then_expr, calls);
            collect_helper_calls(else_expr, calls);
        }
        Expr::Not(inner)
        | Expr::SeqElements(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::Some(inner) => collect_helper_calls(inner, calls),
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right)
        | Expr::SeqStartsWith {
            seq: left,
            prefix: right,
        } => {
            collect_helper_calls(left, calls);
            collect_helper_calls(right, calls);
        }
        Expr::Contains { collection, value } => {
            collect_helper_calls(collection, calls);
            collect_helper_calls(value, calls);
        }
        Expr::MapGet { map, key } => {
            collect_helper_calls(map, calls);
            collect_helper_calls(key, calls);
        }
        Expr::Call { helper, args } => {
            calls.insert(helper.clone());
            for arg in args {
                collect_helper_calls(arg, calls);
            }
        }
        Expr::Quantified { over, body, .. } => {
            collect_helper_calls(over, calls);
            collect_helper_calls(body, calls);
        }
    }
}

fn helper_dependency_order(schema: &MachineSchema) -> Vec<&HelperSchema> {
    let defs = schema
        .helpers
        .iter()
        .chain(schema.derived.iter())
        .collect::<Vec<_>>();
    let known = defs
        .iter()
        .map(|helper| &helper.name)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut remaining = defs;
    let mut ordered = Vec::new();

    while !remaining.is_empty() {
        let remaining_names = remaining
            .iter()
            .map(|helper| &helper.name)
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut ready_indices = Vec::new();

        for (index, helper) in remaining.iter().enumerate() {
            let mut deps = BTreeSet::new();
            collect_helper_calls(&helper.body, &mut deps);
            let unresolved = deps.into_iter().filter(|dep| {
                dep != &helper.name && known.contains(dep) && remaining_names.contains(dep)
            });
            if unresolved.count() == 0 {
                ready_indices.push(index);
            }
        }

        if ready_indices.is_empty() {
            ordered.extend(remaining.into_iter());
            break;
        }

        for index in ready_indices.into_iter().rev() {
            ordered.push(remaining.remove(index));
        }
    }

    ordered
}

pub fn render_machine_contract_markdown(
    schema: &MachineSchema,
    coverage: &MachineCoverageManifest,
) -> String {
    let mut out = String::new();

    pushln!(&mut out, "# {}", schema.machine);
    pushln!(&mut out);
    writeln!(
        &mut out,
        "_Generated from the Rust machine catalog. Do not edit by hand._"
    )
    .expect("write to string");
    pushln!(&mut out);
    pushln!(&mut out, "- Version: `{}`", schema.version);
    writeln!(
        &mut out,
        "- Rust owner: `{}` / `{}`",
        schema.rust.crate_name, schema.rust.module
    )
    .expect("write to string");
    pushln!(&mut out);

    render_state_markdown(&mut out, schema);
    render_enum_markdown(&mut out, "Inputs", &schema.inputs);
    render_enum_markdown(&mut out, "Effects", &schema.effects);

    if !schema.helpers.is_empty() {
        pushln!(&mut out, "## Helpers");
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
        pushln!(&mut out);
    }

    if !schema.derived.is_empty() {
        pushln!(&mut out, "## Derived");
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
        pushln!(&mut out);
    }

    pushln!(&mut out, "## Invariants");
    for invariant in &schema.invariants {
        pushln!(&mut out, "- `{}`", invariant.name);
    }
    pushln!(&mut out);

    pushln!(&mut out, "## Transitions");
    for transition in &schema.transitions {
        pushln!(&mut out, "### `{}`", transition.name);
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
            pushln!(&mut out, "- Guards:");
            for guard in &transition.guards {
                pushln!(&mut out, "  - `{}`", guard.name);
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
        pushln!(&mut out, "- To: `{}`", transition.to);
        pushln!(&mut out);
    }

    pushln!(&mut out, "## Coverage");
    pushln!(&mut out, "### Code Anchors");
    for anchor in &coverage.code_anchors {
        pushln!(&mut out, "- `{}` — {}", anchor.path, anchor.note);
    }
    pushln!(&mut out);
    pushln!(&mut out, "### Scenarios");
    for scenario in &coverage.scenarios {
        pushln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary);
    }

    out
}

pub fn render_composition_contract_markdown(
    schema: &CompositionSchema,
    coverage: &CompositionCoverageManifest,
) -> String {
    let mut out = String::new();

    pushln!(&mut out, "# {}", schema.name);
    pushln!(&mut out);
    writeln!(
        &mut out,
        "_Generated from the Rust composition catalog. Do not edit by hand._"
    )
    .expect("write to string");
    pushln!(&mut out);

    pushln!(&mut out, "## Machines");
    for machine in &schema.machines {
        writeln!(
            &mut out,
            "- `{}`: `{}` @ actor `{}`",
            machine.instance_id, machine.machine_name, machine.actor
        )
        .expect("write to string");
    }
    pushln!(&mut out);

    pushln!(&mut out, "## Routes");
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
    pushln!(&mut out);

    pushln!(&mut out, "## Scheduler Rules");
    if schema.scheduler_rules.is_empty() {
        pushln!(&mut out, "- `(none)`");
    } else {
        for rule in &schema.scheduler_rules {
            pushln!(&mut out, "- `{}`", render_scheduler_rule(rule));
        }
    }
    pushln!(&mut out);

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

    pushln!(&mut out, "## Structural Requirements");
    if structural_invariants.is_empty() {
        pushln!(&mut out, "- `(none)`");
    } else {
        for invariant in &structural_invariants {
            pushln!(&mut out, "- `{}` — {}", invariant.name, invariant.statement);
        }
    }
    pushln!(&mut out);

    writeln!(&mut out, "## Behavioral Invariants");
    if behavioral_invariants.is_empty() {
        pushln!(&mut out, "- `(none)`");
    } else {
        for invariant in &behavioral_invariants {
            pushln!(&mut out, "- `{}` — {}", invariant.name, invariant.statement);
        }
    }
    pushln!(&mut out);

    writeln!(&mut out, "## Coverage");
    pushln!(&mut out, "### Code Anchors");
    for anchor in &coverage.code_anchors {
        pushln!(&mut out, "- `{}` — {}", anchor.path, anchor.note);
    }
    pushln!(&mut out);
    pushln!(&mut out, "### Scenarios");
    for scenario in &coverage.scenarios {
        pushln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary);
    }

    out
}

pub fn render_machine_ci_cfg(schema: &MachineSchema, deep: bool) -> String {
    let mut out = String::new();
    let domains = collect_binding_domains(schema);
    let named_samples = collect_machine_named_type_samples(schema);

    pushln!(&mut out, "SPECIFICATION Spec");
    if !domains.is_empty() {
        pushln!(&mut out, "CONSTANTS");
        for (name, ty) in domains {
            if matches!(
                ty,
                TypeRef::Seq(_) | TypeRef::Option(_) | TypeRef::Map(_, _)
            ) {
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
        pushln!(&mut out, "INVARIANTS");
        for invariant in &schema.invariants {
            pushln!(&mut out, "  {}", invariant.name);
        }
    }
    pushln!(&mut out, "CONSTRAINTS");
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
        .filter_map(|instance| {
            machine_by_name
                .get(instance.machine_name.as_str())
                .copied()
                .map(|machine| (instance.instance_id.as_str(), machine))
        })
        .collect::<BTreeMap<_, _>>();
    let domains = collect_composition_binding_domains(schema, &machine_by_instance);
    let named_samples = collect_composition_named_type_samples(schema, &machine_by_instance);
    let mut instance_invariants = Vec::new();

    for instance in &schema.machines {
        let Some(machine) = machine_by_instance
            .get(instance.instance_id.as_str())
            .copied()
        else {
            continue;
        };
        for invariant in &machine.invariants {
            instance_invariants.push(format!(
                "{}_{}",
                tla_ident(&instance.instance_id),
                tla_ident(&invariant.name)
            ));
        }
    }

    pushln!(&mut out, "SPECIFICATION Spec");
    if !domains.is_empty() {
        pushln!(&mut out, "CONSTANTS");
        for (name, ty) in domains {
            if matches!(
                ty,
                TypeRef::Seq(_) | TypeRef::Option(_) | TypeRef::Map(_, _)
            ) {
                continue;
            }
            let cardinality = if deep {
                schema
                    .deep_domain_overrides
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(schema.deep_domain_cardinality)
            } else {
                default_sample_cardinality(false)
            };
            writeln!(
                &mut out,
                "  {} = {}",
                name,
                render_default_domain_assignment(&ty, cardinality, &named_samples,)
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
        pushln!(&mut out, "INVARIANTS");
        for invariant in behavioral_invariants {
            pushln!(&mut out, "  {}", invariant.name);
        }
        for invariant_name in instance_invariants {
            pushln!(&mut out, "  {}", invariant_name);
        }
        if deep {
            pushln!(&mut out, "  CoverageInstrumentation");
        }
    }
    pushln!(&mut out, "CONSTRAINTS");
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
        .filter_map(|instance| {
            machine_by_name
                .get(instance.machine_name.as_str())
                .copied()
                .map(|machine| (instance.instance_id.as_str(), machine))
        })
        .collect::<BTreeMap<_, _>>();
    let domains = collect_composition_binding_domains(schema, &machine_by_instance);
    let named_samples =
        collect_composition_witness_named_type_samples(schema, witness, &machine_by_instance);
    let witness_sample_cardinality = schema
        .witness_domain_cardinality
        .max(max_named_sample_cardinality(&named_samples));
    let mut instance_invariants = Vec::new();

    for instance in &schema.machines {
        let Some(machine) = machine_by_instance
            .get(instance.instance_id.as_str())
            .copied()
        else {
            continue;
        };
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
        pushln!(&mut out, "CONSTANTS");
        for (name, ty) in domains {
            if matches!(
                ty,
                TypeRef::Seq(_) | TypeRef::Option(_) | TypeRef::Map(_, _)
            ) {
                continue;
            }
            writeln!(
                &mut out,
                "  {} = {}",
                name,
                render_default_domain_assignment(&ty, witness_sample_cardinality, &named_samples,)
            )
            .expect("write to string");
        }
    }
    if !schema.invariants.is_empty() || !instance_invariants.is_empty() {
        pushln!(&mut out, "INVARIANTS");
        for invariant in &schema.invariants {
            pushln!(&mut out, "  {}", invariant.name);
        }
        for invariant_name in instance_invariants {
            pushln!(&mut out, "  {}", invariant_name);
        }
        pushln!(&mut out, "  CoverageInstrumentation");
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
        pushln!(&mut out, "PROPERTIES");
        for property in witness_properties {
            pushln!(&mut out, "  {property}");
        }
    }
    pushln!(&mut out, "CONSTRAINTS");
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

fn max_named_sample_cardinality(named_samples: &BTreeMap<String, BTreeSet<String>>) -> usize {
    named_samples.values().map(BTreeSet::len).max().unwrap_or(1)
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
    match CompositionTlaCompiler::new(schema, &machine_catalog) {
        Ok(compiler) => compiler.render().unwrap_or_default(),
        Err(_) => String::new(),
    }
}

pub fn render_machine_semantic_model(schema: &MachineSchema) -> String {
    let mut compiler = MachineTlaCompiler::new(schema);
    compiler.render()
}

fn render_state_markdown(out: &mut String, schema: &MachineSchema) {
    pushln!(out, "## State");
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
        pushln!(out, "- `{}`: `{}`", field.name, render_type_ref(&field.ty));
    }
    pushln!(out);
}

fn render_enum_markdown(out: &mut String, label: &str, schema: &EnumSchema) {
    writeln!(out, "## {label}");
    for variant in &schema.variants {
        if variant.fields.is_empty() {
            pushln!(out, "- `{}`", variant.name);
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
    pushln!(out);
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
                    collect_type_domains(&field.ty, &mut domains);
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
        let Some(machine) = machine_by_instance
            .get(entry_input.machine.as_str())
            .copied()
        else {
            continue;
        };
        let Ok(variant) = machine.inputs.variant_named(&entry_input.input_variant) else {
            continue;
        };
        for field in &variant.fields {
            collect_type_domains(&field.ty, &mut domains);
        }
    }

    domains
}

fn collect_type_domains(ty: &TypeRef, domains: &mut BTreeMap<String, TypeRef>) {
    let name = domain_constant_name(ty);
    domains.entry(name).or_insert_with(|| ty.clone());
    match ty {
        TypeRef::Option(inner) | TypeRef::Seq(inner) | TypeRef::Set(inner) => {
            collect_type_domains(inner, domains);
        }
        TypeRef::Map(key, value) => {
            collect_type_domains(key, domains);
            collect_type_domains(value, domains);
        }
        TypeRef::Bool
        | TypeRef::U32
        | TypeRef::U64
        | TypeRef::String
        | TypeRef::Named(_)
        | TypeRef::Enum(_) => {}
    }
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
            .ok()
            .map(|variant| {
                variant
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
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();

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
            let Ok(effect_variant) = schema.effects.variant_named(&effect.variant) else {
                continue;
            };
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
        let Some(machine) = machine_by_instance
            .get(entry_input.machine.as_str())
            .copied()
        else {
            continue;
        };
        merge_named_type_samples(&mut samples, collect_machine_named_type_samples(machine));
    }

    for route in &schema.routes {
        let Some(source_machine) = machine_by_instance
            .get(route.from_machine.as_str())
            .copied()
        else {
            continue;
        };
        let Ok(source_variant) = source_machine.effects.variant_named(&route.effect_variant) else {
            continue;
        };
        let source_field_types = source_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();
        let Some(target_machine) = machine_by_instance.get(route.to.machine.as_str()).copied()
        else {
            continue;
        };
        let Ok(target_variant) = target_machine.inputs.variant_named(&route.to.input_variant)
        else {
            continue;
        };
        let field_types = target_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();

        for binding in &route.bindings {
            if let RouteBindingSource::Literal(expr) = &binding.source
                && let Some(field_ty) = field_types.get(&binding.to_field)
            {
                collect_named_literals_from_expr(
                    &mut samples,
                    expr,
                    Some(field_ty),
                    &field_types,
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                );
            }
            if let RouteBindingSource::Field { from_field, .. } = &binding.source
                && let (Some(source_ty), Some(target_ty)) = (
                    source_field_types.get(from_field),
                    field_types.get(&binding.to_field),
                )
            {
                propagate_named_samples_between_types(&mut samples, source_ty, target_ty);
            }
        }
    }

    for witness in &schema.witnesses {
        for preload in &witness.preload_inputs {
            let Some(machine) = machine_by_instance.get(preload.machine.as_str()).copied() else {
                continue;
            };
            let Ok(variant) = machine.inputs.variant_named(&preload.input_variant) else {
                continue;
            };
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

        let Some(source_machine) = machine_by_instance
            .get(route.from_machine.as_str())
            .copied()
        else {
            continue;
        };
        let Ok(source_variant) = source_machine.effects.variant_named(&route.effect_variant) else {
            continue;
        };
        let source_field_types = source_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();
        let Some(target_machine) = machine_by_instance.get(route.to.machine.as_str()).copied()
        else {
            continue;
        };
        let Ok(target_variant) = target_machine.inputs.variant_named(&route.to.input_variant)
        else {
            continue;
        };
        let field_types = target_variant
            .fields
            .iter()
            .map(|field| (field.name.clone(), field.ty.clone()))
            .collect::<BTreeMap<_, _>>();

        for binding in &route.bindings {
            if let RouteBindingSource::Literal(expr) = &binding.source
                && let Some(field_ty) = field_types.get(&binding.to_field)
            {
                collect_named_literals_from_expr(
                    &mut samples,
                    expr,
                    Some(field_ty),
                    &field_types,
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                );
            }
            if let RouteBindingSource::Field { from_field, .. } = &binding.source
                && let (Some(source_ty), Some(target_ty)) = (
                    source_field_types.get(from_field),
                    field_types.get(&binding.to_field),
                )
            {
                propagate_named_samples_between_types(&mut samples, source_ty, target_ty);
            }
        }
    }

    for preload in &witness.preload_inputs {
        let Some(machine) = machine_by_instance.get(preload.machine.as_str()).copied() else {
            continue;
        };
        let Ok(variant) = machine.inputs.variant_named(&preload.input_variant) else {
            continue;
        };
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
            let Some(source_machine) = machine_by_instance
                .get(route.from_machine.as_str())
                .copied()
            else {
                continue;
            };
            let Ok(source_variant) = source_machine.effects.variant_named(&route.effect_variant)
            else {
                continue;
            };
            let source_field_types = source_variant
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.ty.clone()))
                .collect::<BTreeMap<_, _>>();
            let Some(target_machine) = machine_by_instance.get(route.to.machine.as_str()).copied()
            else {
                continue;
            };
            let Ok(target_variant) = target_machine.inputs.variant_named(&route.to.input_variant)
            else {
                continue;
            };
            let target_field_types = target_variant
                .fields
                .iter()
                .map(|field| (field.name.clone(), field.ty.clone()))
                .collect::<BTreeMap<_, _>>();
            for binding in &route.bindings {
                if let RouteBindingSource::Field { from_field, .. } = &binding.source
                    && let (Some(source_ty), Some(target_ty)) = (
                        source_field_types.get(from_field),
                        target_field_types.get(&binding.to_field),
                    )
                {
                    propagate_named_samples_between_types(&mut samples, source_ty, target_ty);
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
        | Expr::SeqElements(inner)
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
                Some(TypeRef::Set(inner_ty) | TypeRef::Seq(inner_ty)) => Some(*inner_ty),
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
        | Expr::NamedVariant { .. }
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
        Expr::NamedVariant { enum_name, .. } => Some(TypeRef::Named(enum_name.clone())),
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
        Expr::SeqElements(inner) => {
            match infer_expr_type(inner, field_types, helper_returns, binding_types) {
                Some(TypeRef::Seq(inner_ty)) => Some(TypeRef::Set(inner_ty)),
                _ => None,
            }
        }
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
        TypeRef::Named(name) | TypeRef::Enum(name) => {
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
            let samples = sample_values(inner, sample_cardinality, named_samples)
                .into_iter()
                .map(|sample| format!("[tag |-> {}, value |-> {}]", tla_string("some"), sample))
                .collect::<Vec<_>>();
            if samples.is_empty() {
                format!(
                    "{{[tag |-> {}, value |-> {}]}}",
                    tla_string("none"),
                    tla_string("none")
                )
            } else {
                format!(
                    "{{[tag |-> {}, value |-> {}], {}}}",
                    tla_string("none"),
                    tla_string("none"),
                    samples.join(", ")
                )
            }
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
        TypeRef::Named(name) | TypeRef::Enum(name) => {
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
        TypeRef::Option(inner) => {
            let mut values = vec![format!(
                "[tag |-> {}, value |-> {}]",
                tla_string("none"),
                tla_string("none")
            )];
            values.extend(
                sample_values(inner, sample_cardinality, named_samples)
                    .into_iter()
                    .map(|sample| {
                        format!("[tag |-> {}, value |-> {}]", tla_string("some"), sample)
                    }),
            );
            values
        }
        TypeRef::Seq(inner) => sample_values(inner, sample_cardinality, named_samples),
        TypeRef::Set(inner) => sample_values(inner, sample_cardinality, named_samples),
        TypeRef::Map(_, _) => vec![],
    }
}

fn render_sequence_domain_definition(inner: &TypeRef) -> String {
    let inner_domain = render_type_domain_expr(inner);
    format!(
        "{{<<>>}} \\cup {{<<x>> : x \\in {inner_domain}}} \\cup {{<<x, y>> : x \\in {inner_domain}, y \\in {inner_domain}}}"
    )
}

fn render_option_domain_definition(inner: &TypeRef) -> String {
    let inner_domain = render_type_domain_expr(inner);
    format!("{{None}} \\cup {{Some(x) : x \\in {inner_domain}}}")
}

fn render_map_domain_definition(key: &TypeRef, value: &TypeRef) -> String {
    let key_domain = render_type_domain_expr(key);
    let value_domain = render_type_domain_expr(value);
    let empty_map = "[x \\in {} |-> None]";
    format!(
        "{{{empty_map}}} \\cup {{ [x \\in {{k}} |-> v] : k \\in {key_domain}, v \\in {value_domain} }}"
    )
}

fn render_type_domain_expr(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "BOOLEAN".into(),
        TypeRef::U32 | TypeRef::U64 => "NatValues".into(),
        TypeRef::String => "StringValues".into(),
        TypeRef::Named(_)
        | TypeRef::Enum(_)
        | TypeRef::Set(_)
        | TypeRef::Seq(_)
        | TypeRef::Option(_)
        | TypeRef::Map(_, _) => domain_constant_name(ty),
    }
}

fn domain_constant_name(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named(name) | TypeRef::Enum(name) => format!("{}Values", tla_ident(name)),
        TypeRef::Seq(inner) => format!("SeqOf{}Values", tla_ident(&type_ref_name(inner))),
        TypeRef::Set(inner) => format!("SetOf{}Values", tla_ident(&type_ref_name(inner))),
        TypeRef::String => "StringValues".into(),
        TypeRef::Bool => "BooleanValues".into(),
        TypeRef::U32 | TypeRef::U64 => "NatValues".into(),
        TypeRef::Option(inner) => format!("Option{}Values", tla_ident(&type_ref_name(inner))),
        TypeRef::Map(key, value) => {
            format!(
                "Map{}{}Values",
                tla_ident(&type_ref_name(key)),
                tla_ident(&type_ref_name(value))
            )
        }
    }
}

fn type_ref_name(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "Bool".into(),
        TypeRef::U32 => "U32".into(),
        TypeRef::U64 => "U64".into(),
        TypeRef::String => "String".into(),
        TypeRef::Named(name) | TypeRef::Enum(name) => name.clone(),
        TypeRef::Option(inner) => format!("Option{}", type_ref_name(inner)),
        TypeRef::Set(inner) => format!("Set{}", type_ref_name(inner)),
        TypeRef::Seq(inner) => format!("Seq{}", type_ref_name(inner)),
        TypeRef::Map(key, value) => format!("Map{}{}", type_ref_name(key), type_ref_name(value)),
    }
}

struct CompositionTlaCompiler<'a> {
    schema: &'a CompositionSchema,
    machine_by_instance: BTreeMap<&'a str, &'a MachineSchema>,
    fallback_machine: &'a MachineSchema,
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
            fallback_machine: machine_catalog.first().ok_or_else(|| {
                format!(
                    "no machine catalog available for composition `{}`",
                    schema.name
                )
            })?,
        })
    }

    fn render(&self) -> std::result::Result<String, String> {
        let mut out = String::new();
        let constants = self.collect_binding_domains();
        let machine_vars = self.machine_vars();
        let obligation_vars = self.obligation_vars();
        let mut machine_invariant_names = Vec::new();

        pushln!(&mut out, "---- MODULE model ----");
        pushln!(&mut out, "EXTENDS TLC, Naturals, Sequences, FiniteSets");
        pushln!(&mut out);
        writeln!(
            &mut out,
            "\\* Generated composition model for {}.",
            self.schema.name
        )
        .expect("write to string");
        pushln!(&mut out);

        let model_constants = constants
            .iter()
            .filter(|(_, ty)| {
                !matches!(
                    ty,
                    TypeRef::Seq(_) | TypeRef::Option(_) | TypeRef::Map(_, _)
                )
            })
            .map(|(name, _)| name)
            .cloned()
            .collect::<Vec<_>>();
        if !model_constants.is_empty() {
            writeln!(&mut out, "CONSTANTS {}", model_constants.join(", "))
                .expect("write to string");
            pushln!(&mut out);
        }

        writeln!(&mut out, "None == [tag |-> \"none\", value |-> \"none\"]")
            .expect("write to string");
        writeln!(&mut out, "Some(v) == [tag |-> \"some\", value |-> v]");
        pushln!(&mut out);
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
        for (name, ty) in &constants {
            if let TypeRef::Option(inner) = ty {
                writeln!(
                    &mut out,
                    "{} == {}",
                    name,
                    render_option_domain_definition(inner)
                )
                .expect("write to string");
            }
        }
        for (name, ty) in &constants {
            if let TypeRef::Map(key, value) = ty {
                writeln!(
                    &mut out,
                    "{} == {}",
                    name,
                    render_map_domain_definition(key, value)
                )
                .expect("write to string");
            }
        }
        if constants.values().any(|ty| {
            matches!(
                ty,
                TypeRef::Seq(_) | TypeRef::Option(_) | TypeRef::Map(_, _)
            )
        }) {
            pushln!(&mut out);
        }
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

        let all_vars = {
            let mut v = machine_vars;
            v.extend(obligation_vars.clone());
            v.push("model_step_count".into());
            v.push("pending_inputs".into());
            v.push("observed_inputs".into());
            v.push("pending_routes".into());
            v.push("delivered_routes".into());
            v.push("emitted_effects".into());
            v.push("observed_transitions".into());
            v.push("witness_current_script_input".into());
            v.push("witness_remaining_script_inputs".into());
            v
        };
        writeln!(&mut out, "VARIABLES {}", all_vars.join(", ")).expect("write to string");
        writeln!(&mut out, "vars == << {} >>", all_vars.join(", ")).expect("write to string");
        pushln!(&mut out);

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
        pushln!(&mut out);

        pushln!(&mut out, "BaseInit ==");
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
        for var in &obligation_vars {
            pushln!(&mut out, "    /\\ {} = {{}}", var);
        }
        pushln!(&mut out, "    /\\ model_step_count = 0");
        pushln!(&mut out, "    /\\ pending_routes = <<>>");
        pushln!(&mut out, "    /\\ delivered_routes = {{}}");
        pushln!(&mut out, "    /\\ emitted_effects = {{}}");
        pushln!(&mut out, "    /\\ observed_transitions = {{}}");
        pushln!(&mut out);

        pushln!(&mut out, "Init ==");
        pushln!(&mut out, "    /\\ BaseInit");
        pushln!(&mut out, "    /\\ pending_inputs = <<>>");
        pushln!(&mut out, "    /\\ observed_inputs = {{}}");
        pushln!(&mut out, "    /\\ witness_current_script_input = None");
        pushln!(&mut out, "    /\\ witness_remaining_script_inputs = <<>>");
        pushln!(&mut out);

        for witness in &self.schema.witnesses {
            writeln!(
                &mut out,
                "{} ==",
                composition_witness_init_name(&witness.name)
            )
            .expect("write to string");
            writeln!(&mut out, "    /\\ BaseInit");
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
            pushln!(&mut out);
        }

        for instance in &self.schema.machines {
            let machine = self.machine(instance.instance_id.as_str());
            let helper_prefix = format!("{}__", tla_ident(&instance.instance_id));
            let mut compiler = MachineTlaCompiler::new_with_helper_prefix(machine, helper_prefix)
                .with_phase_symbol(self.phase_var(instance.instance_id.as_str()))
                .with_field_env_override(self.machine_field_env(instance.instance_id.as_str()));
            for derived in helper_dependency_order(machine) {
                compiler.render_helper(&mut out, derived);
                pushln!(&mut out);
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
                pushln!(&mut out, "{helper}");
                pushln!(&mut out);
            }

            for action in &rendered_actions {
                pushln!(&mut out, "{action}");
                pushln!(&mut out);
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
                pushln!(&mut out);
            }
        }

        self.render_entry_input_actions(&mut out);
        self.render_deliver_queued_route_action(&mut out);
        self.render_quiescent_stutter(&mut out);
        self.render_witness_inject_actions(&mut out);

        let core_next_branches = self.core_next_branches();

        pushln!(&mut out, "CoreNext ==");
        for branch in &core_next_branches {
            pushln!(&mut out, "    \\/ {branch}");
        }
        pushln!(&mut out);

        let inject_branches = self
            .schema
            .entry_inputs
            .iter()
            .map(|entry_input| self.render_entry_input_call(entry_input))
            .collect::<Vec<_>>();

        pushln!(&mut out, "InjectNext ==");
        if inject_branches.is_empty() {
            pushln!(&mut out, "    FALSE");
        } else {
            for branch in &inject_branches {
                pushln!(&mut out, "    \\/ {branch}");
            }
        }
        pushln!(&mut out);

        pushln!(&mut out, "Next ==");
        pushln!(&mut out, "    \\/ CoreNext");
        if !inject_branches.is_empty() {
            pushln!(&mut out, "    \\/ InjectNext");
        }
        pushln!(&mut out);

        for witness in &self.schema.witnesses {
            writeln!(
                &mut out,
                "{} ==",
                composition_witness_next_name(&witness.name)
            )
            .expect("write to string");
            pushln!(&mut out, "    \\/ CoreNext");
            writeln!(
                &mut out,
                "    \\/ {}",
                self.render_witness_inject_next_call(witness)
            )
            .expect("write to string");
            pushln!(&mut out);
        }
        pushln!(&mut out);

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
            pushln!(&mut out);
        }

        self.render_obligation_invariants(&mut out, &mut machine_invariant_names)?;
        self.render_owner_actor_processes(&mut out)?;

        self.render_coverage_instrumentation(&mut out);

        writeln!(
            &mut out,
            "CiStateConstraint == {}",
            render_composition_state_constraint(
                self.schema,
                self.schema
                    .ci_limits
                    .as_ref()
                    .unwrap_or(&CompositionStateLimits::ci_defaults()),
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
        pushln!(&mut out);

        pushln!(&mut out, "Spec == Init /\\ [][Next]_vars");
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
        pushln!(&mut out);
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
            pushln!(&mut out);
        }
        for invariant in &self.schema.invariants {
            pushln!(&mut out, "THEOREM Spec => []{}", invariant.name);
        }
        for invariant_name in &machine_invariant_names {
            pushln!(&mut out, "THEOREM Spec => []{}", invariant_name);
        }
        pushln!(&mut out);
        writeln!(
            &mut out,
            "============================================================================="
        )
        .expect("write to string");

        Ok(out)
    }

    fn collect_binding_domains(&self) -> BTreeMap<String, TypeRef> {
        collect_composition_binding_domains(self.schema, &self.machine_by_instance)
    }

    fn machine(&self, instance_id: &str) -> &MachineSchema {
        self.machine_by_instance
            .get(instance_id)
            .copied()
            .unwrap_or(self.fallback_machine)
    }

    fn actor(&self, instance_id: &str) -> &str {
        self.schema
            .machines
            .iter()
            .find(|machine| machine.instance_id == instance_id)
            .map(|machine| machine.actor.as_str())
            .unwrap_or("")
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
                .ok()
                .map(|variant| {
                    variant
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
                        .collect::<BTreeMap<_, _>>()
                })
                .unwrap_or_default();
            let mut prefix = String::new();
            for binding in &transition.on.bindings {
                let local = format!("arg_{}", tla_ident(binding));
                let Some(ty) = binding_types.get(binding) else {
                    return self.machine_transition_name(instance_id, transition);
                };
                let domain = self.binding_domain_for_type(ty);
                push_fmt(&mut prefix, format_args!("\\E {local} \\in {domain} : "));
            }
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
            TypeRef::Named(_)
            | TypeRef::Enum(_)
            | TypeRef::Seq(_)
            | TypeRef::Set(_)
            | TypeRef::Option(_)
            | TypeRef::Map(_, _) => domain_constant_name(ty),
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

    /// Returns TLA+ variable names for obligation tracking — one per handoff protocol.
    fn obligation_vars(&self) -> Vec<String> {
        self.schema
            .handoff_protocols
            .iter()
            .map(|protocol| format!("obligation_{}", tla_ident(&protocol.name)))
            .collect()
    }

    fn feedback_variant(
        &self,
        feedback: &FeedbackInputRef,
    ) -> std::result::Result<&VariantSchema, String> {
        self.machine(&feedback.machine_instance)
            .inputs
            .variant_named(&feedback.input_variant)
            .map_err(|_| {
                format!(
                    "feedback variant `{}` not found for machine `{}`",
                    feedback.input_variant, feedback.machine_instance
                )
            })
    }

    fn correlation_match_expr(
        &self,
        protocol: &meerkat_machine_schema::EffectHandoffProtocol,
        feedback: &FeedbackInputRef,
        obligation_var: &str,
        input_packet_var: &str,
    ) -> std::result::Result<String, String> {
        if protocol.correlation_fields.is_empty() {
            return Ok(format!("{obligation_var} /= {{}}"));
        }

        let clauses = protocol
            .correlation_fields
            .iter()
            .map(|correlation_field| {
                let binding = feedback
                    .field_bindings
                    .iter()
                    .find(|binding| {
                        matches!(
                            &binding.source,
                            FeedbackFieldSource::ObligationField(name)
                                if name == correlation_field
                        )
                    })
                    .ok_or_else(|| {
                        format!(
                            "correlation binding for field `{}` not found",
                            correlation_field
                        )
                    })?;
                Ok::<String, String>(format!(
                    "record.{} = {}.payload.{}",
                    tla_ident(correlation_field),
                    input_packet_var,
                    tla_ident(&binding.input_field)
                ))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(format!(
            "(\\E record \\in {} : ({}))",
            obligation_var,
            clauses.join(" /\\ ")
        ))
    }

    fn feedback_payload_expr(
        &self,
        feedback: &FeedbackInputRef,
        token_var: &str,
    ) -> std::result::Result<(String, Vec<String>), String> {
        let variant = self.feedback_variant(feedback)?;
        let mut owner_context_quantifiers = Vec::new();
        let mut payload_fields = Vec::new();

        for field in &variant.fields {
            let binding = feedback
                .field_bindings
                .iter()
                .find(|binding| binding.input_field == field.name)
                .ok_or_else(|| format!("feedback binding for field `{}` not found", field.name))?;
            let expr = match &binding.source {
                FeedbackFieldSource::ObligationField(source_field) => {
                    format!("{token_var}.{}", tla_ident(source_field))
                }
                FeedbackFieldSource::OwnerContext(name) => {
                    let var_name = format!("owner_ctx_{}", tla_ident(name));
                    let domain = render_type_domain_expr(&field.ty);
                    owner_context_quantifiers.push(format!("{var_name} \\in {domain}"));
                    var_name
                }
            };
            payload_fields.push(format!("{} |-> {}", tla_ident(&field.name), expr));
        }

        let payload_expr = if payload_fields.is_empty() {
            "[tag |-> \"unit\"]".to_string()
        } else {
            format!("[{}]", payload_fields.join(", "))
        };

        Ok((payload_expr, owner_context_quantifiers))
    }

    /// Renders obligation closure invariants into the TLA+ output.
    ///
    /// For each handoff protocol, generates:
    /// - `NoOpenObligationsOnTerminal_<protocol>`: when the producer machine is
    ///   in a terminal phase, the obligation set must be empty.
    /// - `NoFeedbackWithoutObligation_<protocol>`: a feedback input can only be
    ///   observed if there's a corresponding outstanding obligation.
    fn render_obligation_invariants(
        &self,
        out: &mut String,
        invariant_names: &mut Vec<String>,
    ) -> std::result::Result<(), String> {
        for protocol in &self.schema.handoff_protocols {
            let var = format!("obligation_{}", tla_ident(&protocol.name));
            let phase_var = self.phase_var(&protocol.producer_instance);

            // Find terminal phases for the producer machine.
            let machine = self.machine(&protocol.producer_instance);
            let terminal_phases: Vec<&str> = machine
                .state
                .terminal_phases
                .iter()
                .map(String::as_str)
                .collect();

            // NoOpenObligationsOnTerminal: terminal phase => obligation set is empty
            if !terminal_phases.is_empty() {
                let inv_name = format!("NoOpenObligationsOnTerminal_{}", tla_ident(&protocol.name));
                let terminal_disjuncts: Vec<String> = terminal_phases
                    .iter()
                    .map(|phase| format!("{} = {}", phase_var, tla_string(phase)))
                    .collect();
                writeln!(
                    out,
                    "{} == ({}) => {} = {{}}",
                    inv_name,
                    terminal_disjuncts.join(" \\/ "),
                    var
                )
                .expect("write to string");
                invariant_names.push(inv_name);
            }

            // NoFeedbackWithoutObligation: feedback input observed => matching obligation exists
            if !protocol.allowed_feedback_inputs.is_empty() {
                let inv_name = format!("NoFeedbackWithoutObligation_{}", tla_ident(&protocol.name));
                let obligation_checks: Vec<String> = protocol
                    .allowed_feedback_inputs
                    .iter()
                    .map(|feedback| {
                        let packet_match = format!(
                            "(input_packet.machine = {} /\\ input_packet.variant = {})",
                            tla_string(&feedback.machine_instance),
                            tla_string(&feedback.input_variant)
                        );
                        let obligation_check =
                            self.correlation_match_expr(protocol, feedback, &var, "input_packet")?;
                        Ok::<String, String>(format!("(({packet_match}) => ({obligation_check}))"))
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                writeln!(
                    out,
                    "{} == \\A input_packet \\in observed_inputs : ({})",
                    inv_name,
                    obligation_checks.join(" /\\ ")
                )
                .expect("write to string");
                invariant_names.push(inv_name);
            }
        }

        if !self.schema.handoff_protocols.is_empty() {
            pushln!(out);
        }
        Ok(())
    }

    /// Renders owner-actor process definitions: nondeterministic actions that
    /// model the owner choosing to submit protocol-allowed feedback when an
    /// obligation is outstanding.
    ///
    /// Each protocol generates an `OwnerFeedback_<protocol>` action that:
    /// - Is enabled when the obligation set is non-empty
    /// - Nondeterministically picks one of the allowed feedback inputs
    /// - Removes one obligation token from the set
    /// - Injects the feedback as a pending input
    ///
    /// If any protocols have `liveness_annotation`, fairness comments are added.
    fn render_owner_actor_processes(&self, out: &mut String) -> std::result::Result<(), String> {
        for protocol in &self.schema.handoff_protocols {
            let var = format!("obligation_{}", tla_ident(&protocol.name));
            let action_name = format!("OwnerFeedback_{}", tla_ident(&protocol.name));

            if protocol.allowed_feedback_inputs.is_empty() {
                continue;
            }

            if let Some(liveness) = &protocol.liveness_annotation {
                writeln!(out, "\\* Liveness: {liveness}").expect("write to string");
            }

            writeln!(out, "{action_name} ==").expect("write to string");
            writeln!(out, "    /\\ {} /= {{}}", var).expect("write to string");
            writeln!(out, "    /\\ \\E token \\in {} :", var).expect("write to string");
            let feedback_branches: Vec<String> = protocol
                .allowed_feedback_inputs
                .iter()
                .map(|feedback| {
                    let (payload_expr, owner_context_quantifiers) =
                        self.feedback_payload_expr(feedback, "token")?;
                    let input_expr = format!(
                        "[machine |-> {}, variant |-> {}, source_kind |-> \"owner\", source_machine |-> {}, source_effect |-> {}, source_route |-> \"none\", effect_id |-> token, payload |-> {}]",
                        tla_string(&feedback.machine_instance),
                        tla_string(&feedback.input_variant),
                        tla_string(&protocol.producer_instance),
                        tla_string(&protocol.effect_variant),
                        payload_expr,
                    );
                    let quantifier_prefix = if owner_context_quantifiers.is_empty() {
                        String::new()
                    } else {
                        format!("\\E {} : ", owner_context_quantifiers.join(", "))
                    };
                    Ok::<String, String>(format!(
                        "{}(/\\ pending_inputs' = Append(pending_inputs, {}) /\\ {}' = {} \\ {{token}})",
                        quantifier_prefix,
                        input_expr,
                        var,
                        var
                    ))
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;

            writeln!(out, "        /\\ ({})", feedback_branches.join(" \\/ "))
                .expect("write to string");

            // Frame: all other variables unchanged
            let unchanged_vars: Vec<String> = self
                .machine_vars()
                .into_iter()
                .chain(self.obligation_vars().into_iter().filter(|v| *v != var))
                .chain(
                    [
                        "model_step_count",
                        "observed_inputs",
                        "pending_routes",
                        "delivered_routes",
                        "emitted_effects",
                        "observed_transitions",
                        "witness_current_script_input",
                        "witness_remaining_script_inputs",
                    ]
                    .iter()
                    .map(std::string::ToString::to_string),
                )
                .collect();
            writeln!(out, "    /\\ UNCHANGED << {} >>", unchanged_vars.join(", "))
                .expect("write to string");
            pushln!(out);
        }
        Ok(())
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

        pushln!(out, "CoverageInstrumentation == {instrumentation}");
        pushln!(out);
    }

    fn render_static_sets(&self, out: &mut String) {
        pushln!(out, "Machines == {{");
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
        pushln!(out, "}}");
        pushln!(out);

        pushln!(out, "RouteNames == {{");
        for (idx, route) in self.schema.routes.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.routes.len() {
                ""
            } else {
                ","
            };
            pushln!(out, "    {}{}", tla_string(&route.name), suffix);
        }
        pushln!(out, "}}");
        pushln!(out);

        pushln!(out, "Actors == {{");
        for (idx, machine) in self.schema.machines.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.machines.len() {
                ""
            } else {
                ","
            };
            pushln!(out, "    {}{}", tla_string(&machine.actor), suffix);
        }
        pushln!(out, "}}");
        pushln!(out);

        pushln!(out, "ActorPriorities == {{");
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
        pushln!(out, "}}");
        pushln!(out);

        pushln!(out, "SchedulerRules == {{");
        for (idx, rule) in self.schema.scheduler_rules.iter().enumerate() {
            let suffix = if idx + 1 == self.schema.scheduler_rules.len() {
                ""
            } else {
                ","
            };
            pushln!(out, "    {}{}", render_scheduler_rule_tuple(rule), suffix);
        }
        writeln!(out, "}}");
        pushln!(out);

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
        pushln!(out);
    }

    fn render_entry_input_actions(&self, out: &mut String) {
        for entry_input in &self.schema.entry_inputs {
            let machine = self.machine(entry_input.machine.as_str());
            let Ok(variant) = machine.inputs.variant_named(&entry_input.input_variant) else {
                continue;
            };
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
                pushln!(out, "{} ==", action_name);
            } else {
                pushln!(out, "{}({}) ==", action_name, params.join(", "));
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
            {
                let mut unchanged = self.machine_vars();
                unchanged.extend(self.obligation_vars());
                unchanged.extend([
                    "pending_routes".into(),
                    "delivered_routes".into(),
                    "emitted_effects".into(),
                    "observed_transitions".into(),
                    "witness_current_script_input".into(),
                    "witness_remaining_script_inputs".into(),
                ]);
                writeln!(out, "    /\\ UNCHANGED << {} >>", unchanged.join(", "))
                    .expect("write to string");
            }
            pushln!(out);
        }
    }

    fn render_entry_input_call(&self, entry_input: &EntryInput) -> String {
        let machine = self.machine(entry_input.machine.as_str());
        let Ok(variant) = machine.inputs.variant_named(&entry_input.input_variant) else {
            return self.entry_input_action_name(entry_input);
        };
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
        writeln!(out, "DeliverQueuedRoute ==");
        pushln!(out, "    /\\ Len(pending_routes) > 0");
        pushln!(out, "    /\\ LET route == Head(pending_routes) IN");
        pushln!(out, "       /\\ pending_routes' = Tail(pending_routes)");
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
        {
            let mut unchanged = self.machine_vars();
            unchanged.extend(self.obligation_vars());
            unchanged.extend([
                "emitted_effects".into(),
                "observed_transitions".into(),
                "witness_current_script_input".into(),
                "witness_remaining_script_inputs".into(),
            ]);
            writeln!(out, "       /\\ UNCHANGED << {} >>", unchanged.join(", "))
                .expect("write to string");
        }
        pushln!(out);
    }

    fn render_quiescent_stutter(&self, out: &mut String) {
        writeln!(out, "QuiescentStutter ==");
        pushln!(out, "    /\\ Len(pending_routes) = 0");
        pushln!(out, "    /\\ Len(pending_inputs) = 0");
        pushln!(out, "    /\\ UNCHANGED vars");
        pushln!(out);
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
                branches
                    .push(self.machine_transition_call(instance.instance_id.as_str(), transition));
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
        clauses.extend(self.schema.machines.iter().flat_map(|instance| {
            let machine = self.machine(instance.instance_id.as_str());
            machine
                .transitions
                .iter()
                .map(|transition| {
                    self.machine_transition_call(instance.instance_id.as_str(), transition)
                })
                .collect::<Vec<_>>()
        }));
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
            pushln!(out, "{action_name} ==");
            if witness.preload_inputs.len() <= 1 {
                pushln!(out, "    FALSE");
                pushln!(out);
                continue;
            }

            pushln!(out, "    /\\ witness_current_script_input # None");
            writeln!(
                out,
                "    /\\ ~(witness_current_script_input \\in SeqElements(pending_inputs))"
            )
            .expect("write to string");
            pushln!(out, "    /\\ Len(pending_inputs) = 0");
            pushln!(out, "    /\\ Len(pending_routes) = 0");
            pushln!(out, "    /\\ Len(witness_remaining_script_inputs) > 0");
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
            {
                let mut unchanged = self.machine_vars();
                unchanged.extend(self.obligation_vars());
                unchanged.extend([
                    "pending_routes".into(),
                    "delivered_routes".into(),
                    "emitted_effects".into(),
                    "observed_transitions".into(),
                ]);
                writeln!(out, "    /\\ UNCHANGED << {} >>", unchanged.join(", "))
                    .expect("write to string");
            }
            pushln!(out);
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
            Expr::NamedVariant { variant, .. } => tla_string(variant),
            Expr::None => "None".into(),
            Expr::Some(inner) => format!("Some({})", self.render_literal_expr(inner)),
            Expr::SeqLiteral(items) => {
                let rendered = items
                    .iter()
                    .map(|item| self.render_literal_expr(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("<<{rendered}>>")
            }
            _ => "None".into(),
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
            writeln!(out, "{} ==", action_name);
        } else {
            pushln!(out, "{}({}) ==", action_name, params.join(", "));
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

        pushln!(out, "    /\\ \\E packet \\in SeqElements(pending_inputs) :");
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
        writeln!(out, "       /\\ {}", from_guard);

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
                if field_env
                    .get(&field.name)
                    .is_some_and(|current| next != current)
                {
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

        unchanged_machine_vars.push("witness_current_script_input".into());
        unchanged_machine_vars.push("witness_remaining_script_inputs".into());

        if !unchanged_machine_vars.is_empty() {
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
                .map(|(_, input_packet)| input_packet)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let pending_routes_next = self.append_if_missing_chain(
            "pending_routes".into(),
            &queued_route_packets
                .iter()
                .map(|(route_packet, _)| route_packet)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let delivered_routes_next = if immediate_route_packets.is_empty() {
            "delivered_routes".into()
        } else {
            format!(
                "delivered_routes \\cup {{ {} }}",
                immediate_route_packets
                    .iter()
                    .map(|(route_packet, _)| route_packet)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let observed_inputs_next = {
            let immediate_inputs = immediate_route_packets
                .iter()
                .map(|(_, input_packet)| input_packet)
                .cloned()
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
                    .map(|(_, effect_packet, _)| effect_packet)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let observed_transitions_next = format!(
            "observed_transitions \\cup {{{}}}",
            self.transition_packet_expr(instance_id, transition)
        );

        // Compute obligation set updates: when a transition emits an effect
        // that matches a handoff_protocol, add a record to the obligation set
        // with the protocol's declared obligation fields from the effect payload.
        let mut obligation_updates: BTreeMap<String, String> = BTreeMap::new();
        for (effect_variant, _effect_packet, payload_fields) in &effect_packets {
            for protocol in &self.schema.handoff_protocols {
                if protocol.producer_instance == instance_id
                    && protocol.effect_variant == *effect_variant
                {
                    let var = format!("obligation_{}", tla_ident(&protocol.name));
                    let record_fields: Vec<String> = protocol
                        .obligation_fields
                        .iter()
                        .map(|field| {
                            let tla_field = tla_ident(field);
                            let value = payload_fields
                                .get(field)
                                .cloned()
                                .unwrap_or_else(|| format!("\"unknown_{}\"", field));
                            format!("{} |-> {}", tla_field, value)
                        })
                        .collect();
                    let record = if record_fields.is_empty() {
                        "\"token\"".to_string()
                    } else {
                        format!("[{}]", record_fields.join(", "))
                    };
                    obligation_updates.insert(var.clone(), format!("{} \\cup {{{}}}", var, record));
                }
            }
        }

        pushln!(out, "       /\\ pending_inputs' = {}", pending_inputs_next);
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
        // Write obligation variable updates — touched protocols get \cup, untouched get UNCHANGED
        let all_obligation_vars = self.obligation_vars();
        let mut unchanged_obligation_vars = Vec::new();
        for ovar in &all_obligation_vars {
            if let Some(next_expr) = obligation_updates.get(ovar) {
                writeln!(out, "       /\\ {}' = {}", ovar, next_expr).expect("write to string");
            } else {
                unchanged_obligation_vars.push(ovar.clone());
            }
        }
        if !unchanged_obligation_vars.is_empty() {
            writeln!(
                out,
                "       /\\ UNCHANGED << {} >>",
                unchanged_obligation_vars.join(", ")
            )
            .expect("write to string");
        }
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
        let Ok(target_variant) = target_machine.inputs.variant_named(&route.to.input_variant)
        else {
            return String::new();
        };
        let Ok(source_effect_variant) = source_machine.effects.variant_named(effect_variant) else {
            return String::new();
        };
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
                        .unwrap_or_else(|| "None".to_string()),
                    RouteBindingSource::Literal(expr) => {
                        compiler.render_expr_with_types(expr, next_env, binding_env, binding_types)
                    }
                    RouteBindingSource::OwnerProvided => "\"owner_val\"".to_string(),
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

        pushln!(&mut out, "---- MODULE model ----");
        pushln!(&mut out, "EXTENDS TLC, Naturals, Sequences, FiniteSets");
        pushln!(&mut out);
        writeln!(
            &mut out,
            "\\* Generated semantic machine model for {}.",
            self.schema.machine
        )
        .expect("write to string");
        pushln!(&mut out);

        let model_constants = constants
            .iter()
            .filter(|(_, ty)| {
                !matches!(
                    ty,
                    TypeRef::Seq(_) | TypeRef::Option(_) | TypeRef::Map(_, _)
                )
            })
            .map(|(name, _)| name)
            .cloned()
            .collect::<Vec<_>>();
        if !model_constants.is_empty() {
            let constant_list = model_constants.join(", ");
            writeln!(&mut out, "CONSTANTS {constant_list}");
            pushln!(&mut out);
        }

        pushln!(&mut out, "None == [tag |-> \"none\", value |-> \"none\"]");
        writeln!(&mut out, "Some(v) == [tag |-> \"some\", value |-> v]");
        pushln!(&mut out);
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
        for (name, ty) in &constants {
            if let TypeRef::Option(inner) = ty {
                writeln!(
                    &mut out,
                    "{} == {}",
                    name,
                    render_option_domain_definition(inner)
                )
                .expect("write to string");
            }
        }
        for (name, ty) in &constants {
            if let TypeRef::Map(key, value) = ty {
                writeln!(
                    &mut out,
                    "{} == {}",
                    name,
                    render_map_domain_definition(key, value)
                )
                .expect("write to string");
            }
        }
        if constants.values().any(|ty| {
            matches!(
                ty,
                TypeRef::Seq(_) | TypeRef::Option(_) | TypeRef::Map(_, _)
            )
        }) {
            pushln!(&mut out);
        }
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
        pushln!(&mut out);

        let variable_list = std::iter::once("phase".to_string())
            .chain(std::iter::once("model_step_count".to_string()))
            .chain(
                self.schema
                    .state
                    .fields
                    .iter()
                    .map(|field| &field.name)
                    .cloned(),
            )
            .collect::<Vec<_>>()
            .join(", ");
        pushln!(&mut out, "VARIABLES {variable_list}");
        pushln!(&mut out);
        let vars = std::iter::once("phase".to_string())
            .chain(std::iter::once("model_step_count".to_string()))
            .chain(
                self.schema
                    .state
                    .fields
                    .iter()
                    .map(|field| &field.name)
                    .cloned(),
            )
            .collect::<Vec<_>>();
        pushln!(&mut out, "vars == << {} >>", vars.join(", "));
        pushln!(&mut out);

        for helper in helper_dependency_order(self.schema) {
            self.render_helper(&mut out, helper);
        }
        if !self.schema.helpers.is_empty() || !self.schema.derived.is_empty() {
            pushln!(&mut out);
        }

        pushln!(&mut out, "Init ==");
        writeln!(
            &mut out,
            "    /\\ phase = {}",
            tla_string(&self.schema.state.init.phase)
        )
        .expect("write to string");
        pushln!(&mut out, "    /\\ model_step_count = 0");
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
            pushln!(&mut out, "    /\\ {} = {}", field.name, init_expr);
        }
        pushln!(&mut out);

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
            pushln!(&mut out, "TerminalStutter ==");
            pushln!(&mut out, "    /\\ {}", terminal_guard);
            pushln!(&mut out, "    /\\ UNCHANGED vars");
            pushln!(&mut out);
        }

        if !self.helper_defs.is_empty() {
            for helper in &self.helper_defs {
                pushln!(&mut out, "{helper}");
                pushln!(&mut out);
            }
        }

        for transition in &rendered_transitions {
            pushln!(&mut out, "{transition}");
            pushln!(&mut out);
        }

        pushln!(&mut out, "Next ==");
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
                    let Some(ty) = binding_types.get(binding) else {
                        return String::new();
                    };
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
            pushln!(&mut out, "    \\/ {}", call);
        }
        if !self.schema.state.terminal_phases.is_empty() {
            pushln!(&mut out, "    \\/ TerminalStutter");
        }
        pushln!(&mut out);

        for invariant in &self.schema.invariants {
            writeln!(
                &mut out,
                "{} == {}",
                invariant.name,
                self.render_expr(&invariant.expr, &BTreeMap::new(), &BTreeMap::new())
            )
            .expect("write to string");
        }
        pushln!(&mut out);

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
        pushln!(&mut out);

        pushln!(&mut out, "Spec == Init /\\ [][Next]_vars");
        pushln!(&mut out);
        for invariant in &self.schema.invariants {
            pushln!(&mut out, "THEOREM Spec => []{}", invariant.name);
        }
        pushln!(&mut out);
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
            writeln!(out, "{} ==", transition.name);
        } else {
            pushln!(out, "{}({}) ==", transition.name, params);
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
        pushln!(out, "    /\\ {}", from_guard);

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

        pushln!(out, "    /\\ phase' = {}", tla_string(&transition.to));
        pushln!(out, "    /\\ model_step_count' = model_step_count + 1");
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
            pushln!(out, "    /\\ {}' = {}", field, expr);
        }

        let unchanged = self
            .schema
            .state
            .fields
            .iter()
            .filter(|field| !touched.iter().any(|(name, _)| *name == field.name))
            .map(|field| &field.name)
            .cloned()
            .collect::<Vec<_>>();
        if !unchanged.is_empty() {
            pushln!(out, "    /\\ UNCHANGED << {} >>", unchanged.join(", "));
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
            TypeRef::Named(_)
            | TypeRef::Enum(_)
            | TypeRef::Seq(_)
            | TypeRef::Set(_)
            | TypeRef::Option(_)
            | TypeRef::Map(_, _) => domain_constant_name(ty),
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
            Expr::NamedVariant { variant, .. } => tla_string(variant),
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
            Expr::SeqElements(inner) => format!(
                "SeqElements({})",
                self.render_expr_with_types(inner, env, binding_env, binding_types)
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
                    "({} {} \\in {} : {})",
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
            Expr::NamedVariant { enum_name, .. } => Some(TypeRef::Named(enum_name.clone())),
            Expr::Field(name) => self
                .schema
                .state
                .fields
                .iter()
                .find(|field| field.name == *name)
                .map(|field| &field.ty)
                .cloned(),
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
        | Expr::SeqElements(inner)
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
        | Expr::NamedVariant { .. }
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
        | Expr::SeqElements(inner)
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
        | Expr::NamedVariant { .. }
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
        TypeRef::Named(name) | TypeRef::Enum(name) => {
            tla_string(&format!("{}_default", tla_ident(name).to_lowercase()))
        }
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
        TypeRef::String | TypeRef::Named(_) | TypeRef::Enum(_) => tla_string("None"),
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
        TypeRef::Named(name) | TypeRef::Enum(name) => name.clone(),
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
        CompositionInvariantKind::HandoffProtocolCovered { .. } => {
            // Structural invariant — validated at schema level, always TRUE in TLA+.
            "TRUE".into()
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
    writeln!(out, "{fn_name}({param}) ==");
    if mappings.is_empty() {
        pushln!(out, "    {}", default_expr);
        pushln!(out);
        return;
    }

    let mut first = true;
    for (key, value) in mappings {
        if first {
            pushln!(out, "    CASE {} = {} -> {}", param, tla_string(key), value);
            first = false;
        } else {
            writeln!(out, "      [] {} = {} -> {}", param, tla_string(key), value)
                .expect("write to string");
        }
    }
    writeln!(out, "      [] OTHER -> {}", default_expr);
    pushln!(out);
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

use std::fmt::Write;

use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionInvariantKind, CompositionSchema, EffectEmit,
    EnumSchema, Expr, FieldInit, FieldSchema, Guard, HelperSchema, MachineCoverageManifest,
    MachineSchema, Quantifier, RouteDelivery, SchedulerRule, SemanticCoverageEntry,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub const GENERATED_COVERAGE_START: &str = "<!-- GENERATED_COVERAGE_START -->";
pub const GENERATED_COVERAGE_END: &str = "<!-- GENERATED_COVERAGE_END -->";

pub fn render_machine_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let module_name = format!("Machine_{}", tla_ident(&schema.machine));

    writeln!(&mut out, "---- MODULE {} ----", module_name).expect("write to string");
    writeln!(
        &mut out,
        "(* machine = {} ; version = {} *)",
        schema.machine, schema.version
    )
    .expect("write to string");
    writeln!(
        &mut out,
        "(* rust = {}::{} *)",
        schema.rust.crate_name, schema.rust.module
    )
    .expect("write to string");
    out.push('\n');

    writeln!(&mut out, "STATE").expect("write to string");
    writeln!(
        &mut out,
        "  phase : {}",
        render_enum_variants_inline(&schema.state.phase)
    )
    .expect("write to string");
    for field in &schema.state.fields {
        writeln!(
            &mut out,
            "  {} : {}",
            field.name,
            render_type_ref(&field.ty)
        )
        .expect("write to string");
    }
    out.push('\n');

    writeln!(&mut out, "INIT").expect("write to string");
    writeln!(
        &mut out,
        "  phase = {}",
        tla_string(&schema.state.init.phase)
    )
    .expect("write to string");
    for field in &schema.state.init.fields {
        render_field_init(&mut out, field);
    }
    if !schema.state.terminal_phases.is_empty() {
        writeln!(
            &mut out,
            "  terminal_phases = {}",
            render_string_list(&schema.state.terminal_phases)
        )
        .expect("write to string");
    }
    out.push('\n');

    render_enum_section(&mut out, "INPUTS", &schema.inputs);
    render_enum_section(&mut out, "EFFECTS", &schema.effects);

    render_helpers_section(&mut out, "HELPERS", &schema.helpers);
    render_helpers_section(&mut out, "DERIVED", &schema.derived);
    render_invariants_section(&mut out, schema);
    render_transitions_section(&mut out, schema);

    writeln!(&mut out, "====").expect("write to string");
    out
}

pub fn render_composition_module(schema: &CompositionSchema) -> String {
    let mut out = String::new();
    let module_name = format!("Composition_{}", tla_ident(&schema.name));

    writeln!(&mut out, "---- MODULE {} ----", module_name).expect("write to string");
    writeln!(&mut out, "(* composition = {} *)", schema.name).expect("write to string");
    out.push('\n');

    writeln!(&mut out, "MACHINES").expect("write to string");
    for machine in &schema.machines {
        writeln!(
            &mut out,
            "  {} : {} @ actor {}",
            machine.instance_id, machine.machine_name, machine.actor
        )
        .expect("write to string");
    }
    out.push('\n');

    writeln!(&mut out, "ROUTES").expect("write to string");
    for route in &schema.routes {
        writeln!(
            &mut out,
            "  {} == {}.{} -> {}.{} [{}]",
            route.name,
            route.from_machine,
            route.effect_variant,
            route.to.machine,
            route.to.input_variant,
            render_route_delivery(&route.delivery)
        )
        .expect("write to string");
        if !route.bindings.is_empty() {
            writeln!(&mut out, "    bindings = {}", render_route_bindings(route))
                .expect("write to string");
        }
    }
    out.push('\n');

    writeln!(&mut out, "ACTOR_PRIORITIES").expect("write to string");
    for priority in &schema.actor_priorities {
        writeln!(
            &mut out,
            "  {} > {} ; {}",
            priority.higher, priority.lower, priority.reason
        )
        .expect("write to string");
    }
    out.push('\n');

    writeln!(&mut out, "SCHEDULER_RULES").expect("write to string");
    for rule in &schema.scheduler_rules {
        writeln!(&mut out, "  {}", render_scheduler_rule(rule)).expect("write to string");
    }
    out.push('\n');

    writeln!(&mut out, "INVARIANTS").expect("write to string");
    for invariant in &schema.invariants {
        writeln!(
            &mut out,
            "  {} == {}",
            invariant.name,
            render_composition_invariant_kind(&invariant.kind)
        )
        .expect("write to string");
        writeln!(
            &mut out,
            "    statement = {}",
            tla_string(&invariant.statement)
        )
        .expect("write to string");
    }

    writeln!(&mut out, "====").expect("write to string");
    out
}

pub fn render_machine_mapping_coverage(
    schema: &MachineSchema,
    coverage: &MachineCoverageManifest,
) -> String {
    let mut out = String::new();

    writeln!(&mut out, "## Generated Coverage").expect("write to string");
    writeln!(
        &mut out,
        "This section is generated from the Rust machine catalog. Do not edit it by hand."
    )
    .expect("write to string");
    out.push('\n');

    writeln!(&mut out, "### Machine").expect("write to string");
    writeln!(&mut out, "- `{}`", schema.machine).expect("write to string");
    out.push('\n');

    writeln!(&mut out, "### Code Anchors").expect("write to string");
    for anchor in &coverage.code_anchors {
        writeln!(
            &mut out,
            "- `{}`: `{}` — {}",
            anchor.id, anchor.path, anchor.note
        )
        .expect("write to string");
    }
    out.push('\n');

    writeln!(&mut out, "### Scenarios").expect("write to string");
    for scenario in &coverage.scenarios {
        writeln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary).expect("write to string");
    }
    out.push('\n');

    render_semantic_mapping_section(&mut out, "Transitions", &coverage.transition_coverage);
    render_semantic_mapping_section(&mut out, "Effects", &coverage.effect_coverage);
    render_semantic_mapping_section(&mut out, "Invariants", &coverage.invariant_coverage);

    out
}

pub fn render_composition_mapping_coverage(
    schema: &CompositionSchema,
    coverage: &CompositionCoverageManifest,
) -> String {
    let mut out = String::new();

    writeln!(&mut out, "## Generated Coverage").expect("write to string");
    writeln!(
        &mut out,
        "This section is generated from the Rust composition catalog. Do not edit it by hand."
    )
    .expect("write to string");
    out.push('\n');

    writeln!(&mut out, "### Composition").expect("write to string");
    writeln!(&mut out, "- `{}`", schema.name).expect("write to string");
    out.push('\n');

    writeln!(&mut out, "### Code Anchors").expect("write to string");
    for anchor in &coverage.code_anchors {
        writeln!(
            &mut out,
            "- `{}`: `{}` — {}",
            anchor.id, anchor.path, anchor.note
        )
        .expect("write to string");
    }
    out.push('\n');

    writeln!(&mut out, "### Scenarios").expect("write to string");
    for scenario in &coverage.scenarios {
        writeln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary).expect("write to string");
    }
    out.push('\n');

    render_semantic_mapping_section(&mut out, "Routes", &coverage.route_coverage);
    render_semantic_mapping_section(
        &mut out,
        "Scheduler Rules",
        &coverage.scheduler_rule_coverage,
    );
    render_semantic_mapping_section(&mut out, "Invariants", &coverage.invariant_coverage);

    out
}

pub fn render_machine_kernel_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let module_name = machine_slug(&schema.machine);
    let catalog_fn = format!("{module_name}_machine");

    writeln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    )
    .expect("write");
    writeln!(
        &mut out,
        "use crate::runtime::{{GeneratedMachineKernel, KernelInput, KernelState, TransitionOutcome, TransitionRefusal}};"
    )
    .expect("write");
    writeln!(&mut out).expect("write");
    writeln!(
        &mut out,
        "pub fn schema() -> meerkat_machine_schema::MachineSchema {{"
    )
    .expect("write");
    writeln!(&mut out, "    meerkat_machine_schema::{catalog_fn}()").expect("write");
    writeln!(&mut out, "}}").expect("write");
    writeln!(&mut out).expect("write");
    writeln!(
        &mut out,
        "pub fn kernel() -> GeneratedMachineKernel {{ GeneratedMachineKernel::new(schema()) }}"
    )
    .expect("write");
    writeln!(&mut out).expect("write");
    writeln!(
        &mut out,
        "pub fn initial_state() -> Result<KernelState, TransitionRefusal> {{ kernel().initial_state() }}"
    )
    .expect("write");
    writeln!(&mut out).expect("write");
    writeln!(&mut out, "pub fn transition(").expect("write");
    writeln!(&mut out, "    state: &KernelState,").expect("write");
    writeln!(&mut out, "    input: &KernelInput,").expect("write");
    writeln!(
        &mut out,
        ") -> Result<TransitionOutcome, TransitionRefusal> {{"
    )
    .expect("write");
    writeln!(&mut out, "    kernel().transition(state, input)").expect("write");
    writeln!(&mut out, "}}").expect("write");

    out
}

pub fn render_generated_kernel_mod(schemas: &[MachineSchema]) -> String {
    let mut out = String::new();
    writeln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    )
    .expect("write");
    for schema in schemas {
        writeln!(&mut out, "pub mod {};", machine_slug(&schema.machine)).expect("write");
    }
    out.push('\n');
    writeln!(&mut out, "use crate::runtime::GeneratedMachineKernel;").expect("write");
    writeln!(&mut out).expect("write");
    writeln!(
        &mut out,
        "pub fn all_kernels() -> Vec<GeneratedMachineKernel> {{"
    )
    .expect("write");
    writeln!(&mut out, "    vec![").expect("write");
    for schema in schemas {
        let slug = machine_slug(&schema.machine);
        writeln!(&mut out, "        {slug}::kernel(),").expect("write");
    }
    writeln!(&mut out, "    ]").expect("write");
    writeln!(&mut out, "}}").expect("write");
    out
}

fn render_semantic_mapping_section(
    out: &mut String,
    heading: &str,
    entries: &[SemanticCoverageEntry],
) {
    writeln!(out, "### {heading}").expect("write to string");
    if entries.is_empty() {
        writeln!(out, "- `(none)`").expect("write to string");
        out.push('\n');
        return;
    }

    for entry in entries {
        writeln!(out, "- `{}`", entry.name).expect("write to string");
        writeln!(
            out,
            "  - anchors: {}",
            entry
                .anchor_ids
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("write to string");
        writeln!(
            out,
            "  - scenarios: {}",
            entry
                .scenario_ids
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("write to string");
    }
    out.push('\n');
}

fn machine_slug(machine_name: &str) -> String {
    let trimmed = machine_name.strip_suffix("Machine").unwrap_or(machine_name);
    to_snake_case(trimmed)
}

fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    let mut previous_is_sep = true;

    for ch in value.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            if !previous_is_sep {
                out.push('_');
                previous_is_sep = true;
            }
            continue;
        }

        if ch.is_ascii_uppercase() {
            if !out.is_empty() && !previous_is_sep {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            previous_is_sep = false;
        } else {
            out.push(ch.to_ascii_lowercase());
            previous_is_sep = false;
        }
    }

    out.trim_matches('_').to_owned()
}

pub fn merge_mapping_document(existing: Option<&str>, title: &str, generated: &str) -> String {
    let generated_block =
        format!("{GENERATED_COVERAGE_START}\n{generated}\n{GENERATED_COVERAGE_END}\n");

    if let Some(existing) = existing {
        if let Some(start) = existing.find(GENERATED_COVERAGE_START) {
            if let Some(end) = existing.find(GENERATED_COVERAGE_END) {
                let end_idx = end + GENERATED_COVERAGE_END.len();
                let before = existing[..start].trim_end();
                let after = existing[end_idx..].trim_start();
                return if after.is_empty() {
                    format!("{before}\n\n{generated_block}")
                } else {
                    format!("{before}\n\n{generated_block}\n{after}")
                };
            }
        }

        let trimmed = existing.trim_end();
        if trimmed.is_empty() {
            return generated_block;
        }
        return format!("{trimmed}\n\n{generated_block}");
    }

    format!("# {title} Mapping Note\n\n{generated_block}")
}

fn render_enum_section(out: &mut String, label: &str, schema: &EnumSchema) {
    writeln!(out, "{label}").expect("write to string");
    writeln!(
        out,
        "  {} = {}",
        schema.name,
        render_enum_variants_inline(schema)
    )
    .expect("write to string");
    for variant in &schema.variants {
        writeln!(
            out,
            "  {} == {}",
            variant.name,
            render_variant_fields(variant)
        )
        .expect("write to string");
    }
    out.push('\n');
}

fn render_helpers_section(out: &mut String, label: &str, helpers: &[HelperSchema]) {
    if helpers.is_empty() {
        return;
    }
    writeln!(out, "{label}").expect("write to string");
    for helper in helpers {
        writeln!(
            out,
            "  {}({}) : {}",
            helper.name,
            render_fields(&helper.params),
            render_type_ref(&helper.returns)
        )
        .expect("write to string");
        writeln!(out, "    == {}", render_expr(&helper.body)).expect("write to string");
    }
    out.push('\n');
}

fn render_invariants_section(out: &mut String, schema: &MachineSchema) {
    writeln!(out, "INVARIANTS").expect("write to string");
    for invariant in &schema.invariants {
        writeln!(
            out,
            "  {} == {}",
            invariant.name,
            render_expr(&invariant.expr)
        )
        .expect("write to string");
    }
    out.push('\n');
}

fn render_transitions_section(out: &mut String, schema: &MachineSchema) {
    writeln!(out, "TRANSITIONS").expect("write to string");
    for transition in &schema.transitions {
        render_transition(out, transition);
    }
    out.push('\n');
}

fn render_transition(out: &mut String, transition: &TransitionSchema) {
    writeln!(out, "  {}", transition.name).expect("write to string");
    writeln!(out, "    from = {}", render_string_list(&transition.from)).expect("write to string");
    writeln!(
        out,
        "    on = {}({})",
        transition.on.variant,
        transition.on.bindings.join(", ")
    )
    .expect("write to string");
    if !transition.guards.is_empty() {
        writeln!(out, "    guards =").expect("write to string");
        for guard in &transition.guards {
            render_guard(out, guard);
        }
    }
    if !transition.updates.is_empty() {
        writeln!(out, "    updates =").expect("write to string");
        for update in &transition.updates {
            writeln!(out, "      - {}", render_update(update)).expect("write to string");
        }
    }
    writeln!(out, "    to = {}", tla_string(&transition.to)).expect("write to string");
    if !transition.emit.is_empty() {
        writeln!(out, "    emit =").expect("write to string");
        for effect in &transition.emit {
            writeln!(out, "      - {}", render_effect_emit(effect)).expect("write to string");
        }
    }
}

fn render_guard(out: &mut String, guard: &Guard) {
    writeln!(
        out,
        "      - {} == {}",
        guard.name,
        render_expr(&guard.expr)
    )
    .expect("write to string");
}

fn render_field_init(out: &mut String, field: &FieldInit) {
    writeln!(out, "  {} = {}", field.field, render_expr(&field.expr)).expect("write to string");
}

fn render_variant_fields(variant: &VariantSchema) -> String {
    if variant.fields.is_empty() {
        "[]".to_owned()
    } else {
        format!("[{}]", render_fields(&variant.fields))
    }
}

fn render_fields(fields: &[FieldSchema]) -> String {
    fields
        .iter()
        .map(|field| format!("{}: {}", field.name, render_type_ref(&field.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_update(update: &Update) -> String {
    match update {
        Update::Assign { field, expr } => format!("{field}' = {}", render_expr(expr)),
        Update::Increment { field, amount } => format!("{field}' = {field} + {amount}"),
        Update::Decrement { field, amount } => format!("{field}' = {field} - {amount}"),
        Update::MapInsert { field, key, value } => {
            format!(
                "{field}' = [ {field} EXCEPT ![{}] = {} ]",
                render_expr(key),
                render_expr(value)
            )
        }
        Update::SetInsert { field, value } => {
            format!("{field}' = {field} \\cup {{ {} }}", render_expr(value))
        }
        Update::SetRemove { field, value } => {
            format!("{field}' = {field} \\ {{ {} }}", render_expr(value))
        }
        Update::SeqAppend { field, value } => {
            format!("{field}' = Append({field}, {})", render_expr(value))
        }
        Update::SeqPrepend { field, values } => {
            format!("{field}' = {} \\o {field}", render_expr(values))
        }
        Update::SeqPopFront { field } => {
            format!("{field}' = Tail({field})")
        }
        Update::SeqRemoveValue { field, value } => {
            format!("{field}' = SeqRemove({field}, {})", render_expr(value))
        }
        Update::SeqRemoveAll { field, values } => {
            format!("{field}' = SeqRemoveAll({field}, {})", render_expr(values))
        }
        Update::ForEach {
            binding,
            over,
            updates,
        } => {
            let body = updates
                .iter()
                .map(render_update)
                .collect::<Vec<_>>()
                .join("; ");
            format!("ForEach({binding} \\in {}) {{ {body} }}", render_expr(over))
        }
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            let then_body = then_updates
                .iter()
                .map(render_update)
                .collect::<Vec<_>>()
                .join("; ");
            let else_body = else_updates
                .iter()
                .map(render_update)
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "If ({}) {{ {} }} Else {{ {} }}",
                render_expr(condition),
                then_body,
                else_body
            )
        }
    }
}

fn render_effect_emit(effect: &EffectEmit) -> String {
    if effect.fields.is_empty() {
        effect.variant.clone()
    } else {
        let fields = effect
            .fields
            .iter()
            .map(|(name, expr)| format!("{name} |-> {}", render_expr(expr)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}([{}])", effect.variant, fields)
    }
}

fn render_expr(expr: &Expr) -> String {
    match expr {
        Expr::Bool(value) => value.to_string().to_uppercase(),
        Expr::U64(value) => value.to_string(),
        Expr::String(value) => tla_string(value),
        Expr::EmptySet => "{}".to_owned(),
        Expr::EmptyMap => "[x \\in {} |-> None]".to_owned(),
        Expr::SeqLiteral(items) => format!(
            "<<{}>>",
            items.iter().map(render_expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::CurrentPhase => "phase".to_owned(),
        Expr::Phase(value) => tla_string(value),
        Expr::Field(name) => name.clone(),
        Expr::Binding(name) => name.clone(),
        Expr::Variant(name) => name.clone(),
        Expr::None => "None".to_owned(),
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => format!(
            "IF {} THEN {} ELSE {}",
            render_expr(condition),
            render_expr(then_expr),
            render_expr(else_expr)
        ),
        Expr::Not(inner) => format!("~({})", render_expr(inner)),
        Expr::And(items) => join_exprs(items, " /\\ "),
        Expr::Or(items) => join_exprs(items, " \\/ "),
        Expr::Eq(left, right) => format!("{} = {}", render_expr(left), render_expr(right)),
        Expr::Neq(left, right) => format!("{} # {}", render_expr(left), render_expr(right)),
        Expr::Add(left, right) => format!("{} + {}", render_expr(left), render_expr(right)),
        Expr::Sub(left, right) => format!("{} - {}", render_expr(left), render_expr(right)),
        Expr::Gt(left, right) => format!("{} > {}", render_expr(left), render_expr(right)),
        Expr::Gte(left, right) => format!("{} >= {}", render_expr(left), render_expr(right)),
        Expr::Lt(left, right) => format!("{} < {}", render_expr(left), render_expr(right)),
        Expr::Lte(left, right) => format!("{} <= {}", render_expr(left), render_expr(right)),
        Expr::Contains { collection, value } => {
            format!("{} \\in {}", render_expr(value), render_expr(collection))
        }
        Expr::SeqStartsWith { seq, prefix } => {
            format!("StartsWith({}, {})", render_expr(seq), render_expr(prefix))
        }
        Expr::Len(inner) => format!("Len({})", render_expr(inner)),
        Expr::Head(inner) => format!("Head({})", render_expr(inner)),
        Expr::MapKeys(inner) => format!("DOMAIN {}", render_expr(inner)),
        Expr::MapGet { map, key } => format!("{}[{}]", render_expr(map), render_expr(key)),
        Expr::Some(inner) => format!("Some({})", render_expr(inner)),
        Expr::Call { helper, args } => format!(
            "{}({})",
            helper,
            args.iter().map(render_expr).collect::<Vec<_>>().join(", ")
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
            format!(
                "({} {} \\in {} : {})",
                keyword,
                binding,
                render_expr(over),
                render_expr(body)
            )
        }
    }
}

fn join_exprs(items: &[Expr], separator: &str) -> String {
    if items.is_empty() {
        return "TRUE".to_owned();
    }
    items
        .iter()
        .map(render_expr)
        .collect::<Vec<_>>()
        .join(separator)
}

fn render_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "Bool".to_owned(),
        TypeRef::U32 => "Nat".to_owned(),
        TypeRef::U64 => "Nat".to_owned(),
        TypeRef::String => "String".to_owned(),
        TypeRef::Named(name) => name.clone(),
        TypeRef::Option(inner) => format!("Option({})", render_type_ref(inner)),
        TypeRef::Set(inner) => format!("Set({})", render_type_ref(inner)),
        TypeRef::Seq(inner) => format!("Seq({})", render_type_ref(inner)),
        TypeRef::Map(key, value) => {
            format!("Map({}, {})", render_type_ref(key), render_type_ref(value))
        }
    }
}

fn render_enum_variants_inline(schema: &EnumSchema) -> String {
    let variants = schema
        .variants
        .iter()
        .map(|variant| tla_string(&variant.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{variants}}}")
}

fn render_string_list(items: &[String]) -> String {
    let rendered = items
        .iter()
        .map(|item| tla_string(item))
        .collect::<Vec<_>>()
        .join(", ");
    format!("<<{rendered}>>")
}

fn render_route_delivery(delivery: &RouteDelivery) -> &'static str {
    match delivery {
        RouteDelivery::Immediate => "Immediate",
        RouteDelivery::Enqueue => "Enqueue",
    }
}

fn render_route_bindings(route: &meerkat_machine_schema::Route) -> String {
    route
        .bindings
        .iter()
        .map(|binding| match &binding.source {
            meerkat_machine_schema::RouteBindingSource::Field {
                from_field,
                allow_named_alias,
            } => {
                if *allow_named_alias {
                    format!("{from_field} ~> {}", binding.to_field)
                } else {
                    format!("{from_field} -> {}", binding.to_field)
                }
            }
            meerkat_machine_schema::RouteBindingSource::Literal(expr) => {
                format!("{} := {}", binding.to_field, render_expr(expr))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_scheduler_rule(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!("PreemptWhenReady({}, {})", higher, lower)
        }
    }
}

fn render_composition_invariant_kind(kind: &CompositionInvariantKind) -> String {
    match kind {
        CompositionInvariantKind::RoutePresent {
            from_machine,
            effect_variant,
            to_machine,
            input_variant,
        } => format!(
            "RoutePresent({}, {}, {}, {})",
            from_machine, effect_variant, to_machine, input_variant
        ),
        CompositionInvariantKind::ActorPriorityPresent { higher, lower } => {
            format!("ActorPriorityPresent({}, {})", higher, lower)
        }
        CompositionInvariantKind::ObservedInputOriginatesFromEffect {
            to_machine,
            input_variant,
            from_machine,
            effect_variant,
        } => format!(
            "ObservedInputOriginatesFromEffect({}, {}, {}, {})",
            to_machine, input_variant, from_machine, effect_variant
        ),
        CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
            route_name,
            to_machine,
            input_variant,
            from_machine,
            effect_variant,
        } => format!(
            "ObservedRouteInputOriginatesFromEffect({}, {}, {}, {}, {})",
            route_name, to_machine, input_variant, from_machine, effect_variant
        ),
        CompositionInvariantKind::SchedulerRulePresent { rule } => {
            format!("SchedulerRulePresent({})", render_scheduler_rule(rule))
        }
        CompositionInvariantKind::OutcomeHandled {
            from_machine,
            effect_variant,
            required_targets,
        } => format!(
            "OutcomeHandled({}, {}, {})",
            from_machine,
            effect_variant,
            required_targets
                .iter()
                .map(|target| format!("{}.{}", target.machine, target.input_variant))
                .collect::<Vec<_>>()
                .join(" | ")
        ),
    }
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

#[cfg(test)]
mod tests {
    use super::{
        GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
        render_composition_mapping_coverage, render_composition_module,
        render_machine_mapping_coverage, render_machine_module,
    };
    use meerkat_machine_schema::catalog::{
        canonical_composition_coverage_manifests, canonical_machine_coverage_manifests,
        mob_orchestrator_machine, peer_runtime_bundle_composition, runtime_control_machine,
        runtime_pipeline_composition, turn_execution_machine,
    };

    #[test]
    fn renders_machine_fixture_with_stable_sections() {
        let rendered = render_machine_module(&mob_orchestrator_machine());

        assert!(rendered.starts_with("---- MODULE Machine_MobOrchestratorMachine ----"));
        assert!(rendered.contains("STATE\n  phase : {\"Creating\", \"Running\", \"Stopped\", \"Completed\", \"Destroyed\"}"));
        assert!(rendered.contains("INPUTS\n  MobOrchestratorInput = {\"InitializeOrchestrator\", \"BindCoordinator\", \"UnbindCoordinator\", \"StageSpawn\", \"CompleteSpawn\", \"StartFlow\", \"CompleteFlow\", \"StopOrchestrator\", \"ResumeOrchestrator\", \"MarkCompleted\", \"DestroyOrchestrator\"}"));
        assert!(rendered.contains("TRANSITIONS\n  InitializeOrchestrator"));
        assert!(rendered.ends_with("====\n"));
    }

    #[test]
    fn renders_composition_fixture_with_routes_and_scheduler_rules() {
        let rendered = render_composition_module(&runtime_pipeline_composition());

        assert!(rendered.starts_with("---- MODULE Composition_runtime_pipeline ----"));
        assert!(rendered.contains(
            "staged_run_notifies_control == runtime_ingress.ReadyForRun -> runtime_control.BeginRun [Immediate]"
        ));
        assert!(
            rendered
                .contains("SCHEDULER_RULES\n  PreemptWhenReady(control_plane, ordinary_ingress)")
        );
        assert!(rendered.contains("execution_failure_is_handled =="));
        assert!(rendered.ends_with("====\n"));
    }

    #[test]
    fn renders_runtime_control_with_transition_content() {
        let rendered = render_machine_module(&runtime_control_machine());

        assert!(rendered.contains("TRANSITIONS\n  Initialize"));
        assert!(rendered.contains("BeginRunFromIdle"));
        assert!(rendered.contains("AdmissionAcceptedIdleSteer"));
        assert!(rendered.contains("RetireRequestedFromIdle"));
    }

    #[test]
    fn renders_turn_execution_with_boundary_and_terminal_transitions() {
        let rendered = render_machine_module(&turn_execution_machine());

        assert!(rendered.contains("TRANSITIONS\n  StartConversationRun"));
        assert!(rendered.contains("PrimitiveAppliedConversationTurn"));
        assert!(rendered.contains("BoundaryComplete"));
        assert!(rendered.contains("AcknowledgeTerminalFromCancelled"));
        assert!(rendered.contains(
            "BoundaryApplied([run_id |-> run_id, boundary_sequence |-> boundary_count + 1])"
        ));
    }

    #[test]
    fn renders_route_aliases_and_literals_in_compositions() {
        let coverage = canonical_composition_coverage_manifests()
            .into_iter()
            .find(|item| item.composition == "peer_runtime_bundle")
            .expect("peer runtime coverage");
        let rendered = render_composition_module(&peer_runtime_bundle_composition());
        let mapping =
            render_composition_mapping_coverage(&peer_runtime_bundle_composition(), &coverage);

        assert!(rendered.contains(
            "peer_candidate_enters_runtime_admission == peer_comms.SubmitPeerInputCandidate -> runtime_control.SubmitWork [Immediate]"
        ));
        assert!(rendered.contains("raw_item_id ~> work_id"));
        assert!(rendered.contains("handling_mode := \"Steer\""));
        assert!(mapping.contains("### Code Anchors"));
        assert!(mapping.contains("peer-message-admission"));
    }

    #[test]
    fn renders_machine_mapping_coverage_with_named_items() {
        let coverage = canonical_machine_coverage_manifests()
            .into_iter()
            .find(|item| item.machine == "RuntimeControlMachine")
            .expect("runtime control coverage");
        let rendered = render_machine_mapping_coverage(&runtime_control_machine(), &coverage);

        assert!(rendered.contains("## Generated Coverage"));
        assert!(rendered.contains("### Code Anchors"));
        assert!(rendered.contains("### Scenarios"));
        assert!(rendered.contains("### Transitions"));
        assert!(rendered.contains("- `Initialize`"));
        assert!(rendered.contains("- `ResolveAdmission`"));
        assert!(rendered.contains("- `running_implies_active_run`"));
    }

    #[test]
    fn renders_composition_mapping_coverage_with_routes_and_scheduler_rules() {
        let coverage = canonical_composition_coverage_manifests()
            .into_iter()
            .find(|item| item.composition == "runtime_pipeline")
            .expect("runtime pipeline coverage");
        let rendered =
            render_composition_mapping_coverage(&runtime_pipeline_composition(), &coverage);

        assert!(rendered.contains("### Code Anchors"));
        assert!(rendered.contains("### Scenarios"));
        assert!(rendered.contains("### Routes"));
        assert!(rendered.contains("- `staged_run_notifies_control`"));
        assert!(rendered.contains("### Scheduler Rules"));
        assert!(rendered.contains("- `PreemptWhenReady(control_plane, ordinary_ingress)`"));
        assert!(rendered.contains("- `execution_failure_is_handled`"));
    }

    #[test]
    fn merges_mapping_document_by_appending_generated_block() {
        let coverage = canonical_machine_coverage_manifests()
            .into_iter()
            .find(|item| item.machine == "RuntimeControlMachine")
            .expect("runtime control coverage");
        let merged = merge_mapping_document(
            Some("# RuntimeControlMachine Mapping Note\n\nManual text."),
            "RuntimeControlMachine",
            &render_machine_mapping_coverage(&runtime_control_machine(), &coverage),
        );

        assert!(merged.contains("Manual text."));
        assert!(merged.contains(GENERATED_COVERAGE_START));
        assert!(merged.contains("- `Initialize`"));
        assert!(merged.contains(GENERATED_COVERAGE_END));
    }

    #[test]
    fn merges_mapping_document_by_replacing_existing_generated_block() {
        let coverage = canonical_machine_coverage_manifests()
            .into_iter()
            .find(|item| item.machine == "RuntimeControlMachine")
            .expect("runtime control coverage");
        let existing = format!(
            "# RuntimeControlMachine Mapping Note\n\nManual text.\n\n{GENERATED_COVERAGE_START}\nold block\n{GENERATED_COVERAGE_END}\n"
        );
        let merged = merge_mapping_document(
            Some(&existing),
            "RuntimeControlMachine",
            &render_machine_mapping_coverage(&runtime_control_machine(), &coverage),
        );

        assert!(!merged.contains("old block"));
        assert!(merged.contains("Manual text."));
        assert!(merged.contains("- `BeginRunFromIdle`"));
    }
}

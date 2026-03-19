use std::fmt::Write;

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

#[cfg(not(test))]
use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionInvariantKind, CompositionSchema,
    MachineCoverageManifest, RouteDelivery, SchedulerRule, SemanticCoverageEntry,
};
use meerkat_machine_schema::{
    EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema, Guard, HelperSchema, MachineSchema,
    Quantifier, TransitionSchema, TypeRef, Update, VariantSchema,
};

#[cfg(not(test))]
pub const GENERATED_COVERAGE_START: &str = "<!-- GENERATED_COVERAGE_START -->";
#[cfg(not(test))]
pub const GENERATED_COVERAGE_END: &str = "<!-- GENERATED_COVERAGE_END -->";

pub fn render_machine_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let module_name = format!("Machine_{}", tla_ident(&schema.machine));

    pushln!(&mut out, "---- MODULE {module_name} ----");
    pushln!(
        &mut out,
        "(* machine = {} ; version = {} *)",
        schema.machine,
        schema.version
    );
    pushln!(
        &mut out,
        "(* rust = {}::{} *)",
        schema.rust.crate_name,
        schema.rust.module
    );
    out.push('\n');

    pushln!(&mut out, "STATE");
    pushln!(
        &mut out,
        "  phase : {}",
        render_enum_variants_inline(&schema.state.phase)
    );
    for field in &schema.state.fields {
        pushln!(
            &mut out,
            "  {} : {}",
            field.name,
            render_type_ref(&field.ty)
        );
    }
    out.push('\n');

    pushln!(&mut out, "INIT");
    pushln!(
        &mut out,
        "  phase = {}",
        tla_string(&schema.state.init.phase)
    );
    for field in &schema.state.init.fields {
        render_field_init(&mut out, field);
    }
    if !schema.state.terminal_phases.is_empty() {
        pushln!(
            &mut out,
            "  terminal_phases = {}",
            render_string_list(&schema.state.terminal_phases)
        );
    }
    out.push('\n');

    render_enum_section(&mut out, "INPUTS", &schema.inputs);
    render_enum_section(&mut out, "EFFECTS", &schema.effects);

    render_helpers_section(&mut out, "HELPERS", &schema.helpers);
    render_helpers_section(&mut out, "DERIVED", &schema.derived);
    render_invariants_section(&mut out, schema);
    render_transitions_section(&mut out, schema);

    pushln!(&mut out, "====");
    out
}

#[cfg(not(test))]
pub fn render_composition_module(schema: &CompositionSchema) -> String {
    let mut out = String::new();
    let module_name = format!("Composition_{}", tla_ident(&schema.name));

    pushln!(&mut out, "---- MODULE {module_name} ----");
    pushln!(&mut out, "(* composition = {} *)", schema.name);
    out.push('\n');

    pushln!(&mut out, "MACHINES");
    for machine in &schema.machines {
        pushln!(
            &mut out,
            "  {} : {} @ actor {}",
            machine.instance_id,
            machine.machine_name,
            machine.actor
        );
    }
    out.push('\n');

    pushln!(&mut out, "ROUTES");
    for route in &schema.routes {
        pushln!(
            &mut out,
            "  {} == {}.{} -> {}.{} [{}]",
            route.name,
            route.from_machine,
            route.effect_variant,
            route.to.machine,
            route.to.input_variant,
            render_route_delivery(&route.delivery)
        );
        if !route.bindings.is_empty() {
            pushln!(&mut out, "    bindings = {}", render_route_bindings(route));
        }
    }
    out.push('\n');

    pushln!(&mut out, "ACTOR_PRIORITIES");
    for priority in &schema.actor_priorities {
        pushln!(
            &mut out,
            "  {} > {} ; {}",
            priority.higher,
            priority.lower,
            priority.reason
        );
    }
    out.push('\n');

    pushln!(&mut out, "SCHEDULER_RULES");
    for rule in &schema.scheduler_rules {
        pushln!(&mut out, "  {}", render_scheduler_rule(rule));
    }
    out.push('\n');

    pushln!(&mut out, "INVARIANTS");
    for invariant in &schema.invariants {
        pushln!(
            &mut out,
            "  {} == {}",
            invariant.name,
            render_composition_invariant_kind(&invariant.kind)
        );
        pushln!(
            &mut out,
            "    statement = {}",
            tla_string(&invariant.statement)
        );
    }

    pushln!(&mut out, "====");
    out
}

#[cfg(not(test))]
pub fn render_machine_mapping_coverage(
    schema: &MachineSchema,
    coverage: &MachineCoverageManifest,
) -> String {
    let mut out = String::new();

    pushln!(&mut out, "## Generated Coverage");
    pushln!(
        &mut out,
        "This section is generated from the Rust machine catalog. Do not edit it by hand."
    );
    out.push('\n');

    pushln!(&mut out, "### Machine");
    pushln!(&mut out, "- `{}`", schema.machine);
    out.push('\n');

    pushln!(&mut out, "### Code Anchors");
    for anchor in &coverage.code_anchors {
        pushln!(
            &mut out,
            "- `{}`: `{}` — {}",
            anchor.id,
            anchor.path,
            anchor.note
        );
    }
    out.push('\n');

    pushln!(&mut out, "### Scenarios");
    for scenario in &coverage.scenarios {
        pushln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary);
    }
    out.push('\n');

    render_semantic_mapping_section(&mut out, "Transitions", &coverage.transition_coverage);
    render_semantic_mapping_section(&mut out, "Effects", &coverage.effect_coverage);
    render_semantic_mapping_section(&mut out, "Invariants", &coverage.invariant_coverage);

    out
}

#[cfg(not(test))]
pub fn render_composition_mapping_coverage(
    schema: &CompositionSchema,
    coverage: &CompositionCoverageManifest,
) -> String {
    let mut out = String::new();

    pushln!(&mut out, "## Generated Coverage");
    pushln!(
        &mut out,
        "This section is generated from the Rust composition catalog. Do not edit it by hand."
    );
    out.push('\n');

    pushln!(&mut out, "### Composition");
    pushln!(&mut out, "- `{}`", schema.name);
    out.push('\n');

    pushln!(&mut out, "### Code Anchors");
    for anchor in &coverage.code_anchors {
        pushln!(
            &mut out,
            "- `{}`: `{}` — {}",
            anchor.id,
            anchor.path,
            anchor.note
        );
    }
    out.push('\n');

    pushln!(&mut out, "### Scenarios");
    for scenario in &coverage.scenarios {
        pushln!(&mut out, "- `{}` — {}", scenario.id, scenario.summary);
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

#[cfg(not(test))]
pub fn render_machine_kernel_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let module_name = machine_slug(&schema.machine);
    let catalog_fn = format!("{module_name}_machine");

    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    pushln!(&mut out, "use crate::runtime::{{");
    pushln!(
        &mut out,
        "    GeneratedMachineKernel, KernelInput, KernelState, KernelValue, TransitionOutcome, TransitionRefusal,"
    );
    pushln!(&mut out, "}};");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "pub fn schema() -> meerkat_machine_schema::MachineSchema {{"
    );
    pushln!(&mut out, "    meerkat_machine_schema::{catalog_fn}()");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn kernel() -> GeneratedMachineKernel {{");
    pushln!(&mut out, "    GeneratedMachineKernel::new(schema())");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "pub fn initial_state() -> Result<KernelState, TransitionRefusal> {{"
    );
    pushln!(&mut out, "    kernel().initial_state()");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn transition(");
    pushln!(&mut out, "    state: &KernelState,");
    pushln!(&mut out, "    input: &KernelInput,");
    pushln!(
        &mut out,
        ") -> Result<TransitionOutcome, TransitionRefusal> {{"
    );
    pushln!(&mut out, "    kernel().transition(state, input)");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn evaluate_helper(");
    pushln!(&mut out, "    state: &KernelState,");
    pushln!(&mut out, "    helper_name: &str,");
    pushln!(
        &mut out,
        "    args: &std::collections::BTreeMap<String, KernelValue>,"
    );
    pushln!(&mut out, ") -> Result<KernelValue, TransitionRefusal> {{");
    pushln!(
        &mut out,
        "    kernel().evaluate_helper(state, helper_name, args)"
    );
    pushln!(&mut out, "}}");

    out
}

#[cfg(not(test))]
pub fn render_generated_kernel_mod(schemas: &[MachineSchema]) -> String {
    let mut out = String::new();
    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    let mut module_slugs: Vec<_> = schemas
        .iter()
        .map(|schema| machine_slug(&schema.machine))
        .collect();
    module_slugs.sort();
    for slug in module_slugs {
        pushln!(&mut out, "pub mod {slug};");
    }
    out.push('\n');
    pushln!(&mut out, "use crate::runtime::GeneratedMachineKernel;");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "pub fn all_kernels() -> Vec<GeneratedMachineKernel> {{"
    );
    pushln!(&mut out, "    vec![");
    for schema in schemas {
        let slug = machine_slug(&schema.machine);
        pushln!(&mut out, "        {slug}::kernel(),");
    }
    pushln!(&mut out, "    ]");
    pushln!(&mut out, "}}");
    out
}

#[cfg(not(test))]
fn render_semantic_mapping_section(
    out: &mut String,
    heading: &str,
    entries: &[SemanticCoverageEntry],
) {
    pushln!(out, "### {heading}");
    if entries.is_empty() {
        pushln!(out, "- `(none)`");
        out.push('\n');
        return;
    }

    for entry in entries {
        pushln!(out, "- `{}`", entry.name);
        pushln!(
            out,
            "  - anchors: {}",
            entry
                .anchor_ids
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        pushln!(
            out,
            "  - scenarios: {}",
            entry
                .scenario_ids
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    out.push('\n');
}

#[cfg(not(test))]
fn machine_slug(machine_name: &str) -> String {
    let trimmed = machine_name.strip_suffix("Machine").unwrap_or(machine_name);
    to_snake_case(trimmed)
}

#[cfg(not(test))]
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

#[cfg(not(test))]
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
    pushln!(out, "{label}");
    pushln!(
        out,
        "  {} = {}",
        schema.name,
        render_enum_variants_inline(schema)
    );
    for variant in &schema.variants {
        pushln!(
            out,
            "  {} == {}",
            variant.name,
            render_variant_fields(variant)
        );
    }
    out.push('\n');
}

fn render_helpers_section(out: &mut String, label: &str, helpers: &[HelperSchema]) {
    if helpers.is_empty() {
        return;
    }
    pushln!(out, "{label}");
    for helper in helpers {
        pushln!(
            out,
            "  {}({}) : {}",
            helper.name,
            render_fields(&helper.params),
            render_type_ref(&helper.returns)
        );
        pushln!(out, "    == {}", render_expr(&helper.body));
    }
    out.push('\n');
}

fn render_invariants_section(out: &mut String, schema: &MachineSchema) {
    pushln!(out, "INVARIANTS");
    for invariant in &schema.invariants {
        pushln!(
            out,
            "  {} == {}",
            invariant.name,
            render_expr(&invariant.expr)
        );
    }
    out.push('\n');
}

fn render_transitions_section(out: &mut String, schema: &MachineSchema) {
    pushln!(out, "TRANSITIONS");
    for transition in &schema.transitions {
        render_transition(out, transition);
    }
    out.push('\n');
}

fn render_transition(out: &mut String, transition: &TransitionSchema) {
    pushln!(out, "  {}", transition.name);
    pushln!(out, "    from = {}", render_string_list(&transition.from));
    pushln!(
        out,
        "    on = {}({})",
        transition.on.variant,
        transition.on.bindings.join(", ")
    );
    if !transition.guards.is_empty() {
        pushln!(out, "    guards =");
        for guard in &transition.guards {
            render_guard(out, guard);
        }
    }
    if !transition.updates.is_empty() {
        pushln!(out, "    updates =");
        for update in &transition.updates {
            pushln!(out, "      - {}", render_update(update));
        }
    }
    pushln!(out, "    to = {}", tla_string(&transition.to));
    if !transition.emit.is_empty() {
        pushln!(out, "    emit =");
        for effect in &transition.emit {
            pushln!(out, "      - {}", render_effect_emit(effect));
        }
    }
}

fn render_guard(out: &mut String, guard: &Guard) {
    pushln!(
        out,
        "      - {} == {}",
        guard.name,
        render_expr(&guard.expr)
    );
}

fn render_field_init(out: &mut String, field: &FieldInit) {
    pushln!(out, "  {} = {}", field.field, render_expr(&field.expr));
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
        Expr::NamedVariant { variant, .. } => tla_string(variant),
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
        Expr::SeqElements(inner) => format!("SeqElements({})", render_expr(inner)),
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
        TypeRef::Named(name) | TypeRef::Enum(name) => name.clone(),
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

#[cfg(not(test))]
fn render_route_delivery(delivery: &RouteDelivery) -> &'static str {
    match delivery {
        RouteDelivery::Immediate => "Immediate",
        RouteDelivery::Enqueue => "Enqueue",
    }
}

#[cfg(not(test))]
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

#[cfg(not(test))]
fn render_scheduler_rule(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!("PreemptWhenReady({}, {})", higher, lower)
        }
    }
}

#[cfg(not(test))]
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
    use super::render_machine_module;
    use meerkat_machine_schema::{
        EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema, InitSchema, InputMatch,
        MachineSchema, RustBinding, StateSchema, TransitionSchema, TypeRef, Update, VariantSchema,
    };

    fn turn_execution_fixture() -> MachineSchema {
        MachineSchema {
            machine: "TurnExecutionMachine".to_owned(),
            version: 1,
            rust: RustBinding {
                crate_name: "meerkat-core".to_owned(),
                module: "agent::turn_execution".to_owned(),
            },
            state: StateSchema {
                phase: EnumSchema {
                    name: "TurnExecutionPhase".to_owned(),
                    variants: vec![
                        VariantSchema {
                            name: "Idle".to_owned(),
                            fields: vec![],
                        },
                        VariantSchema {
                            name: "Running".to_owned(),
                            fields: vec![],
                        },
                        VariantSchema {
                            name: "AwaitingBoundary".to_owned(),
                            fields: vec![],
                        },
                        VariantSchema {
                            name: "Cancelled".to_owned(),
                            fields: vec![],
                        },
                    ],
                },
                fields: vec![FieldSchema {
                    name: "boundary_count".to_owned(),
                    ty: TypeRef::U64,
                }],
                init: InitSchema {
                    phase: "Idle".to_owned(),
                    fields: vec![FieldInit {
                        field: "boundary_count".to_owned(),
                        expr: Expr::U64(0),
                    }],
                },
                terminal_phases: vec!["Cancelled".to_owned()],
            },
            inputs: EnumSchema {
                name: "TurnExecutionInput".to_owned(),
                variants: vec![
                    VariantSchema {
                        name: "StartConversationRun".to_owned(),
                        fields: vec![FieldSchema {
                            name: "run_id".to_owned(),
                            ty: TypeRef::String,
                        }],
                    },
                    VariantSchema {
                        name: "PrimitiveAppliedConversationTurn".to_owned(),
                        fields: vec![FieldSchema {
                            name: "run_id".to_owned(),
                            ty: TypeRef::String,
                        }],
                    },
                    VariantSchema {
                        name: "BoundaryComplete".to_owned(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "AcknowledgeTerminalFromCancelled".to_owned(),
                        fields: vec![],
                    },
                ],
            },
            effects: EnumSchema {
                name: "TurnExecutionEffect".to_owned(),
                variants: vec![VariantSchema {
                    name: "BoundaryApplied".to_owned(),
                    fields: vec![
                        FieldSchema {
                            name: "run_id".to_owned(),
                            ty: TypeRef::String,
                        },
                        FieldSchema {
                            name: "boundary_sequence".to_owned(),
                            ty: TypeRef::U64,
                        },
                    ],
                }],
            },
            helpers: vec![],
            derived: vec![],
            invariants: vec![],
            transitions: vec![
                TransitionSchema {
                    name: "StartConversationRun".to_owned(),
                    from: vec!["Idle".to_owned()],
                    on: InputMatch {
                        variant: "StartConversationRun".to_owned(),
                        bindings: vec!["run_id".to_owned()],
                    },
                    guards: vec![],
                    updates: vec![],
                    to: "Running".to_owned(),
                    emit: vec![],
                },
                TransitionSchema {
                    name: "PrimitiveAppliedConversationTurn".to_owned(),
                    from: vec!["Running".to_owned()],
                    on: InputMatch {
                        variant: "PrimitiveAppliedConversationTurn".to_owned(),
                        bindings: vec!["run_id".to_owned()],
                    },
                    guards: vec![],
                    updates: vec![Update::Increment {
                        field: "boundary_count".to_owned(),
                        amount: 1,
                    }],
                    to: "AwaitingBoundary".to_owned(),
                    emit: vec![EffectEmit {
                        variant: "BoundaryApplied".to_owned(),
                        fields: [
                            ("run_id".to_owned(), Expr::Binding("run_id".to_owned())),
                            (
                                "boundary_sequence".to_owned(),
                                Expr::Add(
                                    Box::new(Expr::Field("boundary_count".to_owned())),
                                    Box::new(Expr::U64(1)),
                                ),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    }],
                },
                TransitionSchema {
                    name: "BoundaryComplete".to_owned(),
                    from: vec!["AwaitingBoundary".to_owned()],
                    on: InputMatch {
                        variant: "BoundaryComplete".to_owned(),
                        bindings: vec![],
                    },
                    guards: vec![],
                    updates: vec![],
                    to: "Cancelled".to_owned(),
                    emit: vec![],
                },
                TransitionSchema {
                    name: "AcknowledgeTerminalFromCancelled".to_owned(),
                    from: vec!["Cancelled".to_owned()],
                    on: InputMatch {
                        variant: "AcknowledgeTerminalFromCancelled".to_owned(),
                        bindings: vec![],
                    },
                    guards: vec![],
                    updates: vec![],
                    to: "Cancelled".to_owned(),
                    emit: vec![],
                },
            ],
        }
    }

    #[test]
    fn renders_turn_execution_with_boundary_and_terminal_transitions() {
        let rendered = render_machine_module(&turn_execution_fixture());

        assert!(rendered.contains("TRANSITIONS\n  StartConversationRun"));
        assert!(rendered.contains("PrimitiveAppliedConversationTurn"));
        assert!(rendered.contains("BoundaryComplete"));
        assert!(rendered.contains("AcknowledgeTerminalFromCancelled"));
        assert!(rendered.contains(
            "BoundaryApplied([run_id |-> run_id, boundary_sequence |-> boundary_count + 1])"
        ));
    }
}

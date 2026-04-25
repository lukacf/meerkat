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
    MachineCoverageManifest, RouteDelivery, RouteTargetKind, SchedulerRule, SemanticCoverageEntry,
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
    render_enum_section(&mut out, "SIGNALS", &schema.signals);
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
            "  {} == {}.{} -> {}.{} ({}) [{}]",
            route.name,
            route.from_machine,
            route.effect_variant,
            route.to.machine,
            route.to.input_variant,
            match route.to.kind {
                RouteTargetKind::Input => "Input",
                RouteTargetKind::Signal => "Signal",
            },
            render_route_delivery(&route.delivery)
        );
        if !route.bindings.is_empty() {
            pushln!(&mut out, "    bindings = {}", render_route_bindings(route));
        }
    }
    out.push('\n');

    pushln!(&mut out, "TARGET_SELECTORS");
    for selector in &schema.route_target_selectors {
        pushln!(
            &mut out,
            "  {} selects {} from {:?}",
            selector.route_name,
            selector.selector_field,
            selector.source
        );
    }
    out.push('\n');

    pushln!(&mut out, "DRIVER");
    match &schema.driver {
        Some(driver) => {
            pushln!(
                &mut out,
                "  {} ({} @ {})",
                driver.name,
                driver.rust.driver_type,
                driver.rust.module_path
            );
            for watched in &driver.watched_effects {
                pushln!(
                    &mut out,
                    "    watches {}::{}",
                    watched.producer_instance,
                    watched.effect_variant
                );
            }
            for dispatch in &driver.dispatch_routes {
                pushln!(
                    &mut out,
                    "    dispatches {} -> {}::{} ({:?})",
                    dispatch.name,
                    dispatch.target_instance,
                    dispatch.input_variant,
                    dispatch.target_kind
                );
            }
        }
        None => pushln!(&mut out, "  (none)"),
    }
    out.push('\n');

    pushln!(&mut out, "TRANSACTION_PLANS");
    for plan in &schema.transaction_plans {
        pushln!(
            &mut out,
            "  {} == {} via {}",
            plan.name,
            plan.trigger,
            plan.store_primitive
        );
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
    let module_name = machine_slug(&schema.machine);
    if schema.rust.crate_name == "meerkat-mob" {
        return render_canonical_stub_modeled_module(schema);
    }
    match module_name.as_str() {
        "meerkat" => render_meerkat_modeled_module(),
        "mob" => render_include_kernel_source(
            "/../meerkat-mob/src/machines/mob_machine.rs",
            "catalog::dsl::dsl_mob_machine",
        ),
        "schedule_lifecycle" => render_include_kernel_source(
            "/../meerkat-schedule/src/machines/schedule_lifecycle.rs",
            "catalog::dsl::dsl_schedule_lifecycle_machine",
        ),
        "occurrence_lifecycle" => render_include_kernel_source(
            "/../meerkat-schedule/src/machines/occurrence_lifecycle.rs",
            "catalog::dsl::dsl_occurrence_lifecycle_machine",
        ),
        "auth" => render_include_kernel_source(
            "/../meerkat-runtime/src/auth_machine/dsl.rs",
            "catalog::dsl::dsl_auth_machine",
        ),
        _ => render_canonical_stub_modeled_module(schema),
    }
}

#[cfg(not(test))]
fn render_include_kernel_source(path: &str, catalog_fn: &str) -> String {
    let rel = path.trim_start_matches("/../");
    let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(rel);
    let source = match std::fs::read_to_string(&source_path) {
        Ok(source) => source,
        Err(error) => {
            return format!(
                "compile_error!(\"failed to read canonical kernel source {}: {}\");\n",
                source_path.display(),
                error
            );
        }
    };
    let source = source.replace("#[cfg(test)]", "#[cfg(any())]");
    format!(
        "mod source {{\n#![allow(dead_code, clippy::expect_used, clippy::assign_op_pattern)]\n{source}\n}}\npub use source::*;\n\npub fn schema() -> meerkat_machine_schema::MachineSchema {{\n    meerkat_machine_schema::{catalog_fn}()\n}}\n"
    )
}

#[cfg(not(test))]
fn render_canonical_stub_modeled_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let module_name = machine_slug(&schema.machine);
    let catalog_fn = format!("catalog::dsl::dsl_{module_name}_machine");
    let has_signals = !schema.signals.variants.is_empty();

    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    pushln!(
        &mut out,
        "#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::implicit_clone, clippy::unnecessary_cast, clippy::redundant_clone)]"
    );
    pushln!(&mut out);
    pushln!(
        &mut out,
        "pub fn schema() -> meerkat_machine_schema::MachineSchema {{"
    );
    pushln!(&mut out, "    meerkat_machine_schema::{catalog_fn}()");
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    let named_type_aliases = collect_machine_named_types(schema);
    let enum_type_defs = collect_machine_enum_types(schema);
    let generated_enum_names: std::collections::BTreeSet<String> = enum_type_defs
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    let enum_defaults: std::collections::BTreeMap<String, String> = enum_type_defs
        .iter()
        .map(|(name, variants)| {
            (
                name.clone(),
                format!(
                    "{}::{}",
                    name,
                    rust_ident(
                        variants
                            .first()
                            .cloned()
                            .unwrap_or_else(|| "Running".to_string())
                            .as_str()
                    )
                ),
            )
        })
        .collect();
    if !named_type_aliases.is_empty() {
        for alias in named_type_aliases {
            if generated_enum_names.contains(&rust_ident(&alias)) {
                continue;
            }
            if let Some(atom) = lookup_named_type_atom(schema, &alias) {
                pushln!(
                    &mut out,
                    "pub type {} = {};",
                    rust_ident(&alias),
                    render_rust_type_atom(atom)
                );
            }
        }
        pushln!(&mut out);
    }
    if !enum_type_defs.is_empty() {
        for (enum_name, variants) in enum_type_defs {
            pushln!(&mut out, "#[allow(non_camel_case_types)]");
            pushln!(
                &mut out,
                "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
            );
            pushln!(&mut out, "pub enum {} {{", enum_name);
            for variant in &variants {
                pushln!(&mut out, "    {},", rust_ident(variant));
            }
            pushln!(&mut out, "}}");
            pushln!(&mut out, "impl {} {{", enum_name);
            pushln!(&mut out, "    pub fn as_str(&self) -> &'static str {{");
            pushln!(&mut out, "        match self {{");
            for variant in &variants {
                pushln!(
                    &mut out,
                    "            Self::{} => \"{}\",",
                    rust_ident(variant),
                    variant
                );
            }
            pushln!(&mut out, "        }}");
            pushln!(&mut out, "    }}");
            pushln!(&mut out, "}}");
            pushln!(&mut out, "impl std::fmt::Display for {} {{", enum_name);
            pushln!(
                &mut out,
                "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{"
            );
            pushln!(&mut out, "        f.write_str(self.as_str())");
            pushln!(&mut out, "    }}");
            pushln!(&mut out, "}}");
        }
        pushln!(&mut out);
    }

    pushln!(&mut out, "pub trait Context {{}}");
    pushln!(&mut out, "pub struct EmptyContext;");
    pushln!(&mut out, "impl Context for EmptyContext {{}}");
    pushln!(&mut out);

    pushln!(&mut out, "#[allow(non_camel_case_types)]");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum Phase {{");
    for variant in &schema.state.phase.variants {
        pushln!(&mut out, "    {},", rust_ident(&variant.name));
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub struct State {{");
    pushln!(&mut out, "    pub phase: Phase,");
    for field in &schema.state.fields {
        pushln!(
            &mut out,
            "    pub {}: {},",
            rust_field_ident(&field.name),
            render_rust_type_ref(&field.ty)
        );
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out, "impl Default for State {{");
    pushln!(&mut out, "    fn default() -> Self {{");
    pushln!(&mut out, "        initial_state()");
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    pushln!(&mut out, "pub mod inputs {{");
    pushln!(&mut out, "    use super::*;");
    for variant in &schema.inputs.variants {
        pushln!(
            &mut out,
            "    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
        );
        pushln!(&mut out, "    pub struct {} {{", rust_ident(&variant.name));
        for field in &variant.fields {
            pushln!(
                &mut out,
                "        pub {}: {},",
                rust_field_ident(&field.name),
                render_rust_type_ref(&field.ty)
            );
        }
        pushln!(&mut out, "    }}");
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum Input {{");
    for variant in &schema.inputs.variants {
        pushln!(
            &mut out,
            "    {}(inputs::{}),",
            rust_ident(&variant.name),
            rust_ident(&variant.name)
        );
    }
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum InputKind {{");
    for variant in &schema.inputs.variants {
        pushln!(&mut out, "    {},", rust_ident(&variant.name));
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    if has_signals {
        pushln!(&mut out, "pub mod signals {{");
        pushln!(&mut out, "    use super::*;");
        for variant in &schema.signals.variants {
            pushln!(
                &mut out,
                "    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
            );
            pushln!(&mut out, "    pub struct {} {{", rust_ident(&variant.name));
            for field in &variant.fields {
                pushln!(
                    &mut out,
                    "        pub {}: {},",
                    rust_field_ident(&field.name),
                    render_rust_type_ref(&field.ty)
                );
            }
            pushln!(&mut out, "    }}");
        }
        pushln!(&mut out, "}}");
        pushln!(&mut out);
        pushln!(
            &mut out,
            "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
        );
        pushln!(&mut out, "pub enum Signal {{");
        for variant in &schema.signals.variants {
            pushln!(
                &mut out,
                "    {}(signals::{}),",
                rust_ident(&variant.name),
                rust_ident(&variant.name)
            );
        }
        pushln!(&mut out, "}}");
        pushln!(
            &mut out,
            "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
        );
        pushln!(&mut out, "pub enum SignalKind {{");
        for variant in &schema.signals.variants {
            pushln!(&mut out, "    {},", rust_ident(&variant.name));
        }
        pushln!(&mut out, "}}");
        pushln!(&mut out);
    }

    pushln!(&mut out, "pub mod effects {{");
    pushln!(&mut out, "    use super::*;");
    for variant in &schema.effects.variants {
        pushln!(
            &mut out,
            "    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
        );
        pushln!(&mut out, "    pub struct {} {{", rust_ident(&variant.name));
        for field in &variant.fields {
            pushln!(
                &mut out,
                "        pub {}: {},",
                rust_field_ident(&field.name),
                render_rust_type_ref(&field.ty)
            );
        }
        pushln!(&mut out, "    }}");
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum Effect {{");
    for variant in &schema.effects.variants {
        pushln!(
            &mut out,
            "    {}(effects::{}),",
            rust_ident(&variant.name),
            rust_ident(&variant.name)
        );
    }
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum EffectKind {{");
    for variant in &schema.effects.variants {
        pushln!(&mut out, "    {},", rust_ident(&variant.name));
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    pushln!(&mut out, "#[allow(non_camel_case_types)]");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum TransitionId {{");
    for transition in &schema.transitions {
        pushln!(&mut out, "    {},", rust_ident(&transition.name));
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out, "#[allow(non_camel_case_types)]");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum GuardId {{");
    pushln!(&mut out, "    None,");
    pushln!(&mut out, "}}");
    pushln!(&mut out, "#[allow(non_camel_case_types)]");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum HelperId {{");
    pushln!(&mut out, "    None,");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub struct GuardRejection {{");
    pushln!(&mut out, "    pub transition_id: TransitionId,");
    pushln!(&mut out, "    pub guard_id: GuardId,");
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum TriggerDiscriminant {{");
    pushln!(&mut out, "    Input(InputKind),");
    if has_signals {
        pushln!(&mut out, "    Signal(SignalKind),");
    }
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum TransitionRefusal {{");
    pushln!(
        &mut out,
        "    NoMatchingTransition {{ phase: Phase, trigger: TriggerDiscriminant }},"
    );
    pushln!(
        &mut out,
        "    GuardRejected {{ rejections: Vec<GuardRejection> }},"
    );
    pushln!(
        &mut out,
        "    AmbiguousTransition {{ transitions: Vec<TransitionId> }},"
    );
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum KernelError {{");
    pushln!(
        &mut out,
        "    ContextViolation {{ transition_id: TransitionId, detail: String }},"
    );
    pushln!(
        &mut out,
        "    HelperEvaluation {{ helper_id: HelperId, detail: String }},"
    );
    pushln!(&mut out, "    CodegenInvariant {{ detail: String }},");
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum TransitionError {{");
    pushln!(&mut out, "    Refusal(TransitionRefusal),");
    pushln!(&mut out, "    Kernel(KernelError),");
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub struct Outcome {{");
    pushln!(&mut out, "    pub transition_id: TransitionId,");
    pushln!(&mut out, "    pub next_state: State,");
    pushln!(&mut out, "    pub effects: Vec<Effect>,");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    let default_transition = schema
        .transitions
        .first()
        .map(|transition| rust_ident(&transition.name))
        .unwrap_or_else(|| "None".to_string());
    pushln!(&mut out, "fn default_transition_id() -> TransitionId {{");
    pushln!(&mut out, "    TransitionId::{default_transition}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    pushln!(&mut out, "pub mod helpers {{");
    pushln!(&mut out, "    use super::*;");
    pushln!(
        &mut out,
        "    pub fn none<C: Context>(_: &State, context: &C) -> Result<(), KernelError> {{"
    );
    pushln!(&mut out, "        let _ = context;");
    pushln!(&mut out, "        Ok(())");
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    let first_phase = schema
        .state
        .phase
        .variants
        .first()
        .map(|variant| rust_ident(&variant.name))
        .unwrap_or_else(|| "Running".to_string());
    pushln!(&mut out, "pub fn initial_state() -> State {{");
    pushln!(&mut out, "    State {{");
    pushln!(&mut out, "        phase: Phase::{first_phase},");
    for field in &schema.state.fields {
        pushln!(
            &mut out,
            "        {}: {},",
            rust_field_ident(&field.name),
            direct_default_value_expr(schema, &field.ty, &enum_defaults)
        );
    }
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    pushln!(&mut out, "pub fn transition<C: Context>(");
    pushln!(&mut out, "    state: &State,");
    pushln!(&mut out, "    input: Input,");
    pushln!(&mut out, "    context: &C,");
    pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
    pushln!(&mut out, "    let _ = (input, context);");
    pushln!(&mut out, "    Ok(Outcome {{");
    pushln!(&mut out, "        transition_id: default_transition_id(),");
    pushln!(&mut out, "        next_state: state.clone(),");
    pushln!(&mut out, "        effects: Vec::new(),");
    pushln!(&mut out, "    }})");
    pushln!(&mut out, "}}");
    if has_signals {
        pushln!(&mut out);
        pushln!(&mut out, "pub fn transition_signal<C: Context>(");
        pushln!(&mut out, "    state: &State,");
        pushln!(&mut out, "    signal: Signal,");
        pushln!(&mut out, "    context: &C,");
        pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
        pushln!(&mut out, "    let _ = (signal, context);");
        pushln!(&mut out, "    Ok(Outcome {{");
        pushln!(&mut out, "        transition_id: default_transition_id(),");
        pushln!(&mut out, "        next_state: state.clone(),");
        pushln!(&mut out, "        effects: Vec::new(),");
        pushln!(&mut out, "    }})");
        pushln!(&mut out, "}}");
    }
    out
}

#[cfg(not(test))]
#[allow(clippy::needless_raw_string_hashes)]
fn render_meerkat_modeled_module() -> String {
    r#"// Generated by `cargo xtask machine-codegen --all`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::implicit_clone, clippy::unnecessary_cast, clippy::redundant_clone)]

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine()
}

pub use crate::ids::{AgentRuntimeId, SessionId, ToolFilter, ToolVisibilityWitness};
pub type FenceToken = u64;
pub type Generation = u64;

pub trait Context {}
pub struct EmptyContext;
impl Context for EmptyContext {}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub phase: Phase,
    pub session_id: Option<SessionId>,
    pub active_runtime_id: Option<AgentRuntimeId>,
}
impl Default for State {
    fn default() -> Self {
        initial_state()
    }
}

pub mod inputs {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RegisterSession {
        pub session_id: SessionId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PrepareBindings {
        pub agent_runtime_id: AgentRuntimeId,
        pub fence_token: FenceToken,
        pub generation: Generation,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct InterruptCurrentRun {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CancelAfterBoundary {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RequestDeferredTools {
        pub names: std::collections::BTreeSet<String>,
        pub witnesses: std::collections::BTreeMap<String, ToolVisibilityWitness>,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PublishCommittedVisibleSet {
        pub active_filter: ToolFilter,
        pub staged_filter: ToolFilter,
        pub active_requested_deferred_names: std::collections::BTreeSet<String>,
        pub staged_requested_deferred_names: std::collections::BTreeSet<String>,
        pub active_visibility_revision: u64,
        pub staged_visibility_revision: u64,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input {
    RegisterSession(inputs::RegisterSession),
    PrepareBindings(inputs::PrepareBindings),
    InterruptCurrentRun(inputs::InterruptCurrentRun),
    CancelAfterBoundary(inputs::CancelAfterBoundary),
    RequestDeferredTools(inputs::RequestDeferredTools),
    PublishCommittedVisibleSet(inputs::PublishCommittedVisibleSet),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputKind {
    RegisterSession,
    PrepareBindings,
    InterruptCurrentRun,
    CancelAfterBoundary,
    RequestDeferredTools,
    PublishCommittedVisibleSet,
}

pub mod signals {
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct Initialize {}
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Signal {
    Initialize(signals::Initialize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SignalKind {
    Initialize,
}

pub mod effects {
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CommittedVisibleSetPublished {}
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    CommittedVisibleSetPublished(effects::CommittedVisibleSetPublished),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    CommittedVisibleSetPublished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionId {
    Initialize,
    RegisterSession,
    PrepareBindings,
    InterruptCurrentRun,
    CancelAfterBoundary,
    RequestDeferredTools,
    PublishCommittedVisibleSet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuardId {
    Phase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperId {
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuardRejection {
    pub transition_id: TransitionId,
    pub guard_id: GuardId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TriggerDiscriminant {
    Input(InputKind),
    Signal(SignalKind),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionRefusal {
    NoMatchingTransition { phase: Phase, trigger: TriggerDiscriminant },
    GuardRejected { rejections: Vec<GuardRejection> },
    AmbiguousTransition { transitions: Vec<TransitionId> },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KernelError {
    ContextViolation { transition_id: TransitionId, detail: String },
    HelperEvaluation { helper_id: HelperId, detail: String },
    CodegenInvariant { detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionError {
    Refusal(TransitionRefusal),
    Kernel(KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub transition_id: TransitionId,
    pub next_state: State,
    pub effects: Vec<Effect>,
}

pub mod helpers {
    use super::*;
    pub fn none<C: Context>(_: &State, context: &C) -> Result<(), KernelError> {
        let _ = context;
        Ok(())
    }
}

pub fn initial_state() -> State {
    State {
        phase: Phase::Initializing,
        session_id: None,
        active_runtime_id: None,
    }
}

fn refusal_input(phase: &Phase, kind: InputKind) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
        phase: phase.clone(),
        trigger: TriggerDiscriminant::Input(kind),
    })
}

fn refusal_signal(phase: &Phase, kind: SignalKind) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
        phase: phase.clone(),
        trigger: TriggerDiscriminant::Signal(kind),
    })
}

fn guard(transition_id: TransitionId) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::GuardRejected {
        rejections: vec![GuardRejection {
            transition_id,
            guard_id: GuardId::Phase,
        }],
    })
}

pub fn transition_signal<C: Context>(
    state: &State,
    signal: Signal,
    context: &C,
) -> Result<Outcome, TransitionError> {
    let _ = context;
    match signal {
        Signal::Initialize(_) => {
            if state.phase != Phase::Initializing {
                return Err(refusal_signal(&state.phase, SignalKind::Initialize));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Idle;
            Ok(Outcome {
                transition_id: TransitionId::Initialize,
                next_state,
                effects: Vec::new(),
            })
        }
    }
}

pub fn transition<C: Context>(
    state: &State,
    input: Input,
    context: &C,
) -> Result<Outcome, TransitionError> {
    let _ = context;
    match input {
        Input::RegisterSession(payload) => {
            if state.phase != Phase::Idle {
                return Err(refusal_input(&state.phase, InputKind::RegisterSession));
            }
            let mut next_state = state.clone();
            next_state.session_id = Some(payload.session_id);
            Ok(Outcome {
                transition_id: TransitionId::RegisterSession,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::PrepareBindings(payload) => {
            if !matches!(state.phase, Phase::Idle | Phase::Attached) {
                return Err(guard(TransitionId::PrepareBindings));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Attached;
            next_state.active_runtime_id = Some(payload.agent_runtime_id);
            Ok(Outcome {
                transition_id: TransitionId::PrepareBindings,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::InterruptCurrentRun(_) => {
            if state.phase != Phase::Attached {
                return Err(refusal_input(&state.phase, InputKind::InterruptCurrentRun));
            }
            Ok(Outcome {
                transition_id: TransitionId::InterruptCurrentRun,
                next_state: state.clone(),
                effects: Vec::new(),
            })
        }
        Input::CancelAfterBoundary(_) => {
            if state.phase != Phase::Attached {
                return Err(refusal_input(&state.phase, InputKind::CancelAfterBoundary));
            }
            Ok(Outcome {
                transition_id: TransitionId::CancelAfterBoundary,
                next_state: state.clone(),
                effects: Vec::new(),
            })
        }
        Input::RequestDeferredTools(_) => {
            if state.phase != Phase::Attached {
                return Err(refusal_input(&state.phase, InputKind::RequestDeferredTools));
            }
            Ok(Outcome {
                transition_id: TransitionId::RequestDeferredTools,
                next_state: state.clone(),
                effects: Vec::new(),
            })
        }
        Input::PublishCommittedVisibleSet(_) => {
            if state.phase != Phase::Attached {
                return Err(refusal_input(
                    &state.phase,
                    InputKind::PublishCommittedVisibleSet,
                ));
            }
            Ok(Outcome {
                transition_id: TransitionId::PublishCommittedVisibleSet,
                next_state: state.clone(),
                effects: vec![Effect::CommittedVisibleSetPublished(
                    effects::CommittedVisibleSetPublished {},
                )],
            })
        }
    }
}
"#
    .to_string()
}

#[cfg(not(test))]
fn direct_default_value_expr(
    schema: &MachineSchema,
    ty: &TypeRef,
    enum_defaults: &std::collections::BTreeMap<String, String>,
) -> String {
    match ty {
        TypeRef::Bool => "false".to_string(),
        TypeRef::U32 | TypeRef::U64 => "0".to_string(),
        TypeRef::String => "String::new()".to_string(),
        TypeRef::Named(name) if named_type_lowers_to_u64(schema, name.as_str()) => "0".to_string(),
        TypeRef::Named(name) => format!("{}::from(String::new())", rust_ident(name.as_str())),
        TypeRef::Enum(name) => enum_defaults
            .get(name.as_str())
            .cloned()
            .unwrap_or_else(|| format!("{}::default()", rust_ident(name.as_str()))),
        TypeRef::Option(_) => "None".to_string(),
        TypeRef::Set(_) => "Default::default()".to_string(),
        TypeRef::Seq(_) => "Vec::new()".to_string(),
        TypeRef::Map(_, _) => "Default::default()".to_string(),
    }
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
fn machine_slug(machine_name: impl AsRef<str>) -> String {
    let machine_name = machine_name.as_ref();
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
        transition.on.variant_str(),
        transition
            .on
            .bindings()
            .iter()
            .map(meerkat_machine_schema::identity::FieldId::as_str)
            .collect::<Vec<_>>()
            .join(", ")
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
        Update::MapRemove { field, key } => {
            format!("{field}' = MapRemove({field}, {})", render_expr(key))
        }
        Update::MapIncrement { field, key, amount } => {
            format!(
                "{field}' = [ {field} EXCEPT ![{}] = @ + {amount} ]",
                render_expr(key)
            )
        }
        Update::MapDecrement { field, key, amount } => {
            format!(
                "{field}' = [ {field} EXCEPT ![{}] = @ - {amount} ]",
                render_expr(key)
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
        effect.variant.as_str().to_owned()
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
        Expr::Field(name) => name.as_str().to_owned(),
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
        Expr::MapContainsKey { map, key } => {
            format!("{} \\in DOMAIN {}", render_expr(key), render_expr(map))
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
        TypeRef::Named(name) => name.as_str().to_owned(),
        TypeRef::Enum(name) => name.as_str().to_owned(),
        TypeRef::Option(inner) => format!("Option({})", render_type_ref(inner)),
        TypeRef::Set(inner) => format!("Set({})", render_type_ref(inner)),
        TypeRef::Seq(inner) => format!("Seq({})", render_type_ref(inner)),
        TypeRef::Map(key, value) => {
            format!("Map({}, {})", render_type_ref(key), render_type_ref(value))
        }
    }
}

#[cfg(not(test))]
fn render_rust_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "bool".to_owned(),
        TypeRef::U32 => "u32".to_owned(),
        TypeRef::U64 => "u64".to_owned(),
        TypeRef::String => "String".to_owned(),
        TypeRef::Named(name) => rust_ident(name),
        TypeRef::Enum(name) => rust_ident(name),
        TypeRef::Option(inner) => format!("Option<{}>", render_rust_type_ref(inner)),
        TypeRef::Set(inner) => {
            format!(
                "std::collections::BTreeSet<{}>",
                render_rust_type_ref(inner)
            )
        }
        TypeRef::Seq(inner) => format!("Vec<{}>", render_rust_type_ref(inner)),
        TypeRef::Map(key, value) => format!(
            "std::collections::BTreeMap<{}, {}>",
            render_rust_type_ref(key),
            render_rust_type_ref(value)
        ),
    }
}

#[cfg(not(test))]
fn collect_machine_named_types(schema: &MachineSchema) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();

    for field in &schema.state.fields {
        collect_named_types_from_type_ref(&field.ty, &mut names);
    }
    for variant in &schema.inputs.variants {
        for field in &variant.fields {
            collect_named_types_from_type_ref(&field.ty, &mut names);
        }
    }
    for variant in &schema.signals.variants {
        for field in &variant.fields {
            collect_named_types_from_type_ref(&field.ty, &mut names);
        }
    }
    for variant in &schema.effects.variants {
        for field in &variant.fields {
            collect_named_types_from_type_ref(&field.ty, &mut names);
        }
    }
    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        for field in &helper.params {
            collect_named_types_from_type_ref(&field.ty, &mut names);
        }
        collect_named_types_from_type_ref(&helper.returns, &mut names);
    }

    names.into_iter().collect()
}

#[cfg(not(test))]
fn collect_named_types_from_type_ref(ty: &TypeRef, names: &mut std::collections::BTreeSet<String>) {
    match ty {
        TypeRef::Named(name) => {
            names.insert(rust_ident(name));
        }
        TypeRef::Enum(name) => {
            names.insert(rust_ident(name));
        }
        TypeRef::Option(inner) | TypeRef::Set(inner) | TypeRef::Seq(inner) => {
            collect_named_types_from_type_ref(inner, names);
        }
        TypeRef::Map(key, value) => {
            collect_named_types_from_type_ref(key, names);
            collect_named_types_from_type_ref(value, names);
        }
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 | TypeRef::String => {}
    }
}

#[cfg(not(test))]
fn collect_machine_enum_types(schema: &MachineSchema) -> Vec<(String, Vec<String>)> {
    if schema.rust.crate_name == "meerkat-mob" {
        return Vec::new();
    }
    let mut enums: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    enums.insert(schema.state.phase.name.clone());
    for field in &schema.state.fields {
        collect_enum_types_from_type_ref(&field.ty, &mut enums);
    }
    for variant in &schema.inputs.variants {
        for field in &variant.fields {
            collect_enum_types_from_type_ref(&field.ty, &mut enums);
        }
    }
    for variant in &schema.signals.variants {
        for field in &variant.fields {
            collect_enum_types_from_type_ref(&field.ty, &mut enums);
        }
    }
    for variant in &schema.effects.variants {
        for field in &variant.fields {
            collect_enum_types_from_type_ref(&field.ty, &mut enums);
        }
    }
    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        for field in &helper.params {
            collect_enum_types_from_type_ref(&field.ty, &mut enums);
        }
        collect_enum_types_from_type_ref(&helper.returns, &mut enums);
    }

    enums
        .into_iter()
        .filter_map(|name| known_enum_variants(&name).map(|variants| (rust_ident(&name), variants)))
        .collect()
}

#[cfg(not(test))]
fn collect_enum_types_from_type_ref(ty: &TypeRef, enums: &mut std::collections::BTreeSet<String>) {
    match ty {
        TypeRef::Enum(name) => {
            enums.insert(name.as_str().to_owned());
        }
        TypeRef::Option(inner) | TypeRef::Set(inner) | TypeRef::Seq(inner) => {
            collect_enum_types_from_type_ref(inner, enums);
        }
        TypeRef::Map(key, value) => {
            collect_enum_types_from_type_ref(key, enums);
            collect_enum_types_from_type_ref(value, enums);
        }
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 | TypeRef::String | TypeRef::Named(_) => {}
    }
}

/// Render the Rust-code text for a [`RustTypeAtom`] — what appears on the
/// right-hand side of a `pub type Alias = ...;` declaration or the
/// return position of a default-value helper.
#[cfg(not(test))]
fn render_rust_type_atom(atom: &meerkat_machine_schema::RustTypeAtom) -> String {
    use meerkat_machine_schema::RustTypeAtom;
    match atom {
        RustTypeAtom::U8 => "u8".to_string(),
        RustTypeAtom::U16 => "u16".to_string(),
        RustTypeAtom::U32 => "u32".to_string(),
        RustTypeAtom::U64 => "u64".to_string(),
        RustTypeAtom::Bool => "bool".to_string(),
        RustTypeAtom::String => "String".to_string(),
        RustTypeAtom::TypePath(path) => path.clone(),
    }
}

/// Look up the authoritative Rust atom for a `TypeRef::Named` slug.
///
#[cfg(not(test))]
fn lookup_named_type_atom<'a>(
    schema: &'a MachineSchema,
    name: &str,
) -> Option<&'a meerkat_machine_schema::RustTypeAtom> {
    schema
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == name)
        .map(|binding| &binding.rust)
}

/// Named-type slugs whose authoritative binding resolves to `u64`.
/// Convenience predicate consumed by the default-value and RawValue
/// helpers.
#[cfg(not(test))]
fn named_type_lowers_to_u64(schema: &MachineSchema, name: &str) -> bool {
    use meerkat_machine_schema::RustTypeAtom;
    lookup_named_type_atom(schema, name).is_some_and(|atom| {
        matches!(
            atom,
            RustTypeAtom::U64 | RustTypeAtom::U32 | RustTypeAtom::U16 | RustTypeAtom::U8
        )
    })
}

#[cfg(not(test))]
fn rust_ident(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

#[cfg(not(test))]
fn known_enum_variants(name: &str) -> Option<Vec<String>> {
    Some(
        match name {
            "FlowRunStatus" => vec![
                "Absent",
                "Pending",
                "Running",
                "Completed",
                "Failed",
                "Canceled",
            ],
            "DependencyMode" => vec!["All", "Any"],
            "CollectionPolicyKind" => vec!["All", "Any", "Quorum"],
            "StepRunStatus" => vec!["Dispatched", "Completed", "Failed", "Skipped", "Canceled"],
            "FrameScope" => vec!["Root", "Body"],
            "FlowNodeKind" => vec!["Step", "Loop"],
            "NodeRunStatus" => vec![
                "Pending",
                "Ready",
                "Running",
                "Completed",
                "Failed",
                "Skipped",
                "Canceled",
            ],
            "LoopIterationStage" => vec!["AwaitingBodyFrame", "BodyFrameActive", "AwaitingUntil"],
            _ => return None,
        }
        .into_iter()
        .map(str::to_string)
        .collect(),
    )
}

#[cfg(not(test))]
fn rust_field_ident(value: impl AsRef<str>) -> String {
    rust_ident(value)
}

#[cfg(not(test))]
#[allow(dead_code)]
fn rust_fn_ident(value: &str) -> String {
    simple_snake_case(value)
}

#[cfg(not(test))]
#[allow(dead_code)]
fn simple_snake_case(value: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if idx != 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    out
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

fn render_string_list<S: AsRef<str>>(items: &[S]) -> String {
    let rendered = items.iter().map(tla_string).collect::<Vec<_>>().join(", ");
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
            meerkat_machine_schema::RouteBindingSource::OwnerProvided => {
                format!("{} := <owner-provided>", binding.to_field)
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
        CompositionInvariantKind::HandoffProtocolCovered {
            producer_instance,
            effect_variant,
            protocol_name,
        } => format!(
            "HandoffProtocolCovered({}, {}, {})",
            producer_instance, effect_variant, protocol_name
        ),
    }
}

fn tla_ident(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn tla_string(value: impl AsRef<str>) -> String {
    format!("\"{}\"", value.as_ref().replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::render_machine_module;
    use meerkat_machine_schema::identity::{
        EffectVariantId, EnumVariantId, FieldId, InputVariantId, MachineId, PhaseId, TransitionId,
    };
    use meerkat_machine_schema::{
        EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema, InitSchema, MachineSchema,
        RustBinding, StateSchema, TransitionSchema, TriggerMatch, TypeRef, Update, VariantSchema,
    };

    fn vid(s: &str) -> EnumVariantId {
        EnumVariantId::parse(s).expect("valid enum variant slug")
    }

    fn fid(s: &str) -> FieldId {
        FieldId::parse(s).expect("valid field slug")
    }

    fn pid(s: &str) -> PhaseId {
        PhaseId::parse(s).expect("valid phase slug")
    }

    fn tid(s: &str) -> TransitionId {
        TransitionId::parse(s).expect("valid transition slug")
    }

    fn ivid(s: &str) -> InputVariantId {
        InputVariantId::parse(s).expect("valid input variant slug")
    }

    fn eid(s: &str) -> EffectVariantId {
        EffectVariantId::parse(s).expect("valid effect variant slug")
    }

    fn turn_execution_fixture() -> MachineSchema {
        MachineSchema {
            machine: MachineId::parse("TurnExecutionMachine").expect("valid machine slug"),
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
                            name: vid("Idle"),
                            fields: vec![],
                        },
                        VariantSchema {
                            name: vid("Running"),
                            fields: vec![],
                        },
                        VariantSchema {
                            name: vid("AwaitingBoundary"),
                            fields: vec![],
                        },
                        VariantSchema {
                            name: vid("Cancelled"),
                            fields: vec![],
                        },
                    ],
                },
                fields: vec![FieldSchema {
                    name: fid("boundary_count"),
                    ty: TypeRef::U64,
                }],
                init: InitSchema {
                    phase: pid("Idle"),
                    fields: vec![FieldInit {
                        field: fid("boundary_count"),
                        expr: Expr::U64(0),
                    }],
                },
                terminal_phases: vec![pid("Cancelled")],
            },
            inputs: EnumSchema {
                name: "TurnExecutionInput".to_owned(),
                variants: vec![
                    VariantSchema {
                        name: vid("StartConversationRun"),
                        fields: vec![FieldSchema {
                            name: fid("run_id"),
                            ty: TypeRef::String,
                        }],
                    },
                    VariantSchema {
                        name: vid("PrimitiveAppliedConversationTurn"),
                        fields: vec![FieldSchema {
                            name: fid("run_id"),
                            ty: TypeRef::String,
                        }],
                    },
                    VariantSchema {
                        name: vid("BoundaryComplete"),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: vid("AcknowledgeTerminalFromCancelled"),
                        fields: vec![],
                    },
                ],
            },
            surface_only_inputs: vec![],
            signals: EnumSchema {
                name: "TurnExecutionSignal".to_owned(),
                variants: vec![],
            },
            effects: EnumSchema {
                name: "TurnExecutionEffect".to_owned(),
                variants: vec![VariantSchema {
                    name: vid("BoundaryApplied"),
                    fields: vec![
                        FieldSchema {
                            name: fid("run_id"),
                            ty: TypeRef::String,
                        },
                        FieldSchema {
                            name: fid("boundary_sequence"),
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
                    name: tid("StartConversationRun"),
                    from: vec![pid("Idle")],
                    on: TriggerMatch::Input {
                        variant: ivid("StartConversationRun"),
                        bindings: vec![fid("run_id")],
                    },
                    guards: vec![],
                    updates: vec![],
                    to: pid("Running"),
                    emit: vec![],
                },
                TransitionSchema {
                    name: tid("PrimitiveAppliedConversationTurn"),
                    from: vec![pid("Running")],
                    on: TriggerMatch::Input {
                        variant: ivid("PrimitiveAppliedConversationTurn"),
                        bindings: vec![fid("run_id")],
                    },
                    guards: vec![],
                    updates: vec![Update::Increment {
                        field: fid("boundary_count"),
                        amount: 1,
                    }],
                    to: pid("AwaitingBoundary"),
                    emit: vec![EffectEmit {
                        variant: eid("BoundaryApplied"),
                        fields: [
                            (fid("run_id"), Expr::Binding("run_id".to_owned())),
                            (
                                fid("boundary_sequence"),
                                Expr::Add(
                                    Box::new(Expr::Field(fid("boundary_count"))),
                                    Box::new(Expr::U64(1)),
                                ),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    }],
                },
                TransitionSchema {
                    name: tid("BoundaryComplete"),
                    from: vec![pid("AwaitingBoundary")],
                    on: TriggerMatch::Input {
                        variant: ivid("BoundaryComplete"),
                        bindings: vec![],
                    },
                    guards: vec![],
                    updates: vec![],
                    to: pid("Cancelled"),
                    emit: vec![],
                },
                TransitionSchema {
                    name: tid("AcknowledgeTerminalFromCancelled"),
                    from: vec![pid("Cancelled")],
                    on: TriggerMatch::Input {
                        variant: ivid("AcknowledgeTerminalFromCancelled"),
                        bindings: vec![],
                    },
                    guards: vec![],
                    updates: vec![],
                    to: pid("Cancelled"),
                    emit: vec![],
                },
            ],
            ci_step_limit: None,
            effect_dispositions: vec![],
            named_types: vec![],
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

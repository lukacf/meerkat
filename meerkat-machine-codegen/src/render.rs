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
                "  {} @ {}",
                driver.driver_type,
                driver.module_path
            );
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
    let mut out = String::new();
    let module_name = machine_slug(&schema.machine);
    let catalog_fn = match module_name.as_str() {
        "flow_run" => "flow_run_machine".to_string(),
        "flow_frame" => "flow_frame_machine".to_string(),
        "loop_iteration" => "loop_iteration_machine".to_string(),
        _ => format!("catalog::dsl::dsl_{module_name}_machine"),
    };
    let has_signals = !schema.signals.variants.is_empty();
    let named_type_aliases = collect_machine_named_types(schema);
    let enum_type_defs = collect_machine_enum_types(schema);

    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    pushln!(&mut out, "#![allow(warnings)]");
    pushln!(
        &mut out,
        "#![allow(unused_imports, unused_mut, unused_variables, dead_code, non_camel_case_types)]"
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
    if schema.rust.crate_name == "meerkat-mob" {
        pushln!(&mut out, "mod modeled_runtime {{");
        pushln!(&mut out, "    #![allow(warnings)]");
        pushln!(
            &mut out,
            "    include!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../meerkat-machine-kernels/src/runtime.rs\"));"
        );
        pushln!(&mut out, "}}");
        pushln!(
            &mut out,
            "use modeled_runtime::{{evaluate_helper_from_schema, initial_state_from_schema, transition_from_schema, transition_signal_from_schema, RawEffect, RawInput, RawOutcome, RawRefusal, RawSignal, RawState, RawValue}};"
        );
    } else {
        pushln!(
            &mut out,
            "use crate::runtime::{{evaluate_helper_from_schema, initial_state_from_schema, transition_from_schema, transition_signal_from_schema, RawEffect, RawInput, RawOutcome, RawRefusal, RawSignal, RawState, RawValue}};"
        );
    }
    pushln!(&mut out);
    let generated_enum_names: std::collections::BTreeSet<String> = enum_type_defs
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    if !named_type_aliases.is_empty() {
        for alias in named_type_aliases {
            if generated_enum_names.contains(&rust_ident(&alias)) {
                continue;
            }
            pushln!(
                &mut out,
                "pub type {} = {};",
                rust_ident(&alias),
                render_named_type_alias_target(&alias)
            );
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
    pushln!(
        &mut out,
        "fn phase_from_raw(raw: &str) -> Result<Phase, KernelError> {{"
    );
    pushln!(&mut out, "    match raw {{");
    for variant in &schema.state.phase.variants {
        pushln!(
            &mut out,
            "        \"{}\" => Ok(Phase::{}),",
            variant.name,
            rust_ident(&variant.name)
        );
    }
    pushln!(
        &mut out,
        "        other => Err(KernelError::CodegenInvariant {{ detail: format!(\"unknown phase {{other}}\") }}),"
    );
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "fn state_from_raw(raw: &RawState) -> Result<State, KernelError> {{"
    );
    pushln!(&mut out, "    Ok(State {{");
    pushln!(&mut out, "        phase: phase_from_raw(&raw.phase)?,");
    for field in &schema.state.fields {
        pushln!(
            &mut out,
            "        {}: {},",
            rust_field_ident(&field.name),
            render_from_raw_expr(
                &format!(
                    "raw.fields.get(\"{}\").ok_or_else(|| KernelError::CodegenInvariant {{ detail: \"missing field {}\".into() }})?",
                    field.name, field.name
                ),
                &field.ty
            )
        );
    }
    pushln!(&mut out, "    }})");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "fn state_to_raw(state: &State) -> RawState {{");
    pushln!(
        &mut out,
        "    let mut fields = std::collections::BTreeMap::new();"
    );
    for field in &schema.state.fields {
        pushln!(
            &mut out,
            "    fields.insert(\"{}\".into(), {});",
            field.name,
            render_to_raw_expr(
                &format!("state.{}", rust_field_ident(&field.name)),
                &field.ty
            )
        );
    }
    pushln!(&mut out, "    RawState {{");
    pushln!(&mut out, "        phase: match state.phase {{");
    for variant in &schema.state.phase.variants {
        pushln!(
            &mut out,
            "            Phase::{} => \"{}\".into(),",
            rust_ident(&variant.name),
            variant.name
        );
    }
    pushln!(&mut out, "        }},");
    pushln!(&mut out, "        fields,");
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
    pushln!(
        &mut out,
        "fn input_kind_from_raw(raw: &str) -> Option<InputKind> {{"
    );
    pushln!(&mut out, "    match raw {{");
    for variant in &schema.inputs.variants {
        pushln!(
            &mut out,
            "        \"{}\" => Some(InputKind::{}),",
            variant.name,
            rust_ident(&variant.name)
        );
    }
    pushln!(&mut out, "        _ => None,");
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out, "fn input_to_raw(input: Input) -> RawInput {{");
    pushln!(&mut out, "    match input {{");
    for variant in &schema.inputs.variants {
        pushln!(
            &mut out,
            "        Input::{}(payload) => RawInput {{ variant: \"{}\".into(), fields: {{ let mut fields = std::collections::BTreeMap::new();",
            rust_ident(&variant.name),
            variant.name
        );
        for field in &variant.fields {
            pushln!(
                &mut out,
                "            fields.insert(\"{}\".into(), {});",
                field.name,
                render_to_raw_expr(
                    &format!("payload.{}", rust_field_ident(&field.name)),
                    &field.ty
                )
            );
        }
        pushln!(&mut out, "            fields }} }},");
    }
    pushln!(&mut out, "    }}");
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
        pushln!(
            &mut out,
            "fn signal_kind_from_raw(raw: &str) -> Option<SignalKind> {{"
        );
        pushln!(&mut out, "    match raw {{");
        for variant in &schema.signals.variants {
            pushln!(
                &mut out,
                "        \"{}\" => Some(SignalKind::{}),",
                variant.name,
                rust_ident(&variant.name)
            );
        }
        pushln!(&mut out, "        _ => None,");
        pushln!(&mut out, "    }}");
        pushln!(&mut out, "}}");
        pushln!(&mut out, "fn signal_to_raw(signal: Signal) -> RawSignal {{");
        pushln!(&mut out, "    match signal {{");
        for variant in &schema.signals.variants {
            pushln!(
                &mut out,
                "        Signal::{}(payload) => RawSignal {{ variant: \"{}\".into(), fields: {{ let mut fields = std::collections::BTreeMap::new();",
                rust_ident(&variant.name),
                variant.name
            );
            for field in &variant.fields {
                pushln!(
                    &mut out,
                    "            fields.insert(\"{}\".into(), {});",
                    field.name,
                    render_to_raw_expr(
                        &format!("payload.{}", rust_field_ident(&field.name)),
                        &field.ty
                    )
                );
            }
            pushln!(&mut out, "            fields }} }},");
        }
        pushln!(&mut out, "    }}");
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
    pushln!(
        &mut out,
        "fn effect_from_raw(raw: &RawEffect) -> Result<Effect, KernelError> {{"
    );
    pushln!(&mut out, "    match raw.variant.as_str() {{");
    for variant in &schema.effects.variants {
        pushln!(
            &mut out,
            "        \"{}\" => Ok(Effect::{}(effects::{} {{",
            variant.name,
            rust_ident(&variant.name),
            rust_ident(&variant.name)
        );
        for field in &variant.fields {
            pushln!(
                &mut out,
                "            {}: {},",
                rust_field_ident(&field.name),
                render_from_raw_expr(
                    &format!(
                        "raw.fields.get(\"{}\").ok_or_else(|| KernelError::CodegenInvariant {{ detail: \"missing effect field {}\".into() }})?",
                        field.name, field.name
                    ),
                    &field.ty
                )
            );
        }
        pushln!(&mut out, "        }})),");
    }
    pushln!(
        &mut out,
        "        other => Err(KernelError::CodegenInvariant {{ detail: format!(\"unknown effect {{other}}\") }}),"
    );
    pushln!(&mut out, "    }}");
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
    let mut emitted_guard = false;
    for transition in &schema.transitions {
        for guard in &transition.guards {
            emitted_guard = true;
            pushln!(
                &mut out,
                "    {},",
                rust_ident(&format!("{}_{}", transition.name, guard.name))
            );
        }
    }
    if !emitted_guard {
        pushln!(&mut out, "    None,");
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out, "#[allow(non_camel_case_types)]");
    pushln!(
        &mut out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(&mut out, "pub enum HelperId {{");
    if schema.helpers.is_empty() && schema.derived.is_empty() {
        pushln!(&mut out, "    None,");
    } else {
        for helper in schema.helpers.iter().chain(schema.derived.iter()) {
            pushln!(&mut out, "    {},", rust_ident(&helper.name));
        }
    }
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
    pushln!(
        &mut out,
        "fn transition_id_from_raw(raw: &str) -> Result<TransitionId, KernelError> {{"
    );
    pushln!(&mut out, "    match raw {{");
    for transition in &schema.transitions {
        pushln!(
            &mut out,
            "        \"{}\" => Ok(TransitionId::{}),",
            transition.name,
            rust_ident(&transition.name)
        );
    }
    pushln!(
        &mut out,
        "        other => Err(KernelError::CodegenInvariant {{ detail: format!(\"unknown transition {{other}}\") }}),"
    );
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(
        &mut out,
        "fn outcome_from_raw(raw: RawOutcome) -> Result<Outcome, TransitionError> {{"
    );
    pushln!(
        &mut out,
        "    let effects = raw.effects.iter().map(effect_from_raw).collect::<Result<Vec<_>, _>>().map_err(TransitionError::Kernel)?;"
    );
    pushln!(&mut out, "    Ok(Outcome {{");
    pushln!(
        &mut out,
        "        transition_id: transition_id_from_raw(&raw.transition).map_err(TransitionError::Kernel)?,"
    );
    pushln!(
        &mut out,
        "        next_state: state_from_raw(&raw.next_state).map_err(TransitionError::Kernel)?,"
    );
    pushln!(&mut out, "        effects,");
    pushln!(&mut out, "    }})");
    pushln!(&mut out, "}}");
    let fallback_phase = schema
        .state
        .phase
        .variants
        .first()
        .map(|variant| rust_ident(&variant.name))
        .unwrap_or_else(|| "Unknown".to_string());
    pushln!(
        &mut out,
        "fn refusal_from_raw(raw: RawRefusal) -> TransitionError {{"
    );
    pushln!(&mut out, "    match raw {{");
    pushln!(
        &mut out,
        "        RawRefusal::NoMatchingTransition {{ phase, variant, .. }} => TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {{"
    );
    pushln!(
        &mut out,
        "            phase: phase_from_raw(&phase).unwrap_or(Phase::{fallback_phase}),"
    );
    if has_signals {
        pushln!(
            &mut out,
            "            trigger: if let Some(kind) = input_kind_from_raw(&variant) {{ TriggerDiscriminant::Input(kind) }} else {{ TriggerDiscriminant::Signal(signal_kind_from_raw(&variant).unwrap()) }},"
        );
    } else {
        pushln!(
            &mut out,
            "            trigger: TriggerDiscriminant::Input(input_kind_from_raw(&variant).unwrap()),"
        );
    }
    pushln!(&mut out, "        }}),");
    pushln!(
        &mut out,
        "        RawRefusal::AmbiguousTransition {{ transitions, .. }} => TransitionError::Refusal(TransitionRefusal::AmbiguousTransition {{"
    );
    pushln!(
        &mut out,
        "            transitions: transitions.iter().filter_map(|transition| transition_id_from_raw(transition).ok()).collect(),"
    );
    pushln!(&mut out, "        }}),");
    pushln!(
        &mut out,
        "        RawRefusal::UnknownInputVariant {{ variant, .. }} => TransitionError::Kernel(KernelError::CodegenInvariant {{ detail: format!(\"unknown input variant {{variant}}\") }}),"
    );
    pushln!(
        &mut out,
        "        RawRefusal::UnknownSignalVariant {{ variant, .. }} => TransitionError::Kernel(KernelError::CodegenInvariant {{ detail: format!(\"unknown signal variant {{variant}}\") }}),"
    );
    pushln!(
        &mut out,
        "        RawRefusal::InvalidInputPayload {{ reason, .. }} => TransitionError::Kernel(KernelError::CodegenInvariant {{ detail: reason }}),"
    );
    pushln!(
        &mut out,
        "        RawRefusal::InvalidSignalPayload {{ reason, .. }} => TransitionError::Kernel(KernelError::CodegenInvariant {{ detail: reason }}),"
    );
    pushln!(
        &mut out,
        "        RawRefusal::EvaluationError {{ transition, reason, .. }} => TransitionError::Kernel(KernelError::CodegenInvariant {{ detail: format!(\"{{transition}}: {{reason}}\") }}),"
    );
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    pushln!(&mut out, "pub mod helpers {{");
    if schema.helpers.is_empty() && schema.derived.is_empty() {
        pushln!(&mut out, "    use super::*;");
        pushln!(
            &mut out,
            "    pub fn none<C: Context>(_: &State, context: &C) -> Result<(), KernelError> {{"
        );
        pushln!(&mut out, "        let _ = context;");
        pushln!(&mut out, "        Ok(())");
        pushln!(&mut out, "    }}");
    } else {
        pushln!(&mut out, "    use super::*;");
        for helper in schema.helpers.iter().chain(schema.derived.iter()) {
            let mut params = String::new();
            for field in &helper.params {
                params.push_str(", ");
                params.push_str(&format!(
                    "{}: {}",
                    rust_field_ident(&field.name),
                    render_rust_type_ref(&field.ty)
                ));
            }
            pushln!(
                &mut out,
                "    pub fn {}<C: Context>(state: &State{} , context: &C) -> Result<{}, KernelError> {{",
                rust_fn_ident(&helper.name),
                params,
                render_rust_type_ref(&helper.returns)
            );
            pushln!(&mut out, "        let _ = context;");
            pushln!(&mut out, "        let raw_state = state_to_raw(state);");
            pushln!(
                &mut out,
                "        let mut args = std::collections::BTreeMap::<String, RawValue>::new();"
            );
            for field in &helper.params {
                pushln!(
                    &mut out,
                    "        args.insert(\"{}\".into(), {});",
                    field.name,
                    render_to_raw_expr(&rust_field_ident(&field.name), &field.ty)
                );
            }
            pushln!(
                &mut out,
                "        let raw = evaluate_helper_from_schema(schema(), &raw_state, \"{}\", &args).map_err(|error| KernelError::HelperEvaluation {{ helper_id: HelperId::{}, detail: format!(\"{{error:?}}\") }})?;",
                helper.name,
                rust_ident(&helper.name)
            );
            pushln!(
                &mut out,
                "        {}",
                render_try_from_owned_raw_expr("raw", &helper.returns)
            );
            pushln!(&mut out, "    }}");
        }
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);

    pushln!(&mut out, "pub fn initial_state() -> State {{");
    pushln!(
        &mut out,
        "    let raw = initial_state_from_schema(schema()).expect(\"typed modeled-kernel initial state should be derivable from schema\");"
    );
    pushln!(
        &mut out,
        "    state_from_raw(&raw).expect(\"typed modeled-kernel initial state should convert from raw state\")"
    );
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn transition<C: Context>(");
    pushln!(&mut out, "    state: &State,");
    pushln!(&mut out, "    input: Input,");
    pushln!(&mut out, "    context: &C,");
    pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
    pushln!(&mut out, "    let _ = context;");
    pushln!(&mut out, "    let raw_state = state_to_raw(state);");
    pushln!(&mut out, "    let raw_input = input_to_raw(input);");
    pushln!(
        &mut out,
        "    transition_from_schema(schema(), &raw_state, &raw_input).map_err(refusal_from_raw).and_then(outcome_from_raw)"
    );
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    if has_signals {
        pushln!(&mut out);
        pushln!(&mut out, "pub fn transition_signal<C: Context>(");
        pushln!(&mut out, "    state: &State,");
        pushln!(&mut out, "    signal: Signal,");
        pushln!(&mut out, "    context: &C,");
        pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
        pushln!(&mut out, "    let _ = context;");
        pushln!(&mut out, "    let raw_state = state_to_raw(state);");
        pushln!(&mut out, "    let raw_signal = signal_to_raw(signal);");
        pushln!(
            &mut out,
            "    transition_signal_from_schema(schema(), &raw_state, &raw_signal).map_err(refusal_from_raw).and_then(outcome_from_raw)"
        );
        pushln!(&mut out, "}}");
    }

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
        TypeRef::Named(name) | TypeRef::Enum(name) => name.clone(),
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
        TypeRef::Named(name) | TypeRef::Enum(name) => rust_ident(name),
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
        TypeRef::Named(name) | TypeRef::Enum(name) => {
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
    if !matches!(
        schema.machine.as_str(),
        "FlowRunMachine" | "FlowFrameMachine" | "LoopIterationMachine"
    ) {
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
            enums.insert(name.clone());
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

#[cfg(not(test))]
fn render_named_type_alias_target(name: &str) -> &'static str {
    match name {
        "BoundarySequence" | "TurnNumber" | "FenceToken" | "Generation" => "u64",
        _ => "String",
    }
}

#[cfg(not(test))]
fn render_to_raw_expr(expr: &str, ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => format!("RawValue::Bool(({expr}).to_owned())"),
        TypeRef::U32 | TypeRef::U64 => format!("RawValue::U64(({expr}).to_owned() as u64)"),
        TypeRef::String => format!("RawValue::String(({expr}).to_owned())"),
        TypeRef::Named(name) if render_named_type_alias_target(&rust_ident(name)) == "u64" => {
            format!("RawValue::U64(({expr}).to_owned() as u64)")
        }
        TypeRef::Named(_) => format!("RawValue::String(({expr}).to_owned())"),
        TypeRef::Enum(name) => {
            if let Some(variants) = known_enum_variants(name) {
                let enum_name = rust_ident(name);
                let arms = variants
                    .iter()
                    .map(|variant| {
                        format!(
                            "{enum_name}::{} => \"{variant}\".into()",
                            rust_ident(variant)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "RawValue::NamedVariant {{ enum_name: \"{}\".into(), variant: match {expr} {{ {} }} }}",
                    name, arms
                )
            } else {
                format!(
                    "RawValue::NamedVariant {{ enum_name: \"{}\".into(), variant: ({expr}).to_owned() }}",
                    name
                )
            }
        }
        TypeRef::Option(inner) => {
            let inner_expr = render_to_raw_expr("value", inner);
            format!(
                "match {expr}.as_ref() {{ Some(value) => RawValue::Map(std::collections::BTreeMap::from([(RawValue::String(\"value\".into()), {inner_expr})])), None => RawValue::None }}"
            )
        }
        TypeRef::Set(inner) => format!(
            "RawValue::Set({expr}.iter().map(|value| {}).collect())",
            render_to_raw_expr("value", inner)
        ),
        TypeRef::Seq(inner) => format!(
            "RawValue::Seq({expr}.iter().map(|value| {}).collect())",
            render_to_raw_expr("value", inner)
        ),
        TypeRef::Map(key, value) => format!(
            "RawValue::Map({expr}.iter().map(|(key, value)| ({}, {})).collect())",
            render_to_raw_expr("key", key),
            render_to_raw_expr("value", value)
        ),
    }
}

#[cfg(not(test))]
fn render_from_raw_expr(expr: &str, ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => format!(
            "match {expr} {{ RawValue::Bool(value) => *value, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected bool, found {{other:?}}\") }}) }}"
        ),
        TypeRef::U32 => format!(
            "match {expr} {{ RawValue::U64(value) => u32::try_from(*value).map_err(|_| KernelError::CodegenInvariant {{ detail: format!(\"u64 {{value}} does not fit u32\") }})?, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected u64, found {{other:?}}\") }}) }}"
        ),
        TypeRef::U64 => format!(
            "match {expr} {{ RawValue::U64(value) => *value, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected u64, found {{other:?}}\") }}) }}"
        ),
        TypeRef::String => format!(
            "match {expr} {{ RawValue::String(value) => value.clone(), other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected string, found {{other:?}}\") }}) }}"
        ),
        TypeRef::Named(name) if render_named_type_alias_target(&rust_ident(name)) == "u64" => {
            format!(
                "match {expr} {{ RawValue::U64(value) => *value, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected numeric named value, found {{other:?}}\") }}) }}"
            )
        }
        TypeRef::Named(_) => format!(
            "match {expr} {{ RawValue::String(value) => value.clone(), other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected named string value, found {{other:?}}\") }}) }}"
        ),
        TypeRef::Enum(name) => {
            if let Some(variants) = known_enum_variants(name) {
                let enum_name = rust_ident(name);
                let arms = variants
                    .iter()
                    .map(|variant| {
                        format!("\"{variant}\" => {enum_name}::{},", rust_ident(variant))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    "match {expr} {{ RawValue::NamedVariant {{ enum_name, variant }} if enum_name == \"{}\" => match variant.as_str() {{ {} other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {} variant, found {{other}}\") }}) }}, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {}, found {{other:?}}\") }}) }}",
                    name, arms, name, name
                )
            } else {
                format!(
                    "match {expr} {{ RawValue::NamedVariant {{ enum_name, variant }} if enum_name == \"{}\" => variant.clone(), other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {}, found {{other:?}}\") }}) }}",
                    name, name
                )
            }
        }
        TypeRef::Option(inner) => {
            let inner_expr = render_from_raw_expr("value", inner);
            format!(
                "match {expr} {{ RawValue::None => None, RawValue::Map(entries) => match entries.get(&RawValue::String(\"value\".into())) {{ Some(value) => Some({inner_expr}), None => None }}, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected option, found {{other:?}}\") }}) }}"
            )
        }
        TypeRef::Set(inner) => format!(
            "match {expr} {{ RawValue::Set(values) => values.iter().map(|value| {}).collect::<Result<std::collections::BTreeSet<_>, _>>()?, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected set, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("value", inner)
        ),
        TypeRef::Seq(inner) => format!(
            "match {expr} {{ RawValue::Seq(values) => values.iter().map(|value| {}).collect::<Result<Vec<_>, _>>()?, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected seq, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("value", inner)
        ),
        TypeRef::Map(key, value) => format!(
            "match {expr} {{ RawValue::Map(entries) => entries.iter().map(|(key, value)| Ok((({})?, ({})?))).collect::<Result<std::collections::BTreeMap<_, _>, KernelError>>()?, other => return Err(KernelError::CodegenInvariant {{ detail: format!(\"expected map, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("key", key),
            render_try_from_raw_expr("value", value)
        ),
    }
}

#[cfg(not(test))]
fn render_try_from_raw_expr(expr: &str, ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => format!(
            "match {expr} {{ RawValue::Bool(value) => Ok(*value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected bool, found {{other:?}}\") }}) }}"
        ),
        TypeRef::U32 => format!(
            "match {expr} {{ RawValue::U64(value) => u32::try_from(*value).map_err(|_| KernelError::CodegenInvariant {{ detail: format!(\"u64 {{value}} does not fit u32\") }}), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected u64, found {{other:?}}\") }}) }}"
        ),
        TypeRef::U64 => format!(
            "match {expr} {{ RawValue::U64(value) => Ok(*value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected u64, found {{other:?}}\") }}) }}"
        ),
        TypeRef::String => format!(
            "match {expr} {{ RawValue::String(value) => Ok(value.clone()), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected string, found {{other:?}}\") }}) }}"
        ),
        TypeRef::Named(name) if render_named_type_alias_target(&rust_ident(name)) == "u64" => {
            format!(
                "match {expr} {{ RawValue::U64(value) => Ok(*value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected numeric named value, found {{other:?}}\") }}) }}"
            )
        }
        TypeRef::Named(_) => format!(
            "match {expr} {{ RawValue::String(value) => Ok(value.clone()), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected named string value, found {{other:?}}\") }}) }}"
        ),
        TypeRef::Enum(name) => {
            if let Some(variants) = known_enum_variants(name) {
                let enum_name = rust_ident(name);
                let arms = variants
                    .iter()
                    .map(|variant| {
                        format!("\"{variant}\" => Ok({enum_name}::{}),", rust_ident(variant))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    "match {expr} {{ RawValue::NamedVariant {{ enum_name, variant }} if enum_name == \"{}\" => match variant.as_str() {{ {} other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {} variant, found {{other}}\") }}) }}, other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {}, found {{other:?}}\") }}) }}",
                    name, arms, name, name
                )
            } else {
                format!(
                    "match {expr} {{ RawValue::NamedVariant {{ enum_name, variant }} if enum_name == \"{}\" => Ok(variant.clone()), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {}, found {{other:?}}\") }}) }}",
                    name, name
                )
            }
        }
        TypeRef::Option(inner) => {
            let inner_expr = render_try_from_raw_expr("value", inner);
            format!(
                "match {expr} {{ RawValue::None => Ok(None), RawValue::Map(entries) => match entries.get(&RawValue::String(\"value\".into())) {{ Some(value) => Ok(Some({inner_expr}?)), None => Ok(None) }}, other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected option, found {{other:?}}\") }}) }}"
            )
        }
        TypeRef::Set(inner) => format!(
            "match {expr} {{ RawValue::Set(values) => values.iter().map(|value| {}).collect::<Result<std::collections::BTreeSet<_>, _>>(), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected set, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("value", inner)
        ),
        TypeRef::Seq(inner) => format!(
            "match {expr} {{ RawValue::Seq(values) => values.iter().map(|value| {}).collect::<Result<Vec<_>, _>>(), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected seq, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("value", inner)
        ),
        TypeRef::Map(key, value) => format!(
            "match {expr} {{ RawValue::Map(entries) => entries.iter().map(|(key, value)| Ok(({}, {}))).collect::<Result<std::collections::BTreeMap<_, _>, KernelError>>(), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected map, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("key", key),
            render_try_from_raw_expr("value", value)
        ),
    }
}

#[cfg(not(test))]
fn render_try_from_owned_raw_expr(expr: &str, ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => format!(
            "match {expr} {{ RawValue::Bool(value) => Ok(value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected bool, found {{other:?}}\") }}) }}"
        ),
        TypeRef::U32 => format!(
            "match {expr} {{ RawValue::U64(value) => u32::try_from(value).map_err(|_| KernelError::CodegenInvariant {{ detail: format!(\"u64 {{value}} does not fit u32\") }}), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected u64, found {{other:?}}\") }}) }}"
        ),
        TypeRef::U64 => format!(
            "match {expr} {{ RawValue::U64(value) => Ok(value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected u64, found {{other:?}}\") }}) }}"
        ),
        TypeRef::String => format!(
            "match {expr} {{ RawValue::String(value) => Ok(value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected string, found {{other:?}}\") }}) }}"
        ),
        TypeRef::Named(name) if render_named_type_alias_target(&rust_ident(name)) == "u64" => {
            format!(
                "match {expr} {{ RawValue::U64(value) => Ok(value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected numeric named value, found {{other:?}}\") }}) }}"
            )
        }
        TypeRef::Named(_) => format!(
            "match {expr} {{ RawValue::String(value) => Ok(value), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected named string value, found {{other:?}}\") }}) }}"
        ),
        TypeRef::Enum(name) => {
            let enum_name = rust_ident(name);
            let variants = known_enum_variants(name).unwrap_or_default();
            let arms = variants
                .iter()
                .map(|variant| {
                    format!("\"{variant}\" => Ok({enum_name}::{}),", rust_ident(variant))
                })
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "match {expr} {{ RawValue::NamedVariant {{ enum_name, variant }} if enum_name == \"{}\" => match variant.as_str() {{ {} other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {} variant, found {{other}}\") }}) }}, other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected enum {}, found {{other:?}}\") }}) }}",
                name, arms, name, name
            )
        }
        TypeRef::Option(inner) => {
            let inner_expr = render_try_from_raw_expr("value", inner);
            format!(
                "match {expr} {{ RawValue::None => Ok(None), RawValue::Map(entries) => match entries.get(&RawValue::String(\"value\".into())) {{ Some(value) => Ok(Some(({})?)), None => Ok(None) }}, other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected option, found {{other:?}}\") }}) }}",
                inner_expr
            )
        }
        TypeRef::Set(inner) => format!(
            "match {expr} {{ RawValue::Set(values) => values.iter().map(|value| {}).collect::<Result<std::collections::BTreeSet<_>, _>>(), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected set, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("value", inner)
        ),
        TypeRef::Seq(inner) => format!(
            "match {expr} {{ RawValue::Seq(values) => values.iter().map(|value| {}).collect::<Result<Vec<_>, _>>(), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected seq, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("value", inner)
        ),
        TypeRef::Map(key, value) => format!(
            "match {expr} {{ RawValue::Map(entries) => entries.iter().map(|(key, value)| Ok((({})?, ({})?))).collect::<Result<std::collections::BTreeMap<_, _>, KernelError>>(), other => Err(KernelError::CodegenInvariant {{ detail: format!(\"expected map, found {{other:?}}\") }}) }}",
            render_try_from_raw_expr("key", key),
            render_try_from_raw_expr("value", value)
        ),
    }
}

#[cfg(not(test))]
fn rust_ident(value: &str) -> String {
    value
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
fn rust_field_ident(value: &str) -> String {
    rust_ident(value)
}

#[cfg(not(test))]
fn rust_fn_ident(value: &str) -> String {
    simple_snake_case(value)
}

#[cfg(not(test))]
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
        MachineSchema, RustBinding, StateSchema, TransitionSchema, TriggerKind, TypeRef, Update,
        VariantSchema,
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
            surface_only_inputs: vec![],
            signals: EnumSchema {
                name: "TurnExecutionSignal".to_owned(),
                variants: vec![],
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
                        kind: TriggerKind::Input,
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
                        kind: TriggerKind::Input,
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
                        kind: TriggerKind::Input,
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
                        kind: TriggerKind::Input,
                        variant: "AcknowledgeTerminalFromCancelled".to_owned(),
                        bindings: vec![],
                    },
                    guards: vec![],
                    updates: vec![],
                    to: "Cancelled".to_owned(),
                    emit: vec![],
                },
            ],
            ci_step_limit: None,
            effect_dispositions: vec![],
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

use std::fmt::Write;

#[cfg(not(test))]
use crate::compat_substrate;

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
use meerkat_machine_schema::TriggerKind;
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
    if is_compat_machine(schema) {
        return render_compat_machine_kernel_module(schema);
    }

    render_canonical_machine_kernel_module(schema)
}

#[cfg(not(test))]
fn render_canonical_machine_kernel_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let module_name = dsl_machine_slug(&schema.machine);
    let schema_fn = schema_constructor_path(schema);
    let substrate_mod = format!("meerkat_machine_schema::catalog::dsl::{module_name}");
    let state_name = format!("{}State", schema.machine);
    let legacy_transition = format!("{}Transition", schema.machine);
    let legacy_error = format!("{}TransitionError", schema.machine);
    let authority_name = format!("{}Authority", schema.machine);
    let mutator_name = format!("{}Mutator", schema.machine);
    let transition_id = format!("{}TransitionId", schema.machine);
    let guard_id = format!("{}GuardId", schema.machine);
    let helper_id = format!("{}HelperId", schema.machine);
    let input_name = schema.inputs.name.clone();
    let input_kind_name = format!("{}Kind", schema.inputs.name);
    let effect_name = schema.effects.name.clone();
    let effect_kind_name = format!("{}Kind", schema.effects.name);
    let phase_name = schema.state.phase.name.clone();
    let has_signals = !schema.signals.variants.is_empty();
    let signal_name = schema.signals.name.clone();
    let signal_kind_name = format!("{}Kind", schema.signals.name);
    let phase_field_name = canonical_phase_field_name(schema);
    let wrapper_fields = schema
        .state
        .fields
        .iter()
        .filter(|field| Some(field.name.as_str()) != phase_field_name)
        .collect::<Vec<_>>();

    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    pushln!(&mut out, "use crate::runtime::GeneratedMachineKernel;");
    pushln!(&mut out, "use {substrate_mod} as substrate;");
    pushln!(&mut out);
    pushln!(&mut out, "type InnerState = substrate::{state_name};");
    pushln!(&mut out, "pub type Phase = substrate::{phase_name};");
    pushln!(&mut out, "pub type Input = substrate::{input_name};");
    pushln!(
        &mut out,
        "pub type InputKind = substrate::{input_kind_name};"
    );
    if has_signals {
        pushln!(&mut out, "pub type Signal = substrate::{signal_name};");
        pushln!(
            &mut out,
            "pub type SignalKind = substrate::{signal_kind_name};"
        );
    }
    pushln!(&mut out, "pub type Effect = substrate::{effect_name};");
    pushln!(
        &mut out,
        "pub type EffectKind = substrate::{effect_kind_name};"
    );
    pushln!(
        &mut out,
        "pub type TransitionId = substrate::{transition_id};"
    );
    pushln!(&mut out, "pub type GuardId = substrate::{guard_id};");
    pushln!(&mut out, "pub type HelperId = substrate::{helper_id};");
    pushln!(
        &mut out,
        "type LegacyTransition = substrate::{legacy_transition};"
    );
    pushln!(
        &mut out,
        "type LegacyTransitionError = substrate::{legacy_error};"
    );
    pushln!(&mut out, "type Authority = substrate::{authority_name};");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub struct State {{");
    pushln!(&mut out, "    pub phase: Phase,");
    for field in &wrapper_fields {
        let ty = rust_type_for_substrate(&field.ty);
        pushln!(&mut out, "    pub {}: {},", field.name, ty);
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "impl Default for State {{");
    pushln!(&mut out, "    fn default() -> Self {{");
    pushln!(&mut out, "        state_from_inner(InnerState::default())");
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "impl State {{");
    pushln!(&mut out, "    pub fn into_inner(self) -> InnerState {{");
    pushln!(&mut out, "        state_to_inner(&self)");
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "pub fn schema() -> meerkat_machine_schema::MachineSchema {{"
    );
    pushln!(&mut out, "    {schema_fn}()");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn kernel() -> GeneratedMachineKernel {{");
    pushln!(&mut out, "    GeneratedMachineKernel::new(schema())");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub struct Outcome {{");
    pushln!(&mut out, "    pub transition_id: TransitionId,");
    pushln!(&mut out, "    pub next_state: State,");
    pushln!(&mut out, "    pub effects: Vec<Effect>,");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub struct GuardRejection {{");
    pushln!(&mut out, "    pub transition_id: TransitionId,");
    pushln!(&mut out, "    pub guard_id: GuardId,");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub enum TriggerDiscriminant {{");
    pushln!(&mut out, "    Input(InputKind),");
    if has_signals {
        pushln!(&mut out, "    Signal(SignalKind),");
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
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
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
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
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub enum TransitionError {{");
    pushln!(&mut out, "    Refusal(TransitionRefusal),");
    pushln!(&mut out, "    Kernel(KernelError),");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub trait Context {{}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, Default)]");
    pushln!(&mut out, "pub struct EmptyContext;");
    pushln!(&mut out);
    pushln!(&mut out, "impl Context for EmptyContext {{}}");
    pushln!(&mut out);
    let helper_entries = schema
        .helpers
        .iter()
        .chain(schema.derived.iter())
        .collect::<Vec<_>>();
    if helper_entries.is_empty() {
        pushln!(&mut out, "pub mod helpers {{}}");
    } else {
        pushln!(&mut out, "pub mod helpers {{");
        pushln!(&mut out, "    use super::*;");
        for helper in helper_entries {
            let return_ty = rust_type_for_substrate(&helper.returns);
            let params = helper
                .params
                .iter()
                .map(|field| format!("{}: {}", field.name, rust_type_for_substrate(&field.ty)))
                .collect::<Vec<_>>();
            let call_args = helper
                .params
                .iter()
                .map(|field| format!("&{}", field.name))
                .collect::<Vec<_>>();
            pushln!(&mut out, "    pub fn {}(", helper.name);
            pushln!(&mut out, "        state: &State,");
            for param in &params {
                pushln!(&mut out, "        {param},");
            }
            pushln!(&mut out, "        _context: &impl Context,");
            pushln!(&mut out, "    ) -> Result<{return_ty}, KernelError> {{");
            pushln!(
                &mut out,
                "        let authority = Authority::from_state(state_to_inner(state));"
            );
            pushln!(
                &mut out,
                "        Ok(authority.{}({}))",
                helper.name,
                call_args.join(", ")
            );
            pushln!(&mut out, "    }}");
        }
        pushln!(&mut out, "}}");
    }
    pushln!(&mut out);
    pushln!(&mut out, "pub fn initial_state() -> State {{");
    pushln!(&mut out, "    State::default()");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn transition(");
    pushln!(&mut out, "    state: &State,");
    pushln!(&mut out, "    input: Input,");
    pushln!(&mut out, "    _context: &impl Context,");
    pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
    pushln!(
        &mut out,
        "    let trigger = TriggerDiscriminant::Input(input.kind());"
    );
    pushln!(
        &mut out,
        "    let mut authority = Authority::from_state(state_to_inner(state));"
    );
    pushln!(
        &mut out,
        "    let transition = substrate::{mutator_name}::apply(&mut authority, input)"
    );
    pushln!(
        &mut out,
        "        .map_err(|error| map_legacy_error(error, state.phase, trigger.clone()))?;"
    );
    pushln!(
        &mut out,
        "    Ok(outcome_from_transition(&authority, transition))"
    );
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    if has_signals {
        pushln!(&mut out, "pub fn transition_signal(");
        pushln!(&mut out, "    state: &State,");
        pushln!(&mut out, "    signal: Signal,");
        pushln!(&mut out, "    _context: &impl Context,");
        pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
        pushln!(
            &mut out,
            "    let trigger = TriggerDiscriminant::Signal(signal.kind());"
        );
        pushln!(
            &mut out,
            "    let mut authority = Authority::from_state(state_to_inner(state));"
        );
        pushln!(&mut out, "    let transition = authority");
        pushln!(&mut out, "        .apply_signal(signal)");
        pushln!(
            &mut out,
            "        .map_err(|error| map_legacy_error(error, state.phase, trigger.clone()))?;"
        );
        pushln!(
            &mut out,
            "    Ok(outcome_from_transition(&authority, transition))"
        );
        pushln!(&mut out, "}}");
        pushln!(&mut out);
    }
    pushln!(
        &mut out,
        "fn outcome_from_transition(authority: &Authority, transition: LegacyTransition) -> Outcome {{"
    );
    pushln!(&mut out, "    Outcome {{");
    pushln!(&mut out, "        transition_id: transition.transition_id,");
    pushln!(
        &mut out,
        "        next_state: state_from_inner(authority.state.clone()),"
    );
    pushln!(&mut out, "        effects: transition.effects,");
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "fn state_from_inner(inner: InnerState) -> State {{"
    );
    pushln!(&mut out, "    State {{");
    pushln!(&mut out, "        phase: inner.phase(),");
    for field in &wrapper_fields {
        pushln!(&mut out, "        {}: inner.{},", field.name, field.name);
    }
    pushln!(&mut out, "    }}");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "fn state_to_inner(state: &State) -> InnerState {{"
    );
    if let Some(phase_field_name) = phase_field_name {
        pushln!(&mut out, "    InnerState {{");
        pushln!(&mut out, "        {}: state.phase,", phase_field_name);
        for field in &wrapper_fields {
            let value = if type_is_copy_for_substrate(&field.ty) {
                format!("state.{}", field.name)
            } else {
                format!("state.{}.clone()", field.name)
            };
            pushln!(&mut out, "        {}: {},", field.name, value);
        }
        pushln!(&mut out, "    }}");
    } else {
        pushln!(&mut out, "    let mut inner = InnerState::default();");
        for field in &wrapper_fields {
            let value = if type_is_copy_for_substrate(&field.ty) {
                format!("state.{}", field.name)
            } else {
                format!("state.{}.clone()", field.name)
            };
            pushln!(&mut out, "    inner.{} = {};", field.name, value);
        }
        pushln!(&mut out, "    inner");
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "fn map_legacy_error(");
    pushln!(&mut out, "    error: LegacyTransitionError,");
    pushln!(&mut out, "    phase: Phase,");
    pushln!(&mut out, "    trigger: TriggerDiscriminant,");
    pushln!(&mut out, ") -> TransitionError {{");
    pushln!(&mut out, "    let refusal = match error {{");
    pushln!(
        &mut out,
        "        LegacyTransitionError::NoMatchingTransition {{ .. }} => TransitionRefusal::NoMatchingTransition {{ phase, trigger }},"
    );
    pushln!(
        &mut out,
        "        LegacyTransitionError::GuardRejected {{ .. }} => TransitionRefusal::GuardRejected {{ rejections: guard_rejections_for_trigger(&phase, &trigger) }},"
    );
    pushln!(&mut out, "    }};");
    pushln!(&mut out, "    TransitionError::Refusal(refusal)");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "fn guard_rejections_for_trigger(phase: &Phase, trigger: &TriggerDiscriminant) -> Vec<GuardRejection> {{"
    );
    let mut grouped_rejections =
        std::collections::BTreeMap::<(String, String), Vec<(String, usize)>>::new();
    for transition in &schema.transitions {
        if transition.guards.is_empty() {
            continue;
        }
        let trigger_expr = match transition.on.kind {
            TriggerKind::Input => {
                format!(
                    "TriggerDiscriminant::Input(InputKind::{})",
                    transition.on.variant
                )
            }
            TriggerKind::Signal => {
                format!(
                    "TriggerDiscriminant::Signal(SignalKind::{})",
                    transition.on.variant
                )
            }
        };
        for from_phase in &transition.from {
            let entry = grouped_rejections
                .entry((from_phase.clone(), trigger_expr.clone()))
                .or_default();
            for (index, _) in transition.guards.iter().enumerate() {
                entry.push((transition.name.clone(), index + 1));
            }
        }
    }
    if grouped_rejections.is_empty() {
        pushln!(&mut out, "    let _ = (phase, trigger);");
        pushln!(&mut out, "    Vec::new()");
    } else {
        pushln!(&mut out, "    match (phase, trigger) {{");
        for ((from_phase, trigger_expr), rejections) in grouped_rejections {
            pushln!(
                &mut out,
                "        (Phase::{}, {}) => vec![",
                from_phase,
                trigger_expr
            );
            for (transition_name, index) in rejections {
                pushln!(
                    &mut out,
                    "            GuardRejection {{ transition_id: TransitionId::{}, guard_id: GuardId::{}Guard{} }},",
                    transition_name,
                    transition_name,
                    index
                );
            }
            pushln!(&mut out, "        ],");
        }
        pushln!(&mut out, "        _ => Vec::new(),");
        pushln!(&mut out, "    }}");
    }
    pushln!(&mut out, "}}");

    out
}

#[cfg(not(test))]
fn render_compat_machine_kernel_module(schema: &MachineSchema) -> String {
    render_compat_machine_kernel_module_direct(schema)
}

#[cfg(not(test))]
fn render_compat_machine_kernel_module_direct(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let schema_fn = schema_constructor_path(schema);
    let has_signals = !schema.signals.variants.is_empty();
    let helper_entries = schema
        .helpers
        .iter()
        .chain(schema.derived.iter())
        .collect::<Vec<_>>();
    let substrate = compat_substrate::render_substrate(schema);

    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    pushln!(
        &mut out,
        "#![allow(clippy::bool_comparison, clippy::clone_on_copy, clippy::cloned_instead_of_copied, clippy::collapsible_else_if, clippy::expect_used, clippy::explicit_iter_loop, clippy::if_not_else, clippy::len_zero, clippy::overly_complex_bool_expr, clippy::partialeq_to_none, clippy::ptr_arg, clippy::redundant_clone, clippy::redundant_closure, clippy::uninlined_format_args, dead_code, non_camel_case_types, non_snake_case, unused_imports, unused_parens, unused_variables)]"
    );
    pushln!(
        &mut out,
        "use meerkat_machine_schema::compat::types as compat_types;"
    );
    pushln!(&mut out);
    pushln!(&mut out, "mod substrate {{");
    pushln!(&mut out, "    use super::compat_types::*;");
    pushln!(&mut out, "    use serde::{{Deserialize, Serialize}};");
    for line in substrate.lines() {
        pushln!(&mut out, "    {line}");
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub type Phase = substrate::CompatPhase;");
    pushln!(
        &mut out,
        "pub type TransitionId = substrate::CompatMachineTransitionId;"
    );
    pushln!(
        &mut out,
        "pub type GuardId = substrate::CompatMachineGuardId;"
    );
    pushln!(
        &mut out,
        "pub type HelperId = substrate::CompatMachineHelperId;"
    );
    pushln!(&mut out);

    render_compat_payload_module(&mut out, "inputs", &schema.inputs.variants);
    if has_signals {
        render_compat_payload_module(&mut out, "signals", &schema.signals.variants);
    }
    render_compat_payload_module(&mut out, "effects", &schema.effects.variants);
    render_compat_variant_enum(
        &mut out,
        "Input",
        "InputKind",
        "inputs",
        &schema.inputs.variants,
    );
    if has_signals {
        render_compat_variant_enum(
            &mut out,
            "Signal",
            "SignalKind",
            "signals",
            &schema.signals.variants,
        );
    }
    render_compat_variant_enum(
        &mut out,
        "Effect",
        "EffectKind",
        "effects",
        &schema.effects.variants,
    );
    render_compat_state_struct(&mut out, schema);

    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub struct Outcome {{");
    pushln!(&mut out, "    pub transition_id: TransitionId,");
    pushln!(&mut out, "    pub next_state: State,");
    pushln!(&mut out, "    pub effects: Vec<Effect>,");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub struct GuardRejection {{");
    pushln!(&mut out, "    pub transition_id: TransitionId,");
    pushln!(&mut out, "    pub guard_id: GuardId,");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub enum TriggerDiscriminant {{");
    pushln!(&mut out, "    Input(InputKind),");
    if has_signals {
        pushln!(&mut out, "    Signal(SignalKind),");
    }
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
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
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
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
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(&mut out, "pub enum TransitionError {{");
    pushln!(&mut out, "    Refusal(TransitionRefusal),");
    pushln!(&mut out, "    Kernel(KernelError),");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub trait Context {{}}");
    pushln!(&mut out);
    pushln!(&mut out, "#[derive(Debug, Clone, Default)]");
    pushln!(&mut out, "pub struct EmptyContext;");
    pushln!(&mut out);
    pushln!(&mut out, "impl Context for EmptyContext {{}}");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "pub fn schema() -> meerkat_machine_schema::MachineSchema {{"
    );
    pushln!(&mut out, "    {schema_fn}()");
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn initial_state() -> State {{");
    pushln!(
        &mut out,
        "    state_from_substrate(substrate::CompatMachineState::default())"
    );
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    pushln!(&mut out, "pub fn transition(");
    pushln!(&mut out, "    state: &State,");
    pushln!(&mut out, "    input: Input,");
    pushln!(&mut out, "    _context: &impl Context,");
    pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
    pushln!(
        &mut out,
        "    let trigger = TriggerDiscriminant::Input(input.kind());"
    );
    pushln!(
        &mut out,
        "    let mut authority = substrate::CompatMachineAuthority::from_state(state_to_substrate(state));"
    );
    pushln!(
        &mut out,
        "    let transition = substrate::CompatMachineMutator::apply(&mut authority, input_to_substrate(input))"
    );
    pushln!(
        &mut out,
        "        .map_err(|error| map_substrate_error(error, state.phase, trigger.clone()))?;"
    );
    pushln!(
        &mut out,
        "    Ok(outcome_from_substrate(&authority, transition))"
    );
    pushln!(&mut out, "}}");
    pushln!(&mut out);
    if has_signals {
        pushln!(&mut out, "pub fn transition_signal(");
        pushln!(&mut out, "    state: &State,");
        pushln!(&mut out, "    signal: Signal,");
        pushln!(&mut out, "    _context: &impl Context,");
        pushln!(&mut out, ") -> Result<Outcome, TransitionError> {{");
        pushln!(
            &mut out,
            "    let trigger = TriggerDiscriminant::Signal(signal.kind());"
        );
        pushln!(
            &mut out,
            "    let mut authority = substrate::CompatMachineAuthority::from_state(state_to_substrate(state));"
        );
        pushln!(
            &mut out,
            "    let transition = authority.apply_signal(signal_to_substrate(signal))"
        );
        pushln!(
            &mut out,
            "        .map_err(|error| map_substrate_error(error, state.phase, trigger.clone()))?;"
        );
        pushln!(
            &mut out,
            "    Ok(outcome_from_substrate(&authority, transition))"
        );
        pushln!(&mut out, "}}");
        pushln!(&mut out);
    }

    if helper_entries.is_empty() {
        pushln!(&mut out, "pub mod helpers {{}}");
        pushln!(&mut out);
    } else {
        pushln!(&mut out, "pub mod helpers {{");
        pushln!(&mut out, "    use super::*;");
        for helper in &helper_entries {
            let fn_name = to_snake_case(&helper.name);
            let return_ty = compat_rust_type(&helper.returns);
            let helper_name = rust_enum_variant_ident(&helper.name);
            pushln!(&mut out, "    pub fn {fn_name}(");
            pushln!(&mut out, "        state: &State,");
            for param in &helper.params {
                pushln!(
                    &mut out,
                    "        {}: {},",
                    param.name,
                    compat_rust_type(&param.ty)
                );
            }
            pushln!(&mut out, "        _context: &impl Context,");
            pushln!(&mut out, "    ) -> Result<{return_ty}, KernelError> {{");
            pushln!(
                &mut out,
                "        let authority = substrate::CompatMachineAuthority::from_state(super::state_to_substrate(state));"
            );
            if helper.params.is_empty() {
                pushln!(&mut out, "        Ok(authority.{helper_name}())");
            } else {
                let args = helper
                    .params
                    .iter()
                    .map(|param| format!("&{}", param.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                pushln!(&mut out, "        Ok(authority.{helper_name}({args}))");
            }
            pushln!(&mut out, "    }}");
        }
        pushln!(&mut out, "}}");
        pushln!(&mut out);
    }

    render_compat_input_to_substrate(&mut out, schema);
    if has_signals {
        render_compat_signal_to_substrate(&mut out, schema);
    }
    render_compat_effect_from_substrate(&mut out, schema);
    render_compat_state_substrate_converters(&mut out, schema);
    render_compat_direct_error_helpers(&mut out, schema);

    out
}

#[cfg(not(test))]
pub fn render_compat_test_oracle_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let module_slug = machine_slug(&schema.machine);

    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    pushln!(
        &mut out,
        "#![allow(clippy::expect_used, clippy::ptr_arg, clippy::redundant_clone, clippy::redundant_closure, clippy::uninlined_format_args, dead_code)]"
    );
    pushln!(
        &mut out,
        "use crate::compat_generated::{module_slug}::{{Effect, KernelError, Phase, State}};"
    );
    pushln!(
        &mut out,
        "use crate::runtime::{{KernelEffect as LegacyEffect, KernelState as LegacyState, KernelValue}};"
    );
    pushln!(
        &mut out,
        "use meerkat_machine_schema::compat::types as compat_types;"
    );
    pushln!(&mut out);

    render_compat_effect_to_kernel(&mut out, schema);
    render_compat_state_converters(&mut out, schema);
    render_compat_value_helpers(&mut out, schema);

    out
}

#[cfg(not(test))]
fn render_compat_input_to_substrate(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "fn input_to_substrate(input: Input) -> substrate::CompatInput {{"
    );
    pushln!(out, "    match input {{");
    for variant in &schema.inputs.variants {
        let payload_binding = if variant.fields.is_empty() {
            "_payload"
        } else {
            "payload"
        };
        pushln!(
            out,
            "        Input::{}({payload_binding}) => {{",
            variant.name
        );
        if variant.fields.is_empty() {
            pushln!(out, "            substrate::CompatInput::{}", variant.name);
        } else {
            pushln!(
                out,
                "            substrate::CompatInput::{} {{",
                variant.name
            );
            for field in &variant.fields {
                pushln!(
                    out,
                    "                {}: payload.{},",
                    field.name,
                    field.name
                );
            }
            pushln!(out, "            }}");
        }
        pushln!(out, "        }},");
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_signal_to_substrate(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "fn signal_to_substrate(signal: Signal) -> substrate::CompatSignal {{"
    );
    pushln!(out, "    match signal {{");
    for variant in &schema.signals.variants {
        let payload_binding = if variant.fields.is_empty() {
            "_payload"
        } else {
            "payload"
        };
        pushln!(
            out,
            "        Signal::{}({payload_binding}) => {{",
            variant.name
        );
        if variant.fields.is_empty() {
            pushln!(out, "            substrate::CompatSignal::{}", variant.name);
        } else {
            pushln!(
                out,
                "            substrate::CompatSignal::{} {{",
                variant.name
            );
            for field in &variant.fields {
                pushln!(
                    out,
                    "                {}: payload.{},",
                    field.name,
                    field.name
                );
            }
            pushln!(out, "            }}");
        }
        pushln!(out, "        }},");
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_effect_from_substrate(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "fn effect_from_substrate(effect: substrate::CompatEffect) -> Effect {{"
    );
    pushln!(out, "    match effect {{");
    for variant in &schema.effects.variants {
        if variant.fields.is_empty() {
            pushln!(
                out,
                "        substrate::CompatEffect::{} => Effect::{}(effects::{}),",
                variant.name,
                variant.name,
                variant.name
            );
        } else {
            pushln!(out, "        substrate::CompatEffect::{} {{", variant.name);
            for field in &variant.fields {
                pushln!(out, "            {},", field.name);
            }
            pushln!(
                out,
                "        }} => Effect::{}(effects::{} {{",
                variant.name,
                variant.name
            );
            for field in &variant.fields {
                pushln!(out, "            {},", field.name);
            }
            pushln!(out, "        }}),");
        }
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_state_substrate_converters(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "fn state_to_substrate(state: &State) -> substrate::CompatMachineState {{"
    );
    pushln!(out, "    substrate::CompatMachineState {{");
    pushln!(out, "        phase: state.phase,");
    for field in &schema.state.fields {
        pushln!(out, "        {}: state.{}.clone(),", field.name, field.name);
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn state_from_substrate(state: substrate::CompatMachineState) -> State {{"
    );
    pushln!(out, "    State {{");
    pushln!(out, "        phase: state.phase,");
    for field in &schema.state.fields {
        pushln!(out, "        {}: state.{},", field.name, field.name);
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn outcome_from_substrate(authority: &substrate::CompatMachineAuthority, transition: substrate::CompatMachineTransition) -> Outcome {{"
    );
    pushln!(out, "    Outcome {{");
    pushln!(out, "        transition_id: transition.transition_id,");
    pushln!(
        out,
        "        next_state: state_from_substrate(authority.state.clone()),"
    );
    pushln!(
        out,
        "        effects: transition.effects.into_iter().map(effect_from_substrate).collect(),"
    );
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_direct_error_helpers(out: &mut String, schema: &MachineSchema) {
    pushln!(out, "fn map_substrate_error(");
    pushln!(out, "    error: substrate::CompatMachineTransitionError,");
    pushln!(out, "    phase: Phase,");
    pushln!(out, "    trigger: TriggerDiscriminant,");
    pushln!(out, ") -> TransitionError {{");
    pushln!(out, "    match error {{");
    pushln!(
        out,
        "        substrate::CompatMachineTransitionError::NoMatchingTransition {{ .. }} => {{"
    );
    pushln!(
        out,
        "            let rejections = guard_rejections_for_trigger(&phase, &trigger);"
    );
    pushln!(out, "            if rejections.is_empty() {{");
    pushln!(
        out,
        "                TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {{ phase, trigger }})"
    );
    pushln!(out, "            }} else {{");
    pushln!(
        out,
        "                TransitionError::Refusal(TransitionRefusal::GuardRejected {{ rejections }})"
    );
    pushln!(out, "            }}");
    pushln!(out, "        }}");
    pushln!(
        out,
        "        substrate::CompatMachineTransitionError::GuardRejected {{ .. }} => {{"
    );
    pushln!(
        out,
        "            TransitionError::Refusal(TransitionRefusal::GuardRejected {{ rejections: guard_rejections_for_trigger(&phase, &trigger) }})"
    );
    pushln!(out, "        }}");
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn guard_rejections_for_trigger(phase: &Phase, trigger: &TriggerDiscriminant) -> Vec<GuardRejection> {{"
    );
    let mut grouped_rejections =
        std::collections::BTreeMap::<(String, String), Vec<(String, usize)>>::new();
    for transition in &schema.transitions {
        if transition.guards.is_empty() {
            continue;
        }
        let trigger_expr = match transition.on.kind {
            TriggerKind::Input => format!(
                "TriggerDiscriminant::Input(InputKind::{})",
                transition.on.variant
            ),
            TriggerKind::Signal => format!(
                "TriggerDiscriminant::Signal(SignalKind::{})",
                transition.on.variant
            ),
        };
        for from_phase in &transition.from {
            grouped_rejections
                .entry((from_phase.clone(), trigger_expr.clone()))
                .or_default()
                .extend(
                    transition
                        .guards
                        .iter()
                        .enumerate()
                        .map(|(index, _)| (transition.name.clone(), index + 1)),
                );
        }
    }
    pushln!(out, "    match (phase, trigger) {{");
    for ((phase_name, trigger_expr), rejections) in grouped_rejections {
        pushln!(
            out,
            "        (Phase::{phase_name}, {trigger_expr}) => vec!["
        );
        for (transition_name, index) in rejections {
            pushln!(
                out,
                "            GuardRejection {{ transition_id: TransitionId::{}, guard_id: GuardId::{}Guard{} }},",
                transition_name,
                transition_name,
                index
            );
        }
        pushln!(out, "        ],");
    }
    pushln!(out, "        _ => Vec::new(),");
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_legacy_machine_kernel_module(schema: &MachineSchema) -> String {
    let mut out = String::new();
    let schema_fn = schema_constructor_path(schema);

    pushln!(
        &mut out,
        "// Generated by `cargo xtask machine-codegen --all`."
    );
    pushln!(&mut out, "use crate::runtime::{{");
    pushln!(
        &mut out,
        "    GeneratedMachineKernel, KernelInput, KernelSignal, KernelState, KernelValue, TransitionOutcome, TransitionRefusal,"
    );
    pushln!(&mut out, "}};");
    pushln!(&mut out);
    pushln!(
        &mut out,
        "pub fn schema() -> meerkat_machine_schema::MachineSchema {{"
    );
    pushln!(&mut out, "    {schema_fn}()");
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
    pushln!(&mut out, "pub fn transition_signal(");
    pushln!(&mut out, "    state: &KernelState,");
    pushln!(&mut out, "    signal: &KernelSignal,");
    pushln!(
        &mut out,
        ") -> Result<TransitionOutcome, TransitionRefusal> {{"
    );
    pushln!(&mut out, "    kernel().transition_signal(state, signal)");
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
fn render_compat_payload_module(out: &mut String, module_name: &str, variants: &[VariantSchema]) {
    pushln!(out, "pub mod {module_name} {{");
    pushln!(out, "    use super::compat_types;");
    pushln!(out);
    for variant in variants {
        pushln!(out, "    #[derive(Debug, Clone, PartialEq, Eq)]");
        if variant.fields.is_empty() {
            pushln!(out, "    pub struct {};", variant.name);
        } else {
            pushln!(out, "    pub struct {} {{", variant.name);
            for field in &variant.fields {
                pushln!(
                    out,
                    "        pub {}: {},",
                    field.name,
                    compat_rust_type(&field.ty)
                );
            }
            pushln!(out, "    }}");
        }
        pushln!(out);
    }
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_variant_enum(
    out: &mut String,
    enum_name: &str,
    kind_name: &str,
    module_name: &str,
    variants: &[VariantSchema],
) {
    pushln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]");
    pushln!(out, "pub enum {enum_name} {{");
    for variant in variants {
        pushln!(
            out,
            "    {}({module_name}::{}),",
            variant.name,
            variant.name
        );
    }
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]"
    );
    pushln!(out, "pub enum {kind_name} {{");
    for variant in variants {
        pushln!(out, "    {},", variant.name);
    }
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "impl {enum_name} {{");
    pushln!(out, "    pub fn kind(&self) -> {kind_name} {{");
    pushln!(out, "        match self {{");
    for variant in variants {
        pushln!(
            out,
            "            Self::{}(_) => {kind_name}::{},",
            variant.name,
            variant.name
        );
    }
    pushln!(out, "        }}");
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_transition_id_enum(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]"
    );
    pushln!(out, "pub enum TransitionId {{");
    for transition in &schema.transitions {
        pushln!(out, "    {},", rust_enum_variant_ident(&transition.name));
    }
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn transition_id_from_str(name: &str) -> Option<TransitionId> {{"
    );
    pushln!(out, "    match name {{");
    for transition in &schema.transitions {
        pushln!(
            out,
            "        \"{}\" => Some(TransitionId::{}),",
            transition.name,
            rust_enum_variant_ident(&transition.name)
        );
    }
    pushln!(out, "        _ => None,");
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_guard_id_enum(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]"
    );
    pushln!(out, "pub enum GuardId {{");
    for transition in &schema.transitions {
        for index in 0..transition.guards.len() {
            pushln!(
                out,
                "    {}Guard{},",
                rust_enum_variant_ident(&transition.name),
                index + 1
            );
        }
    }
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_helper_id_enum(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]"
    );
    pushln!(out, "pub enum HelperId {{");
    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        pushln!(out, "    {},", rust_enum_variant_ident(&helper.name));
    }
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_state_struct(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
    );
    pushln!(out, "pub struct State {{");
    pushln!(out, "    pub phase: Phase,");
    for field in &schema.state.fields {
        pushln!(
            out,
            "    pub {}: {},",
            field.name,
            compat_rust_type(&field.ty)
        );
    }
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "impl Default for State {{");
    pushln!(out, "    fn default() -> Self {{");
    pushln!(out, "        initial_state()");
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_input_to_kernel(out: &mut String, schema: &MachineSchema) {
    pushln!(out, "fn input_to_kernel(input: &Input) -> LegacyInput {{");
    pushln!(out, "    match input {{");
    for variant in &schema.inputs.variants {
        let payload_binding = if variant.fields.is_empty() {
            "_payload"
        } else {
            "payload"
        };
        pushln!(
            out,
            "        Input::{}({payload_binding}) => LegacyInput {{",
            variant.name,
        );
        pushln!(
            out,
            "            variant: \"{}\".to_string(),",
            variant.name
        );
        if variant.fields.is_empty() {
            pushln!(
                out,
                "            fields: std::collections::BTreeMap::new(),"
            );
        } else {
            pushln!(
                out,
                "            fields: std::collections::BTreeMap::from(["
            );
            for field in &variant.fields {
                pushln!(
                    out,
                    "                (\"{}\".to_string(), {}),",
                    field.name,
                    compat_value_to_kernel_call(
                        &field.ty,
                        &format!("payload.{}", field.name),
                        false
                    )
                );
            }
            pushln!(out, "            ]),");
        }
        pushln!(out, "        }},");
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_signal_to_kernel(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "fn signal_to_kernel(signal: &Signal) -> LegacySignal {{"
    );
    pushln!(out, "    match signal {{");
    for variant in &schema.signals.variants {
        let payload_binding = if variant.fields.is_empty() {
            "_payload"
        } else {
            "payload"
        };
        pushln!(
            out,
            "        Signal::{}({payload_binding}) => LegacySignal {{",
            variant.name,
        );
        pushln!(
            out,
            "            variant: \"{}\".to_string(),",
            variant.name
        );
        if variant.fields.is_empty() {
            pushln!(
                out,
                "            fields: std::collections::BTreeMap::new(),"
            );
        } else {
            pushln!(
                out,
                "            fields: std::collections::BTreeMap::from(["
            );
            for field in &variant.fields {
                pushln!(
                    out,
                    "                (\"{}\".to_string(), {}),",
                    field.name,
                    compat_value_to_kernel_call(
                        &field.ty,
                        &format!("payload.{}", field.name),
                        false
                    )
                );
            }
            pushln!(out, "            ]),");
        }
        pushln!(out, "        }},");
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_effect_from_kernel(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "fn effect_from_kernel(effect: LegacyEffect) -> Result<Effect, KernelError> {{"
    );
    pushln!(out, "    match effect.variant.as_str() {{");
    for variant in &schema.effects.variants {
        pushln!(out, "        \"{}\" => {{", variant.name);
        if variant.fields.is_empty() {
            pushln!(
                out,
                "            Ok(Effect::{}(effects::{}))",
                variant.name,
                variant.name
            );
        } else {
            pushln!(out, "            let mut fields = effect.fields;");
            pushln!(
                out,
                "            Ok(Effect::{}(effects::{} {{",
                variant.name,
                variant.name
            );
            for field in &variant.fields {
                pushln!(
                    out,
                    "                {}: {}?,",
                    field.name,
                    compat_value_from_kernel_call(
                        &field.ty,
                        &format!(
                            "take_field(&mut fields, \"effect `{}`\", \"{}\")?",
                            variant.name, field.name
                        ),
                        &format!("\"effect `{}.{}`\"", variant.name, field.name)
                    )
                );
            }
            pushln!(out, "            }}))");
        }
        pushln!(out, "        }},");
    }
    pushln!(
        out,
        "        other => Err(KernelError::CodegenInvariant {{ detail: format!(\"unknown effect variant `{{other}}`\") }}),"
    );
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_effect_to_kernel(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "#[cfg(feature = \"test-oracle\")]\nfn effect_to_kernel(effect: &Effect) -> LegacyEffect {{"
    );
    pushln!(out, "    match effect {{");
    for variant in &schema.effects.variants {
        pushln!(
            out,
            "        Effect::{}(payload) => LegacyEffect {{",
            variant.name
        );
        pushln!(
            out,
            "            variant: \"{}\".to_string(),",
            variant.name
        );
        if variant.fields.is_empty() {
            pushln!(
                out,
                "            fields: std::collections::BTreeMap::new(),"
            );
        } else {
            pushln!(
                out,
                "            fields: std::collections::BTreeMap::from(["
            );
            for field in &variant.fields {
                pushln!(
                    out,
                    "                (\"{}\".to_string(), {}),",
                    field.name,
                    compat_value_to_kernel_call(
                        &field.ty,
                        &format!("payload.{}", field.name),
                        false
                    )
                );
            }
            pushln!(out, "            ]),");
        }
        pushln!(out, "        }},");
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "#[cfg(feature = \"test-oracle\")]");
    pushln!(
        out,
        "pub fn to_test_oracle_effect(effect: &Effect) -> crate::test_oracle::KernelEffect {{"
    );
    pushln!(out, "    effect_to_kernel(effect)");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_state_converters(out: &mut String, schema: &MachineSchema) {
    pushln!(out, "fn state_to_kernel(state: &State) -> LegacyState {{");
    pushln!(out, "    LegacyState {{");
    pushln!(
        out,
        "        phase: phase_to_string(&state.phase).to_string(),"
    );
    pushln!(out, "        fields: std::collections::BTreeMap::from([");
    for field in &schema.state.fields {
        pushln!(
            out,
            "            (\"{}\".to_string(), {}),",
            field.name,
            compat_value_to_kernel_call(&field.ty, &format!("state.{}", field.name), false)
        );
    }
    pushln!(out, "        ]),");
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn state_from_kernel(state: LegacyState) -> Result<State, KernelError> {{"
    );
    pushln!(out, "    let mut fields = state.fields;");
    pushln!(out, "    Ok(State {{");
    pushln!(out, "        phase: phase_from_string(&state.phase)?,");
    for field in &schema.state.fields {
        pushln!(
            out,
            "        {}: {}?,",
            field.name,
            compat_value_from_kernel_call(
                &field.ty,
                &format!("take_field(&mut fields, \"state\", \"{}\")?", field.name),
                &format!("\"state field `{}`\"", field.name)
            )
        );
    }
    pushln!(out, "    }})");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "#[cfg(feature = \"test-oracle\")]");
    pushln!(
        out,
        "pub fn from_test_oracle_state(state: crate::test_oracle::KernelState) -> Result<State, KernelError> {{"
    );
    pushln!(out, "    state_from_kernel(state)");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "#[cfg(feature = \"test-oracle\")]");
    pushln!(
        out,
        "pub fn to_test_oracle_state(state: &State) -> crate::test_oracle::KernelState {{"
    );
    pushln!(out, "    state_to_kernel(state)");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "fn phase_to_string(phase: &Phase) -> &'static str {{");
    pushln!(out, "    match phase {{");
    for variant in &schema.state.phase.variants {
        pushln!(
            out,
            "        Phase::{} => \"{}\",",
            variant.name,
            variant.name
        );
    }
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn phase_from_string(phase: &str) -> Result<Phase, KernelError> {{"
    );
    pushln!(out, "    match phase {{");
    for variant in &schema.state.phase.variants {
        pushln!(
            out,
            "        \"{}\" => Ok(Phase::{}),",
            variant.name,
            variant.name
        );
    }
    pushln!(
        out,
        "        other => Err(KernelError::CodegenInvariant {{ detail: format!(\"unknown phase `{{other}}`\") }}),"
    );
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "fn take_field(");
    pushln!(
        out,
        "    fields: &mut std::collections::BTreeMap<String, KernelValue>,"
    );
    pushln!(out, "    container: &str,");
    pushln!(out, "    field: &str,");
    pushln!(out, ") -> Result<KernelValue, KernelError> {{");
    pushln!(
        out,
        "    fields.remove(field).ok_or_else(|| KernelError::CodegenInvariant {{"
    );
    pushln!(
        out,
        "        detail: format!(\"{{container}} missing field `{{field}}`\"),"
    );
    pushln!(out, "    }})");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_outcome_converters(out: &mut String, _schema: &MachineSchema) {
    pushln!(
        out,
        "fn outcome_from_kernel(outcome: LegacyTransitionOutcome) -> Result<Outcome, KernelError> {{"
    );
    pushln!(
        out,
        "    let transition_id = transition_id_from_str(&outcome.transition).ok_or_else(|| KernelError::CodegenInvariant {{"
    );
    pushln!(
        out,
        "        detail: format!(\"unknown transition id `{{}}`\", outcome.transition),"
    );
    pushln!(out, "    }})?;");
    pushln!(out, "    Ok(Outcome {{");
    pushln!(out, "        transition_id,");
    pushln!(
        out,
        "        next_state: state_from_kernel(outcome.next_state)?,"
    );
    pushln!(
        out,
        "        effects: outcome.effects.into_iter().map(effect_from_kernel).collect::<Result<Vec<_>, _>>()?,"
    );
    pushln!(out, "    }})");
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_compat_error_helpers(out: &mut String, schema: &MachineSchema) {
    pushln!(out, "fn map_legacy_error(");
    pushln!(out, "    error: LegacyTransitionRefusal,");
    pushln!(out, "    phase: Phase,");
    pushln!(out, "    trigger: TriggerDiscriminant,");
    pushln!(out, ") -> TransitionError {{");
    pushln!(out, "    match error {{");
    pushln!(
        out,
        "        LegacyTransitionRefusal::NoMatchingTransition {{ .. }} => {{"
    );
    pushln!(
        out,
        "            let rejections = guard_rejections_for_trigger(&phase, &trigger);"
    );
    pushln!(out, "            if rejections.is_empty() {{");
    pushln!(
        out,
        "                TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {{ phase, trigger }})"
    );
    pushln!(out, "            }} else {{");
    pushln!(
        out,
        "                TransitionError::Refusal(TransitionRefusal::GuardRejected {{ rejections }})"
    );
    pushln!(out, "            }}");
    pushln!(out, "        }}");
    pushln!(
        out,
        "        LegacyTransitionRefusal::AmbiguousTransition {{ transitions, .. }} => {{"
    );
    pushln!(out, "            let mut typed = Vec::new();");
    pushln!(out, "            for transition in transitions {{");
    pushln!(
        out,
        "                let Some(transition_id) = transition_id_from_str(&transition) else {{"
    );
    pushln!(
        out,
        "                    return TransitionError::Kernel(KernelError::CodegenInvariant {{"
    );
    pushln!(
        out,
        "                        detail: format!(\"unknown transition `{{transition}}` in ambiguity report\"),"
    );
    pushln!(out, "                    }});");
    pushln!(out, "                }};");
    pushln!(out, "                typed.push(transition_id);");
    pushln!(out, "            }}");
    pushln!(
        out,
        "            TransitionError::Refusal(TransitionRefusal::AmbiguousTransition {{ transitions: typed }})"
    );
    pushln!(out, "        }}");
    pushln!(
        out,
        "        LegacyTransitionRefusal::EvaluationError {{ transition, reason, .. }} => {{"
    );
    pushln!(
        out,
        "            if let Some(transition_id) = transition_id_from_str(&transition) {{"
    );
    pushln!(
        out,
        "                TransitionError::Kernel(KernelError::ContextViolation {{ transition_id, detail: reason }})"
    );
    pushln!(out, "            }} else {{");
    pushln!(
        out,
        "                TransitionError::Kernel(KernelError::CodegenInvariant {{"
    );
    pushln!(
        out,
        "                    detail: format!(\"unknown transition `{{transition}}` in evaluation error: {{reason}}\"),"
    );
    pushln!(out, "                }})");
    pushln!(out, "            }}");
    pushln!(out, "        }}");
    pushln!(
        out,
        "        other => TransitionError::Kernel(KernelError::CodegenInvariant {{ detail: other.to_string() }}),"
    );
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(out, "#[allow(dead_code)]");
    pushln!(
        out,
        "fn map_helper_error(helper_id: HelperId, error: LegacyTransitionRefusal) -> KernelError {{"
    );
    pushln!(out, "    match error {{");
    pushln!(
        out,
        "        LegacyTransitionRefusal::EvaluationError {{ reason, .. }} => KernelError::HelperEvaluation {{ helper_id, detail: reason }},"
    );
    pushln!(
        out,
        "        other => KernelError::CodegenInvariant {{ detail: other.to_string() }},"
    );
    pushln!(out, "    }}");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn guard_rejections_for_trigger(phase: &Phase, trigger: &TriggerDiscriminant) -> Vec<GuardRejection> {{"
    );
    let mut grouped_rejections =
        std::collections::BTreeMap::<(String, String), Vec<(String, usize)>>::new();
    for transition in &schema.transitions {
        if transition.guards.is_empty() {
            continue;
        }
        let trigger_expr = match transition.on.kind {
            TriggerKind::Input => format!(
                "TriggerDiscriminant::Input({}Kind::{})",
                schema.inputs.name, transition.on.variant
            ),
            TriggerKind::Signal => format!(
                "TriggerDiscriminant::Signal({}Kind::{})",
                schema.signals.name, transition.on.variant
            ),
        };
        for from_phase in &transition.from {
            let entry = grouped_rejections
                .entry((from_phase.clone(), trigger_expr.clone()))
                .or_default();
            for (index, _) in transition.guards.iter().enumerate() {
                entry.push((transition.name.clone(), index + 1));
            }
        }
    }
    if grouped_rejections.is_empty() {
        pushln!(out, "    let _ = (phase, trigger);");
        pushln!(out, "    Vec::new()");
    } else {
        pushln!(out, "    match (phase, trigger) {{");
        for ((from_phase, trigger_expr), rejections) in grouped_rejections {
            pushln!(
                out,
                "        (Phase::{}, {}) => vec![",
                from_phase,
                trigger_expr
            );
            for (transition_name, index) in rejections {
                pushln!(
                    out,
                    "            GuardRejection {{ transition_id: TransitionId::{}, guard_id: GuardId::{}Guard{} }},",
                    rust_enum_variant_ident(&transition_name),
                    rust_enum_variant_ident(&transition_name),
                    index
                );
            }
            pushln!(out, "        ],");
        }
        pushln!(out, "        _ => Vec::new(),");
        pushln!(out, "    }}");
    }
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_value_helpers(out: &mut String, schema: &MachineSchema) {
    pushln!(
        out,
        "fn kernel_option_some(value: KernelValue) -> KernelValue {{"
    );
    pushln!(
        out,
        "    KernelValue::Map(std::collections::BTreeMap::from([("
    );
    pushln!(out, "        KernelValue::String(\"value\".to_string()),");
    pushln!(out, "        value,");
    pushln!(out, "    )]))");
    pushln!(out, "}}");
    pushln!(out);
    pushln!(
        out,
        "fn kernel_type_error(context: &str, expected: &str, value: &KernelValue) -> KernelError {{"
    );
    pushln!(
        out,
        "    KernelError::CodegenInvariant {{ detail: format!(\"{{context}} expected {{expected}}, found {{value:?}}\") }}"
    );
    pushln!(out, "}}");
    pushln!(out);

    for ty in collect_compat_type_refs(schema) {
        render_compat_value_to_kernel_fn(out, &ty);
        render_compat_value_from_kernel_fn(out, schema, &ty);
    }
}

#[cfg(not(test))]
fn compat_rust_type(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "bool".into(),
        TypeRef::U32 => "u32".into(),
        TypeRef::U64 => "u64".into(),
        TypeRef::String => "String".into(),
        TypeRef::Named(name) | TypeRef::Enum(name) => format!("compat_types::{name}"),
        TypeRef::Option(inner) => format!("Option<{}>", compat_rust_type(inner)),
        TypeRef::Set(inner) => format!("std::collections::BTreeSet<{}>", compat_rust_type(inner)),
        TypeRef::Seq(inner) => format!("Vec<{}>", compat_rust_type(inner)),
        TypeRef::Map(key, value) => format!(
            "std::collections::BTreeMap<{}, {}>",
            compat_rust_type(key),
            compat_rust_type(value)
        ),
    }
}

#[cfg(not(test))]
fn compat_type_slug(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "bool".into(),
        TypeRef::U32 => "u32".into(),
        TypeRef::U64 => "u64".into(),
        TypeRef::String => "string".into(),
        TypeRef::Named(name) => format!("named_{}", to_snake_case(name)),
        TypeRef::Enum(name) => format!("enum_{}", to_snake_case(name)),
        TypeRef::Option(inner) => format!("option_{}", compat_type_slug(inner)),
        TypeRef::Set(inner) => format!("set_{}", compat_type_slug(inner)),
        TypeRef::Seq(inner) => format!("seq_{}", compat_type_slug(inner)),
        TypeRef::Map(key, value) => {
            format!("map_{}_{}", compat_type_slug(key), compat_type_slug(value))
        }
    }
}

#[cfg(not(test))]
fn compat_value_to_kernel_call(ty: &TypeRef, expr: &str, already_ref: bool) -> String {
    if already_ref {
        format!("value_to_kernel_{}({expr})", compat_type_slug(ty))
    } else {
        format!("value_to_kernel_{}(&{expr})", compat_type_slug(ty))
    }
}

#[cfg(not(test))]
fn compat_value_from_kernel_call(ty: &TypeRef, expr: &str, context: &str) -> String {
    format!(
        "value_from_kernel_{}({expr}, {context})",
        compat_type_slug(ty)
    )
}

#[cfg(not(test))]
fn compat_expected_type_label(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "bool".into(),
        TypeRef::U32 => "u32".into(),
        TypeRef::U64 => "u64".into(),
        TypeRef::String => "string".into(),
        TypeRef::Named(name) | TypeRef::Enum(name) => name.clone(),
        TypeRef::Option(inner) => format!("Option({})", compat_expected_type_label(inner)),
        TypeRef::Set(inner) => format!("Set({})", compat_expected_type_label(inner)),
        TypeRef::Seq(inner) => format!("Seq({})", compat_expected_type_label(inner)),
        TypeRef::Map(key, value) => format!(
            "Map({}, {})",
            compat_expected_type_label(key),
            compat_expected_type_label(value)
        ),
    }
}

#[cfg(not(test))]
fn collect_compat_type_refs(schema: &MachineSchema) -> Vec<TypeRef> {
    fn collect(
        ty: &TypeRef,
        seen: &mut std::collections::BTreeSet<String>,
        out: &mut Vec<TypeRef>,
    ) {
        match ty {
            TypeRef::Option(inner) | TypeRef::Set(inner) | TypeRef::Seq(inner) => {
                collect(inner, seen, out);
            }
            TypeRef::Map(key, value) => {
                collect(key, seen, out);
                collect(value, seen, out);
            }
            TypeRef::Bool
            | TypeRef::U32
            | TypeRef::U64
            | TypeRef::String
            | TypeRef::Named(_)
            | TypeRef::Enum(_) => {}
        }

        let slug = compat_type_slug(ty);
        if seen.insert(slug) {
            out.push(ty.clone());
        }
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for field in &schema.state.fields {
        collect(&field.ty, &mut seen, &mut out);
    }
    for variant in &schema.inputs.variants {
        for field in &variant.fields {
            collect(&field.ty, &mut seen, &mut out);
        }
    }
    for variant in &schema.signals.variants {
        for field in &variant.fields {
            collect(&field.ty, &mut seen, &mut out);
        }
    }
    for variant in &schema.effects.variants {
        for field in &variant.fields {
            collect(&field.ty, &mut seen, &mut out);
        }
    }
    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        for field in &helper.params {
            collect(&field.ty, &mut seen, &mut out);
        }
        collect(&helper.returns, &mut seen, &mut out);
    }
    out
}

#[cfg(not(test))]
fn render_compat_value_to_kernel_fn(out: &mut String, ty: &TypeRef) {
    let fn_name = format!("value_to_kernel_{}", compat_type_slug(ty));
    let rust_ty = compat_rust_type(ty);
    pushln!(out, "#[allow(dead_code)]");
    pushln!(out, "fn {fn_name}(value: &{rust_ty}) -> KernelValue {{");
    match ty {
        TypeRef::Bool => pushln!(out, "    KernelValue::Bool(*value)"),
        TypeRef::U32 => pushln!(out, "    KernelValue::U64(u64::from(*value))"),
        TypeRef::U64 => pushln!(out, "    KernelValue::U64(*value)"),
        TypeRef::String => pushln!(out, "    KernelValue::String(value.clone())"),
        TypeRef::Named(name) if compat_named_type_is_u64(name) => {
            pushln!(out, "    KernelValue::U64(value.0)")
        }
        TypeRef::Named(_) => pushln!(out, "    KernelValue::String(value.to_string())"),
        TypeRef::Enum(name) => {
            pushln!(out, "    let variant = match value {{");
            for variant in compat_enum_variants(name) {
                pushln!(
                    out,
                    "        compat_types::{}::{} => \"{}\",",
                    name,
                    variant,
                    variant
                );
            }
            pushln!(out, "    }};");
            pushln!(
                out,
                "    KernelValue::NamedVariant {{ enum_name: \"{}\".to_string(), variant: variant.to_string() }}",
                name
            );
        }
        TypeRef::Option(inner) => {
            pushln!(out, "    match value {{");
            pushln!(
                out,
                "        Some(inner) => kernel_option_some({}),",
                compat_value_to_kernel_call(inner, "inner", true)
            );
            pushln!(out, "        None => KernelValue::None,");
            pushln!(out, "    }}");
        }
        TypeRef::Set(inner) => {
            pushln!(
                out,
                "    KernelValue::Set(value.iter().map(|item| {}).collect())",
                compat_value_to_kernel_call(inner, "item", true)
            );
        }
        TypeRef::Seq(inner) => {
            pushln!(
                out,
                "    KernelValue::Seq(value.iter().map(|item| {}).collect())",
                compat_value_to_kernel_call(inner, "item", true)
            );
        }
        TypeRef::Map(key, value) => {
            pushln!(
                out,
                "    KernelValue::Map(value.iter().map(|(key, value)| ({}, {})).collect())",
                compat_value_to_kernel_call(key, "key", true),
                compat_value_to_kernel_call(value, "value", true)
            );
        }
    }
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn render_compat_value_from_kernel_fn(out: &mut String, _schema: &MachineSchema, ty: &TypeRef) {
    let fn_name = format!("value_from_kernel_{}", compat_type_slug(ty));
    let rust_ty = compat_rust_type(ty);
    let expected = compat_expected_type_label(ty);
    pushln!(out, "#[allow(dead_code)]");
    pushln!(
        out,
        "fn {fn_name}(value: KernelValue, context: &str) -> Result<{rust_ty}, KernelError> {{"
    );
    match ty {
        TypeRef::Bool => {
            pushln!(out, "    match value {{");
            pushln!(out, "        KernelValue::Bool(value) => Ok(value),");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::U32 => {
            pushln!(out, "    match value {{");
            pushln!(
                out,
                "        KernelValue::U64(value) => u32::try_from(value).map_err(|_| KernelError::CodegenInvariant {{"
            );
            pushln!(
                out,
                "            detail: format!(\"{{context}} expected u32-compatible value, found {{value}}\")"
            );
            pushln!(out, "        }}),");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::U64 => {
            pushln!(out, "    match value {{");
            pushln!(out, "        KernelValue::U64(value) => Ok(value),");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::String => {
            pushln!(out, "    match value {{");
            pushln!(out, "        KernelValue::String(value) => Ok(value),");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::Named(name) if compat_named_type_is_u64(name) => {
            pushln!(out, "    match value {{");
            pushln!(
                out,
                "        KernelValue::U64(value) => Ok(compat_types::{name}(value)),"
            );
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::Named(name) => {
            pushln!(out, "    match value {{");
            pushln!(
                out,
                "        KernelValue::String(value) => Ok(compat_types::{name}(value)),"
            );
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::Enum(name) => {
            pushln!(out, "    match value {{");
            pushln!(
                out,
                "        KernelValue::NamedVariant {{ enum_name, variant }} if enum_name == \"{name}\" => match variant.as_str() {{"
            );
            for variant in compat_enum_variants(name) {
                pushln!(
                    out,
                    "            \"{}\" => Ok(compat_types::{}::{}),",
                    variant,
                    name,
                    variant
                );
            }
            pushln!(
                out,
                "            other => Err(KernelError::CodegenInvariant {{ detail: format!(\"{{context}} expected {name} variant, found `{{other}}`\") }}),"
            );
            pushln!(out, "        }},");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::Option(inner) => {
            pushln!(out, "    match value {{");
            pushln!(out, "        KernelValue::None => Ok(None),");
            pushln!(out, "        KernelValue::Map(mut entries) => {{");
            pushln!(
                out,
                "            if let Some(inner) = entries.remove(&KernelValue::String(\"value\".to_string())) {{"
            );
            pushln!(
                out,
                "                Ok(Some({}?))",
                compat_value_from_kernel_call(inner, "inner", "context")
            );
            pushln!(out, "            }} else {{");
            pushln!(
                out,
                "                Err(KernelError::CodegenInvariant {{ detail: format!(\"{{context}} expected option value payload\") }})"
            );
            pushln!(out, "            }}");
            pushln!(out, "        }}");
            pushln!(
                out,
                "        other => Ok(Some({}?))",
                compat_value_from_kernel_call(inner, "other", "context")
            );
            pushln!(out, "    }}");
        }
        TypeRef::Set(inner) => {
            pushln!(out, "    match value {{");
            pushln!(out, "        KernelValue::Set(values) => {{");
            pushln!(
                out,
                "            let mut out = std::collections::BTreeSet::new();"
            );
            pushln!(out, "            for value in values {{");
            pushln!(
                out,
                "                out.insert({}?);",
                compat_value_from_kernel_call(inner, "value", "context")
            );
            pushln!(out, "            }}");
            pushln!(out, "            Ok(out)");
            pushln!(out, "        }}");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::Seq(inner) => {
            pushln!(out, "    match value {{");
            pushln!(out, "        KernelValue::Seq(values) => {{");
            pushln!(out, "            let mut out = Vec::new();");
            pushln!(out, "            for value in values {{");
            pushln!(
                out,
                "                out.push({}?);",
                compat_value_from_kernel_call(inner, "value", "context")
            );
            pushln!(out, "            }}");
            pushln!(out, "            Ok(out)");
            pushln!(out, "        }}");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
        TypeRef::Map(key, value) => {
            pushln!(out, "    match value {{");
            pushln!(out, "        KernelValue::Map(values) => {{");
            pushln!(
                out,
                "            let mut out = std::collections::BTreeMap::new();"
            );
            pushln!(out, "            for (key, value) in values {{");
            pushln!(
                out,
                "                out.insert({}?, {}?);",
                compat_value_from_kernel_call(key, "key", "context"),
                compat_value_from_kernel_call(value, "value", "context")
            );
            pushln!(out, "            }}");
            pushln!(out, "            Ok(out)");
            pushln!(out, "        }}");
            pushln!(
                out,
                "        other => Err(kernel_type_error(context, \"{expected}\", &other)),"
            );
            pushln!(out, "    }}");
        }
    }
    pushln!(out, "}}");
    pushln!(out);
}

#[cfg(not(test))]
fn compat_named_type_is_u64(name: &str) -> bool {
    matches!(
        name,
        "BoundarySequence" | "TurnNumber" | "FenceToken" | "Generation"
    )
}

#[cfg(not(test))]
fn compat_enum_variants(name: &str) -> Vec<&'static str> {
    match name {
        "FlowRunStatus" => vec![
            "Absent",
            "Pending",
            "Running",
            "Completed",
            "Failed",
            "Canceled",
        ],
        "StepRunStatus" => vec!["Dispatched", "Completed", "Failed", "Skipped", "Canceled"],
        "DependencyMode" => vec!["All", "Any"],
        "CollectionPolicyKind" => vec!["All", "Any", "Quorum"],
        "FlowNodeKind" => vec!["Step", "Loop"],
        "FrameScope" => vec!["Root", "Body"],
        "NodeRunStatus" => vec![
            "Pending",
            "Ready",
            "Running",
            "Completed",
            "Failed",
            "Skipped",
            "Canceled",
        ],
        "LoopIterationStage" => {
            vec!["AwaitingBodyFrame", "BodyFrameActive", "AwaitingUntil"]
        }
        _ => Vec::new(),
    }
}

#[cfg(not(test))]
fn is_compat_machine(schema: &MachineSchema) -> bool {
    matches!(
        schema.machine.as_str(),
        "FlowRunMachine" | "FlowFrameMachine" | "LoopIterationMachine"
    )
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
fn dsl_machine_slug(machine_name: &str) -> String {
    match machine_name {
        "MeerkatMachine" => "meerkat_machine".into(),
        "MobMachine" => "mob_machine".into(),
        "AuthMachine" => "auth_machine".into(),
        "ScheduleLifecycleMachine" => "schedule_lifecycle".into(),
        "OccurrenceLifecycleMachine" => "occurrence_lifecycle".into(),
        _ => machine_slug(machine_name),
    }
}

#[cfg(not(test))]
fn schema_constructor_path(schema: &MachineSchema) -> String {
    match schema.machine.as_str() {
        "MeerkatMachine" => "meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine".into(),
        "MobMachine" => "meerkat_machine_schema::catalog::dsl::dsl_mob_machine".into(),
        "AuthMachine" => "meerkat_machine_schema::catalog::dsl::dsl_auth_machine".into(),
        "ScheduleLifecycleMachine" => {
            "meerkat_machine_schema::catalog::dsl::dsl_schedule_lifecycle_machine".into()
        }
        "OccurrenceLifecycleMachine" => {
            "meerkat_machine_schema::catalog::dsl::dsl_occurrence_lifecycle_machine".into()
        }
        "FlowRunMachine" => "meerkat_machine_schema::flow_run_machine".into(),
        "FlowFrameMachine" => "meerkat_machine_schema::flow_frame_machine".into(),
        "LoopIterationMachine" => "meerkat_machine_schema::loop_iteration_machine".into(),
        other => {
            let slug = dsl_machine_slug(other);
            format!("meerkat_machine_schema::catalog::dsl::dsl_{slug}")
        }
    }
}

#[cfg(not(test))]
fn canonical_phase_field_name(schema: &MachineSchema) -> Option<&str> {
    if matches!(
        schema.machine.as_str(),
        "MeerkatMachine"
            | "MobMachine"
            | "ScheduleLifecycleMachine"
            | "OccurrenceLifecycleMachine"
            | "AuthMachine"
    ) {
        return Some("lifecycle_phase");
    }
    if schema
        .state
        .fields
        .iter()
        .any(|field| field.name == "phase")
    {
        return Some("phase");
    }
    let phase_name = schema.state.phase.name.as_str();
    schema
        .state
        .fields
        .iter()
        .find_map(|field| match &field.ty {
            TypeRef::Named(name) | TypeRef::Enum(name) if name == phase_name => {
                Some(field.name.as_str())
            }
            _ => None,
        })
}

#[cfg(not(test))]
fn rust_type_for_substrate(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "bool".into(),
        TypeRef::U32 => "u32".into(),
        TypeRef::U64 => "u64".into(),
        TypeRef::String => "String".into(),
        TypeRef::Named(name) | TypeRef::Enum(name) => format!("substrate::{name}"),
        TypeRef::Option(inner) => format!("Option<{}>", rust_type_for_substrate(inner)),
        TypeRef::Set(inner) => {
            format!(
                "std::collections::BTreeSet<{}>",
                rust_type_for_substrate(inner)
            )
        }
        TypeRef::Seq(inner) => format!("Vec<{}>", rust_type_for_substrate(inner)),
        TypeRef::Map(key, value) => format!(
            "std::collections::BTreeMap<{}, {}>",
            rust_type_for_substrate(key),
            rust_type_for_substrate(value)
        ),
    }
}

#[cfg(not(test))]
fn type_is_copy_for_substrate(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 => true,
        TypeRef::Enum(name) => matches!(
            name.as_str(),
            "RealtimeBindingState"
                | "LiveTopologyPhase"
                | "RealtimeProductTurnPhase"
                | "RealtimeProjectionFreshness"
                | "RealtimeReconnectPolicy"
                | "PeerIngressOwnerKind"
                | "SupervisorBindingKind"
                | "MisfirePolicy"
                | "OverlapPolicy"
                | "MissingTargetPolicy"
        ),
        TypeRef::Option(inner) => type_is_copy_for_substrate(inner),
        TypeRef::String
        | TypeRef::Named(_)
        | TypeRef::Set(_)
        | TypeRef::Seq(_)
        | TypeRef::Map(_, _) => false,
    }
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
fn rust_enum_variant_ident(value: &str) -> String {
    let mut out = String::new();
    for chunk in value
        .split(['_', '-', ' '])
        .filter(|chunk| !chunk.is_empty())
    {
        let mut chars = chunk.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    out
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

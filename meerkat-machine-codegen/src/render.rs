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

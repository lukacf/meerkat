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
        return match module_name.as_str() {
            "loop_iteration" => render_loop_iteration_modeled_module(),
            "flow_frame" => render_flow_frame_modeled_module(),
            "flow_run" => render_flow_run_modeled_module(),
            _ => render_canonical_stub_modeled_module(schema),
        };
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
        "mod source {{\n#![allow(clippy::expect_used, clippy::assign_op_pattern)]\n{source}\n}}\npub use source::*;\n\npub fn schema() -> meerkat_machine_schema::MachineSchema {{\n    meerkat_machine_schema::{catalog_fn}()\n}}\n"
    )
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_loop_iteration_modeled_module() -> String {
    r#"// Generated by `cargo xtask machine-codegen --all`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::implicit_clone,
    clippy::unnecessary_cast,
    clippy::redundant_clone
)]

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    meerkat_machine_schema::loop_iteration_machine()
}

pub use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LoopIterationStage {
    AwaitingBodyFrame,
    BodyFrameActive,
    AwaitingUntil,
}
impl LoopIterationStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingBodyFrame => "AwaitingBodyFrame",
            Self::BodyFrameActive => "BodyFrameActive",
            Self::AwaitingUntil => "AwaitingUntil",
        }
    }
}
impl std::fmt::Display for LoopIterationStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait Context {}
pub struct EmptyContext;
impl Context for EmptyContext {}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Absent,
    Running,
    Completed,
    Exhausted,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub phase: Phase,
    pub loop_instance_id: LoopInstanceId,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
    pub depth: u32,
    pub stage: LoopIterationStage,
    pub current_iteration: u32,
    pub last_completed_iteration: u32,
    pub max_iterations: u32,
    pub active_body_frame_id: Option<FrameId>,
}
impl Default for State {
    fn default() -> Self {
        initial_state()
    }
}

pub mod inputs {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct StartLoop {
        pub loop_instance_id: LoopInstanceId,
        pub max_iterations: u32,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
        pub loop_id: LoopId,
        pub depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameStarted {
        pub loop_instance_id: LoopInstanceId,
        pub frame_id: FrameId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameCompleted {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameFailed {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameCanceled {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct UntilConditionMet {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct UntilConditionFailed {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CancelLoop {
        pub loop_instance_id: LoopInstanceId,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input {
    StartLoop(inputs::StartLoop),
    BodyFrameStarted(inputs::BodyFrameStarted),
    BodyFrameCompleted(inputs::BodyFrameCompleted),
    BodyFrameFailed(inputs::BodyFrameFailed),
    BodyFrameCanceled(inputs::BodyFrameCanceled),
    UntilConditionMet(inputs::UntilConditionMet),
    UntilConditionFailed(inputs::UntilConditionFailed),
    CancelLoop(inputs::CancelLoop),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputKind {
    StartLoop,
    BodyFrameStarted,
    BodyFrameCompleted,
    BodyFrameFailed,
    BodyFrameCanceled,
    UntilConditionMet,
    UntilConditionFailed,
    CancelLoop,
}

pub mod effects {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RequestBodyFrameStart {
        pub loop_instance_id: LoopInstanceId,
        pub depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EvaluateUntilCondition {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
        pub loop_id: LoopId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopCompleted {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopExhausted {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopFailed {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopCanceled {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    RequestBodyFrameStart(effects::RequestBodyFrameStart),
    EvaluateUntilCondition(effects::EvaluateUntilCondition),
    LoopCompleted(effects::LoopCompleted),
    LoopExhausted(effects::LoopExhausted),
    LoopFailed(effects::LoopFailed),
    LoopCanceled(effects::LoopCanceled),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    RequestBodyFrameStart,
    EvaluateUntilCondition,
    LoopCompleted,
    LoopExhausted,
    LoopFailed,
    LoopCanceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionId {
    StartLoop,
    BodyFrameStarted,
    BodyFrameCompleted,
    BodyFrameFailed,
    BodyFrameCanceled,
    UntilConditionMet,
    UntilConditionFailed,
    CancelLoop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuardId {
    Phase,
    LoopInstanceId,
    Stage,
    Iteration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperId {
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub transition_id: TransitionId,
    pub next_state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionError {
    Refusal(TransitionRefusal),
    Kernel(KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionRefusal {
    NoMatchingTransition {
        phase: Phase,
        trigger: TriggerDiscriminant,
    },
    GuardRejected {
        rejections: Vec<GuardRejection>,
    },
    AmbiguousTransition {
        transitions: Vec<TransitionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TriggerDiscriminant {
    Input(InputKind),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuardRejection {
    pub transition_id: TransitionId,
    pub guard_id: GuardId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KernelError {
    ContextViolation {
        transition_id: TransitionId,
        detail: String,
    },
    HelperEvaluation {
        helper_id: HelperId,
        detail: String,
    },
    CodegenInvariant {
        detail: String,
    },
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
        phase: Phase::Absent,
        loop_instance_id: LoopInstanceId::from(String::new()),
        parent_frame_id: FrameId::from(String::new()),
        parent_node_id: FlowNodeId::from(String::new()),
        loop_id: LoopId::from(String::new()),
        depth: 0,
        stage: LoopIterationStage::AwaitingBodyFrame,
        current_iteration: 0,
        last_completed_iteration: 0,
        max_iterations: 0,
        active_body_frame_id: None,
    }
}

fn guard(transition_id: TransitionId, guard_id: GuardId) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::GuardRejected {
        rejections: vec![GuardRejection {
            transition_id,
            guard_id,
        }],
    })
}

pub fn transition<C: Context>(
    state: &State,
    input: Input,
    context: &C,
) -> Result<Outcome, TransitionError> {
    let _ = context;
    match input {
        Input::StartLoop(payload) => {
            if state.phase != Phase::Absent {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::StartLoop),
                }));
            }
            let next_state = State {
                phase: Phase::Running,
                loop_instance_id: payload.loop_instance_id.clone(),
                parent_frame_id: payload.parent_frame_id.clone(),
                parent_node_id: payload.parent_node_id.clone(),
                loop_id: payload.loop_id.clone(),
                depth: payload.depth,
                stage: LoopIterationStage::AwaitingBodyFrame,
                current_iteration: 1,
                last_completed_iteration: 0,
                max_iterations: payload.max_iterations,
                active_body_frame_id: None,
            };
            Ok(Outcome {
                transition_id: TransitionId::StartLoop,
                effects: vec![Effect::RequestBodyFrameStart(effects::RequestBodyFrameStart {
                    loop_instance_id: payload.loop_instance_id,
                    depth: payload.depth,
                })],
                next_state,
            })
        }
        Input::BodyFrameStarted(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::BodyFrameStarted),
                }));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::BodyFrameStarted, GuardId::LoopInstanceId));
            }
            if state.stage != LoopIterationStage::AwaitingBodyFrame {
                return Err(guard(TransitionId::BodyFrameStarted, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameStarted, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.stage = LoopIterationStage::BodyFrameActive;
            next_state.active_body_frame_id = Some(payload.frame_id);
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameStarted,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::BodyFrameCompleted(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::BodyFrameCompleted),
                }));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::BodyFrameCompleted, GuardId::LoopInstanceId));
            }
            if state.stage != LoopIterationStage::BodyFrameActive {
                return Err(guard(TransitionId::BodyFrameCompleted, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameCompleted, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.stage = LoopIterationStage::AwaitingUntil;
            next_state.last_completed_iteration = payload.iteration;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameCompleted,
                effects: vec![Effect::EvaluateUntilCondition(
                    effects::EvaluateUntilCondition {
                        loop_instance_id: state.loop_instance_id.clone(),
                        iteration: payload.iteration,
                        parent_frame_id: state.parent_frame_id.clone(),
                        parent_node_id: state.parent_node_id.clone(),
                        loop_id: state.loop_id.clone(),
                    },
                )],
                next_state,
            })
        }
        Input::BodyFrameFailed(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::BodyFrameFailed),
                }));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::BodyFrameFailed, GuardId::LoopInstanceId));
            }
            if state.stage != LoopIterationStage::BodyFrameActive {
                return Err(guard(TransitionId::BodyFrameFailed, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameFailed, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Failed;
            next_state.last_completed_iteration = payload.iteration;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameFailed,
                effects: vec![Effect::LoopFailed(effects::LoopFailed {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
        Input::BodyFrameCanceled(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::BodyFrameCanceled),
                }));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::BodyFrameCanceled, GuardId::LoopInstanceId));
            }
            if state.stage != LoopIterationStage::BodyFrameActive {
                return Err(guard(TransitionId::BodyFrameCanceled, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameCanceled, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Canceled;
            next_state.last_completed_iteration = payload.iteration;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameCanceled,
                effects: vec![Effect::LoopCanceled(effects::LoopCanceled {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
        Input::UntilConditionMet(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::UntilConditionMet),
                }));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::UntilConditionMet, GuardId::LoopInstanceId));
            }
            if state.stage != LoopIterationStage::AwaitingUntil {
                return Err(guard(TransitionId::UntilConditionMet, GuardId::Stage));
            }
            if state.last_completed_iteration != payload.iteration {
                return Err(guard(TransitionId::UntilConditionMet, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Completed;
            Ok(Outcome {
                transition_id: TransitionId::UntilConditionMet,
                effects: vec![Effect::LoopCompleted(effects::LoopCompleted {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
        Input::UntilConditionFailed(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::UntilConditionFailed),
                }));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::UntilConditionFailed, GuardId::LoopInstanceId));
            }
            if state.stage != LoopIterationStage::AwaitingUntil {
                return Err(guard(TransitionId::UntilConditionFailed, GuardId::Stage));
            }
            if state.last_completed_iteration != payload.iteration {
                return Err(guard(TransitionId::UntilConditionFailed, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            let effects = if state.current_iteration >= state.max_iterations {
                next_state.phase = Phase::Exhausted;
                vec![Effect::LoopExhausted(effects::LoopExhausted {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })]
            } else {
                next_state.current_iteration = state.current_iteration + 1;
                next_state.stage = LoopIterationStage::AwaitingBodyFrame;
                vec![Effect::RequestBodyFrameStart(effects::RequestBodyFrameStart {
                    loop_instance_id: state.loop_instance_id.clone(),
                    depth: state.depth,
                })]
            };
            Ok(Outcome {
                transition_id: TransitionId::UntilConditionFailed,
                next_state,
                effects,
            })
        }
        Input::CancelLoop(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
                    phase: state.phase.clone(),
                    trigger: TriggerDiscriminant::Input(InputKind::CancelLoop),
                }));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::CancelLoop, GuardId::LoopInstanceId));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Canceled;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::CancelLoop,
                effects: vec![Effect::LoopCanceled(effects::LoopCanceled {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
    }
}
"#
    .to_string()
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_flow_frame_modeled_module() -> String {
    r#"// Generated by `cargo xtask machine-codegen --all`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::implicit_clone,
    clippy::unnecessary_cast,
    clippy::redundant_clone
)]

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    meerkat_machine_schema::flow_frame_machine()
}

pub use crate::ids::{BranchId, FlowNodeId, FrameId, LoopInstanceId};

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DependencyMode {
    All,
    Any,
}
impl DependencyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Any => "Any",
        }
    }
}
impl std::fmt::Display for DependencyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FlowNodeKind {
    Step,
    Loop,
}
impl FlowNodeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Step => "Step",
            Self::Loop => "Loop",
        }
    }
}
impl std::fmt::Display for FlowNodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FrameScope {
    Root,
    Body,
}
impl FrameScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Root => "Root",
            Self::Body => "Body",
        }
    }
}
impl std::fmt::Display for FrameScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NodeRunStatus {
    Pending,
    Ready,
    Running,
    Completed,
    Failed,
    Skipped,
    Canceled,
}
impl NodeRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Ready => "Ready",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Skipped => "Skipped",
            Self::Canceled => "Canceled",
        }
    }
}
impl std::fmt::Display for NodeRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait Context {}
pub struct EmptyContext;
impl Context for EmptyContext {}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Absent,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub phase: Phase,
    pub frame_id: FrameId,
    pub frame_scope: FrameScope,
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub last_admitted_node: FlowNodeId,
    pub tracked_nodes: std::collections::BTreeSet<FlowNodeId>,
    pub ordered_nodes: Vec<FlowNodeId>,
    pub node_kind: std::collections::BTreeMap<FlowNodeId, FlowNodeKind>,
    pub node_dependencies: std::collections::BTreeMap<FlowNodeId, Vec<FlowNodeId>>,
    pub node_dependency_modes: std::collections::BTreeMap<FlowNodeId, DependencyMode>,
    pub node_branches: std::collections::BTreeMap<FlowNodeId, Option<BranchId>>,
    pub branch_winners: std::collections::BTreeSet<BranchId>,
    pub node_status: std::collections::BTreeMap<FlowNodeId, NodeRunStatus>,
    pub ready_queue: Vec<FlowNodeId>,
    pub output_recorded: std::collections::BTreeMap<FlowNodeId, bool>,
    pub node_condition_results: std::collections::BTreeMap<FlowNodeId, Option<bool>>,
}
impl Default for State {
    fn default() -> Self {
        initial_state()
    }
}

pub mod inputs {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct StartRootFrame {
        pub frame_id: FrameId,
        pub tracked_nodes: std::collections::BTreeSet<FlowNodeId>,
        pub ordered_nodes: Vec<FlowNodeId>,
        pub node_kind: std::collections::BTreeMap<FlowNodeId, FlowNodeKind>,
        pub node_dependencies: std::collections::BTreeMap<FlowNodeId, Vec<FlowNodeId>>,
        pub node_dependency_modes: std::collections::BTreeMap<FlowNodeId, DependencyMode>,
        pub node_branches: std::collections::BTreeMap<FlowNodeId, Option<BranchId>>,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct StartBodyFrame {
        pub frame_id: FrameId,
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
        pub tracked_nodes: std::collections::BTreeSet<FlowNodeId>,
        pub ordered_nodes: Vec<FlowNodeId>,
        pub node_kind: std::collections::BTreeMap<FlowNodeId, FlowNodeKind>,
        pub node_dependencies: std::collections::BTreeMap<FlowNodeId, Vec<FlowNodeId>>,
        pub node_dependency_modes: std::collections::BTreeMap<FlowNodeId, DependencyMode>,
        pub node_branches: std::collections::BTreeMap<FlowNodeId, Option<BranchId>>,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct AdmitNextReadyNode {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CompleteNode {
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordNodeOutput {
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct FailNode {
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct SkipNode {
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CancelNode {
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct SealFrame {}
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input {
    StartRootFrame(inputs::StartRootFrame),
    StartBodyFrame(inputs::StartBodyFrame),
    AdmitNextReadyNode(inputs::AdmitNextReadyNode),
    CompleteNode(inputs::CompleteNode),
    RecordNodeOutput(inputs::RecordNodeOutput),
    FailNode(inputs::FailNode),
    SkipNode(inputs::SkipNode),
    CancelNode(inputs::CancelNode),
    SealFrame(inputs::SealFrame),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputKind {
    StartRootFrame,
    StartBodyFrame,
    AdmitNextReadyNode,
    CompleteNode,
    RecordNodeOutput,
    FailNode,
    SkipNode,
    CancelNode,
    SealFrame,
}

pub mod effects {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ReadyFrontierChanged {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct AdmitStepWork {
        pub frame_id: FrameId,
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct StartLoopNode {
        pub frame_id: FrameId,
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PersistStepOutput {
        pub frame_id: FrameId,
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct NodeExecutionReleased {
        pub frame_id: FrameId,
        pub node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RootFrameCompleted {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RootFrameFailed {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RootFrameCanceled {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameCompleted {
        pub frame_id: FrameId,
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameFailed {
        pub frame_id: FrameId,
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameCanceled {
        pub frame_id: FrameId,
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    ReadyFrontierChanged(effects::ReadyFrontierChanged),
    AdmitStepWork(effects::AdmitStepWork),
    StartLoopNode(effects::StartLoopNode),
    PersistStepOutput(effects::PersistStepOutput),
    NodeExecutionReleased(effects::NodeExecutionReleased),
    RootFrameCompleted(effects::RootFrameCompleted),
    RootFrameFailed(effects::RootFrameFailed),
    RootFrameCanceled(effects::RootFrameCanceled),
    BodyFrameCompleted(effects::BodyFrameCompleted),
    BodyFrameFailed(effects::BodyFrameFailed),
    BodyFrameCanceled(effects::BodyFrameCanceled),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    ReadyFrontierChanged,
    AdmitStepWork,
    StartLoopNode,
    PersistStepOutput,
    NodeExecutionReleased,
    RootFrameCompleted,
    RootFrameFailed,
    RootFrameCanceled,
    BodyFrameCompleted,
    BodyFrameFailed,
    BodyFrameCanceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionId {
    StartRootFrame,
    StartBodyFrame,
    AdmitNextReadyNode,
    CompleteNode,
    RecordNodeOutput,
    FailNode,
    SkipNode,
    CancelNode,
    SealFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuardId {
    Phase,
    NodeExists,
    NodeRunning,
    ReadyFrontier,
    Sealable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperId {
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub transition_id: TransitionId,
    pub next_state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionError {
    Refusal(TransitionRefusal),
    Kernel(KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionRefusal {
    NoMatchingTransition {
        phase: Phase,
        trigger: TriggerDiscriminant,
    },
    GuardRejected {
        rejections: Vec<GuardRejection>,
    },
    AmbiguousTransition {
        transitions: Vec<TransitionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TriggerDiscriminant {
    Input(InputKind),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuardRejection {
    pub transition_id: TransitionId,
    pub guard_id: GuardId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KernelError {
    ContextViolation {
        transition_id: TransitionId,
        detail: String,
    },
    HelperEvaluation {
        helper_id: HelperId,
        detail: String,
    },
    CodegenInvariant {
        detail: String,
    },
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
        phase: Phase::Absent,
        frame_id: FrameId::from(String::new()),
        frame_scope: FrameScope::Root,
        loop_instance_id: LoopInstanceId::from(String::new()),
        iteration: 0,
        last_admitted_node: FlowNodeId::from(String::new()),
        tracked_nodes: Default::default(),
        ordered_nodes: Vec::new(),
        node_kind: Default::default(),
        node_dependencies: Default::default(),
        node_dependency_modes: Default::default(),
        node_branches: Default::default(),
        branch_winners: Default::default(),
        node_status: Default::default(),
        ready_queue: Vec::new(),
        output_recorded: Default::default(),
        node_condition_results: Default::default(),
    }
}

fn refusal(phase: &Phase, kind: InputKind) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
        phase: phase.clone(),
        trigger: TriggerDiscriminant::Input(kind),
    })
}

fn guard(transition_id: TransitionId, guard_id: GuardId) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::GuardRejected {
        rejections: vec![GuardRejection { transition_id, guard_id }],
    })
}

fn is_terminal(status: NodeRunStatus) -> bool {
    matches!(
        status,
        NodeRunStatus::Completed
            | NodeRunStatus::Failed
            | NodeRunStatus::Skipped
            | NodeRunStatus::Canceled
    )
}

fn dependency_satisfied(status: NodeRunStatus) -> bool {
    matches!(status, NodeRunStatus::Completed | NodeRunStatus::Skipped)
}

fn dependency_terminal_failure(status: NodeRunStatus) -> bool {
    matches!(status, NodeRunStatus::Failed | NodeRunStatus::Canceled)
}

fn refresh_ready_frontier(state: &mut State) {
    state.ready_queue.clear();

    for node_id in state.ordered_nodes.clone() {
        let Some(status) = state.node_status.get(&node_id).copied() else {
            continue;
        };
        if status == NodeRunStatus::Ready {
            state.ready_queue.push(node_id);
            continue;
        }
        if status != NodeRunStatus::Pending {
            continue;
        }
        if let Some(branch) = state.node_branches.get(&node_id).and_then(|b| b.as_ref())
            && state.branch_winners.contains(branch)
        {
            continue;
        }
        let deps = state.node_dependencies.get(&node_id).cloned().unwrap_or_default();
        let dep_mode = state
            .node_dependency_modes
            .get(&node_id)
            .copied()
            .unwrap_or(DependencyMode::All);
        let dep_ok = if deps.is_empty() {
            true
        } else {
            match dep_mode {
                DependencyMode::All => deps.iter().all(|dep| {
                    state
                        .node_status
                        .get(dep)
                        .copied()
                        .is_some_and(dependency_satisfied)
                }),
                DependencyMode::Any => deps.iter().any(|dep| {
                    state
                        .node_status
                        .get(dep)
                        .copied()
                        .is_some_and(dependency_satisfied)
                }),
            }
        };
        if dep_ok {
            state.node_status.insert(node_id.clone(), NodeRunStatus::Ready);
            state.ready_queue.push(node_id);
        }
    }
}

fn propagate_blocked_nodes(state: &mut State) {
    loop {
        let mut changed = false;
        for node_id in state.ordered_nodes.clone() {
            let Some(status) = state.node_status.get(&node_id).copied() else {
                continue;
            };
            if status != NodeRunStatus::Pending && status != NodeRunStatus::Ready {
                continue;
            }
            let deps = state.node_dependencies.get(&node_id).cloned().unwrap_or_default();
            if deps.is_empty() {
                continue;
            }
            let dep_mode = state
                .node_dependency_modes
                .get(&node_id)
                .copied()
                .unwrap_or(DependencyMode::All);
            let should_skip = match dep_mode {
                DependencyMode::All => deps.iter().any(|dep| {
                    state
                        .node_status
                        .get(dep)
                        .copied()
                        .is_some_and(dependency_terminal_failure)
                }),
                DependencyMode::Any => deps.iter().all(|dep| {
                    state
                        .node_status
                        .get(dep)
                        .copied()
                        .is_some_and(|status| {
                            status == NodeRunStatus::Skipped || dependency_terminal_failure(status)
                        })
                }),
            };
            if should_skip {
                state.node_status.insert(node_id.clone(), NodeRunStatus::Skipped);
                state.ready_queue.retain(|candidate| candidate != &node_id);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn apply_branch_winner(state: &mut State, winner_node: &FlowNodeId) {
    let Some(branch) = state
        .node_branches
        .get(winner_node)
        .and_then(std::clone::Clone::clone)
    else {
        return;
    };
    state.branch_winners.insert(branch.clone());
    for node_id in state.ordered_nodes.clone() {
        if node_id == *winner_node {
            continue;
        }
        if state
            .node_branches
            .get(&node_id)
            .and_then(|candidate| candidate.as_ref())
            != Some(&branch)
        {
            continue;
        }
        if let Some(status) = state.node_status.get(&node_id).copied()
            && !is_terminal(status)
            && status != NodeRunStatus::Running
        {
            state.node_status.insert(node_id, NodeRunStatus::Skipped);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn start_frame_state(
    frame_id: FrameId,
    frame_scope: FrameScope,
    loop_instance_id: LoopInstanceId,
    iteration: u32,
    tracked_nodes: std::collections::BTreeSet<FlowNodeId>,
    ordered_nodes: Vec<FlowNodeId>,
    node_kind: std::collections::BTreeMap<FlowNodeId, FlowNodeKind>,
    node_dependencies: std::collections::BTreeMap<FlowNodeId, Vec<FlowNodeId>>,
    node_dependency_modes: std::collections::BTreeMap<FlowNodeId, DependencyMode>,
    node_branches: std::collections::BTreeMap<FlowNodeId, Option<BranchId>>,
) -> State {
    let mut state = State {
        phase: Phase::Running,
        frame_id,
        frame_scope,
        loop_instance_id,
        iteration,
        last_admitted_node: FlowNodeId::from(String::new()),
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
        branch_winners: Default::default(),
        node_status: Default::default(),
        ready_queue: Vec::new(),
        output_recorded: Default::default(),
        node_condition_results: Default::default(),
    };
    for node_id in state.ordered_nodes.clone() {
        state.node_status.insert(node_id.clone(), NodeRunStatus::Pending);
        state.output_recorded.insert(node_id.clone(), false);
        state.node_condition_results.insert(node_id, None);
    }
    refresh_ready_frontier(&mut state);
    state
}

fn admit_next_ready_node(mut state: State) -> Result<Outcome, TransitionError> {
    if state.phase != Phase::Running {
        return Err(refusal(&state.phase, InputKind::AdmitNextReadyNode));
    }
    refresh_ready_frontier(&mut state);
    let Some(node_id) = state.ready_queue.first().cloned() else {
        return Err(guard(TransitionId::AdmitNextReadyNode, GuardId::ReadyFrontier));
    };
    state.ready_queue.retain(|candidate| candidate != &node_id);
    state.last_admitted_node = node_id.clone();
    state.node_status.insert(node_id.clone(), NodeRunStatus::Running);
    let effect = match state.node_kind.get(&node_id).copied() {
        Some(FlowNodeKind::Step) => Effect::AdmitStepWork(effects::AdmitStepWork {
            frame_id: state.frame_id.clone(),
            node_id,
        }),
        Some(FlowNodeKind::Loop) => Effect::StartLoopNode(effects::StartLoopNode {
            frame_id: state.frame_id.clone(),
            node_id,
        }),
        None => return Err(guard(TransitionId::AdmitNextReadyNode, GuardId::NodeExists)),
    };
    Ok(Outcome {
        transition_id: TransitionId::AdmitNextReadyNode,
        next_state: state,
        effects: vec![effect],
    })
}

fn apply_node_terminal(
    state: &State,
    node_id: FlowNodeId,
    status: NodeRunStatus,
    transition_id: TransitionId,
) -> Result<Outcome, TransitionError> {
    if state.phase != Phase::Running {
        return Err(refusal(
            &state.phase,
            match transition_id {
                TransitionId::CompleteNode => InputKind::CompleteNode,
                TransitionId::FailNode => InputKind::FailNode,
                TransitionId::SkipNode => InputKind::SkipNode,
                TransitionId::CancelNode => InputKind::CancelNode,
                _ => unreachable!(),
            },
        ));
    }
    let Some(current) = state.node_status.get(&node_id).copied() else {
        return Err(guard(transition_id, GuardId::NodeExists));
    };
    if current != NodeRunStatus::Running {
        return Err(guard(transition_id, GuardId::NodeRunning));
    }
    let mut next_state = state.clone();
    next_state.node_status.insert(node_id.clone(), status);
    if status == NodeRunStatus::Completed {
        apply_branch_winner(&mut next_state, &node_id);
    }
    refresh_ready_frontier(&mut next_state);
    propagate_blocked_nodes(&mut next_state);
    refresh_ready_frontier(&mut next_state);
    Ok(Outcome {
        transition_id,
        next_state,
        effects: vec![Effect::NodeExecutionReleased(effects::NodeExecutionReleased {
            frame_id: state.frame_id.clone(),
            node_id,
        })],
    })
}

fn seal_frame(state: &State) -> Result<Outcome, TransitionError> {
    if state.phase != Phase::Running {
        return Err(refusal(&state.phase, InputKind::SealFrame));
    }
    if !state
        .ordered_nodes
        .iter()
        .all(|node_id| state.node_status.get(node_id).copied().is_some_and(is_terminal))
    {
        return Err(guard(TransitionId::SealFrame, GuardId::Sealable));
    }
    let mut next_state = state.clone();
    let has_failed = state
        .node_status
        .values()
        .copied()
        .any(|status| status == NodeRunStatus::Failed);
    let has_canceled = state
        .node_status
        .values()
        .copied()
        .any(|status| status == NodeRunStatus::Canceled);
    let effect = if has_failed {
        next_state.phase = Phase::Failed;
        match state.frame_scope {
            FrameScope::Root => Effect::RootFrameFailed(effects::RootFrameFailed {
                frame_id: state.frame_id.clone(),
            }),
            FrameScope::Body => Effect::BodyFrameFailed(effects::BodyFrameFailed {
                frame_id: state.frame_id.clone(),
                loop_instance_id: state.loop_instance_id.clone(),
                iteration: state.iteration,
            }),
        }
    } else if has_canceled {
        next_state.phase = Phase::Canceled;
        match state.frame_scope {
            FrameScope::Root => Effect::RootFrameCanceled(effects::RootFrameCanceled {
                frame_id: state.frame_id.clone(),
            }),
            FrameScope::Body => Effect::BodyFrameCanceled(effects::BodyFrameCanceled {
                frame_id: state.frame_id.clone(),
                loop_instance_id: state.loop_instance_id.clone(),
                iteration: state.iteration,
            }),
        }
    } else {
        next_state.phase = Phase::Completed;
        match state.frame_scope {
            FrameScope::Root => Effect::RootFrameCompleted(effects::RootFrameCompleted {
                frame_id: state.frame_id.clone(),
            }),
            FrameScope::Body => Effect::BodyFrameCompleted(effects::BodyFrameCompleted {
                frame_id: state.frame_id.clone(),
                loop_instance_id: state.loop_instance_id.clone(),
                iteration: state.iteration,
            }),
        }
    };
    Ok(Outcome {
        transition_id: TransitionId::SealFrame,
        next_state,
        effects: vec![effect],
    })
}

pub fn transition<C: Context>(
    state: &State,
    input: Input,
    context: &C,
) -> Result<Outcome, TransitionError> {
    let _ = context;
    match input {
        Input::StartRootFrame(payload) => {
            if state.phase != Phase::Absent {
                return Err(refusal(&state.phase, InputKind::StartRootFrame));
            }
            Ok(Outcome {
                transition_id: TransitionId::StartRootFrame,
                next_state: start_frame_state(
                    payload.frame_id,
                    FrameScope::Root,
                    LoopInstanceId::from(String::new()),
                    0,
                    payload.tracked_nodes,
                    payload.ordered_nodes,
                    payload.node_kind,
                    payload.node_dependencies,
                    payload.node_dependency_modes,
                    payload.node_branches,
                ),
                effects: Vec::new(),
            })
        }
        Input::StartBodyFrame(payload) => {
            if state.phase != Phase::Absent {
                return Err(refusal(&state.phase, InputKind::StartBodyFrame));
            }
            Ok(Outcome {
                transition_id: TransitionId::StartBodyFrame,
                next_state: start_frame_state(
                    payload.frame_id,
                    FrameScope::Body,
                    payload.loop_instance_id,
                    payload.iteration,
                    payload.tracked_nodes,
                    payload.ordered_nodes,
                    payload.node_kind,
                    payload.node_dependencies,
                    payload.node_dependency_modes,
                    payload.node_branches,
                ),
                effects: Vec::new(),
            })
        }
        Input::AdmitNextReadyNode(_) => admit_next_ready_node(state.clone()),
        Input::CompleteNode(payload) => {
            apply_node_terminal(state, payload.node_id, NodeRunStatus::Completed, TransitionId::CompleteNode)
        }
        Input::RecordNodeOutput(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordNodeOutput));
            }
            let mut next_state = state.clone();
            if !next_state.node_status.contains_key(&payload.node_id) {
                return Err(guard(TransitionId::RecordNodeOutput, GuardId::NodeExists));
            }
            next_state.output_recorded.insert(payload.node_id.clone(), true);
            Ok(Outcome {
                transition_id: TransitionId::RecordNodeOutput,
                effects: vec![Effect::PersistStepOutput(effects::PersistStepOutput {
                    frame_id: state.frame_id.clone(),
                    node_id: payload.node_id,
                })],
                next_state,
            })
        }
        Input::FailNode(payload) => {
            apply_node_terminal(state, payload.node_id, NodeRunStatus::Failed, TransitionId::FailNode)
        }
        Input::SkipNode(payload) => {
            apply_node_terminal(state, payload.node_id, NodeRunStatus::Skipped, TransitionId::SkipNode)
        }
        Input::CancelNode(payload) => {
            apply_node_terminal(state, payload.node_id, NodeRunStatus::Canceled, TransitionId::CancelNode)
        }
        Input::SealFrame(_) => seal_frame(state),
    }
}
"#
    .to_string()
}

#[cfg(not(test))]
#[allow(dead_code)]
fn render_flow_run_modeled_module() -> String {
    r#"// Generated by `cargo xtask machine-codegen --all`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::implicit_clone,
    clippy::unnecessary_cast,
    clippy::redundant_clone
)]

pub fn schema() -> meerkat_machine_schema::MachineSchema {
    meerkat_machine_schema::flow_run_machine()
}

pub use crate::ids::{BranchId, FrameId, LoopInstanceId, StepId};
pub use crate::ids::MeerkatId;

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CollectionPolicyKind {
    All,
    Any,
    Quorum,
}
impl CollectionPolicyKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Any => "Any",
            Self::Quorum => "Quorum",
        }
    }
}
impl std::fmt::Display for CollectionPolicyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DependencyMode {
    All,
    Any,
}
impl DependencyMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Any => "Any",
        }
    }
}
impl std::fmt::Display for DependencyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FlowRunStatus {
    Absent,
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}
impl FlowRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Absent => "Absent",
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Canceled => "Canceled",
        }
    }
}
impl std::fmt::Display for FlowRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StepRunStatus {
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}
impl StepRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dispatched => "Dispatched",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Skipped => "Skipped",
            Self::Canceled => "Canceled",
        }
    }
}
impl std::fmt::Display for StepRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait Context {}
pub struct EmptyContext;
impl Context for EmptyContext {}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Absent,
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub phase: Phase,
    pub tracked_steps: std::collections::BTreeSet<StepId>,
    pub ordered_steps: Vec<StepId>,
    pub step_status: std::collections::BTreeMap<StepId, Option<StepRunStatus>>,
    pub output_recorded: std::collections::BTreeMap<StepId, bool>,
    pub step_condition_results: std::collections::BTreeMap<StepId, Option<bool>>,
    pub step_has_conditions: std::collections::BTreeMap<StepId, bool>,
    pub step_dependencies: std::collections::BTreeMap<StepId, Vec<StepId>>,
    pub step_dependency_modes: std::collections::BTreeMap<StepId, DependencyMode>,
    pub step_branches: std::collections::BTreeMap<StepId, Option<BranchId>>,
    pub step_collection_policies: std::collections::BTreeMap<StepId, CollectionPolicyKind>,
    pub step_quorum_thresholds: std::collections::BTreeMap<StepId, u32>,
    pub step_target_counts: std::collections::BTreeMap<StepId, u32>,
    pub step_target_success_counts: std::collections::BTreeMap<StepId, u32>,
    pub step_target_terminal_failure_counts: std::collections::BTreeMap<StepId, u32>,
    pub target_retry_counts: std::collections::BTreeMap<String, u32>,
    pub failure_count: u32,
    pub consecutive_failure_count: u32,
    pub escalation_threshold: u32,
    pub max_step_retries: u32,
    pub ready_frames: Vec<FrameId>,
    pub ready_frame_membership: std::collections::BTreeSet<FrameId>,
    pub pending_body_frame_loops: Vec<LoopInstanceId>,
    pub pending_body_frame_loop_membership: std::collections::BTreeSet<LoopInstanceId>,
    pub active_node_count: u32,
    pub active_frame_count: u32,
    pub max_active_nodes: u32,
    pub max_active_frames: u32,
    pub max_frame_depth: u32,
    pub last_granted_frame: FrameId,
    pub last_granted_loop: LoopInstanceId,
}
impl Default for State {
    fn default() -> Self {
        initial_state()
    }
}

pub mod inputs {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CreateRun {
        pub step_ids: Vec<StepId>,
        pub ordered_steps: Vec<StepId>,
        pub step_has_conditions: std::collections::BTreeMap<StepId, bool>,
        pub step_dependencies: std::collections::BTreeMap<StepId, Vec<StepId>>,
        pub step_dependency_modes: std::collections::BTreeMap<StepId, DependencyMode>,
        pub step_branches: std::collections::BTreeMap<StepId, Option<BranchId>>,
        pub step_collection_policies: std::collections::BTreeMap<StepId, CollectionPolicyKind>,
        pub step_quorum_thresholds: std::collections::BTreeMap<StepId, u32>,
        pub escalation_threshold: u32,
        pub max_step_retries: u32,
        pub max_active_nodes: u32,
        pub max_active_frames: u32,
        pub max_frame_depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct StartRun {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct DispatchStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CompleteStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordStepOutput {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ConditionPassed {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ConditionRejected {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct FailStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct SkipStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectFrameStepStatus {
        pub step_id: StepId,
        pub step_status: StepRunStatus,
        pub append_failure_ledger: bool,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CancelStep {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RegisterTargets {
        pub step_id: StepId,
        pub target_count: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetSuccess {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetTerminalFailure {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetCanceled {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RecordTargetFailure {
        pub step_id: StepId,
        pub target_id: MeerkatId,
        pub retry_key: String,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RegisterReadyFrame {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PumpNodeScheduler {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RegisterPendingBodyFrame {
        pub loop_instance_id: LoopInstanceId,
        pub depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PumpFrameScheduler {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct NodeExecutionReleased {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct FrameTerminated {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TerminalizeCompleted {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TerminalizeFailed {}
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct TerminalizeCanceled {}
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input {
    CreateRun(inputs::CreateRun),
    StartRun(inputs::StartRun),
    DispatchStep(inputs::DispatchStep),
    CompleteStep(inputs::CompleteStep),
    RecordStepOutput(inputs::RecordStepOutput),
    ConditionPassed(inputs::ConditionPassed),
    ConditionRejected(inputs::ConditionRejected),
    FailStep(inputs::FailStep),
    SkipStep(inputs::SkipStep),
    ProjectFrameStepStatus(inputs::ProjectFrameStepStatus),
    CancelStep(inputs::CancelStep),
    RegisterTargets(inputs::RegisterTargets),
    RecordTargetSuccess(inputs::RecordTargetSuccess),
    RecordTargetTerminalFailure(inputs::RecordTargetTerminalFailure),
    RecordTargetCanceled(inputs::RecordTargetCanceled),
    RecordTargetFailure(inputs::RecordTargetFailure),
    RegisterReadyFrame(inputs::RegisterReadyFrame),
    PumpNodeScheduler(inputs::PumpNodeScheduler),
    RegisterPendingBodyFrame(inputs::RegisterPendingBodyFrame),
    PumpFrameScheduler(inputs::PumpFrameScheduler),
    NodeExecutionReleased(inputs::NodeExecutionReleased),
    FrameTerminated(inputs::FrameTerminated),
    TerminalizeCompleted(inputs::TerminalizeCompleted),
    TerminalizeFailed(inputs::TerminalizeFailed),
    TerminalizeCanceled(inputs::TerminalizeCanceled),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputKind {
    CreateRun,
    StartRun,
    DispatchStep,
    CompleteStep,
    RecordStepOutput,
    ConditionPassed,
    ConditionRejected,
    FailStep,
    SkipStep,
    ProjectFrameStepStatus,
    CancelStep,
    RegisterTargets,
    RecordTargetSuccess,
    RecordTargetTerminalFailure,
    RecordTargetCanceled,
    RecordTargetFailure,
    RegisterReadyFrame,
    PumpNodeScheduler,
    RegisterPendingBodyFrame,
    PumpFrameScheduler,
    NodeExecutionReleased,
    FrameTerminated,
    TerminalizeCompleted,
    TerminalizeFailed,
    TerminalizeCanceled,
}

pub mod effects {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EmitFlowRunNotice {
        pub run_status: FlowRunStatus,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EmitStepNotice {
        pub step_id: StepId,
        pub step_status: StepRunStatus,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct AppendFailureLedger {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct PersistStepOutput {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct AdmitStepWork {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct FlowTerminalized {
        pub run_status: FlowRunStatus,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EscalateSupervisor {
        pub step_id: StepId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectTargetSuccess {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectTargetFailure {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct ProjectTargetCanceled {
        pub step_id: StepId,
        pub target_id: MeerkatId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct GrantNodeSlot {
        pub frame_id: FrameId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct GrantBodyFrameStart {
        pub loop_instance_id: LoopInstanceId,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    EmitFlowRunNotice(effects::EmitFlowRunNotice),
    EmitStepNotice(effects::EmitStepNotice),
    AppendFailureLedger(effects::AppendFailureLedger),
    PersistStepOutput(effects::PersistStepOutput),
    AdmitStepWork(effects::AdmitStepWork),
    FlowTerminalized(effects::FlowTerminalized),
    EscalateSupervisor(effects::EscalateSupervisor),
    ProjectTargetSuccess(effects::ProjectTargetSuccess),
    ProjectTargetFailure(effects::ProjectTargetFailure),
    ProjectTargetCanceled(effects::ProjectTargetCanceled),
    GrantNodeSlot(effects::GrantNodeSlot),
    GrantBodyFrameStart(effects::GrantBodyFrameStart),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    EmitFlowRunNotice,
    EmitStepNotice,
    AppendFailureLedger,
    PersistStepOutput,
    AdmitStepWork,
    FlowTerminalized,
    EscalateSupervisor,
    ProjectTargetSuccess,
    ProjectTargetFailure,
    ProjectTargetCanceled,
    GrantNodeSlot,
    GrantBodyFrameStart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionId {
    CreateRun,
    StartRun,
    DispatchStep,
    CompleteStep,
    RecordStepOutput,
    ConditionPassed,
    ConditionRejected,
    FailStep,
    SkipStep,
    ProjectFrameStepStatus,
    CancelStep,
    RegisterTargets,
    RecordTargetSuccess,
    RecordTargetTerminalFailure,
    RecordTargetCanceled,
    RecordTargetFailure,
    RegisterReadyFrame,
    PumpNodeScheduler,
    RegisterPendingBodyFrame,
    PumpFrameScheduler,
    NodeExecutionReleased,
    FrameTerminated,
    TerminalizeCompleted,
    TerminalizeFailed,
    TerminalizeCanceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuardId {
    Phase,
    StepExists,
    StepStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperId {
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub transition_id: TransitionId,
    pub next_state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionError {
    Refusal(TransitionRefusal),
    Kernel(KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionRefusal {
    NoMatchingTransition {
        phase: Phase,
        trigger: TriggerDiscriminant,
    },
    GuardRejected {
        rejections: Vec<GuardRejection>,
    },
    AmbiguousTransition {
        transitions: Vec<TransitionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TriggerDiscriminant {
    Input(InputKind),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuardRejection {
    pub transition_id: TransitionId,
    pub guard_id: GuardId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KernelError {
    ContextViolation {
        transition_id: TransitionId,
        detail: String,
    },
    HelperEvaluation {
        helper_id: HelperId,
        detail: String,
    },
    CodegenInvariant {
        detail: String,
    },
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
        phase: Phase::Absent,
        tracked_steps: Default::default(),
        ordered_steps: Vec::new(),
        step_status: Default::default(),
        output_recorded: Default::default(),
        step_condition_results: Default::default(),
        step_has_conditions: Default::default(),
        step_dependencies: Default::default(),
        step_dependency_modes: Default::default(),
        step_branches: Default::default(),
        step_collection_policies: Default::default(),
        step_quorum_thresholds: Default::default(),
        step_target_counts: Default::default(),
        step_target_success_counts: Default::default(),
        step_target_terminal_failure_counts: Default::default(),
        target_retry_counts: Default::default(),
        failure_count: 0,
        consecutive_failure_count: 0,
        escalation_threshold: 0,
        max_step_retries: 0,
        ready_frames: Vec::new(),
        ready_frame_membership: Default::default(),
        pending_body_frame_loops: Vec::new(),
        pending_body_frame_loop_membership: Default::default(),
        active_node_count: 0,
        active_frame_count: 0,
        max_active_nodes: 0,
        max_active_frames: 0,
        max_frame_depth: 0,
        last_granted_frame: FrameId::from(String::new()),
        last_granted_loop: LoopInstanceId::from(String::new()),
    }
}

fn refusal(phase: &Phase, trigger: InputKind) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::NoMatchingTransition {
        phase: phase.clone(),
        trigger: TriggerDiscriminant::Input(trigger),
    })
}

fn guard(transition_id: TransitionId, guard_id: GuardId) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::GuardRejected {
        rejections: vec![GuardRejection { transition_id, guard_id }],
    })
}

fn ensure_step(
    state: &State,
    step_id: &StepId,
    transition_id: TransitionId,
) -> Result<Option<StepRunStatus>, TransitionError> {
    if !state.tracked_steps.contains(step_id) {
        return Err(guard(transition_id, GuardId::StepExists));
    }
    Ok(state.step_status.get(step_id).copied().flatten())
}

fn emit_run_notice(status: FlowRunStatus) -> Effect {
    Effect::EmitFlowRunNotice(effects::EmitFlowRunNotice { run_status: status })
}

fn emit_terminalized(status: FlowRunStatus) -> Effect {
    Effect::FlowTerminalized(effects::FlowTerminalized { run_status: status })
}

fn emit_step_notice(step_id: StepId, step_status: StepRunStatus) -> Effect {
    Effect::EmitStepNotice(effects::EmitStepNotice {
        step_id,
        step_status,
    })
}

fn update_step_status(
    state: &State,
    step_id: StepId,
    next_status: StepRunStatus,
    transition_id: TransitionId,
    allow_from: &[StepRunStatus],
) -> Result<State, TransitionError> {
    let current = ensure_step(state, &step_id, transition_id)?;
    if let Some(current_status) = current
        && !allow_from.contains(&current_status)
    {
        return Err(guard(transition_id, GuardId::StepStatus));
    }
    let mut next_state = state.clone();
    next_state
        .step_status
        .insert(step_id, Some(next_status));
    Ok(next_state)
}

fn maybe_add_unique<T: Ord + Clone>(seq: &mut Vec<T>, set: &mut std::collections::BTreeSet<T>, value: T) {
    if set.insert(value.clone()) {
        seq.push(value);
    }
}

pub fn transition<C: Context>(
    state: &State,
    input: Input,
    context: &C,
) -> Result<Outcome, TransitionError> {
    let _ = context;
    match input {
        Input::CreateRun(payload) => {
            if state.phase != Phase::Absent {
                return Err(refusal(&state.phase, InputKind::CreateRun));
            }
            let mut next_state = initial_state();
            next_state.phase = Phase::Pending;
            next_state.tracked_steps = payload.step_ids.iter().cloned().collect();
            next_state.ordered_steps = payload.ordered_steps.clone();
            next_state.step_has_conditions = payload.step_has_conditions;
            next_state.step_dependencies = payload.step_dependencies;
            next_state.step_dependency_modes = payload.step_dependency_modes;
            next_state.step_branches = payload.step_branches;
            next_state.step_collection_policies = payload.step_collection_policies;
            next_state.step_quorum_thresholds = payload.step_quorum_thresholds;
            next_state.escalation_threshold = payload.escalation_threshold;
            next_state.max_step_retries = payload.max_step_retries;
            next_state.max_active_nodes = payload.max_active_nodes;
            next_state.max_active_frames = payload.max_active_frames;
            next_state.max_frame_depth = payload.max_frame_depth;
            for step_id in next_state.tracked_steps.clone() {
                next_state.step_status.insert(step_id.clone(), None);
                next_state.output_recorded.insert(step_id.clone(), false);
                next_state.step_condition_results.insert(step_id.clone(), None);
                next_state.step_dependencies.entry(step_id.clone()).or_default();
                next_state
                    .step_dependency_modes
                    .entry(step_id.clone())
                    .or_insert(DependencyMode::All);
                next_state.step_branches.entry(step_id.clone()).or_insert(None);
                next_state
                    .step_collection_policies
                    .entry(step_id.clone())
                    .or_insert(CollectionPolicyKind::All);
                next_state.step_quorum_thresholds.entry(step_id.clone()).or_insert(0);
                next_state.step_target_counts.entry(step_id.clone()).or_insert(0);
                next_state
                    .step_target_success_counts
                    .entry(step_id.clone())
                    .or_insert(0);
                next_state
                    .step_target_terminal_failure_counts
                    .entry(step_id)
                    .or_insert(0);
            }
            Ok(Outcome {
                transition_id: TransitionId::CreateRun,
                next_state,
                effects: vec![emit_run_notice(FlowRunStatus::Pending)],
            })
        }
        Input::StartRun(_) => {
            if state.phase != Phase::Pending {
                return Err(refusal(&state.phase, InputKind::StartRun));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Running;
            Ok(Outcome {
                transition_id: TransitionId::StartRun,
                next_state,
                effects: vec![emit_run_notice(FlowRunStatus::Running)],
            })
        }
        Input::DispatchStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::DispatchStep));
            }
            let next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Dispatched,
                TransitionId::DispatchStep,
                &[StepRunStatus::Failed, StepRunStatus::Skipped, StepRunStatus::Completed, StepRunStatus::Canceled][0..0],
            )?;
            Ok(Outcome {
                transition_id: TransitionId::DispatchStep,
                next_state,
                effects: vec![
                    emit_step_notice(payload.step_id.clone(), StepRunStatus::Dispatched),
                    Effect::AdmitStepWork(effects::AdmitStepWork {
                        step_id: payload.step_id,
                    }),
                ],
            })
        }
        Input::CompleteStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::CompleteStep));
            }
            let mut next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Completed,
                TransitionId::CompleteStep,
                &[StepRunStatus::Dispatched],
            )?;
            next_state.consecutive_failure_count = 0;
            Ok(Outcome {
                transition_id: TransitionId::CompleteStep,
                next_state,
                effects: vec![emit_step_notice(payload.step_id, StepRunStatus::Completed)],
            })
        }
        Input::RecordStepOutput(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordStepOutput));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordStepOutput)?;
            let mut next_state = state.clone();
            next_state.output_recorded.insert(payload.step_id.clone(), true);
            Ok(Outcome {
                transition_id: TransitionId::RecordStepOutput,
                next_state,
                effects: vec![Effect::PersistStepOutput(effects::PersistStepOutput {
                    step_id: payload.step_id,
                })],
            })
        }
        Input::ConditionPassed(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::ConditionPassed));
            }
            ensure_step(state, &payload.step_id, TransitionId::ConditionPassed)?;
            let mut next_state = state.clone();
            next_state
                .step_condition_results
                .insert(payload.step_id, Some(true));
            Ok(Outcome {
                transition_id: TransitionId::ConditionPassed,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::ConditionRejected(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::ConditionRejected));
            }
            ensure_step(state, &payload.step_id, TransitionId::ConditionRejected)?;
            let mut next_state = state.clone();
            next_state
                .step_condition_results
                .insert(payload.step_id, Some(false));
            Ok(Outcome {
                transition_id: TransitionId::ConditionRejected,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::FailStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::FailStep));
            }
            let mut next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Failed,
                TransitionId::FailStep,
                &[StepRunStatus::Dispatched],
            )?;
            next_state.failure_count = next_state.failure_count.saturating_add(1);
            next_state.consecutive_failure_count =
                next_state.consecutive_failure_count.saturating_add(1);
            let should_escalate = next_state.escalation_threshold > 0
                && next_state.consecutive_failure_count >= next_state.escalation_threshold;
            let mut effects = vec![
                emit_step_notice(payload.step_id.clone(), StepRunStatus::Failed),
                Effect::AppendFailureLedger(effects::AppendFailureLedger {
                    step_id: payload.step_id.clone(),
                }),
            ];
            if should_escalate {
                effects.push(Effect::EscalateSupervisor(effects::EscalateSupervisor {
                    step_id: payload.step_id.clone(),
                }));
            }
            Ok(Outcome {
                transition_id: TransitionId::FailStep,
                next_state,
                effects,
            })
        }
        Input::SkipStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::SkipStep));
            }
            let next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Skipped,
                TransitionId::SkipStep,
                &[StepRunStatus::Dispatched],
            )?;
            Ok(Outcome {
                transition_id: TransitionId::SkipStep,
                next_state,
                effects: vec![emit_step_notice(payload.step_id, StepRunStatus::Skipped)],
            })
        }
        Input::ProjectFrameStepStatus(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::ProjectFrameStepStatus));
            }
            ensure_step(state, &payload.step_id, TransitionId::ProjectFrameStepStatus)?;
            let mut next_state = state.clone();
            next_state
                .step_status
                .insert(payload.step_id.clone(), Some(payload.step_status));
            let mut effects = vec![emit_step_notice(payload.step_id.clone(), payload.step_status)];
            if payload.step_status == StepRunStatus::Failed {
                next_state.failure_count = next_state.failure_count.saturating_add(1);
                next_state.consecutive_failure_count =
                    next_state.consecutive_failure_count.saturating_add(1);
                if payload.append_failure_ledger {
                    effects.push(Effect::AppendFailureLedger(effects::AppendFailureLedger {
                        step_id: payload.step_id.clone(),
                    }));
                }
                if next_state.escalation_threshold > 0
                    && next_state.consecutive_failure_count >= next_state.escalation_threshold
                {
                    effects.push(Effect::EscalateSupervisor(effects::EscalateSupervisor {
                        step_id: payload.step_id.clone(),
                    }));
                }
            } else if payload.step_status == StepRunStatus::Completed {
                next_state.consecutive_failure_count = 0;
            }
            Ok(Outcome {
                transition_id: TransitionId::ProjectFrameStepStatus,
                next_state,
                effects,
            })
        }
        Input::CancelStep(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::CancelStep));
            }
            let next_state = update_step_status(
                state,
                payload.step_id.clone(),
                StepRunStatus::Canceled,
                TransitionId::CancelStep,
                &[StepRunStatus::Dispatched],
            )?;
            Ok(Outcome {
                transition_id: TransitionId::CancelStep,
                next_state,
                effects: vec![emit_step_notice(payload.step_id, StepRunStatus::Canceled)],
            })
        }
        Input::RegisterTargets(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RegisterTargets));
            }
            ensure_step(state, &payload.step_id, TransitionId::RegisterTargets)?;
            let mut next_state = state.clone();
            next_state
                .step_target_counts
                .insert(payload.step_id, payload.target_count);
            Ok(Outcome {
                transition_id: TransitionId::RegisterTargets,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::RecordTargetSuccess(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordTargetSuccess));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordTargetSuccess)?;
            let mut next_state = state.clone();
            let entry = next_state
                .step_target_success_counts
                .entry(payload.step_id.clone())
                .or_insert(0);
            *entry = entry.saturating_add(1);
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetSuccess,
                next_state,
                effects: vec![Effect::ProjectTargetSuccess(effects::ProjectTargetSuccess {
                    step_id: payload.step_id,
                    target_id: payload.target_id,
                })],
            })
        }
        Input::RecordTargetTerminalFailure(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(
                    &state.phase,
                    InputKind::RecordTargetTerminalFailure,
                ));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordTargetTerminalFailure)?;
            let mut next_state = state.clone();
            let entry = next_state
                .step_target_terminal_failure_counts
                .entry(payload.step_id)
                .or_insert(0);
            *entry = entry.saturating_add(1);
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetTerminalFailure,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::RecordTargetCanceled(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordTargetCanceled));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordTargetCanceled)?;
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetCanceled,
                next_state: state.clone(),
                effects: vec![Effect::ProjectTargetCanceled(effects::ProjectTargetCanceled {
                    step_id: payload.step_id,
                    target_id: payload.target_id,
                })],
            })
        }
        Input::RecordTargetFailure(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RecordTargetFailure));
            }
            ensure_step(state, &payload.step_id, TransitionId::RecordTargetFailure)?;
            let mut next_state = state.clone();
            let entry = next_state
                .target_retry_counts
                .entry(payload.retry_key)
                .or_insert(0);
            *entry = entry.saturating_add(1);
            Ok(Outcome {
                transition_id: TransitionId::RecordTargetFailure,
                next_state,
                effects: vec![Effect::ProjectTargetFailure(effects::ProjectTargetFailure {
                    step_id: payload.step_id,
                    target_id: payload.target_id,
                })],
            })
        }
        Input::RegisterReadyFrame(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RegisterReadyFrame));
            }
            let mut next_state = state.clone();
            maybe_add_unique(
                &mut next_state.ready_frames,
                &mut next_state.ready_frame_membership,
                payload.frame_id,
            );
            Ok(Outcome {
                transition_id: TransitionId::RegisterReadyFrame,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::PumpNodeScheduler(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::PumpNodeScheduler));
            }
            let mut next_state = state.clone();
            if next_state.ready_frames.is_empty() {
                return Err(guard(TransitionId::PumpNodeScheduler, GuardId::StepStatus));
            }
            if next_state.max_active_nodes > 0
                && next_state.active_node_count >= next_state.max_active_nodes
            {
                return Err(guard(TransitionId::PumpNodeScheduler, GuardId::StepStatus));
            }
            let frame_id = next_state.ready_frames.remove(0);
            next_state.ready_frame_membership.remove(&frame_id);
            next_state.active_node_count = next_state.active_node_count.saturating_add(1);
            next_state.last_granted_frame = frame_id.clone();
            Ok(Outcome {
                transition_id: TransitionId::PumpNodeScheduler,
                next_state,
                effects: vec![Effect::GrantNodeSlot(effects::GrantNodeSlot { frame_id })],
            })
        }
        Input::RegisterPendingBodyFrame(payload) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::RegisterPendingBodyFrame));
            }
            let mut next_state = state.clone();
            maybe_add_unique(
                &mut next_state.pending_body_frame_loops,
                &mut next_state.pending_body_frame_loop_membership,
                payload.loop_instance_id,
            );
            Ok(Outcome {
                transition_id: TransitionId::RegisterPendingBodyFrame,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::PumpFrameScheduler(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::PumpFrameScheduler));
            }
            let mut next_state = state.clone();
            if next_state.pending_body_frame_loops.is_empty() {
                return Err(guard(TransitionId::PumpFrameScheduler, GuardId::StepStatus));
            }
            if next_state.max_active_frames > 0
                && next_state.active_frame_count >= next_state.max_active_frames
            {
                return Err(guard(TransitionId::PumpFrameScheduler, GuardId::StepStatus));
            }
            let loop_instance_id = next_state.pending_body_frame_loops.remove(0);
            next_state
                .pending_body_frame_loop_membership
                .remove(&loop_instance_id);
            next_state.active_frame_count = next_state.active_frame_count.saturating_add(1);
            next_state.last_granted_loop = loop_instance_id.clone();
            Ok(Outcome {
                transition_id: TransitionId::PumpFrameScheduler,
                next_state,
                effects: vec![Effect::GrantBodyFrameStart(effects::GrantBodyFrameStart {
                    loop_instance_id,
                })],
            })
        }
        Input::NodeExecutionReleased(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::NodeExecutionReleased));
            }
            let mut next_state = state.clone();
            next_state.active_node_count = next_state.active_node_count.saturating_sub(1);
            Ok(Outcome {
                transition_id: TransitionId::NodeExecutionReleased,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::FrameTerminated(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::FrameTerminated));
            }
            let mut next_state = state.clone();
            next_state.active_frame_count = next_state.active_frame_count.saturating_sub(1);
            Ok(Outcome {
                transition_id: TransitionId::FrameTerminated,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::TerminalizeCompleted(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::TerminalizeCompleted));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Completed;
            Ok(Outcome {
                transition_id: TransitionId::TerminalizeCompleted,
                next_state,
                effects: vec![
                    emit_run_notice(FlowRunStatus::Completed),
                    emit_terminalized(FlowRunStatus::Completed),
                ],
            })
        }
        Input::TerminalizeFailed(_) => {
            if state.phase != Phase::Running {
                return Err(refusal(&state.phase, InputKind::TerminalizeFailed));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Failed;
            Ok(Outcome {
                transition_id: TransitionId::TerminalizeFailed,
                next_state,
                effects: vec![
                    emit_run_notice(FlowRunStatus::Failed),
                    emit_terminalized(FlowRunStatus::Failed),
                ],
            })
        }
        Input::TerminalizeCanceled(_) => {
            if matches!(state.phase, Phase::Completed | Phase::Canceled) {
                return Err(refusal(&state.phase, InputKind::TerminalizeCanceled));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Canceled;
            Ok(Outcome {
                transition_id: TransitionId::TerminalizeCanceled,
                next_state,
                effects: vec![
                    emit_run_notice(FlowRunStatus::Canceled),
                    emit_terminalized(FlowRunStatus::Canceled),
                ],
            })
        }
    }
}
"#
    .to_string()
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
            direct_default_value_expr(&field.ty, &enum_defaults)
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
    ty: &TypeRef,
    enum_defaults: &std::collections::BTreeMap<String, String>,
) -> String {
    match ty {
        TypeRef::Bool => "false".to_string(),
        TypeRef::U32 | TypeRef::U64 => "0".to_string(),
        TypeRef::String => "String::new()".to_string(),
        TypeRef::Named(name) if matches!(render_named_type_alias_target(name), "u64" | "u32") => {
            "0".to_string()
        }
        TypeRef::Named(name) => format!("{}::from(String::new())", rust_ident(name)),
        TypeRef::Enum(name) => enum_defaults
            .get(name)
            .cloned()
            .unwrap_or_else(|| format!("{}::default()", rust_ident(name))),
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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

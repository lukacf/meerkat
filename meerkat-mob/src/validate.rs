//! Spec validation: DAG checks, limits, prompt resolution, diagnostics.
//!
//! The [`validate_spec`] function performs comprehensive validation of a
//! [`MobSpec`], returning a list of [`Diagnostic`] on failure.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet, VecDeque};

use crate::spec::{DispatchMode, MobSpec};

// ---------------------------------------------------------------------------
// Diagnostic types
// ---------------------------------------------------------------------------

/// A diagnostic code identifying the specific validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// A required section is missing (roles, topology, flows).
    MissingRequiredSection,
    /// A flow's step graph contains a cycle.
    DagCycle,
    /// `fan_in` dispatch mode is not supported in v1.
    FanInRejected,
    /// A step's `depends_on` references a nonexistent step ID.
    InvalidDependency,
    /// A limit was exceeded.
    LimitExceeded,
    /// A prompt reference could not be resolved.
    UnresolvedPromptRef,
    /// A `file://` prompt reference was used without a base context.
    FileRefWithoutContext,
    /// Duplicate role name in the spec.
    DuplicateRole,
    /// Duplicate flow ID in the spec.
    DuplicateFlow,
    /// Duplicate step ID within a flow.
    DuplicateStep,
    /// Schema reference could not be resolved.
    UnresolvedSchemaRef,
    /// Tool bundle reference could not be resolved.
    UnresolvedToolBundle,
    /// Resolver reference could not be resolved.
    UnresolvedResolver,
    /// Target role does not exist.
    UnresolvedTargetRole,
}

/// A single validation diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    /// The diagnostic code.
    pub code: DiagnosticCode,
    /// Human-readable description.
    pub message: String,
    /// Optional location hint (e.g., `flow.triage.step.dispatch`).
    pub location: Option<String>,
}

impl Diagnostic {
    /// Create a new diagnostic.
    pub fn new(code: DiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            location: None,
        }
    }

    /// Attach a location hint.
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}", self.code, self.message)?;
        if let Some(loc) = &self.location {
            write!(f, " at {loc}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Validation options
// ---------------------------------------------------------------------------

/// Options controlling validation behavior.
#[derive(Debug, Clone, Default)]
pub struct ValidateOptions {
    /// Base directory for resolving `file://` prompt references.
    /// If `None`, `file://` refs will produce a diagnostic.
    pub base_dir: Option<std::path::PathBuf>,
}

// ---------------------------------------------------------------------------
// Validation entry point
// ---------------------------------------------------------------------------

/// Validate a mob spec, returning any diagnostics found.
///
/// An empty diagnostics vector means the spec is valid.
pub fn validate_spec(spec: &MobSpec, opts: &ValidateOptions) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    check_required_sections(spec, &mut diags);
    check_duplicate_roles(spec, &mut diags);
    check_duplicate_flows(spec, &mut diags);
    check_duplicate_steps(spec, &mut diags);
    check_fan_in_rejected(spec, &mut diags);
    check_dag_acyclicity(spec, &mut diags);
    check_dependency_references(spec, &mut diags);
    check_limits(spec, &mut diags);
    check_prompt_refs(spec, opts, &mut diags);
    check_schema_refs(spec, &mut diags);
    check_tool_bundle_refs(spec, &mut diags);
    check_resolver_refs(spec, &mut diags);
    check_target_roles(spec, &mut diags);

    diags
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn check_required_sections(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    if spec.roles.is_empty() {
        diags.push(Diagnostic::new(
            DiagnosticCode::MissingRequiredSection,
            "spec must have at least one role",
        ));
    }
    if spec.flows.is_empty() {
        diags.push(Diagnostic::new(
            DiagnosticCode::MissingRequiredSection,
            "spec must have at least one flow",
        ));
    }
}

fn check_duplicate_roles(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for role in &spec.roles {
        if !seen.insert(&role.role) {
            diags.push(
                Diagnostic::new(
                    DiagnosticCode::DuplicateRole,
                    format!("duplicate role: {}", role.role),
                )
                .with_location(format!("roles.{}", role.role)),
            );
        }
    }
}

fn check_duplicate_flows(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for flow in &spec.flows {
        if !seen.insert(&flow.flow_id) {
            diags.push(
                Diagnostic::new(
                    DiagnosticCode::DuplicateFlow,
                    format!("duplicate flow: {}", flow.flow_id),
                )
                .with_location(format!("flows.{}", flow.flow_id)),
            );
        }
    }
}

fn check_duplicate_steps(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for flow in &spec.flows {
        let mut seen = HashSet::new();
        for step in &flow.steps {
            if !seen.insert(&step.step_id) {
                diags.push(
                    Diagnostic::new(
                        DiagnosticCode::DuplicateStep,
                        format!("duplicate step: {}", step.step_id),
                    )
                    .with_location(format!("flows.{}.steps.{}", flow.flow_id, step.step_id)),
                );
            }
        }
    }
}

fn check_fan_in_rejected(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for flow in &spec.flows {
        for step in &flow.steps {
            if step.dispatch_mode == DispatchMode::FanIn {
                diags.push(
                    Diagnostic::new(
                        DiagnosticCode::FanInRejected,
                        format!(
                            "fan_in dispatch mode is not supported in v1 (step: {})",
                            step.step_id
                        ),
                    )
                    .with_location(format!("flows.{}.steps.{}", flow.flow_id, step.step_id)),
                );
            }
        }
    }
}

fn check_dag_acyclicity(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for flow in &spec.flows {
        // Build adjacency list and in-degree map for topological sort
        let step_ids: HashSet<&str> = flow.steps.iter().map(|s| s.step_id.as_str()).collect();
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        let mut successors: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        for step in &flow.steps {
            in_degree.entry(step.step_id.as_str()).or_insert(0);
            successors
                .entry(step.step_id.as_str())
                .or_default();
            for dep in &step.depends_on {
                if step_ids.contains(dep.as_str()) {
                    *in_degree.entry(step.step_id.as_str()).or_insert(0) += 1;
                    successors
                        .entry(dep.as_str())
                        .or_default()
                        .push(step.step_id.as_str());
                }
            }
        }

        // Kahn's algorithm for topological sort (cycle detection)
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut visited = 0usize;
        while let Some(node) = queue.pop_front() {
            visited += 1;
            if let Some(succs) = successors.get(node) {
                for &succ in succs {
                    if let Some(deg) = in_degree.get_mut(succ) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(succ);
                        }
                    }
                }
            }
        }

        if visited < step_ids.len() {
            diags.push(
                Diagnostic::new(
                    DiagnosticCode::DagCycle,
                    format!("flow {} contains a cycle in step dependencies", flow.flow_id),
                )
                .with_location(format!("flows.{}", flow.flow_id)),
            );
        }
    }
}

fn check_dependency_references(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for flow in &spec.flows {
        let step_ids: HashSet<&str> = flow.steps.iter().map(|s| s.step_id.as_str()).collect();
        for step in &flow.steps {
            for dep in &step.depends_on {
                if !step_ids.contains(dep.as_str()) {
                    diags.push(
                        Diagnostic::new(
                            DiagnosticCode::InvalidDependency,
                            format!(
                                "step {} depends on nonexistent step {}",
                                step.step_id, dep
                            ),
                        )
                        .with_location(format!("flows.{}.steps.{}", flow.flow_id, step.step_id)),
                    );
                }
            }
        }
    }
}

fn check_limits(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for flow in &spec.flows {
        if flow.steps.len() > spec.limits.max_steps_per_flow {
            diags.push(
                Diagnostic::new(
                    DiagnosticCode::LimitExceeded,
                    format!(
                        "flow {} has {} steps, exceeding max_steps_per_flow ({})",
                        flow.flow_id,
                        flow.steps.len(),
                        spec.limits.max_steps_per_flow
                    ),
                )
                .with_location(format!("flows.{}", flow.flow_id)),
            );
        }
    }
}

fn check_prompt_refs(spec: &MobSpec, opts: &ValidateOptions, diags: &mut Vec<Diagnostic>) {
    for role in &spec.roles {
        if let Some(ref prompt_ref) = role.prompt_ref {
            if let Some(config_name) = prompt_ref.strip_prefix("config://prompts/") {
                if !spec.prompts.contains_key(config_name) {
                    diags.push(
                        Diagnostic::new(
                            DiagnosticCode::UnresolvedPromptRef,
                            format!(
                                "prompt ref '{}' not found in spec prompts map",
                                prompt_ref
                            ),
                        )
                        .with_location(format!("roles.{}.prompt_ref", role.role)),
                    );
                }
            } else if prompt_ref.starts_with("file://")
                && opts.base_dir.is_none()
            {
                diags.push(
                    Diagnostic::new(
                        DiagnosticCode::FileRefWithoutContext,
                        format!(
                            "file:// prompt ref '{}' requires a base context",
                            prompt_ref
                        ),
                    )
                    .with_location(format!("roles.{}.prompt_ref", role.role)),
                );
            }
        }
    }
}

fn check_schema_refs(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for flow in &spec.flows {
        for step in &flow.steps {
            if let Some(ref schema_ref) = step.expected_schema_ref
                && !spec.schemas.contains_key(schema_ref)
            {
                diags.push(
                    Diagnostic::new(
                        DiagnosticCode::UnresolvedSchemaRef,
                        format!("schema ref '{}' not found in spec schemas map", schema_ref),
                    )
                    .with_location(format!("flows.{}.steps.{}", flow.flow_id, step.step_id)),
                );
            }
        }
    }
}

fn check_tool_bundle_refs(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for role in &spec.roles {
        for bundle_name in &role.tool_bundles {
            if !spec.tool_bundles.contains_key(bundle_name) {
                diags.push(
                    Diagnostic::new(
                        DiagnosticCode::UnresolvedToolBundle,
                        format!(
                            "tool bundle '{}' referenced by role '{}' not found",
                            bundle_name, role.role
                        ),
                    )
                    .with_location(format!("roles.{}.tool_bundles", role.role)),
                );
            }
        }
    }
}

fn check_resolver_refs(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    for role in &spec.roles {
        if let crate::spec::CardinalitySpec::PerMeerkat { ref resolver } = role.cardinality
            && !spec.resolvers.contains_key(resolver)
        {
            diags.push(
                Diagnostic::new(
                    DiagnosticCode::UnresolvedResolver,
                    format!(
                        "resolver '{}' referenced by role '{}' not found",
                        resolver, role.role
                    ),
                )
                .with_location(format!("roles.{}.cardinality", role.role)),
            );
        }
    }
}

fn check_target_roles(spec: &MobSpec, diags: &mut Vec<Diagnostic>) {
    let role_names: HashSet<&str> = spec.roles.iter().map(|r| r.role.as_str()).collect();
    for flow in &spec.flows {
        for step in &flow.steps {
            if !role_names.contains(step.targets.role.as_str()) {
                diags.push(
                    Diagnostic::new(
                        DiagnosticCode::UnresolvedTargetRole,
                        format!(
                            "step {} targets role '{}' which does not exist",
                            step.step_id, step.targets.role
                        ),
                    )
                    .with_location(format!("flows.{}.steps.{}", flow.flow_id, step.step_id)),
                );
            }
        }
    }
}

/// Resolve a prompt reference to its content.
///
/// Supports two schemes:
/// - `config://prompts/<name>` — looks up in the provided prompts map.
/// - `file://<path>` — reads from disk relative to `base_dir`.
///
/// Returns `None` if the reference cannot be resolved.
pub fn resolve_prompt_ref(
    prompt_ref: &str,
    prompts: &BTreeMap<String, String>,
    base_dir: Option<&std::path::Path>,
) -> Option<String> {
    if let Some(name) = prompt_ref.strip_prefix("config://prompts/") {
        return prompts.get(name).cloned();
    }
    if let Some(path) = prompt_ref.strip_prefix("file://") {
        let base = base_dir?;
        let full_path = base.join(path);
        return std::fs::read_to_string(full_path).ok();
    }
    None
}

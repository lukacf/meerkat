//! Flow specification validation.

use crate::definition::{CollectionPolicy, DependencyMode, FlowSpec, FlowStepSpec, MobDefinition};
use crate::ids::{BranchId, FlowId, StepId};
use crate::validate::{Diagnostic, DiagnosticCode, DiagnosticSeverity};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Validates flow/topology/supervisor constraints.
pub struct SpecValidator;

impl SpecValidator {
    /// Validate flow-related definition rules.
    pub fn validate(definition: &MobDefinition) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        if let Some(supervisor) = &definition.supervisor
            && !definition.profiles.contains_key(supervisor.role.as_str())
        {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::FlowUnknownRole,
                message: format!("supervisor role '{}' is not defined", supervisor.role),
                location: Some("supervisor.role".to_string()),
                severity: DiagnosticSeverity::Error,
            });
        }

        if let Some(topology) = &definition.topology {
            for (index, rule) in topology.rules.iter().enumerate() {
                if rule.from_role.as_str() != "*"
                    && !definition.profiles.contains_key(rule.from_role.as_str())
                {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::TopologyUnknownRole,
                        message: format!(
                            "topology rule references unknown from_role '{}'",
                            rule.from_role
                        ),
                        location: Some(format!("topology.rules[{index}].from_role")),
                        severity: DiagnosticSeverity::Error,
                    });
                }
                if rule.to_role.as_str() != "*"
                    && !definition.profiles.contains_key(rule.to_role.as_str())
                {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::TopologyUnknownRole,
                        message: format!(
                            "topology rule references unknown to_role '{}'",
                            rule.to_role
                        ),
                        location: Some(format!("topology.rules[{index}].to_role")),
                        severity: DiagnosticSeverity::Error,
                    });
                }
            }
        }

        for (flow_name, flow) in &definition.flows {
            Self::validate_flow(definition, flow_name, flow, &mut diagnostics);
        }

        diagnostics
    }

    fn validate_flow(
        definition: &MobDefinition,
        flow_name: &FlowId,
        flow: &FlowSpec,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let step_keys = flow.steps.keys().cloned().collect::<BTreeSet<_>>();
        let mut branch_groups: BTreeMap<BranchId, Vec<(&StepId, &FlowStepSpec)>> = BTreeMap::new();

        for (step_id, step) in &flow.steps {
            if !definition.profiles.contains_key(step.role.as_str()) {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::FlowUnknownRole,
                    message: format!("step '{}' references unknown role '{}'", step_id, step.role),
                    location: Some(format!("flows.{flow_name}.steps.{step_id}.role")),
                    severity: DiagnosticSeverity::Error,
                });
            }

            for dep in &step.depends_on {
                if !step_keys.contains(dep) {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::FlowUnknownStep,
                        message: format!("step '{step_id}' depends on unknown step '{dep}'"),
                        location: Some(format!("flows.{flow_name}.steps.{step_id}.depends_on")),
                        severity: DiagnosticSeverity::Error,
                    });
                }
            }

            if let CollectionPolicy::Quorum { n } = step.collection_policy
                && n == 0
            {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::QuorumInvalid,
                    message: format!("step '{step_id}' has invalid quorum n=0"),
                    location: Some(format!(
                        "flows.{flow_name}.steps.{step_id}.collection_policy.n"
                    )),
                    severity: DiagnosticSeverity::Error,
                });
            }

            if let Some(group) = &step.branch {
                branch_groups
                    .entry(group.clone())
                    .or_default()
                    .push((step_id, step));
            }

            if step.depends_on_mode == DependencyMode::Any {
                let has_branch_dependency = step
                    .depends_on
                    .iter()
                    .filter_map(|dep| flow.steps.get(dep))
                    .any(|dep_step| dep_step.branch.is_some());
                if !has_branch_dependency {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::BranchJoinWithoutBranch,
                        message: format!(
                            "step '{step_id}' uses depends_on_mode=any without branch dependencies"
                        ),
                        location: Some(format!(
                            "flows.{flow_name}.steps.{step_id}.depends_on_mode"
                        )),
                        severity: DiagnosticSeverity::Warning,
                    });
                }
            }
        }

        for (branch_name, members) in &branch_groups {
            if members.len() < 2 {
                diagnostics.push(Diagnostic {
                    code: DiagnosticCode::BranchGroupEmpty,
                    message: format!(
                        "branch group '{branch_name}' in flow '{flow_name}' must contain at least two steps"
                    ),
                    location: Some(format!("flows.{flow_name}.steps")),
                    severity: DiagnosticSeverity::Error,
                });
            }

            let first_dep_set = members
                .first()
                .map(|(_, step)| step.depends_on.iter().cloned().collect::<BTreeSet<_>>())
                .unwrap_or_default();

            for (step_id, step) in members {
                if step.condition.is_none() {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::BranchStepMissingCondition,
                        message: format!(
                            "branch step '{step_id}' in group '{branch_name}' is missing condition"
                        ),
                        location: Some(format!("flows.{flow_name}.steps.{step_id}.condition")),
                        severity: DiagnosticSeverity::Error,
                    });
                }

                let deps = step.depends_on.iter().cloned().collect::<BTreeSet<_>>();
                if deps != first_dep_set {
                    diagnostics.push(Diagnostic {
                        code: DiagnosticCode::BranchStepConflictingDeps,
                        message: format!(
                            "branch step '{step_id}' in group '{branch_name}' has conflicting dependencies"
                        ),
                        location: Some(format!("flows.{flow_name}.steps.{step_id}.depends_on")),
                        severity: DiagnosticSeverity::Error,
                    });
                }
            }
        }

        let (has_cycle, max_depth) = analyze_dag(flow);
        if has_cycle {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::FlowCycleDetected,
                message: format!("flow '{flow_name}' contains a cycle"),
                location: Some(format!("flows.{flow_name}.steps")),
                severity: DiagnosticSeverity::Error,
            });
        } else if max_depth > 32 {
            diagnostics.push(Diagnostic {
                code: DiagnosticCode::FlowDepthExceeded,
                message: format!("flow '{flow_name}' exceeds max depth of 32"),
                location: Some(format!("flows.{flow_name}.steps")),
                severity: DiagnosticSeverity::Error,
            });
        }
    }
}

fn analyze_dag(flow: &FlowSpec) -> (bool, usize) {
    let mut indegree: BTreeMap<&str, usize> = flow.steps.keys().map(|k| (k.as_str(), 0)).collect();
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

    for (step_id, step) in &flow.steps {
        for dep in &step.depends_on {
            if indegree.contains_key(dep.as_str()) {
                if let Some(entry) = indegree.get_mut(step_id.as_str()) {
                    *entry += 1;
                }
                adjacency
                    .entry(dep.as_str())
                    .or_default()
                    .push(step_id.as_str());
            }
        }
    }

    let mut depth: BTreeMap<&str, usize> = indegree
        .iter()
        .map(|(step, deg)| (*step, usize::from(*deg == 0)))
        .collect();
    let mut queue = VecDeque::new();
    for (step, degree) in &indegree {
        if *degree == 0 {
            queue.push_back(*step);
        }
    }

    let mut processed = 0usize;
    let mut max_depth = 0usize;

    while let Some(step) = queue.pop_front() {
        processed += 1;
        let current_depth = match depth.get(step) {
            Some(depth) => *depth,
            None => unreachable!("step must exist in depth map after Kahn initialization"),
        };
        max_depth = max_depth.max(current_depth);

        for next in adjacency.get(step).cloned().unwrap_or_default() {
            let next_entry = depth.entry(next).or_insert(0);
            *next_entry = (*next_entry).max(current_depth + 1);

            if let Some(entry) = indegree.get_mut(next) {
                *entry -= 1;
                if *entry == 0 {
                    queue.push_back(next);
                }
            }
        }
    }

    (processed != flow.steps.len(), max_depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        BackendConfig, DispatchMode, FlowStepSpec, PolicyMode, TopologyRule, TopologySpec,
        WiringRules,
    };
    use crate::ids::{FlowId, MobId, ProfileName, StepId};
    use crate::profile::{Profile, ToolConfig};
    use indexmap::IndexMap;

    fn profile() -> Profile {
        Profile {
            model: "test".to_string(),
            skills: Vec::new(),
            tools: ToolConfig::default(),
            peer_description: "test".to_string(),
            external_addressable: false,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
        }
    }

    fn base_definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(ProfileName::from("lead"), profile());
        profiles.insert(ProfileName::from("worker"), profile());

        MobDefinition {
            id: MobId::from("mob"),
            orchestrator: None,
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
        }
    }

    fn step(role: &str, message: &str) -> FlowStepSpec {
        FlowStepSpec {
            role: ProfileName::from(role),
            message: message.to_string(),
            depends_on: Vec::new(),
            dispatch_mode: DispatchMode::FanOut,
            collection_policy: CollectionPolicy::All,
            condition: None,
            timeout_ms: None,
            expected_schema_ref: None,
            branch: None,
            depends_on_mode: DependencyMode::All,
            allowed_tools: None,
            blocked_tools: None,
        }
    }

    #[test]
    fn test_detects_cycle_and_unknown_step_and_unknown_role() {
        let mut def = base_definition();

        let mut steps = IndexMap::new();
        let mut a = step("worker", "a");
        a.depends_on = vec![StepId::from("b"), StepId::from("missing")];
        steps.insert(StepId::from("a"), a);
        let mut b = step("ghost-role", "b");
        b.depends_on = vec![StepId::from("a")];
        steps.insert(StepId::from("b"), b);

        def.flows.insert(
            FlowId::from("flow"),
            FlowSpec {
                description: None,
                steps,
            },
        );

        let diagnostics = SpecValidator::validate(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::FlowCycleDetected)
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::FlowUnknownStep)
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::FlowUnknownRole)
        );
    }

    #[test]
    fn test_detects_depth_exceeded() {
        let mut def = base_definition();
        let mut steps = IndexMap::new();
        for index in 0..33 {
            let mut current = step("worker", "x");
            if index > 0 {
                current.depends_on = vec![StepId::from(format!("s{}", index - 1))];
            }
            steps.insert(StepId::from(format!("s{index}")), current);
        }
        def.flows.insert(
            FlowId::from("deep"),
            FlowSpec {
                description: None,
                steps,
            },
        );

        let diagnostics = SpecValidator::validate(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::FlowDepthExceeded)
        );
    }

    #[test]
    fn test_branch_rules_and_warning_severity() {
        let mut def = base_definition();
        let mut steps = IndexMap::new();

        let mut branch_a = step("worker", "a");
        branch_a.branch = Some(crate::ids::BranchId::from("pick"));
        branch_a.condition = Some(crate::definition::ConditionExpr::Eq {
            path: "params.choice".to_string(),
            value: serde_json::json!("a"),
        });
        branch_a.depends_on = vec![StepId::from("start")];
        steps.insert(StepId::from("branch_a"), branch_a);

        let mut branch_b = step("worker", "b");
        branch_b.branch = Some(crate::ids::BranchId::from("pick"));
        branch_b.depends_on = vec![StepId::from("different")];
        steps.insert(StepId::from("branch_b"), branch_b);

        steps.insert(StepId::from("start"), step("worker", "start"));

        let mut join = step("worker", "join");
        join.depends_on_mode = DependencyMode::Any;
        join.depends_on = vec![StepId::from("start")];
        steps.insert(StepId::from("join"), join);

        let mut quorum = step("worker", "q");
        quorum.collection_policy = CollectionPolicy::Quorum { n: 0 };
        steps.insert(StepId::from("quorum"), quorum);

        let mut lonely_branch = step("worker", "lonely");
        lonely_branch.branch = Some(crate::ids::BranchId::from("solo"));
        lonely_branch.condition = Some(crate::definition::ConditionExpr::Eq {
            path: "params.x".to_string(),
            value: serde_json::json!(1),
        });
        steps.insert(StepId::from("lonely_branch"), lonely_branch);

        def.flows.insert(
            FlowId::from("flow"),
            FlowSpec {
                description: None,
                steps,
            },
        );

        let diagnostics = SpecValidator::validate(&def);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::BranchStepMissingCondition)
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::BranchStepConflictingDeps)
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::BranchGroupEmpty)
        );
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::QuorumInvalid)
        );
        let warning = diagnostics
            .iter()
            .find(|d| d.code == DiagnosticCode::BranchJoinWithoutBranch)
            .expect("warning exists");
        assert_eq!(warning.severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn test_topology_unknown_role() {
        let mut def = base_definition();
        def.topology = Some(TopologySpec {
            mode: PolicyMode::Strict,
            rules: vec![
                TopologyRule {
                    from_role: ProfileName::from("lead"),
                    to_role: ProfileName::from("ghost"),
                    allowed: true,
                },
                TopologyRule {
                    from_role: ProfileName::from("ghost"),
                    to_role: ProfileName::from("worker"),
                    allowed: false,
                },
            ],
        });

        let diagnostics = SpecValidator::validate(&def);
        assert_eq!(
            diagnostics
                .iter()
                .filter(|d| d.code == DiagnosticCode::TopologyUnknownRole)
                .count(),
            2
        );
    }

    #[test]
    fn test_topology_wildcard_role_is_allowed() {
        let mut def = base_definition();
        def.topology = Some(TopologySpec {
            mode: PolicyMode::Strict,
            rules: vec![TopologyRule {
                from_role: ProfileName::from("*"),
                to_role: ProfileName::from("worker"),
                allowed: false,
            }],
        });

        let diagnostics = SpecValidator::validate(&def);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.code != DiagnosticCode::TopologyUnknownRole),
            "wildcard topology roles should bypass unknown-role diagnostics"
        );
    }
}

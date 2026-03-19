//! Flow run data model.

use crate::definition::{FlowSpec, LimitsSpec, SupervisorSpec, TopologySpec};
use crate::error::MobError;
use crate::ids::{FlowId, MeerkatId, MobId, ProfileName, RunId, StepId};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use meerkat_machine_kernels::generated::flow_run;
use meerkat_machine_kernels::{KernelInput, KernelState, KernelValue};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Persisted flow run aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobRun {
    pub run_id: RunId,
    pub mob_id: MobId,
    pub flow_id: FlowId,
    pub status: MobRunStatus,
    pub flow_state: KernelState,
    pub activation_params: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub step_ledger: Vec<StepLedgerEntry>,
    pub failure_ledger: Vec<FailureLedgerEntry>,
}

impl MobRun {
    pub fn pending(
        mob_id: MobId,
        flow_id: FlowId,
        flow_state: KernelState,
        activation_params: serde_json::Value,
    ) -> Self {
        Self {
            run_id: RunId::new(),
            mob_id,
            flow_id,
            status: MobRunStatus::Pending,
            flow_state,
            activation_params,
            created_at: Utc::now(),
            completed_at: None,
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
        }
    }

    pub fn flow_state_for_config(config: &FlowRunConfig) -> Result<KernelState, MobError> {
        let initial = flow_run::initial_state().map_err(|error| {
            MobError::Internal(format!("flow_run initial_state failed: {error}"))
        })?;
        let ordered_steps = topological_steps(&config.flow_spec)?;
        let step_ids = config
            .flow_spec
            .steps
            .keys()
            .map(|step_id| KernelValue::String(step_id.to_string()))
            .collect();
        let ordered_steps = ordered_steps
            .into_iter()
            .map(|step_id| KernelValue::String(step_id.to_string()))
            .collect();
        let step_dependencies = config
            .flow_spec
            .steps
            .iter()
            .map(|(step_id, step)| {
                (
                    KernelValue::String(step_id.to_string()),
                    KernelValue::Seq(
                        step.depends_on
                            .iter()
                            .map(|dependency| KernelValue::String(dependency.to_string()))
                            .collect(),
                    ),
                )
            })
            .collect();
        let step_dependency_modes = config
            .flow_spec
            .steps
            .iter()
            .map(|(step_id, step)| {
                (
                    KernelValue::String(step_id.to_string()),
                    dependency_mode_value(step.depends_on_mode.clone()),
                )
            })
            .collect();
        let step_has_conditions = config
            .flow_spec
            .steps
            .iter()
            .map(|(step_id, step)| {
                (
                    KernelValue::String(step_id.to_string()),
                    KernelValue::Bool(step.condition.is_some()),
                )
            })
            .collect();
        let step_branches = config
            .flow_spec
            .steps
            .iter()
            .map(|(step_id, step)| {
                (
                    KernelValue::String(step_id.to_string()),
                    step.branch.as_ref().map_or(KernelValue::None, |branch| {
                        KernelValue::String(branch.to_string())
                    }),
                )
            })
            .collect();
        let step_collection_policies = config
            .flow_spec
            .steps
            .iter()
            .map(|(step_id, step)| {
                (
                    KernelValue::String(step_id.to_string()),
                    collection_policy_kind_value(&step.collection_policy),
                )
            })
            .collect();
        let step_quorum_thresholds = config
            .flow_spec
            .steps
            .iter()
            .map(|(step_id, step)| {
                let threshold = match step.collection_policy {
                    crate::definition::CollectionPolicy::Quorum { n } => u64::from(n),
                    _ => 0,
                };
                (
                    KernelValue::String(step_id.to_string()),
                    KernelValue::U64(threshold),
                )
            })
            .collect();
        let escalation_threshold = config
            .supervisor
            .as_ref()
            .map_or(0, |supervisor| u64::from(supervisor.escalation_threshold));
        let max_step_retries = config
            .limits
            .as_ref()
            .and_then(|limits| limits.max_step_retries)
            .map_or(0, u64::from);
        let input = KernelInput {
            variant: "CreateRun".to_string(),
            fields: BTreeMap::from([
                ("step_ids".to_string(), KernelValue::Seq(step_ids)),
                ("ordered_steps".to_string(), KernelValue::Seq(ordered_steps)),
                (
                    "step_dependencies".to_string(),
                    KernelValue::Map(step_dependencies),
                ),
                (
                    "step_dependency_modes".to_string(),
                    KernelValue::Map(step_dependency_modes),
                ),
                (
                    "step_has_conditions".to_string(),
                    KernelValue::Map(step_has_conditions),
                ),
                ("step_branches".to_string(), KernelValue::Map(step_branches)),
                (
                    "step_collection_policies".to_string(),
                    KernelValue::Map(step_collection_policies),
                ),
                (
                    "step_quorum_thresholds".to_string(),
                    KernelValue::Map(step_quorum_thresholds),
                ),
                (
                    "escalation_threshold".to_string(),
                    KernelValue::U64(escalation_threshold),
                ),
                (
                    "max_step_retries".to_string(),
                    KernelValue::U64(max_step_retries),
                ),
            ]),
        };
        let outcome = flow_run::transition(&initial, &input)
            .map_err(|error| MobError::Internal(format!("flow_run CreateRun failed: {error}")))?;
        Ok(outcome.next_state)
    }

    pub fn flow_state_for_steps<I>(step_ids: I) -> Result<KernelState, MobError>
    where
        I: IntoIterator<Item = StepId>,
    {
        let mut steps = IndexMap::new();
        for step_id in step_ids {
            steps.insert(
                step_id,
                crate::definition::FlowStepSpec {
                    role: ProfileName::from("worker"),
                    message: meerkat_core::types::ContentInput::from("placeholder"),
                    depends_on: Vec::new(),
                    dispatch_mode: crate::definition::DispatchMode::FanOut,
                    collection_policy: crate::definition::CollectionPolicy::All,
                    condition: None,
                    timeout_ms: None,
                    expected_schema_ref: None,
                    branch: None,
                    depends_on_mode: crate::definition::DependencyMode::All,
                    allowed_tools: None,
                    blocked_tools: None,
                    output_format: crate::definition::StepOutputFormat::Json,
                },
            );
        }
        Self::flow_state_for_config(&FlowRunConfig {
            flow_id: FlowId::from("placeholder"),
            flow_spec: FlowSpec {
                description: None,
                steps,
            },
            topology: None,
            supervisor: None,
            limits: None,
            orchestrator_role: None,
        })
    }
}

fn dependency_mode_value(mode: crate::definition::DependencyMode) -> KernelValue {
    let variant = match mode {
        crate::definition::DependencyMode::All => "All",
        crate::definition::DependencyMode::Any => "Any",
    };
    KernelValue::NamedVariant {
        enum_name: "DependencyMode".to_string(),
        variant: variant.to_string(),
    }
}

fn collection_policy_kind_value(policy: &crate::definition::CollectionPolicy) -> KernelValue {
    let variant = match policy {
        crate::definition::CollectionPolicy::All => "All",
        crate::definition::CollectionPolicy::Any => "Any",
        crate::definition::CollectionPolicy::Quorum { .. } => "Quorum",
    };
    KernelValue::NamedVariant {
        enum_name: "CollectionPolicyKind".to_string(),
        variant: variant.to_string(),
    }
}

fn topological_steps(flow_spec: &FlowSpec) -> Result<Vec<StepId>, MobError> {
    let mut in_degree: BTreeMap<StepId, usize> = BTreeMap::new();
    let mut outgoing: BTreeMap<StepId, Vec<StepId>> = BTreeMap::new();

    for step_id in flow_spec.steps.keys() {
        in_degree.insert(step_id.clone(), 0);
        outgoing.entry(step_id.clone()).or_default();
    }

    for (step_id, step) in &flow_spec.steps {
        for dependency in &step.depends_on {
            if !in_degree.contains_key(dependency) {
                return Err(MobError::Internal(format!(
                    "step '{step_id}' depends on unknown step '{dependency}'"
                )));
            }
            *in_degree.entry(step_id.clone()).or_insert(0) += 1;
            outgoing
                .entry(dependency.clone())
                .or_default()
                .push(step_id.clone());
        }
    }

    let mut queue = VecDeque::new();
    for step_id in flow_spec.steps.keys() {
        if in_degree.get(step_id) == Some(&0) {
            queue.push_back(step_id.clone());
        }
    }

    let mut ordered = Vec::with_capacity(flow_spec.steps.len());
    while let Some(next) = queue.pop_front() {
        ordered.push(next.clone());
        if let Some(children) = outgoing.get(&next) {
            for child in children {
                if let Some(count) = in_degree.get_mut(child)
                    && *count > 0
                {
                    *count -= 1;
                    if *count == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }
    }

    if ordered.len() != flow_spec.steps.len() {
        return Err(MobError::Internal(
            "flow contains a cycle; cannot compute topological order".to_string(),
        ));
    }

    Ok(ordered)
}

/// Run lifecycle states.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

impl MobRunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }
}

/// Per-target step execution ledger entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepLedgerEntry {
    pub step_id: StepId,
    pub meerkat_id: MeerkatId,
    pub status: StepRunStatus,
    pub output: Option<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

/// Step execution state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepRunStatus {
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

/// Flow-level failure log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureLedgerEntry {
    pub step_id: StepId,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

/// Immutable per-run flow snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowRunConfig {
    pub flow_id: FlowId,
    pub flow_spec: FlowSpec,
    pub topology: Option<TopologySpec>,
    pub supervisor: Option<SupervisorSpec>,
    pub limits: Option<LimitsSpec>,
    pub orchestrator_role: Option<ProfileName>,
}

impl FlowRunConfig {
    pub fn from_definition(
        flow_id: FlowId,
        definition: &crate::definition::MobDefinition,
    ) -> Result<Self, MobError> {
        let flow_spec = definition
            .flows
            .get(&flow_id)
            .cloned()
            .ok_or_else(|| MobError::FlowNotFound(flow_id.clone()))?;
        let topology = definition.topology.clone();
        let orchestrator_role = definition
            .orchestrator
            .as_ref()
            .map(|orchestrator| orchestrator.profile.clone());
        if topology.is_some() && orchestrator_role.is_none() {
            return Err(MobError::Internal(
                "topology requires an orchestrator profile".to_string(),
            ));
        }
        Ok(Self {
            flow_id,
            flow_spec,
            topology,
            supervisor: definition.supervisor.clone(),
            limits: definition.limits.clone(),
            orchestrator_role,
        })
    }
}

/// Runtime context available to condition evaluators.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowContext {
    pub run_id: RunId,
    pub activation_params: serde_json::Value,
    pub step_outputs: IndexMap<StepId, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        BackendConfig, ConditionExpr, DispatchMode, FlowStepSpec, MobDefinition,
        OrchestratorConfig, WiringRules,
    };
    use crate::ids::{BranchId, ProfileName};
    use crate::profile::{Profile, ToolConfig};
    use meerkat_core::types::ContentInput;
    use std::collections::BTreeMap;

    fn sample_definition() -> MobDefinition {
        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("s1"),
            FlowStepSpec {
                role: ProfileName::from("worker"),
                message: ContentInput::from("do it"),
                depends_on: Vec::new(),
                dispatch_mode: DispatchMode::FanOut,
                collection_policy: crate::definition::CollectionPolicy::All,
                condition: Some(ConditionExpr::Eq {
                    path: "params.ok".to_string(),
                    value: serde_json::json!(true),
                }),
                timeout_ms: Some(2000),
                expected_schema_ref: Some("schema.json".to_string()),
                branch: Some(BranchId::from("branch-a")),
                depends_on_mode: crate::definition::DependencyMode::All,
                allowed_tools: None,
                blocked_tools: None,
                output_format: crate::definition::StepOutputFormat::Json,
            },
        );

        let mut flows = BTreeMap::new();
        flows.insert(
            FlowId::from("flow-a"),
            FlowSpec {
                description: Some("demo flow".to_string()),
                steps,
            },
        );

        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            Profile {
                model: "model".to_string(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "lead".to_string(),
                external_addressable: true,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
        );
        profiles.insert(
            ProfileName::from("worker"),
            Profile {
                model: "model".to_string(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::AutonomousHost,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
        );

        MobDefinition {
            id: MobId::from("mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows,
            topology: Some(TopologySpec {
                mode: crate::definition::PolicyMode::Advisory,
                rules: vec![crate::definition::TopologyRule {
                    from_role: ProfileName::from("lead"),
                    to_role: ProfileName::from("worker"),
                    allowed: true,
                }],
            }),
            supervisor: Some(SupervisorSpec {
                role: ProfileName::from("lead"),
                escalation_threshold: 3,
            }),
            limits: Some(LimitsSpec {
                max_flow_duration_ms: Some(60_000),
                max_step_retries: Some(1),
                max_orphaned_turns: Some(8),
                cancel_grace_timeout_ms: None,
            }),
            spawn_policy: None,
            event_router: None,
        }
    }

    #[test]
    fn test_run_status_terminal() {
        assert!(MobRunStatus::Completed.is_terminal());
        assert!(MobRunStatus::Failed.is_terminal());
        assert!(MobRunStatus::Canceled.is_terminal());
        assert!(!MobRunStatus::Pending.is_terminal());
        assert!(!MobRunStatus::Running.is_terminal());
    }

    #[test]
    fn test_flow_run_config_from_definition() {
        let def = sample_definition();
        let config = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def).unwrap();
        assert_eq!(config.flow_id, FlowId::from("flow-a"));
        assert_eq!(config.flow_spec.steps.len(), 1);
        assert_eq!(
            config.orchestrator_role.as_ref(),
            Some(&ProfileName::from("lead"))
        );
    }

    #[test]
    fn test_flow_run_config_from_definition_missing_flow() {
        let def = sample_definition();
        let error = FlowRunConfig::from_definition(FlowId::from("missing"), &def).unwrap_err();
        assert!(matches!(error, MobError::FlowNotFound(name) if name == FlowId::from("missing")));
    }

    #[test]
    fn test_flow_run_config_rejects_topology_without_orchestrator() {
        let mut def = sample_definition();
        def.orchestrator = None;
        let error = FlowRunConfig::from_definition(FlowId::from("flow-a"), &def).unwrap_err();
        assert!(
            matches!(error, MobError::Internal(message) if message.contains("topology requires")),
            "expected explicit topology/orchestrator configuration error"
        );
    }

    #[test]
    fn test_mob_run_roundtrip_json() {
        let now = Utc::now();
        let run = MobRun {
            run_id: RunId::new(),
            mob_id: MobId::from("mob"),
            flow_id: FlowId::from("flow-a"),
            status: MobRunStatus::Running,
            flow_state: MobRun::flow_state_for_steps([StepId::from("step-1")]).unwrap(),
            activation_params: serde_json::json!({"k":"v"}),
            created_at: now,
            completed_at: None,
            step_ledger: vec![StepLedgerEntry {
                step_id: StepId::from("step-1"),
                meerkat_id: MeerkatId::from("agent-1"),
                status: StepRunStatus::Completed,
                output: Some(serde_json::json!({"ok":true})),
                timestamp: now,
            }],
            failure_ledger: vec![FailureLedgerEntry {
                step_id: StepId::from("step-2"),
                reason: "boom".to_string(),
                timestamp: now,
            }],
        };

        let encoded = serde_json::to_string(&run).unwrap();
        let decoded: MobRun = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.flow_id, run.flow_id);
        assert_eq!(decoded.step_ledger.len(), 1);
        assert_eq!(decoded.failure_ledger.len(), 1);
    }

    #[test]
    fn test_flow_context_roundtrip_json() {
        let mut outputs = IndexMap::new();
        outputs.insert(StepId::from("step-1"), serde_json::json!({"a":1}));
        let context = FlowContext {
            run_id: RunId::new(),
            activation_params: serde_json::json!({"input":"x"}),
            step_outputs: outputs,
        };

        let encoded = serde_json::to_string(&context).unwrap();
        let decoded: FlowContext = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.step_outputs.len(), 1);
        assert_eq!(decoded.activation_params["input"], "x");
    }
}

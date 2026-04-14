//! Flow run data model.

use crate::definition::{DependencyMode, FlowSpec, LimitsSpec, SupervisorSpec, TopologySpec};
use crate::error::MobError;
use crate::ids::{
    AgentIdentity, BranchId, FlowId, FrameId, LoopId, LoopInstanceId, MobId, ProfileName, RunId,
    StepId,
};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use meerkat_machine_kernels::generated::flow_run;
use meerkat_machine_kernels::{KernelInput, KernelState, KernelValue};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Snapshot of a FlowFrameMachine kernel state stored per-frame in MobRun.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub kernel_state: KernelState,
}

/// Snapshot of a LoopIterationMachine kernel state stored per-loop in MobRun.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoopSnapshot {
    pub kernel_state: KernelState,
}

/// Ledger entry recording the mapping of a loop iteration to its body frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopIterationLedgerEntry {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u64,
    pub frame_id: FrameId,
}

/// Persisted collection-policy kind stored in the flow kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunCollectionPolicyKind {
    All,
    Any,
    Quorum,
}

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
    /// Per-frame kernel snapshots indexed by FrameId.
    #[serde(default)]
    pub frames: BTreeMap<FrameId, FrameSnapshot>,
    /// Per-loop kernel snapshots indexed by LoopInstanceId.
    #[serde(default)]
    pub loops: BTreeMap<LoopInstanceId, LoopSnapshot>,
    /// Ordered ledger of loop iteration → body frame mappings.
    #[serde(default)]
    pub loop_iteration_ledger: Vec<LoopIterationLedgerEntry>,
    /// Schema version: 0 for legacy runs, 4 for self-describing frame-aware runs.
    #[serde(default)]
    pub schema_version: u32,
    /// Root frame step outputs keyed by step_id string.
    #[serde(default)]
    pub root_step_outputs: IndexMap<StepId, serde_json::Value>,
    /// Loop iteration outputs: key=loop_id, value=per-iteration step outputs.
    ///
    /// Uses `BTreeMap` (not `IndexMap`) so that recovery reconciliation can iterate
    /// in stable key order without depending on insertion order.
    #[serde(default)]
    pub loop_iteration_outputs: BTreeMap<LoopId, Vec<IndexMap<StepId, serde_json::Value>>>,
}

impl MobRun {
    /// Read-only access to the run's current status.
    pub fn status(&self) -> &MobRunStatus {
        &self.status
    }

    /// Read-only access to the run's current flow state.
    pub fn flow_state(&self) -> &KernelState {
        &self.flow_state
    }

    /// Typed view of the kernel-owned ordered step sequence.
    pub fn ordered_steps(&self) -> Result<Vec<StepId>, MobError> {
        let seq = match self.flow_state.fields.get("ordered_steps") {
            Some(KernelValue::Seq(seq)) => seq,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run ordered_steps missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };
        seq.iter()
            .map(|value| match value {
                KernelValue::String(step_id) => Ok(StepId::from(step_id.clone())),
                other => Err(MobError::Internal(format!(
                    "flow_run ordered_steps entry invalid for {}: {other:?}",
                    self.run_id
                ))),
            })
            .collect()
    }

    /// Typed view of the kernel-owned dependency map keyed by step id.
    pub fn step_dependencies(&self) -> Result<BTreeMap<StepId, Vec<StepId>>, MobError> {
        let map = match self.flow_state.fields.get("step_dependencies") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_dependencies missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };

        let mut dependencies = BTreeMap::new();
        for (step_key, value) in map {
            let step_id = match step_key {
                KernelValue::String(step_id) => StepId::from(step_id.clone()),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_dependencies key invalid for {}: {other:?}",
                        self.run_id
                    )));
                }
            };
            let seq = match value {
                KernelValue::Seq(seq) => seq,
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_dependencies entry invalid for {} step '{}': {other:?}",
                        self.run_id, step_id
                    )));
                }
            };
            let deps = seq
                .iter()
                .map(|value| match value {
                    KernelValue::String(step_id) => Ok(StepId::from(step_id.clone())),
                    other => Err(MobError::Internal(format!(
                        "flow_run step_dependencies dependency invalid for {} step '{}': {other:?}",
                        self.run_id, step_id
                    ))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            dependencies.insert(step_id, deps);
        }

        Ok(dependencies)
    }

    /// Typed view of the kernel-owned dependency mode map keyed by step id.
    pub fn step_dependency_modes(&self) -> Result<BTreeMap<StepId, DependencyMode>, MobError> {
        let map = match self.flow_state.fields.get("step_dependency_modes") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_dependency_modes missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };

        let mut modes = BTreeMap::new();
        for (step_key, value) in map {
            let step_id = match step_key {
                KernelValue::String(step_id) => StepId::from(step_id.clone()),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_dependency_modes key invalid for {}: {other:?}",
                        self.run_id
                    )));
                }
            };

            let mode = match value {
                KernelValue::NamedVariant { enum_name, variant }
                    if enum_name == "DependencyMode" =>
                {
                    match variant.as_str() {
                        "All" => DependencyMode::All,
                        "Any" => DependencyMode::Any,
                        _ => {
                            return Err(MobError::Internal(format!(
                                "flow_run step_dependency_modes unknown DependencyMode variant `{variant}` for {} step '{}'",
                                self.run_id, step_id
                            )));
                        }
                    }
                }
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_dependency_modes entry invalid for {} step '{}': {other:?}",
                        self.run_id, step_id
                    )));
                }
            };

            modes.insert(step_id, mode);
        }

        Ok(modes)
    }

    /// Typed view of the kernel-owned condition-presence map keyed by step id.
    pub fn step_has_conditions(&self) -> Result<BTreeMap<StepId, bool>, MobError> {
        let map = match self.flow_state.fields.get("step_has_conditions") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_has_conditions missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };

        let mut condition_flags = BTreeMap::new();
        for (step_key, value) in map {
            let step_id = match step_key {
                KernelValue::String(step_id) => StepId::from(step_id.clone()),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_has_conditions key invalid for {}: {other:?}",
                        self.run_id
                    )));
                }
            };

            let has_condition = match value {
                KernelValue::Bool(flag) => *flag,
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_has_conditions entry invalid for {} step '{}': {other:?}",
                        self.run_id, step_id
                    )));
                }
            };

            condition_flags.insert(step_id, has_condition);
        }

        Ok(condition_flags)
    }

    /// Typed view of the kernel-owned branch label map keyed by step id.
    pub fn step_branches(&self) -> Result<BTreeMap<StepId, Option<BranchId>>, MobError> {
        let map = match self.flow_state.fields.get("step_branches") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_branches missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };

        let mut branches = BTreeMap::new();
        for (step_key, value) in map {
            let step_id = match step_key {
                KernelValue::String(step_id) => StepId::from(step_id.clone()),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_branches key invalid for {}: {other:?}",
                        self.run_id
                    )));
                }
            };

            let branch = match value {
                KernelValue::None => None,
                KernelValue::String(branch_id) => Some(BranchId::from(branch_id.clone())),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_branches entry invalid for {} step '{}': {other:?}",
                        self.run_id, step_id
                    )));
                }
            };

            branches.insert(step_id, branch);
        }

        Ok(branches)
    }

    /// Typed view of the kernel-owned collection policy kind map keyed by step id.
    pub fn step_collection_policy_kinds(
        &self,
    ) -> Result<BTreeMap<StepId, RunCollectionPolicyKind>, MobError> {
        let map = match self.flow_state.fields.get("step_collection_policies") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_collection_policies missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };

        let mut policies = BTreeMap::new();
        for (step_key, value) in map {
            let step_id = match step_key {
                KernelValue::String(step_id) => StepId::from(step_id.clone()),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_collection_policies key invalid for {}: {other:?}",
                        self.run_id
                    )));
                }
            };

            let policy = match value {
                KernelValue::NamedVariant { enum_name, variant }
                    if enum_name == "CollectionPolicyKind" =>
                {
                    match variant.as_str() {
                        "All" => RunCollectionPolicyKind::All,
                        "Any" => RunCollectionPolicyKind::Any,
                        "Quorum" => RunCollectionPolicyKind::Quorum,
                        _ => {
                            return Err(MobError::Internal(format!(
                                "flow_run step_collection_policies unknown CollectionPolicyKind variant `{variant}` for {} step '{}'",
                                self.run_id, step_id
                            )));
                        }
                    }
                }
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_collection_policies entry invalid for {} step '{}': {other:?}",
                        self.run_id, step_id
                    )));
                }
            };

            policies.insert(step_id, policy);
        }

        Ok(policies)
    }

    /// Typed view of the kernel-owned quorum-threshold map keyed by step id.
    pub fn step_quorum_thresholds(&self) -> Result<BTreeMap<StepId, u32>, MobError> {
        let map = match self.flow_state.fields.get("step_quorum_thresholds") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_quorum_thresholds missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };

        let mut thresholds = BTreeMap::new();
        for (step_key, value) in map {
            let step_id = match step_key {
                KernelValue::String(step_id) => StepId::from(step_id.clone()),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_quorum_thresholds key invalid for {}: {other:?}",
                        self.run_id
                    )));
                }
            };

            let threshold = match value {
                KernelValue::U64(value) => u32::try_from(*value).map_err(|_| {
                    MobError::Internal(format!(
                        "flow_run step_quorum_thresholds out of range for {} step '{}'",
                        self.run_id, step_id
                    ))
                })?,
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_quorum_thresholds entry invalid for {} step '{}': {other:?}",
                        self.run_id, step_id
                    )));
                }
            };

            thresholds.insert(step_id, threshold);
        }

        Ok(thresholds)
    }

    /// Typed view of the kernel-owned step status map, excluding `None` entries.
    pub fn step_status_snapshot(&self) -> Result<BTreeMap<StepId, StepRunStatus>, MobError> {
        let map = match self.flow_state.fields.get("step_status") {
            Some(KernelValue::Map(map)) => map,
            other => {
                return Err(MobError::Internal(format!(
                    "flow_run step_status map missing or invalid for {}: {other:?}",
                    self.run_id
                )));
            }
        };

        let mut statuses = BTreeMap::new();
        for (step_key, value) in map {
            let step_id = match step_key {
                KernelValue::String(step_id) => StepId::from(step_id.clone()),
                other => {
                    return Err(MobError::Internal(format!(
                        "flow_run step_status key invalid for {}: {other:?}",
                        self.run_id
                    )));
                }
            };
            if matches!(value, KernelValue::None) {
                continue;
            }
            statuses.insert(
                step_id,
                StepRunStatus::from_flow_run_kernel_value(value, &self.run_id)?,
            );
        }

        Ok(statuses)
    }

    /// Typed view of the kernel-owned cumulative failure counter.
    pub fn failure_count(&self) -> Result<u32, MobError> {
        match self.flow_state.fields.get("failure_count") {
            Some(KernelValue::U64(value)) => u32::try_from(*value).map_err(|_| {
                MobError::Internal(format!(
                    "flow_run failure_count out of range for {}",
                    self.run_id
                ))
            }),
            other => Err(MobError::Internal(format!(
                "flow_run failure_count missing or invalid for {}: {other:?}",
                self.run_id
            ))),
        }
    }

    /// Typed view of the kernel-owned consecutive-failure counter.
    pub fn consecutive_failure_count(&self) -> Result<u32, MobError> {
        match self.flow_state.fields.get("consecutive_failure_count") {
            Some(KernelValue::U64(value)) => u32::try_from(*value).map_err(|_| {
                MobError::Internal(format!(
                    "flow_run consecutive_failure_count out of range for {}",
                    self.run_id
                ))
            }),
            other => Err(MobError::Internal(format!(
                "flow_run consecutive_failure_count missing or invalid for {}: {other:?}",
                self.run_id
            ))),
        }
    }

    /// Typed view of the kernel-owned retry budget.
    pub fn max_step_retries(&self) -> Result<u32, MobError> {
        match self.flow_state.fields.get("max_step_retries") {
            Some(KernelValue::U64(value)) => u32::try_from(*value).map_err(|_| {
                MobError::Internal(format!(
                    "flow_run max_step_retries out of range for {}",
                    self.run_id
                ))
            }),
            other => Err(MobError::Internal(format!(
                "flow_run max_step_retries missing or invalid for {}: {other:?}",
                self.run_id
            ))),
        }
    }

    /// Typed view of the kernel-owned supervisor escalation threshold.
    pub fn escalation_threshold(&self) -> Result<u32, MobError> {
        match self.flow_state.fields.get("escalation_threshold") {
            Some(KernelValue::U64(value)) => u32::try_from(*value).map_err(|_| {
                MobError::Internal(format!(
                    "flow_run escalation_threshold out of range for {}",
                    self.run_id
                ))
            }),
            other => Err(MobError::Internal(format!(
                "flow_run escalation_threshold missing or invalid for {}: {other:?}",
                self.run_id
            ))),
        }
    }
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
            frames: BTreeMap::new(),
            loops: BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 4,
            root_step_outputs: IndexMap::new(),
            loop_iteration_outputs: BTreeMap::new(),
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
                // v2 scheduler limits — read from config, default to 0 (unlimited/disabled)
                (
                    "max_active_nodes".to_string(),
                    KernelValue::U64(
                        config
                            .limits
                            .as_ref()
                            .and_then(|l| l.max_active_nodes)
                            .unwrap_or(0),
                    ),
                ),
                (
                    "max_active_frames".to_string(),
                    KernelValue::U64(
                        config
                            .limits
                            .as_ref()
                            .and_then(|l| l.max_active_frames)
                            .unwrap_or(0),
                    ),
                ),
                (
                    "max_frame_depth".to_string(),
                    KernelValue::U64(
                        config
                            .limits
                            .as_ref()
                            .and_then(|l| l.max_frame_depth)
                            .unwrap_or(0),
                    ),
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
                root: None,
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
            // TLA+ NoSelfDependencyInvariant: a step cannot depend on itself.
            if dependency == step_id {
                return Err(MobError::Internal(format!(
                    "step '{step_id}' has a self-dependency"
                )));
            }
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
    pub agent_identity: AgentIdentity,
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

impl StepRunStatus {
    pub(crate) fn parse_kernel_value(value: &KernelValue) -> Result<Self, String> {
        let variant = match value {
            KernelValue::Map(entries) => {
                let some_value = entries
                    .get(&KernelValue::String("value".to_string()))
                    .ok_or_else(|| {
                        format!("expected option payload with `value`, found {value:?}")
                    })?;
                some_value.as_named_variant("StepRunStatus")?
            }
            _ => value.as_named_variant("StepRunStatus")?,
        };

        match variant {
            "Dispatched" => Ok(Self::Dispatched),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Skipped" => Ok(Self::Skipped),
            "Canceled" => Ok(Self::Canceled),
            other => Err(format!("unknown StepRunStatus variant `{other}`")),
        }
    }

    pub(crate) fn from_flow_run_kernel_value(
        value: &KernelValue,
        run_id: &RunId,
    ) -> Result<Self, MobError> {
        Self::parse_kernel_value(value).map_err(|reason| {
            let message = if reason.starts_with("unknown StepRunStatus variant") {
                format!("{reason} for {run_id}")
            } else {
                format!("flow_run step_status entry invalid for {run_id}: {reason}")
            };
            MobError::Internal(message)
        })
    }

    /// A step is terminal when it can no longer receive work dispatch or
    /// completion events. Only `Dispatched` is non-terminal.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Dispatched)
    }
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

/// Per-loop iteration output history, ordered by iteration index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LoopContextHistory {
    /// One entry per completed iteration, ordered by iteration index.
    pub iterations: Vec<IndexMap<StepId, serde_json::Value>>,
}

/// Runtime context available to condition evaluators.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowContext {
    pub run_id: RunId,
    pub activation_params: serde_json::Value,
    /// Root frame step outputs keyed by step_id.
    pub step_outputs: IndexMap<StepId, serde_json::Value>,
    /// Per-loop iteration history keyed by loop_id.
    #[serde(default)]
    pub loop_outputs: IndexMap<LoopId, LoopContextHistory>,
}

impl FlowContext {
    /// Rebuild a `FlowContext` from a persisted `MobRun` aggregate.
    pub fn from_run_aggregate(
        run: &MobRun,
        run_id: RunId,
        activation_params: serde_json::Value,
    ) -> Self {
        let loop_outputs = run
            .loop_iteration_outputs
            .iter()
            .map(|(loop_id, iterations)| {
                let history = LoopContextHistory {
                    iterations: iterations.clone(),
                };
                (loop_id.clone(), history)
            })
            .collect();

        // Seed step_outputs from root outputs, then project last-iteration
        // outputs from each completed loop into step_outputs (dogma Rule 13:
        // the projection must match what execute_frame_inner does at runtime —
        // the last iteration's body step outputs are merged into step_outputs
        // so that downstream steps/templates see them at steps.<id>).
        let mut step_outputs = run.root_step_outputs.clone();
        for iterations in run.loop_iteration_outputs.values() {
            if let Some(last_iter) = iterations.last() {
                for (sid, out) in last_iter {
                    step_outputs.insert(sid.clone(), out.clone());
                }
            }
        }

        FlowContext {
            run_id,
            activation_params,
            step_outputs,
            loop_outputs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        BackendConfig, ConditionExpr, DispatchMode, FlowStepSpec, MobDefinition,
        OrchestratorConfig, WiringRules,
    };
    use crate::ids::{BranchId, ProfileName};
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
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
                root: None,
            },
        );

        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            ProfileBinding::Inline(Profile {
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
            }),
        );
        profiles.insert(
            ProfileName::from("worker"),
            ProfileBinding::Inline(Profile {
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
            }),
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
                ..Default::default()
            }),
            spawn_policy: None,
            event_router: None,
            owner_bridge_session_id: None,
            session_cleanup_policy: crate::definition::SessionCleanupPolicy::Manual,
            is_implicit: false,
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
    fn test_mob_run_kernel_readers_surface_ordered_steps_and_status_snapshot() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a"), StepId::from("step-b")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_status".to_string(),
            KernelValue::Map(BTreeMap::from([
                (
                    KernelValue::String("step-a".to_string()),
                    KernelValue::NamedVariant {
                        enum_name: "StepRunStatus".to_string(),
                        variant: "Completed".to_string(),
                    },
                ),
                (KernelValue::String("step-b".to_string()), KernelValue::None),
            ])),
        );
        run.flow_state
            .fields
            .insert("failure_count".to_string(), KernelValue::U64(3));
        run.flow_state
            .fields
            .insert("consecutive_failure_count".to_string(), KernelValue::U64(2));
        run.flow_state
            .fields
            .insert("max_step_retries".to_string(), KernelValue::U64(4));
        run.flow_state
            .fields
            .insert("escalation_threshold".to_string(), KernelValue::U64(3));

        assert_eq!(
            run.ordered_steps().unwrap(),
            vec![StepId::from("step-a"), StepId::from("step-b")]
        );
        assert_eq!(
            run.step_dependencies().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), Vec::new()),
                (StepId::from("step-b"), Vec::new()),
            ])
        );
        assert_eq!(
            run.step_dependency_modes().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), DependencyMode::All),
                (StepId::from("step-b"), DependencyMode::All),
            ])
        );
        assert_eq!(
            run.step_has_conditions().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), false),
                (StepId::from("step-b"), false)
            ])
        );
        assert_eq!(
            run.step_branches().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), None),
                (StepId::from("step-b"), None)
            ])
        );
        assert_eq!(
            run.step_collection_policy_kinds().unwrap(),
            BTreeMap::from([
                (StepId::from("step-a"), RunCollectionPolicyKind::All),
                (StepId::from("step-b"), RunCollectionPolicyKind::All),
            ])
        );
        assert_eq!(
            run.step_quorum_thresholds().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), 0), (StepId::from("step-b"), 0)])
        );
        assert_eq!(
            run.step_status_snapshot().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), StepRunStatus::Completed)])
        );
        assert_eq!(run.failure_count().unwrap(), 3);
        assert_eq!(run.consecutive_failure_count().unwrap(), 2);
        assert_eq!(run.max_step_retries().unwrap(), 4);
        assert_eq!(run.escalation_threshold().unwrap(), 3);
    }

    #[test]
    fn test_mob_run_step_status_snapshot_rejects_unknown_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_status".to_string(),
            KernelValue::Map(BTreeMap::from([(
                KernelValue::String("step-a".to_string()),
                KernelValue::NamedVariant {
                    enum_name: "StepRunStatus".to_string(),
                    variant: "Broken".to_string(),
                },
            )])),
        );

        let error = run.step_status_snapshot().unwrap_err();
        assert!(
            matches!(error, MobError::Internal(ref message) if message.contains("unknown StepRunStatus variant `Broken`")),
            "expected explicit step status parse failure, got {error:?}"
        );
    }

    #[test]
    fn test_mob_run_step_status_snapshot_accepts_some_wrapped_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_status".to_string(),
            KernelValue::Map(BTreeMap::from([(
                KernelValue::String("step-a".to_string()),
                KernelValue::Map(BTreeMap::from([(
                    KernelValue::String("value".to_string()),
                    KernelValue::NamedVariant {
                        enum_name: "StepRunStatus".to_string(),
                        variant: "Completed".to_string(),
                    },
                )])),
            )])),
        );

        assert_eq!(
            run.step_status_snapshot().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), StepRunStatus::Completed)])
        );
    }

    #[test]
    fn test_mob_run_step_dependencies_reject_invalid_dependency_entry() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_dependencies".to_string(),
            KernelValue::Map(BTreeMap::from([(
                KernelValue::String("step-a".to_string()),
                KernelValue::Seq(vec![KernelValue::Bool(true)]),
            )])),
        );

        let error = run.step_dependencies().unwrap_err();
        assert!(
            matches!(error, MobError::Internal(ref message) if message.contains("step_dependencies dependency invalid")),
            "expected explicit dependency parse failure, got {error:?}"
        );
    }

    #[test]
    fn test_mob_run_step_dependency_modes_reject_unknown_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_dependency_modes".to_string(),
            KernelValue::Map(BTreeMap::from([(
                KernelValue::String("step-a".to_string()),
                KernelValue::NamedVariant {
                    enum_name: "DependencyMode".to_string(),
                    variant: "Broken".to_string(),
                },
            )])),
        );

        let error = run.step_dependency_modes().unwrap_err();
        assert!(
            matches!(error, MobError::Internal(ref message) if message.contains("unknown DependencyMode variant `Broken`")),
            "expected explicit dependency mode parse failure, got {error:?}"
        );
    }

    #[test]
    fn test_mob_run_step_collection_policy_kinds_reject_unknown_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_collection_policies".to_string(),
            KernelValue::Map(BTreeMap::from([(
                KernelValue::String("step-a".to_string()),
                KernelValue::NamedVariant {
                    enum_name: "CollectionPolicyKind".to_string(),
                    variant: "Broken".to_string(),
                },
            )])),
        );

        let error = run.step_collection_policy_kinds().unwrap_err();
        assert!(
            matches!(error, MobError::Internal(ref message) if message.contains("unknown CollectionPolicyKind variant `Broken`")),
            "expected explicit collection policy parse failure, got {error:?}"
        );
    }

    #[test]
    fn test_mob_run_step_has_conditions_rejects_non_bool_entry() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_has_conditions".to_string(),
            KernelValue::Map(BTreeMap::from([(
                KernelValue::String("step-a".to_string()),
                KernelValue::String("yes".to_string()),
            )])),
        );

        let error = run.step_has_conditions().unwrap_err();
        assert!(
            matches!(error, MobError::Internal(ref message) if message.contains("step_has_conditions entry invalid")),
            "expected explicit condition-presence parse failure, got {error:?}"
        );
    }

    #[test]
    fn test_mob_run_step_branches_reject_invalid_entry() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.fields.insert(
            "step_branches".to_string(),
            KernelValue::Map(BTreeMap::from([(
                KernelValue::String("step-a".to_string()),
                KernelValue::Bool(true),
            )])),
        );

        let error = run.step_branches().unwrap_err();
        assert!(
            matches!(error, MobError::Internal(ref message) if message.contains("step_branches entry invalid")),
            "expected explicit branch parse failure, got {error:?}"
        );
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
                agent_identity: AgentIdentity::from("agent-1"),
                status: StepRunStatus::Completed,
                output: Some(serde_json::json!({"ok":true})),
                timestamp: now,
            }],
            failure_ledger: vec![FailureLedgerEntry {
                step_id: StepId::from("step-2"),
                reason: "boom".to_string(),
                timestamp: now,
            }],
            frames: BTreeMap::new(),
            loops: BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 4,
            root_step_outputs: IndexMap::new(),
            loop_iteration_outputs: BTreeMap::new(),
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
            loop_outputs: IndexMap::new(),
        };

        let encoded = serde_json::to_string(&context).unwrap();
        let decoded: FlowContext = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.step_outputs.len(), 1);
        assert_eq!(decoded.activation_params["input"], "x");
    }

    #[test]
    fn topological_steps_rejects_self_dependency() {
        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("s1"),
            FlowStepSpec {
                role: ProfileName::from("worker"),
                message: ContentInput::from("do it"),
                depends_on: vec![StepId::from("s1")],
                dispatch_mode: DispatchMode::FanOut,
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
        let spec = FlowSpec {
            description: None,
            steps,
            root: None,
        };
        let error = topological_steps(&spec).expect_err("self-dependency should be rejected");
        assert!(
            error.to_string().contains("self-dependency"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn step_run_status_terminal_classification() {
        assert!(StepRunStatus::Completed.is_terminal());
        assert!(StepRunStatus::Failed.is_terminal());
        assert!(StepRunStatus::Skipped.is_terminal());
        assert!(StepRunStatus::Canceled.is_terminal());
        assert!(!StepRunStatus::Dispatched.is_terminal());
    }
}

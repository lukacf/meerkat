//! Flow run data model.

use crate::definition::{
    DependencyMode, FlowNodeSpec, FlowSpec, FrameSpec, LimitsSpec, SupervisorSpec, TopologySpec,
};
use crate::error::MobError;
use crate::generated::{flow_frame, flow_run, loop_iteration};
use crate::ids::{
    AgentIdentity, BranchId, FlowId, FlowNodeId, FrameId, LoopId, LoopInstanceId, MobId,
    ProfileName, RunId, StepId,
};
use crate::machines::mob_machine as mob_dsl;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Snapshot of a FlowFrameMachine kernel state stored per-frame in MobRun.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameSnapshot {
    pub kernel_state: flow_frame::State,
}

/// Snapshot of a LoopIterationMachine kernel state stored per-loop in MobRun.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoopSnapshot {
    pub kernel_state: loop_iteration::State,
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
    pub flow_state: flow_run::State,
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
    pub fn flow_state(&self) -> &flow_run::State {
        &self.flow_state
    }

    /// Typed view of the kernel-owned ordered step sequence.
    pub fn ordered_steps(&self) -> Result<Vec<StepId>, MobError> {
        Ok(self.flow_state.ordered_steps.clone())
    }

    /// Typed view of the kernel-owned dependency map keyed by step id.
    pub fn step_dependencies(&self) -> Result<BTreeMap<StepId, Vec<StepId>>, MobError> {
        Ok(self
            .flow_state
            .step_dependencies
            .iter()
            .map(|(step_id, deps)| (step_id.clone(), deps.clone()))
            .collect())
    }

    /// Typed view of the kernel-owned dependency mode map keyed by step id.
    pub fn step_dependency_modes(&self) -> Result<BTreeMap<StepId, DependencyMode>, MobError> {
        self.flow_state
            .step_dependency_modes
            .iter()
            .map(|(step_id, mode)| {
                let mode = match mode.as_str() {
                    "All" => DependencyMode::All,
                    "Any" => DependencyMode::Any,
                    _ => {
                        return Err(MobError::Internal(format!(
                            "flow_run step_dependency_modes unknown DependencyMode variant `{:?}` for {} step '{}'",
                            mode, self.run_id, step_id
                        )));
                    }
                };
                Ok((step_id.clone(), mode))
            })
            .collect()
    }

    /// Typed view of the kernel-owned condition-presence map keyed by step id.
    pub fn step_has_conditions(&self) -> Result<BTreeMap<StepId, bool>, MobError> {
        Ok(self
            .flow_state
            .step_has_conditions
            .iter()
            .map(|(step_id, flag)| (step_id.clone(), *flag))
            .collect())
    }

    /// Typed view of the kernel-owned branch label map keyed by step id.
    pub fn step_branches(&self) -> Result<BTreeMap<StepId, Option<BranchId>>, MobError> {
        Ok(self
            .flow_state
            .step_branches
            .iter()
            .map(|(step_id, branch)| (step_id.clone(), branch.clone()))
            .collect())
    }

    /// Typed view of the kernel-owned collection policy kind map keyed by step id.
    pub fn step_collection_policy_kinds(
        &self,
    ) -> Result<BTreeMap<StepId, RunCollectionPolicyKind>, MobError> {
        self.flow_state
            .step_collection_policies
            .iter()
            .map(|(step_id, policy)| {
                let policy = match policy.as_str() {
                    "All" => RunCollectionPolicyKind::All,
                    "Any" => RunCollectionPolicyKind::Any,
                    "Quorum" => RunCollectionPolicyKind::Quorum,
                    _ => {
                        return Err(MobError::Internal(format!(
                            "flow_run step_collection_policies unknown CollectionPolicyKind variant `{:?}` for {} step '{}'",
                            policy, self.run_id, step_id
                        )));
                    }
                };
                Ok((step_id.clone(), policy))
            })
            .collect()
    }

    /// Typed view of the kernel-owned quorum-threshold map keyed by step id.
    pub fn step_quorum_thresholds(&self) -> Result<BTreeMap<StepId, u32>, MobError> {
        Ok(self
            .flow_state
            .step_quorum_thresholds
            .iter()
            .map(|(step_id, threshold)| (step_id.clone(), *threshold))
            .collect())
    }

    /// Typed view of the kernel-owned step status map, excluding `None` entries.
    pub fn step_status_snapshot(&self) -> Result<BTreeMap<StepId, StepRunStatus>, MobError> {
        let mut statuses = BTreeMap::new();
        for (step_key, value) in &self.flow_state.step_status {
            let Some(value) = value else {
                continue;
            };
            statuses.insert(
                step_key.clone(),
                StepRunStatus::from_flow_run_status(value.as_str(), &self.run_id)?,
            );
        }

        Ok(statuses)
    }

    /// Typed view of the kernel-owned cumulative failure counter.
    pub fn failure_count(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.failure_count)
    }

    /// Typed view of the kernel-owned consecutive-failure counter.
    pub fn consecutive_failure_count(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.consecutive_failure_count)
    }

    /// Typed view of the kernel-owned retry budget.
    pub fn max_step_retries(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.max_step_retries)
    }

    /// Typed view of the kernel-owned supervisor escalation threshold.
    pub fn escalation_threshold(&self) -> Result<u32, MobError> {
        Ok(self.flow_state.escalation_threshold)
    }
}

impl MobRun {
    pub fn pending(
        mob_id: MobId,
        flow_id: FlowId,
        flow_state: flow_run::State,
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
            schema_version: 5,
            root_step_outputs: IndexMap::new(),
            loop_iteration_outputs: BTreeMap::new(),
        }
    }

    pub fn flow_state_for_config(config: &FlowRunConfig) -> Result<flow_run::State, MobError> {
        let initial = flow_run::initial_state();
        let ordered_steps = topological_steps(&config.flow_spec)?;
        let input = flow_run::Input::CreateRun(flow_run::inputs::CreateRun {
            step_ids: config.flow_spec.steps.keys().cloned().collect(),
            ordered_steps,
            step_has_conditions: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| (step_id.clone(), step.condition.is_some()))
                .collect(),
            step_dependencies: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| (step_id.clone(), step.depends_on.clone()))
                .collect(),
            step_dependency_modes: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        step_id.clone(),
                        dependency_mode_value(step.depends_on_mode.clone()),
                    )
                })
                .collect(),
            step_branches: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| (step_id.clone(), step.branch.clone()))
                .collect(),
            step_collection_policies: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        step_id.clone(),
                        collection_policy_kind_value(&step.collection_policy),
                    )
                })
                .collect(),
            step_quorum_thresholds: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    let threshold = match step.collection_policy {
                        crate::definition::CollectionPolicy::Quorum { n } => u32::from(n),
                        _ => 0,
                    };
                    (step_id.clone(), threshold)
                })
                .collect(),
            escalation_threshold: config
                .supervisor
                .as_ref()
                .map_or(0, |supervisor| supervisor.escalation_threshold),
            max_step_retries: config
                .limits
                .as_ref()
                .and_then(|limits| limits.max_step_retries)
                .unwrap_or(0),
            max_active_nodes: config
                .limits
                .as_ref()
                .and_then(|l| l.max_active_nodes)
                .unwrap_or(0)
                .try_into()
                .map_err(|_| MobError::Internal("max_active_nodes exceeds u32".to_string()))?,
            max_active_frames: config
                .limits
                .as_ref()
                .and_then(|l| l.max_active_frames)
                .unwrap_or(0)
                .try_into()
                .map_err(|_| MobError::Internal("max_active_frames exceeds u32".to_string()))?,
            max_frame_depth: config
                .limits
                .as_ref()
                .and_then(|l| l.max_frame_depth)
                .unwrap_or(0)
                .try_into()
                .map_err(|_| MobError::Internal("max_frame_depth exceeds u32".to_string()))?,
        });
        let outcome = flow_run::transition(&initial, input, &flow_run::EmptyContext)
            .map_err(|error| MobError::Internal(format!("flow_run CreateRun failed: {error:?}")))?;
        Ok(outcome.next_state)
    }

    pub(crate) fn create_run_seed_input(
        run_id: &RunId,
        config: &FlowRunConfig,
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        let ordered_steps = topological_steps(&config.flow_spec)?;
        Ok(mob_dsl::MobMachineInput::CreateRunSeed {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            step_ids: config
                .flow_spec
                .steps
                .keys()
                .map(|step_id| mob_dsl::StepId::from(step_id.as_str()))
                .collect(),
            ordered_steps: ordered_steps
                .iter()
                .map(|step_id| mob_dsl::StepId::from(step_id.as_str()))
                .collect(),
            step_has_conditions: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        step.condition.is_some(),
                    )
                })
                .collect(),
            step_dependencies: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        step.depends_on
                            .iter()
                            .map(|dep| mob_dsl::StepId::from(dep.as_str()))
                            .collect(),
                    )
                })
                .collect(),
            step_dependency_modes: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        dependency_mode_seed_value(step.depends_on_mode.clone()),
                    )
                })
                .collect(),
            step_branches: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        step.branch
                            .as_ref()
                            .map(|branch| mob_dsl::BranchId::from(branch.as_str())),
                    )
                })
                .collect(),
            step_collection_policies: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    (
                        mob_dsl::StepId::from(step_id.as_str()),
                        collection_policy_seed_value(&step.collection_policy),
                    )
                })
                .collect(),
            step_quorum_thresholds: config
                .flow_spec
                .steps
                .iter()
                .map(|(step_id, step)| {
                    let threshold = match step.collection_policy {
                        crate::definition::CollectionPolicy::Quorum { n } => u32::from(n),
                        _ => 0,
                    };
                    (mob_dsl::StepId::from(step_id.as_str()), threshold)
                })
                .collect(),
            escalation_threshold: config
                .supervisor
                .as_ref()
                .map_or(0, |supervisor| supervisor.escalation_threshold),
            max_step_retries: config
                .limits
                .as_ref()
                .and_then(|limits| limits.max_step_retries)
                .unwrap_or(0),
            max_active_nodes: config
                .limits
                .as_ref()
                .and_then(|l| l.max_active_nodes)
                .unwrap_or(0)
                .try_into()
                .map_err(|_| MobError::Internal("max_active_nodes exceeds u32".to_string()))?,
            max_active_frames: config
                .limits
                .as_ref()
                .and_then(|l| l.max_active_frames)
                .unwrap_or(0)
                .try_into()
                .map_err(|_| MobError::Internal("max_active_frames exceeds u32".to_string()))?,
            max_frame_depth: config
                .limits
                .as_ref()
                .and_then(|l| l.max_frame_depth)
                .unwrap_or(0)
                .try_into()
                .map_err(|_| MobError::Internal("max_frame_depth exceeds u32".to_string()))?,
        })
    }

    pub(crate) fn create_frame_seed_input(
        run_id: &RunId,
        frame_id: &FrameId,
        loop_instance_id: Option<&LoopInstanceId>,
        iteration: u32,
        frame_scope: mob_dsl::FrameScope,
        spec: &FrameSpec,
        ordered: &[FlowNodeId],
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        let tracked_nodes = ordered
            .iter()
            .map(|node_id| mob_dsl::FlowNodeId::from(node_id.as_str()))
            .collect();
        let ordered_nodes = ordered
            .iter()
            .map(|node_id| mob_dsl::FlowNodeId::from(node_id.as_str()))
            .collect();
        let mut node_kind = BTreeMap::new();
        let mut node_dependencies = BTreeMap::new();
        let mut node_dependency_modes = BTreeMap::new();
        let mut node_branches = BTreeMap::new();

        for (node_id, node_spec) in &spec.nodes {
            let key = mob_dsl::FlowNodeId::from(node_id.as_str());
            match node_spec {
                FlowNodeSpec::Step(step) => {
                    node_kind.insert(key.clone(), mob_dsl::FlowNodeKind::Step);
                    node_dependencies.insert(
                        key.clone(),
                        step.depends_on
                            .iter()
                            .map(|dep| mob_dsl::FlowNodeId::from(dep.as_str()))
                            .collect(),
                    );
                    node_dependency_modes.insert(
                        key.clone(),
                        dependency_mode_seed_value(step.depends_on_mode.clone()),
                    );
                    node_branches.insert(
                        key,
                        step.branch
                            .as_ref()
                            .map(|branch| mob_dsl::BranchId::from(branch.as_str())),
                    );
                }
                FlowNodeSpec::RepeatUntil(loop_spec) => {
                    node_kind.insert(key.clone(), mob_dsl::FlowNodeKind::Loop);
                    node_dependencies.insert(
                        key.clone(),
                        loop_spec
                            .depends_on
                            .iter()
                            .map(|dep| mob_dsl::FlowNodeId::from(dep.as_str()))
                            .collect(),
                    );
                    node_dependency_modes.insert(
                        key.clone(),
                        dependency_mode_seed_value(loop_spec.depends_on_mode.clone()),
                    );
                    node_branches.insert(key, None);
                }
            }
        }

        Ok(mob_dsl::MobMachineInput::CreateFrameSeed {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            frame_id: mob_dsl::FrameId::from(frame_id.as_str()),
            frame_scope,
            loop_instance_id: loop_instance_id.map(|id| mob_dsl::LoopInstanceId::from(id.as_str())),
            iteration,
            tracked_nodes,
            ordered_nodes,
            node_kind,
            node_dependencies,
            node_dependency_modes,
            node_branches,
        })
    }

    pub(crate) fn create_loop_seed_input(
        snapshot: &LoopSnapshot,
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        Ok(mob_dsl::MobMachineInput::CreateLoopSeed {
            loop_instance_id: mob_dsl::LoopInstanceId::from(
                snapshot.kernel_state.loop_instance_id.as_str(),
            ),
            parent_frame_id: mob_dsl::FrameId::from(snapshot.kernel_state.parent_frame_id.as_str()),
            parent_node_id: mob_dsl::FlowNodeId::from(
                snapshot.kernel_state.parent_node_id.as_str(),
            ),
            loop_id: mob_dsl::LoopId::from(snapshot.kernel_state.loop_id.as_str()),
            depth: snapshot.kernel_state.depth,
            max_iterations: snapshot.kernel_state.max_iterations as u64,
        })
    }

    pub(crate) fn create_run_status_input(
        run_id: &RunId,
        status: mob_dsl::FlowRunStatus,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::ProjectRunStatus {
            run_id: mob_dsl::RunId::from(run_id.to_string()),
            status,
        }
    }

    pub(crate) fn create_run_step_status_input(
        run_id: &RunId,
        step_id: &StepId,
        status: mob_dsl::StepRunStatus,
        output_recorded: bool,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::ProjectRunStepStatus {
            run_step: mob_dsl::RunStepKey::from(format!("{run_id}::{step_id}")),
            status,
            output_recorded,
        }
    }

    pub(crate) fn create_frame_phase_input(
        frame_id: &FrameId,
        phase: mob_dsl::FrameStatus,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::ProjectFramePhase {
            frame_id: mob_dsl::FrameId::from(frame_id.as_str()),
            phase,
        }
    }

    pub(crate) fn create_loop_state_input(
        loop_instance_id: &LoopInstanceId,
        phase: mob_dsl::LoopStatus,
        stage: mob_dsl::LoopIterationStage,
        active_body_frame_id: Option<&FrameId>,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::ProjectLoopState {
            loop_instance_id: mob_dsl::LoopInstanceId::from(loop_instance_id.as_str()),
            phase,
            stage,
            active_body_frame_id: active_body_frame_id
                .map(|frame_id| mob_dsl::FrameId::from(frame_id.as_str())),
        }
    }

    pub(crate) fn record_loop_body_frame_completed_input(
        loop_instance_id: &LoopInstanceId,
        iteration: u32,
    ) -> mob_dsl::MobMachineInput {
        mob_dsl::MobMachineInput::RecordLoopBodyFrameCompleted {
            loop_instance_id: mob_dsl::LoopInstanceId::from(loop_instance_id.as_str()),
            iteration: iteration as u64,
        }
    }

    pub(crate) fn record_loop_until_condition_feedback_input(
        loop_instance_id: &LoopInstanceId,
        iteration: u32,
        until_met: bool,
    ) -> mob_dsl::MobMachineInput {
        let loop_instance_id = mob_dsl::LoopInstanceId::from(loop_instance_id.as_str());
        if until_met {
            mob_dsl::MobMachineInput::RecordLoopUntilConditionMet {
                loop_instance_id,
                iteration: iteration as u64,
            }
        } else {
            mob_dsl::MobMachineInput::RecordLoopUntilConditionFailed {
                loop_instance_id,
                iteration: iteration as u64,
            }
        }
    }

    pub fn flow_state_for_steps<I>(step_ids: I) -> Result<flow_run::State, MobError>
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

fn dependency_mode_value(mode: crate::definition::DependencyMode) -> flow_run::DependencyMode {
    match mode {
        crate::definition::DependencyMode::All => flow_run::DependencyMode::All,
        crate::definition::DependencyMode::Any => flow_run::DependencyMode::Any,
    }
}

fn dependency_mode_seed_value(mode: crate::definition::DependencyMode) -> mob_dsl::DependencyMode {
    match mode {
        crate::definition::DependencyMode::All => mob_dsl::DependencyMode::All,
        crate::definition::DependencyMode::Any => mob_dsl::DependencyMode::Any,
    }
}

fn collection_policy_kind_value(
    policy: &crate::definition::CollectionPolicy,
) -> flow_run::CollectionPolicyKind {
    match policy {
        crate::definition::CollectionPolicy::All => flow_run::CollectionPolicyKind::All,
        crate::definition::CollectionPolicy::Any => flow_run::CollectionPolicyKind::Any,
        crate::definition::CollectionPolicy::Quorum { .. } => {
            flow_run::CollectionPolicyKind::Quorum
        }
    }
}

fn collection_policy_seed_value(
    policy: &crate::definition::CollectionPolicy,
) -> mob_dsl::CollectionPolicyKind {
    match policy {
        crate::definition::CollectionPolicy::All => mob_dsl::CollectionPolicyKind::All,
        crate::definition::CollectionPolicy::Any => mob_dsl::CollectionPolicyKind::Any,
        crate::definition::CollectionPolicy::Quorum { .. } => mob_dsl::CollectionPolicyKind::Quorum,
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
    pub(crate) fn from_flow_run_status(value: &str, run_id: &RunId) -> Result<Self, MobError> {
        match value {
            "Dispatched" => Ok(Self::Dispatched),
            "Completed" => Ok(Self::Completed),
            "Failed" => Ok(Self::Failed),
            "Skipped" => Ok(Self::Skipped),
            "Canceled" => Ok(Self::Canceled),
            other => Err(MobError::Internal(format!(
                "unknown StepRunStatus variant `{other}` for {run_id}"
            ))),
        }
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
        run.flow_state.step_status = BTreeMap::from([
            (
                StepId::from("step-a"),
                Some(flow_run::StepRunStatus::Completed),
            ),
            (StepId::from("step-b"), None),
        ]);
        run.flow_state.failure_count = 3;
        run.flow_state.consecutive_failure_count = 2;
        run.flow_state.max_step_retries = 4;
        run.flow_state.escalation_threshold = 3;

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
    fn test_mob_run_step_status_snapshot_accepts_typed_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_status = BTreeMap::from([(
            StepId::from("step-a"),
            Some(flow_run::StepRunStatus::Completed),
        )]);
        assert_eq!(
            run.step_status_snapshot().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), StepRunStatus::Completed)])
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
        run.flow_state.step_status = BTreeMap::from([(
            StepId::from("step-a"),
            Some(flow_run::StepRunStatus::Completed),
        )]);

        assert_eq!(
            run.step_status_snapshot().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), StepRunStatus::Completed)])
        );
    }

    #[test]
    fn test_mob_run_step_dependencies_reject_invalid_dependency_entry() {
        // Typed state makes invalid dependency payloads unrepresentable.
        let run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        assert_eq!(
            run.step_dependencies().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), Vec::new())])
        );
    }

    #[test]
    fn test_mob_run_step_dependency_modes_accept_typed_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_dependency_modes =
            BTreeMap::from([(StepId::from("step-a"), flow_run::DependencyMode::All)]);

        assert_eq!(
            run.step_dependency_modes().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), DependencyMode::All)])
        );
    }

    #[test]
    fn test_mob_run_step_collection_policy_kinds_accept_typed_variant() {
        let mut run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        run.flow_state.step_collection_policies =
            BTreeMap::from([(StepId::from("step-a"), flow_run::CollectionPolicyKind::All)]);

        assert_eq!(
            run.step_collection_policy_kinds().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), RunCollectionPolicyKind::All)])
        );
    }

    #[test]
    fn test_mob_run_step_has_conditions_rejects_non_bool_entry() {
        let run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        assert_eq!(
            run.step_has_conditions().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), false)])
        );
    }

    #[test]
    fn test_mob_run_step_branches_reject_invalid_entry() {
        let run = MobRun::pending(
            MobId::from("mob"),
            FlowId::from("flow-a"),
            MobRun::flow_state_for_steps([StepId::from("step-a")]).unwrap(),
            serde_json::json!({}),
        );
        assert_eq!(
            run.step_branches().unwrap(),
            BTreeMap::from([(StepId::from("step-a"), None)])
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
        assert!(matches!(error, MobError::FlowNotFound(name) if name == "missing"));
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

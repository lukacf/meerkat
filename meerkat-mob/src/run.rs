//! Flow run data model.

use crate::definition::{FlowSpec, LimitsSpec, SupervisorSpec, TopologySpec};
use crate::error::MobError;
use crate::ids::{FlowId, MeerkatId, MobId, ProfileName, RunId, StepId};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Persisted flow run aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobRun {
    pub run_id: RunId,
    pub mob_id: MobId,
    pub flow_id: FlowId,
    pub status: MobRunStatus,
    pub activation_params: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub step_ledger: Vec<StepLedgerEntry>,
    pub failure_ledger: Vec<FailureLedgerEntry>,
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
    use std::collections::BTreeMap;

    fn sample_definition() -> MobDefinition {
        let mut steps = IndexMap::new();
        steps.insert(
            StepId::from("s1"),
            FlowStepSpec {
                role: ProfileName::from("worker"),
                message: "do it".to_string(),
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

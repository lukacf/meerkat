use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type MobSpecRevision = u64;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_newtype!(MobId);
id_newtype!(RunId);
id_newtype!(FlowId);
id_newtype!(StepId);
id_newtype!(MeerkatId);
id_newtype!(RoleId);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpecUpdateMode {
    HotReload,
    #[default]
    DrainReplace,
    ForceReset,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobSpecDocument {
    pub mob: MobSpecRoot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobSpecRoot {
    pub specs: BTreeMap<String, MobSpecBody>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobSpecBody {
    #[serde(default)]
    pub revision: Option<MobSpecRevision>,
    #[serde(default)]
    pub roles: BTreeMap<String, RoleSpec>,
    #[serde(default)]
    pub topology: TopologySpec,
    #[serde(default)]
    pub flows: BTreeMap<String, FlowSpec>,
    #[serde(default)]
    pub prompts: BTreeMap<String, String>,
    #[serde(default)]
    pub schemas: BTreeMap<String, Value>,
    #[serde(default)]
    pub tool_bundles: BTreeMap<String, ToolBundleSpec>,
    #[serde(default)]
    pub resolvers: BTreeMap<String, ResolverSpec>,
    #[serde(default)]
    pub supervisor: SupervisorSpec,
    #[serde(default)]
    pub limits: LimitsSpec,
    #[serde(default)]
    pub retention: RetentionSpec,
    #[serde(default)]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobSpec {
    pub mob_id: String,
    pub revision: MobSpecRevision,
    pub roles: BTreeMap<String, RoleSpec>,
    pub topology: TopologySpec,
    pub flows: BTreeMap<String, FlowSpec>,
    pub prompts: BTreeMap<String, String>,
    pub schemas: BTreeMap<String, Value>,
    pub tool_bundles: BTreeMap<String, ToolBundleSpec>,
    pub resolvers: BTreeMap<String, ResolverSpec>,
    pub supervisor: SupervisorSpec,
    pub limits: LimitsSpec,
    pub retention: RetentionSpec,
    pub applied_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleSpec {
    pub prompt_ref: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub cardinality: CardinalityKind,
    #[serde(default)]
    pub spawn_strategy: SpawnStrategy,
    #[serde(default)]
    pub resolver: Option<String>,
    #[serde(default)]
    pub preload_skills: Vec<String>,
    #[serde(default)]
    pub tool_bundles: Vec<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CardinalityKind {
    #[default]
    Singleton,
    PerKey,
    PerMeerkat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpawnStrategy {
    Eager,
    #[default]
    Lazy,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeerkatIdentity {
    pub meerkat_id: String,
    pub role: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub attributes: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlowSpec {
    #[serde(default)]
    pub trigger: Option<TriggerSpec>,
    #[serde(default)]
    pub steps: Vec<FlowStepSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerSpec {
    Manual,
    Event { event_type: String },
    AgentOutput { role: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowStepSpec {
    pub step_id: String,
    pub targets: StepTargets,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub dispatch_mode: DispatchMode,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub collection_policy: CollectionPolicy,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub on_timeout: TimeoutPolicy,
    #[serde(default)]
    pub condition: Option<ConditionExpr>,
    #[serde(default)]
    pub expected_schema_ref: Option<String>,
    #[serde(default)]
    pub schema_policy: SchemaPolicy,
    #[serde(default)]
    pub max_hops: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTargets {
    pub role: String,
    #[serde(default = "default_target_meerkat_selector")]
    pub meerkat_id: String,
}

fn default_target_meerkat_selector() -> String {
    "*".to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DispatchMode {
    #[default]
    FanOut,
    OneToOne,
    FanIn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CollectionPolicy {
    All,
    Quorum { n: usize },
    Any,
}

impl Default for CollectionPolicy {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutPolicy {
    #[default]
    Partial,
    Fail,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemaPolicy {
    #[default]
    RetryThenFail,
    RetryThenWarn,
    WarnOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionExpr {
    Eq { left: String, right: Value },
    In { left: String, right: Vec<Value> },
    Gt { left: String, right: Value },
    Lt { left: String, right: Value },
    And { all: Vec<ConditionExpr> },
    Or { any: Vec<ConditionExpr> },
    Not { expr: Box<ConditionExpr> },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TopologySpec {
    #[serde(default)]
    pub flow_dispatched: TopologyDomainSpec,
    #[serde(default)]
    pub ad_hoc: TopologyDomainSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyDomainSpec {
    #[serde(default)]
    pub mode: PolicyMode,
    #[serde(default)]
    pub rules: Vec<TopologyRule>,
}

impl Default for TopologyDomainSpec {
    fn default() -> Self {
        Self {
            mode: PolicyMode::Advisory,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    #[default]
    Advisory,
    Strict,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TopologyRule {
    #[serde(default)]
    pub from_roles: Vec<String>,
    #[serde(default)]
    pub to_roles: Vec<String>,
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub intents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolBundleSpec {
    Builtin {
        #[serde(default = "default_true")]
        enable_builtins: bool,
        #[serde(default)]
        enable_shell: bool,
        #[serde(default)]
        enable_subagents: bool,
        #[serde(default)]
        enable_memory: bool,
    },
    Mcp {
        servers: Vec<String>,
        #[serde(default)]
        unavailable: UnavailablePolicy,
    },
    RustBundle {
        bundle_id: String,
        #[serde(default)]
        unavailable: UnavailablePolicy,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnavailablePolicy {
    #[default]
    FailActivation,
    DegradeWarn,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResolverSpec {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub refresh: RefreshStrategy,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RefreshStrategy {
    Poll { interval_ms: u64 },
    EventDriven,
    OnActivation,
}

impl Default for RefreshStrategy {
    fn default() -> Self {
        Self::OnActivation
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupervisorSpec {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub notify_role: Option<String>,
    #[serde(default = "default_supervisor_retry_backoff_ms")]
    pub retry_backoff_ms: u64,
}

impl Default for SupervisorSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            notify_role: None,
            retry_backoff_ms: default_supervisor_retry_backoff_ms(),
        }
    }
}

fn default_supervisor_retry_backoff_ms() -> u64 {
    500
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsSpec {
    #[serde(default = "default_max_concurrent_ready_steps")]
    pub max_concurrent_ready_steps: usize,
    #[serde(default = "default_max_flow_depth")]
    pub max_flow_depth: u32,
    #[serde(default = "default_step_timeout_ms")]
    pub default_step_timeout_ms: u64,
}

impl Default for LimitsSpec {
    fn default() -> Self {
        Self {
            max_concurrent_ready_steps: default_max_concurrent_ready_steps(),
            max_flow_depth: default_max_flow_depth(),
            default_step_timeout_ms: default_step_timeout_ms(),
        }
    }
}

fn default_max_concurrent_ready_steps() -> usize {
    16
}

fn default_max_flow_depth() -> u32 {
    6
}

fn default_step_timeout_ms() -> u64 {
    120_000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionSpec {
    #[serde(default)]
    pub event_ttl_secs_by_category: BTreeMap<String, u64>,
    #[serde(default = "default_run_ttl_secs")]
    pub run_ttl_secs: u64,
}

impl Default for RetentionSpec {
    fn default() -> Self {
        Self {
            event_ttl_secs_by_category: BTreeMap::new(),
            run_ttl_secs: default_run_ttl_secs(),
        }
    }
}

fn default_run_ttl_secs() -> u64 {
    7 * 24 * 60 * 60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobActivationRequest {
    pub mob_id: String,
    pub flow_id: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[must_use]
pub struct MobActivationResponse {
    pub run_id: String,
    pub status: MobRunStatus,
    pub spec_revision: MobSpecRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobRun {
    pub run_id: String,
    pub mob_id: String,
    pub flow_id: String,
    pub spec_revision: MobSpecRevision,
    pub status: MobRunStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub activation_payload: Value,
    #[serde(default)]
    pub step_statuses: BTreeMap<String, StepRunStatus>,
    #[serde(default)]
    pub step_outputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub step_ledger: Vec<StepLedgerEntry>,
    #[serde(default)]
    pub failure_ledger: Vec<FailureLedgerEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MobRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepRunStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Partial,
    Failed,
    Skipped,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepLedgerEntry {
    pub timestamp: DateTime<Utc>,
    pub step_id: String,
    pub target_meerkat: String,
    #[serde(default)]
    pub logical_key: String,
    pub attempt: u32,
    pub status: StepRunStatus,
    #[serde(default)]
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureLedgerEntry {
    pub timestamp: DateTime<Utc>,
    pub step_id: Option<String>,
    pub target_meerkat: Option<String>,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobRunFilter {
    #[serde(default)]
    pub status: Option<MobRunStatus>,
    #[serde(default)]
    pub mob_id: Option<String>,
    #[serde(default)]
    pub flow_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

impl Default for MobRunFilter {
    fn default() -> Self {
        Self {
            status: None,
            mob_id: None,
            flow_id: None,
            limit: Some(50),
            offset: Some(0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobEvent {
    pub cursor: u64,
    pub timestamp: DateTime<Utc>,
    pub category: MobEventCategory,
    pub mob_id: String,
    pub run_id: Option<String>,
    pub flow_id: Option<String>,
    pub step_id: Option<String>,
    pub meerkat_id: Option<String>,
    pub kind: MobEventKind,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMobEvent {
    pub timestamp: DateTime<Utc>,
    pub category: MobEventCategory,
    pub mob_id: String,
    pub run_id: Option<String>,
    pub flow_id: Option<String>,
    pub step_id: Option<String>,
    pub meerkat_id: Option<String>,
    pub kind: MobEventKind,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MobEventCategory {
    Lifecycle,
    Flow,
    Dispatch,
    Topology,
    Supervisor,
    Audit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobEventKind {
    SpecApplied,
    SpecDeleted,
    RunActivated,
    RunCompleted,
    RunFailed,
    RunCanceled,
    StepStarted,
    StepCompleted,
    StepPartial,
    StepFailed,
    TopologyViolation,
    SupervisorEscalation,
    ForceResetTransition,
    ReconcileApplied,
    MeerkatSpawned,
    MeerkatRetired,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobReconcileRequest {
    pub mob_id: String,
    #[serde(default)]
    pub mode: ReconcileMode,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileMode {
    Report,
    #[default]
    Apply,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[must_use]
pub struct MobReconcileResult {
    #[serde(default)]
    pub spawned: Vec<String>,
    #[serde(default)]
    pub retired: Vec<String>,
    #[serde(default)]
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollEventsRequest {
    #[serde(default)]
    pub cursor: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[must_use]
pub struct PollEventsResponse {
    pub next_cursor: u64,
    pub events: Vec<MobEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeerkatInstance {
    pub mob_id: String,
    pub role: String,
    pub meerkat_id: String,
    pub session_id: String,
    pub comms_name: String,
    pub labels: BTreeMap<String, String>,
    pub last_activity_at: DateTime<Utc>,
    pub status: MeerkatInstanceStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StepOutput {
    pub status: StepRunStatus,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub output: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConditionContext {
    #[serde(default)]
    pub activation: Value,
    #[serde(default)]
    pub steps: BTreeMap<String, StepOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowContext {
    pub run_id: String,
    pub mob_id: String,
    pub flow_id: String,
    pub spec_revision: MobSpecRevision,
    #[serde(default)]
    pub activation_payload: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeerkatInstanceStatus {
    Running,
    Idle,
    Processing,
    Error,
    Retired,
}

impl MobRun {
    pub fn new(
        run_id: String,
        mob_id: String,
        flow_id: String,
        spec_revision: MobSpecRevision,
        activation_payload: Value,
    ) -> Self {
        let now = Utc::now();
        Self {
            run_id,
            mob_id,
            flow_id,
            spec_revision,
            status: MobRunStatus::Pending,
            created_at: now,
            updated_at: now,
            completed_at: None,
            activation_payload,
            step_statuses: BTreeMap::new(),
            step_outputs: BTreeMap::new(),
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn id_newtypes_serde_roundtrip() {
        let ids = [
            serde_json::to_string(&MobId::from("mob-1")).unwrap(),
            serde_json::to_string(&RunId::from("run-1")).unwrap(),
            serde_json::to_string(&FlowId::from("flow-1")).unwrap(),
            serde_json::to_string(&StepId::from("step-1")).unwrap(),
            serde_json::to_string(&MeerkatId::from("meerkat-1")).unwrap(),
            serde_json::to_string(&RoleId::from("role-1")).unwrap(),
        ];
        assert_eq!(ids[0], "\"mob-1\"");
        assert_eq!(ids[1], "\"run-1\"");
        assert_eq!(ids[2], "\"flow-1\"");
        assert_eq!(ids[3], "\"step-1\"");
        assert_eq!(ids[4], "\"meerkat-1\"");
        assert_eq!(ids[5], "\"role-1\"");
    }
}

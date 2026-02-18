//! Mob spec types, TOML parsing, and declarative configuration.
//!
//! A [`MobSpec`] describes the declarative configuration for a mob: roles,
//! flows (DAG-based step graphs), topology policy, tool bundles, resolver
//! configuration, supervisor config, and operational limits.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Top-level spec
// ---------------------------------------------------------------------------

/// Complete declarative specification for a mob.
///
/// Parsed from TOML under `[mob.specs.<mob_id>]`. Contains roles, flows,
/// topology policy, tool bundles, resolver config, supervisor config, and
/// operational limits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MobSpec {
    /// Unique identifier for this mob.
    pub mob_id: String,
    /// Monotonically increasing revision number for CAS updates.
    #[serde(default = "default_revision")]
    pub spec_revision: u64,
    /// How spec updates are applied to running instances.
    #[serde(default)]
    pub apply_mode: ApplyMode,
    /// Role definitions for meerkats in this mob.
    pub roles: Vec<RoleSpec>,
    /// Topology policy governing inter-meerkat communication.
    #[serde(default)]
    pub topology: TopologySpec,
    /// DAG-based flow definitions.
    pub flows: Vec<FlowSpec>,
    /// Named prompt templates, referenced via `config://prompts/<name>`.
    #[serde(default)]
    pub prompts: BTreeMap<String, String>,
    /// Named JSON schemas for step output validation.
    #[serde(default)]
    pub schemas: BTreeMap<String, serde_json::Value>,
    /// Named tool bundle definitions.
    #[serde(default)]
    pub tool_bundles: BTreeMap<String, ToolBundleSpec>,
    /// Named resolver configurations for dynamic meerkat discovery.
    #[serde(default)]
    pub resolvers: BTreeMap<String, ResolverSpec>,
    /// Supervisor configuration.
    #[serde(default)]
    pub supervisor: SupervisorSpec,
    /// Operational limits.
    #[serde(default)]
    pub limits: MobLimits,
    /// Event retention policy.
    #[serde(default)]
    pub retention: RetentionSpec,
}

fn default_revision() -> u64 {
    1
}

// ---------------------------------------------------------------------------
// Apply mode
// ---------------------------------------------------------------------------

/// How spec updates are applied when runs are already in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    /// Running runs are pinned to old revision; new runs use latest.
    #[default]
    DrainReplace,
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// Configuration for a single role within a mob.
///
/// Each role maps to one or more meerkat agent instances. The role defines
/// the agent's prompt, tools, model, cardinality, and spawn strategy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleSpec {
    /// Role name (unique within a mob spec).
    pub role: String,
    /// Reference to a prompt template (e.g., `config://prompts/reviewer`).
    #[serde(default)]
    pub prompt_ref: Option<String>,
    /// Inline prompt text (used when `prompt_ref` is `None`).
    #[serde(default)]
    pub prompt_inline: Option<String>,
    /// Skills to pre-load at agent build time.
    #[serde(default)]
    pub preload_skills: Vec<String>,
    /// How many instances of this role to create.
    #[serde(default)]
    pub cardinality: CardinalitySpec,
    /// When to create instances of this role.
    #[serde(default)]
    pub spawn_strategy: SpawnStrategy,
    /// Named tool bundles to attach to agents in this role.
    #[serde(default)]
    pub tool_bundles: Vec<String>,
    /// Tool access policy override for this role.
    #[serde(default)]
    pub tool_policy: Option<ToolPolicySpec>,
    /// LLM model override. If `None`, uses the realm/runtime config default.
    #[serde(default)]
    pub model: Option<String>,
}

// ---------------------------------------------------------------------------
// Cardinality and spawn strategy
// ---------------------------------------------------------------------------

/// How many meerkat instances a role should have.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CardinalitySpec {
    /// Exactly one instance.
    #[default]
    Singleton,
    /// One instance per key (key provided at activation time).
    PerKey {
        /// Description of what the key represents.
        #[serde(default)]
        key_description: Option<String>,
    },
    /// Instances sourced from a resolver.
    PerMeerkat {
        /// Name of the resolver to use.
        resolver: String,
    },
}

/// When to create meerkat instances for a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpawnStrategy {
    /// Create instances at mob activation time.
    #[default]
    Eager,
    /// Create instances on first dispatch to the role.
    Lazy,
}

// ---------------------------------------------------------------------------
// Flows
// ---------------------------------------------------------------------------

/// A flow is a directed acyclic graph of steps.
///
/// Steps define dispatch targets, collection policies, and dependencies.
/// Steps with no dependencies are entry points and run immediately.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowSpec {
    /// Unique identifier for this flow within the mob spec.
    pub flow_id: String,
    /// Ordered list of steps in this flow.
    pub steps: Vec<FlowStepSpec>,
}

/// A single step within a flow DAG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowStepSpec {
    /// Unique identifier for this step within its flow.
    pub step_id: String,
    /// Step IDs that must complete before this step can run.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Which meerkats to dispatch to.
    pub targets: TargetSelector,
    /// How to distribute work across targets.
    #[serde(default)]
    pub dispatch_mode: DispatchMode,
    /// How to collect responses from targets.
    #[serde(default)]
    pub collection_policy: CollectionPolicy,
    /// Timeout for this step in milliseconds.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Retry configuration for failed dispatches.
    #[serde(default)]
    pub retry: RetrySpec,
    /// Reference to a JSON schema for validating step output.
    #[serde(default)]
    pub expected_schema_ref: Option<String>,
    /// Policy for handling schema validation failures.
    #[serde(default)]
    pub schema_policy: SchemaPolicy,
    /// Optional condition that must be true for the step to execute.
    #[serde(default)]
    pub condition: Option<Condition>,
}

fn default_timeout_ms() -> u64 {
    30_000
}

// ---------------------------------------------------------------------------
// Target selector
// ---------------------------------------------------------------------------

/// Identifies which meerkats a step dispatches to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TargetSelector {
    /// Role name to target.
    pub role: String,
    /// Specific meerkat ID. `None` = singleton, `"*"` = all in role.
    #[serde(default)]
    pub meerkat_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Dispatch mode
// ---------------------------------------------------------------------------

/// How work is distributed across target meerkats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DispatchMode {
    /// Send to all targets simultaneously.
    #[default]
    FanOut,
    /// Send to exactly one target.
    OneToOne,
    /// Reserved keyword. Hard validation error if used.
    FanIn,
}

// ---------------------------------------------------------------------------
// Collection policy
// ---------------------------------------------------------------------------

/// Policy for collecting responses from dispatched targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectionPolicy {
    /// How many responses are required.
    #[serde(default)]
    pub mode: CollectionMode,
    /// What to do when the timeout expires.
    #[serde(default)]
    pub timeout_behavior: TimeoutBehavior,
}

impl Default for CollectionPolicy {
    fn default() -> Self {
        Self {
            mode: CollectionMode::All,
            timeout_behavior: TimeoutBehavior::Partial,
        }
    }
}

/// How many responses must be collected for a step to succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CollectionMode {
    /// All targets must respond.
    #[default]
    All,
    /// At least `n` targets must respond.
    Quorum(u32),
    /// At least one target must respond.
    Any,
}

/// What happens when a collection timeout expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TimeoutBehavior {
    /// Continue with whatever responses have been collected.
    #[default]
    Partial,
    /// Fail the step.
    Fail,
}

// ---------------------------------------------------------------------------
// Retry
// ---------------------------------------------------------------------------

/// Retry configuration for failed step dispatches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrySpec {
    /// Maximum number of attempts (including the initial attempt).
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    /// Initial backoff duration in milliseconds.
    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: u64,
    /// Backoff multiplier for exponential backoff.
    #[serde(default = "default_multiplier")]
    pub multiplier: f64,
    /// Maximum backoff duration in milliseconds.
    #[serde(default = "default_max_backoff_ms")]
    pub max_backoff_ms: u64,
}

impl Default for RetrySpec {
    fn default() -> Self {
        Self {
            attempts: default_attempts(),
            backoff_ms: default_backoff_ms(),
            multiplier: default_multiplier(),
            max_backoff_ms: default_max_backoff_ms(),
        }
    }
}

fn default_attempts() -> u32 {
    3
}
fn default_backoff_ms() -> u64 {
    1000
}
fn default_multiplier() -> f64 {
    2.0
}
fn default_max_backoff_ms() -> u64 {
    30_000
}

// ---------------------------------------------------------------------------
// Schema policy
// ---------------------------------------------------------------------------

/// Policy for handling step output schema validation failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SchemaPolicy {
    /// Retry the step, then fail if still invalid.
    #[default]
    RetryThenFail,
    /// Retry the step, then warn if still invalid.
    RetryThenWarn,
    /// Warn on validation failure without retrying.
    WarnOnly,
}

// ---------------------------------------------------------------------------
// Topology
// ---------------------------------------------------------------------------

/// Topology policy governing inter-meerkat communication within a mob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopologySpec {
    /// Enforcement mode for flow-dispatched communication.
    #[serde(default)]
    pub mode: TopologyMode,
    /// Enforcement mode for ad-hoc (non-flow) communication.
    #[serde(default)]
    pub ad_hoc_mode: AdHocMode,
    /// Default action for communication not covered by explicit rules.
    #[serde(default)]
    pub default_action: TopologyAction,
    /// Explicit communication rules.
    #[serde(default)]
    pub rules: Vec<TopologyRule>,
}

impl Default for TopologySpec {
    fn default() -> Self {
        Self {
            mode: TopologyMode::Advisory,
            ad_hoc_mode: AdHocMode::Advisory,
            default_action: TopologyAction::Allow,
            rules: Vec::new(),
        }
    }
}

/// Enforcement mode for flow-dispatched topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TopologyMode {
    /// Log violations but allow communication.
    #[default]
    Advisory,
    /// Block disallowed communication.
    Strict,
}

/// Enforcement mode for ad-hoc (non-flow) topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AdHocMode {
    /// Log violations but allow communication.
    #[default]
    Advisory,
    /// Require explicit opt-in for ad-hoc communication.
    StrictOptIn,
}

/// Action to take for a topology rule match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TopologyAction {
    /// Allow the communication.
    #[default]
    Allow,
    /// Deny the communication.
    Deny,
}

/// A single topology rule governing communication between roles.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopologyRule {
    /// Source role name.
    pub from: String,
    /// Destination role name.
    pub to: String,
    /// Action to take when this rule matches.
    pub action: TopologyAction,
}

// ---------------------------------------------------------------------------
// Tool bundles
// ---------------------------------------------------------------------------

/// A named bundle of tools to attach to a role's agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolBundleSpec {
    /// Kind of tool bundle.
    pub kind: ToolBundleKind,
    /// What to do when the bundle is unavailable at activation time.
    #[serde(default)]
    pub on_unavailable: OnUnavailable,
}

/// Kind of tool bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolBundleKind {
    /// Built-in tools from meerkat-tools.
    Builtin,
    /// MCP server reference (uses existing realm MCP registry).
    Mcp {
        /// MCP server name.
        server: String,
    },
    /// Rust-native tool bundle.
    RustBundle {
        /// Bundle identifier.
        bundle_id: String,
    },
}

/// Behavior when a tool bundle is unavailable at activation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnUnavailable {
    /// Fail the entire activation.
    #[default]
    FailActivation,
    /// Warn and degrade gracefully.
    DegradeWarn,
}

// ---------------------------------------------------------------------------
// Tool policy
// ---------------------------------------------------------------------------

/// Tool access policy override for a role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolPolicySpec {
    /// List of tool names to allow. If empty, all tools are allowed.
    #[serde(default)]
    pub allow: Vec<String>,
    /// List of tool names to deny. Applied after `allow`.
    #[serde(default)]
    pub deny: Vec<String>,
}

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

/// Configuration for a meerkat resolver.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolverSpec {
    /// Resolver kind.
    pub kind: ResolverKind,
}

/// Kind of resolver.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ResolverKind {
    /// Static list of meerkat identities.
    Static {
        /// Pre-defined identities.
        identities: Vec<StaticIdentity>,
    },
}

/// A statically defined meerkat identity for a resolver.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StaticIdentity {
    /// Meerkat ID.
    pub meerkat_id: String,
    /// Labels for discovery.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Supervisor
// ---------------------------------------------------------------------------

/// Configuration for the mob supervisor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SupervisorSpec {
    /// Whether the supervisor is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Timeout for detecting stalled dispatches, in milliseconds.
    #[serde(default = "default_supervisor_timeout_ms")]
    pub dispatch_timeout_ms: u64,
    /// Optional role to notify on escalation.
    #[serde(default)]
    pub notify_role: Option<String>,
}

impl Default for SupervisorSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            dispatch_timeout_ms: default_supervisor_timeout_ms(),
            notify_role: None,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_supervisor_timeout_ms() -> u64 {
    60_000
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// Operational limits for a mob.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MobLimits {
    /// Maximum number of steps per flow.
    #[serde(default = "default_max_steps_per_flow")]
    pub max_steps_per_flow: usize,
    /// Maximum DAG depth for flows.
    #[serde(default = "default_max_flow_depth")]
    pub max_flow_depth: usize,
    /// Maximum number of concurrently ready steps.
    #[serde(default = "default_max_concurrent_ready_steps")]
    pub max_concurrent_ready_steps: usize,
    /// Maximum number of concurrent active runs.
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: usize,
}

impl Default for MobLimits {
    fn default() -> Self {
        Self {
            max_steps_per_flow: default_max_steps_per_flow(),
            max_flow_depth: default_max_flow_depth(),
            max_concurrent_ready_steps: default_max_concurrent_ready_steps(),
            max_concurrent_runs: default_max_concurrent_runs(),
        }
    }
}

fn default_max_steps_per_flow() -> usize {
    50
}
fn default_max_flow_depth() -> usize {
    20
}
fn default_max_concurrent_ready_steps() -> usize {
    10
}
fn default_max_concurrent_runs() -> usize {
    5
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Event retention policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetentionSpec {
    /// TTL for audit events in seconds.
    #[serde(default = "default_audit_ttl")]
    pub audit_ttl_secs: u64,
    /// TTL for operational events in seconds.
    #[serde(default = "default_ops_ttl")]
    pub ops_ttl_secs: u64,
    /// TTL for debug events in seconds.
    #[serde(default = "default_debug_ttl")]
    pub debug_ttl_secs: u64,
}

impl Default for RetentionSpec {
    fn default() -> Self {
        Self {
            audit_ttl_secs: default_audit_ttl(),
            ops_ttl_secs: default_ops_ttl(),
            debug_ttl_secs: default_debug_ttl(),
        }
    }
}

fn default_audit_ttl() -> u64 {
    7 * 24 * 3600 // 7 days
}
fn default_ops_ttl() -> u64 {
    24 * 3600 // 1 day
}
fn default_debug_ttl() -> u64 {
    3600 // 1 hour
}

// ---------------------------------------------------------------------------
// Condition (v1 DSL)
// ---------------------------------------------------------------------------

/// A simple condition expression for conditional step execution.
///
/// v1 supports boolean combinators (`all`, `any`, `not`) and simple predicates
/// (`eq`, `in_set`, `gt`) evaluated against activation params and step results.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// All sub-conditions must be true.
    All(Vec<Condition>),
    /// At least one sub-condition must be true.
    Any(Vec<Condition>),
    /// Negate a sub-condition.
    Not(Box<Condition>),
    /// Equality predicate: `path == value`.
    Eq {
        /// Dot-separated path (e.g., `activation.priority`, `step.analyze.status`).
        path: String,
        /// Expected value.
        value: serde_json::Value,
    },
    /// Set membership predicate: `path in [values]`.
    InSet {
        /// Dot-separated path.
        path: String,
        /// Set of acceptable values.
        values: Vec<serde_json::Value>,
    },
    /// Greater-than predicate: `path > value`.
    Gt {
        /// Dot-separated path.
        path: String,
        /// Threshold value.
        value: serde_json::Value,
    },
}

// ---------------------------------------------------------------------------
// TOML parsing
// ---------------------------------------------------------------------------

/// Intermediate TOML wrapper for mob spec parsing.
///
/// The TOML format is:
/// ```toml
/// [mob.specs.<mob_id>]
/// roles = [...]
/// flows = [...]
/// ...
/// ```
#[derive(Debug, Deserialize)]
struct MobTomlWrapper {
    mob: MobTomlMob,
}

#[derive(Debug, Deserialize)]
struct MobTomlMob {
    specs: BTreeMap<String, MobSpecInner>,
}

/// Inner spec without mob_id (which comes from the TOML key).
#[derive(Debug, Deserialize)]
struct MobSpecInner {
    #[serde(default = "default_revision")]
    spec_revision: u64,
    #[serde(default)]
    apply_mode: ApplyMode,
    roles: Vec<RoleSpec>,
    #[serde(default)]
    topology: TopologySpec,
    flows: Vec<FlowSpec>,
    #[serde(default)]
    prompts: BTreeMap<String, String>,
    #[serde(default)]
    schemas: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    tool_bundles: BTreeMap<String, ToolBundleSpec>,
    #[serde(default)]
    resolvers: BTreeMap<String, ResolverSpec>,
    #[serde(default)]
    supervisor: SupervisorSpec,
    #[serde(default)]
    limits: MobLimits,
    #[serde(default)]
    retention: RetentionSpec,
}

/// Parse a TOML string containing mob specs into a list of [`MobSpec`].
///
/// The expected format is `[mob.specs.<mob_id>]`. Returns all specs found
/// in the TOML document.
pub fn parse_mob_specs_from_toml(toml_str: &str) -> Result<Vec<MobSpec>, crate::error::MobError> {
    let wrapper: MobTomlWrapper =
        toml::from_str(toml_str).map_err(|e| crate::error::MobError::Internal(e.to_string()))?;

    let specs: Vec<MobSpec> = wrapper
        .mob
        .specs
        .into_iter()
        .map(|(mob_id, inner)| MobSpec {
            mob_id,
            spec_revision: inner.spec_revision,
            apply_mode: inner.apply_mode,
            roles: inner.roles,
            topology: inner.topology,
            flows: inner.flows,
            prompts: inner.prompts,
            schemas: inner.schemas,
            tool_bundles: inner.tool_bundles,
            resolvers: inner.resolvers,
            supervisor: inner.supervisor,
            limits: inner.limits,
            retention: inner.retention,
        })
        .collect();

    Ok(specs)
}

use crate::definition::{
    CollectionPolicy, DispatchMode, FlowSchemaRef, FlowSpec, FlowStepSpec, OrchestratorConfig,
    RoleWiringRule, StepOutputFormat, WiringRules,
};
use crate::ids::{FlowId, MobId, ProfileName, StepId};
use crate::profile::{Profile, ProfileBinding};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
#[cfg(target_arch = "wasm32")]
use crate::tokio::time as tokio_time;
use crate::{MobDefinition, MobHandle, MobRun, RunId, SpawnMemberSpec};
use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::schema::MeerkatSchema;
use meerkat_core::types::ContentInput;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time as tokio_time;

const ADAPTIVE_RETRY_INITIAL: Duration = Duration::from_millis(25);
const ADAPTIVE_RETRY_MAX: Duration = Duration::from_secs(1);
const ADAPTIVE_RETRY_DIAGNOSTIC_LIMIT: usize = 8;
const ADAPTIVE_TERMINALIZATION_GRACE: Duration = Duration::from_secs(5);
const ADAPTIVE_AUTHORITY_GRACE: Duration = Duration::from_secs(1);
const ADAPTIVE_CUSTODY_HANDOFF_GRACE: Duration = Duration::from_millis(100);

/// The runtime boundary that owned an adaptive operation when its wall-clock
/// budget expired.
///
/// Keeping this typed prevents a timeout from degrading into an opaque outer
/// test failure and gives production diagnostics a stable stage vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveRuntimeStage {
    Initialization,
    Planning,
    PlanningAdmission,
    PlanningTerminalWait,
    PlanningDecision,
    PlanRejection,
    PlanningCancellation,
    LayerProvision,
    LayerAdmission,
    LayerProvisionAuthority,
    LayerStart,
    LayerRunStartAuthority,
    LayerTerminal,
    LayerTerminalAuthority,
    LayerCancellation,
    LayerInterruption,
    LayerResultAuthority,
    LayerCleanup,
    LayerDisposition,
    FinishAuthority,
    RunCancellation,
    DeadlineObservation,
}

impl std::fmt::Display for AdaptiveRuntimeStage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Initialization => "initialization",
            Self::Planning => "planning",
            Self::PlanningAdmission => "planning flow admission and target provisioning",
            Self::PlanningTerminalWait => "planning flow terminal wait",
            Self::PlanningDecision => "planning-decision authority",
            Self::PlanRejection => "plan-rejection authority",
            Self::PlanningCancellation => "planning cancellation",
            Self::LayerProvision => "layer provisioning",
            Self::LayerAdmission => "layer-admission authority",
            Self::LayerProvisionAuthority => "layer-provision authority",
            Self::LayerStart => "layer start",
            Self::LayerRunStartAuthority => "layer-run-start authority",
            Self::LayerTerminal => "layer terminal wait",
            Self::LayerTerminalAuthority => "layer-terminal authority",
            Self::LayerCancellation => "layer cancellation",
            Self::LayerInterruption => "layer interruption",
            Self::LayerResultAuthority => "layer-result authority",
            Self::LayerCleanup => "layer cleanup",
            Self::LayerDisposition => "layer disposition",
            Self::FinishAuthority => "finish authority",
            Self::RunCancellation => "adaptive run cancellation",
            Self::DeadlineObservation => "deadline observation",
        })
    }
}

/// One absolute adaptive wall-clock budget plus a small, bounded window in
/// which the shell can drain physical custody before asking MobMachine to
/// terminalize the run as `DeadlineExceeded`.
#[derive(Debug, Clone)]
pub struct AdaptiveOperationDeadline {
    deadline_ms: u64,
    execution_deadline: tokio_time::Instant,
    terminalization_deadline: tokio_time::Instant,
}

impl AdaptiveOperationDeadline {
    pub(crate) fn from_policy(policy: &AdaptivePolicy, started_at_ms: u64) -> Self {
        let wall_clock = Duration::from_millis(policy.limits.max_wall_clock_ms);
        let execution_deadline = tokio_time::Instant::now() + wall_clock;
        Self {
            deadline_ms: started_at_ms.saturating_add(policy.limits.max_wall_clock_ms),
            execution_deadline,
            terminalization_deadline: execution_deadline + ADAPTIVE_TERMINALIZATION_GRACE,
        }
    }

    pub fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }

    pub fn observed_expired_at_ms(&self) -> u64 {
        self.deadline_ms.saturating_add(1)
    }

    pub fn execution_remaining(&self) -> Duration {
        self.execution_deadline
            .saturating_duration_since(tokio_time::Instant::now())
    }

    pub(crate) fn execution_subphase_remaining(&self) -> Duration {
        self.execution_remaining()
            .saturating_sub(ADAPTIVE_CUSTODY_HANDOFF_GRACE)
    }

    pub fn terminalization_remaining(&self) -> Duration {
        self.terminalization_deadline
            .saturating_duration_since(tokio_time::Instant::now())
    }

    pub(crate) fn terminalization_subphase_remaining(&self) -> Duration {
        self.terminalization_remaining()
            .saturating_sub(ADAPTIVE_CUSTODY_HANDOFF_GRACE)
    }

    /// Physical cleanup must leave time for the authoritative disposition and
    /// deadline-observation inputs to commit.
    pub fn cleanup_remaining(&self) -> Duration {
        self.terminalization_remaining()
            .saturating_sub(ADAPTIVE_AUTHORITY_GRACE)
    }

    pub fn execution_expired(&self) -> bool {
        self.execution_remaining().is_zero()
    }

    pub fn terminalization_expired(&self) -> bool {
        self.terminalization_remaining().is_zero()
    }

    pub fn execution_error(
        &self,
        stage: AdaptiveRuntimeStage,
        diagnostics: impl Into<String>,
    ) -> AdaptiveError {
        AdaptiveError::DeadlineExceeded {
            stage,
            deadline_ms: self.deadline_ms,
            observed_at_ms: self.observed_expired_at_ms(),
            diagnostics: diagnostics.into(),
        }
    }

    pub(crate) async fn run_execution<T, F>(
        &self,
        stage: AdaptiveRuntimeStage,
        diagnostics: impl FnOnce() -> String,
        future: F,
    ) -> Result<T, AdaptiveError>
    where
        F: Future<Output = Result<T, AdaptiveError>>,
    {
        if self.execution_expired() {
            return Err(self.execution_error(stage, diagnostics()));
        }
        match tokio_time::timeout_at(self.execution_deadline, future).await {
            Ok(result) => result,
            Err(_) => Err(self.execution_error(stage, diagnostics())),
        }
    }

    /// Bound a nested runtime operation slightly inside the absolute
    /// execution deadline so it can return exact custody diagnostics to the
    /// outer owner before that owner performs terminal cancellation.
    pub(crate) async fn run_execution_subphase<T, F>(
        &self,
        stage: AdaptiveRuntimeStage,
        diagnostics: impl FnOnce() -> String,
        future: F,
    ) -> Result<T, AdaptiveError>
    where
        F: Future<Output = Result<T, AdaptiveError>>,
    {
        let remaining = self.execution_subphase_remaining();
        if remaining.is_zero() {
            return Err(self.execution_error(stage, diagnostics()));
        }
        match tokio_time::timeout(remaining, future).await {
            Ok(result) => result,
            Err(_) => Err(self.execution_error(stage, diagnostics())),
        }
    }

    async fn run_terminalization<T, F>(
        &self,
        stage: AdaptiveRuntimeStage,
        future: F,
    ) -> Result<T, AdaptiveError>
    where
        F: Future<Output = Result<T, AdaptiveError>>,
    {
        if self.terminalization_expired() {
            return Err(AdaptiveError::TerminalizationDeadlineExceeded {
                stage,
                deadline_ms: self.deadline_ms,
            });
        }
        tokio_time::timeout_at(self.terminalization_deadline, future)
            .await
            .map_err(|_| AdaptiveError::TerminalizationDeadlineExceeded {
                stage,
                deadline_ms: self.deadline_ms,
            })?
    }
}

fn push_bounded_retry_error(errors: &mut Vec<AdaptiveError>, error: AdaptiveError) {
    if errors.len() == ADAPTIVE_RETRY_DIAGNOSTIC_LIMIT {
        errors.remove(0);
    }
    errors.push(error);
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdaptiveRunId(String);

impl AdaptiveRunId {
    pub fn new(value: impl Into<String>) -> Result<Self, AdaptiveError> {
        let value = value.into();
        validate_identifier("adaptive_run_id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayerId(String);

impl LayerId {
    pub fn new(value: impl Into<String>) -> Result<Self, AdaptiveError> {
        let value = value.into();
        validate_identifier("layer_id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaName(String);

impl SchemaName {
    pub fn new(value: impl Into<String>) -> Result<Self, AdaptiveError> {
        let value = value.into();
        validate_identifier("schema_name", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum LayerDecision {
    RunLayer {
        reason: String,
        plan: LayerPlan,
    },
    Finish {
        reason: String,
        result: FinishResult,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishResult {
    pub result: serde_json::Value,
}

/// The canonical JSON schema for [`LayerDecision`].
///
/// Single source of truth for the `adaptive/layer-decision.schema.json`
/// artifact bundled in adaptive mobpacks: the pack builder emits this schema
/// when packing an adaptive pack, and pack validation requires the bundled
/// bytes to match it (structural JSON equality, i.e. byte equality after
/// canonical serialization), so a stale or hand-rolled schema fails closed.
#[cfg(feature = "schema")]
pub fn layer_decision_schema() -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(schemars::schema_for!(LayerDecision))
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerPlan {
    pub id: LayerId,
    pub objective: String,
    pub shape: LayerShape,
    #[serde(default)]
    pub spawn: Vec<LayerSpawnSpec>,
    #[serde(default)]
    pub spawn_groups: Vec<LayerSpawnGroup>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(with = "BTreeMap<String, LayerProfile>"))]
    pub profiles: BTreeMap<ProfileName, LayerProfile>,
    pub collector: CollectorContract,
    #[serde(default)]
    pub activation_params: BTreeMap<String, AdaptiveValue>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayerShape {
    FanOutCollect {
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        worker_role: ProfileName,
        collection: LayerCollection,
    },
    Solo,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerCollection {
    All,
    Any,
    Quorum { n: u8 },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerSpawnSpec {
    pub identity: String,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub profile: ProfileName,
    pub initial_message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_limits: Option<meerkat_core::BudgetLimits>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerSpawnGroup {
    pub prefix: String,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub profile: ProfileName,
    pub items_ref: String,
    pub key_path: String,
    pub initial_message_template: String,
    pub max_items: usize,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectorContract {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub profile: ProfileName,
    pub output_schema: SchemaRef,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LayerProfile {
    Template {
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        template: ProfileName,
    },
    Inline {
        // `Profile` derives `JsonSchema` itself (schema feature), so inline
        // profiles are validated structurally instead of as an opaque
        // `serde_json::Value` the bundled schema would accept as `true`.
        inline: Box<Profile>,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SchemaRef {
    Inline { inline: serde_json::Value },
    Registry { registry: SchemaName },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AdaptiveValue {
    Ref { r#ref: String },
    Literal(serde_json::Value),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptivePolicy {
    pub limits: AdaptiveLimitRecord,
    #[serde(default)]
    pub allowed_model_classes: BTreeSet<String>,
    #[serde(default)]
    pub allowed_tool_classes: BTreeSet<String>,
    #[serde(default)]
    pub allowed_skill_classes: BTreeSet<String>,
    #[serde(default)]
    pub allowed_auth_bindings: BTreeSet<String>,
    #[serde(default)]
    pub allow_inline_profiles: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveLimitRecord {
    pub max_depth: u64,
    pub max_total_decisions: u64,
    pub max_repair_attempts: u64,
    pub max_layer_failures: u64,
    pub max_attempts_per_layer: u64,
    pub max_members_per_layer: u64,
    pub max_total_spawned_members: u64,
    pub max_active_members: u64,
    pub max_retained_layer_mobs: u64,
    pub max_wall_clock_ms: u64,
    pub max_aggregate_tokens: u64,
    pub max_aggregate_tool_calls: u64,
}

impl AdaptivePolicy {
    pub fn compose(pack: &Self, host: &Self) -> Result<Self, AdaptiveError> {
        pack.limits.validate_complete("pack")?;
        host.limits.validate_complete("host")?;
        Ok(Self {
            limits: pack.limits.compose(&host.limits),
            allowed_model_classes: intersect(
                &pack.allowed_model_classes,
                &host.allowed_model_classes,
            ),
            allowed_tool_classes: intersect(&pack.allowed_tool_classes, &host.allowed_tool_classes),
            allowed_skill_classes: intersect(
                &pack.allowed_skill_classes,
                &host.allowed_skill_classes,
            ),
            allowed_auth_bindings: intersect(
                &pack.allowed_auth_bindings,
                &host.allowed_auth_bindings,
            ),
            allow_inline_profiles: pack.allow_inline_profiles && host.allow_inline_profiles,
        })
    }
}

impl AdaptiveLimitRecord {
    pub fn validate_complete(&self, owner: &str) -> Result<(), AdaptiveError> {
        for (field, value) in [
            ("max_depth", self.max_depth),
            ("max_total_decisions", self.max_total_decisions),
            ("max_repair_attempts", self.max_repair_attempts),
            ("max_layer_failures", self.max_layer_failures),
            ("max_attempts_per_layer", self.max_attempts_per_layer),
            ("max_members_per_layer", self.max_members_per_layer),
            ("max_total_spawned_members", self.max_total_spawned_members),
            ("max_active_members", self.max_active_members),
            ("max_retained_layer_mobs", self.max_retained_layer_mobs),
            ("max_wall_clock_ms", self.max_wall_clock_ms),
            ("max_aggregate_tokens", self.max_aggregate_tokens),
            ("max_aggregate_tool_calls", self.max_aggregate_tool_calls),
        ] {
            if value == 0 {
                return Err(AdaptiveError::IncompletePolicy {
                    owner: owner.to_string(),
                    field,
                });
            }
        }
        Ok(())
    }

    fn compose(&self, host: &Self) -> Self {
        Self {
            max_depth: self.max_depth.min(host.max_depth),
            max_total_decisions: self.max_total_decisions.min(host.max_total_decisions),
            max_repair_attempts: self.max_repair_attempts.min(host.max_repair_attempts),
            max_layer_failures: self.max_layer_failures.min(host.max_layer_failures),
            max_attempts_per_layer: self.max_attempts_per_layer.min(host.max_attempts_per_layer),
            max_members_per_layer: self.max_members_per_layer.min(host.max_members_per_layer),
            max_total_spawned_members: self
                .max_total_spawned_members
                .min(host.max_total_spawned_members),
            max_active_members: self.max_active_members.min(host.max_active_members),
            max_retained_layer_mobs: self
                .max_retained_layer_mobs
                .min(host.max_retained_layer_mobs),
            max_wall_clock_ms: self.max_wall_clock_ms.min(host.max_wall_clock_ms),
            max_aggregate_tokens: self.max_aggregate_tokens.min(host.max_aggregate_tokens),
            max_aggregate_tool_calls: self
                .max_aggregate_tool_calls
                .min(host.max_aggregate_tool_calls),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdaptiveRef {
    Args(Vec<String>),
    PreviousLayerResult(Vec<String>),
    PreviousLayerPlan(Vec<String>),
    PriorLayer {
        layer_id: LayerId,
        body: PriorLayerBody,
        path: Vec<String>,
    },
    Limits(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorLayerBody {
    Plan,
    Result,
}

impl AdaptiveRef {
    pub fn parse(raw: &str) -> Result<Self, AdaptiveError> {
        let segments = parse_path(raw)?;
        match segments.as_slice() {
            ["args", rest @ ..] => Ok(Self::Args(rest.iter().map(ToString::to_string).collect())),
            ["previous_layer", "result", rest @ ..] => Ok(Self::PreviousLayerResult(
                rest.iter().map(ToString::to_string).collect(),
            )),
            ["previous_layer", "plan", rest @ ..] => Ok(Self::PreviousLayerPlan(
                rest.iter().map(ToString::to_string).collect(),
            )),
            ["limits", field] => Ok(Self::Limits((*field).to_string())),
            ["prior_layer", layer, "result", rest @ ..] => Ok(Self::PriorLayer {
                layer_id: LayerId::new(*layer)?,
                body: PriorLayerBody::Result,
                path: rest.iter().map(ToString::to_string).collect(),
            }),
            ["prior_layer", layer, "plan", rest @ ..] => Ok(Self::PriorLayer {
                layer_id: LayerId::new(*layer)?,
                body: PriorLayerBody::Plan,
                path: rest.iter().map(ToString::to_string).collect(),
            }),
            _ => Err(AdaptiveError::InvalidRef(raw.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BodyDigest(String);

impl BodyDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Default)]
pub struct InMemoryBodyStore {
    bodies: BTreeMap<BodyDigest, Vec<u8>>,
}

impl InMemoryBodyStore {
    pub fn put_json(&mut self, value: &serde_json::Value) -> Result<BodyDigest, AdaptiveError> {
        let bytes = serde_json::to_vec(value)?;
        let digest = digest_bytes(&bytes);
        self.bodies.insert(digest.clone(), bytes);
        Ok(digest)
    }

    pub fn get_json(&self, digest: &BodyDigest) -> Result<serde_json::Value, AdaptiveError> {
        let bytes = self
            .bodies
            .get(digest)
            .ok_or_else(|| AdaptiveError::BodyMissing(digest.clone()))?;
        let actual = digest_bytes(bytes);
        if &actual != digest {
            return Err(AdaptiveError::BodyDigestMismatch {
                expected: digest.clone(),
                actual,
            });
        }
        Ok(serde_json::from_slice(bytes)?)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchemaRegistry {
    schemas: BTreeMap<SchemaName, MeerkatSchema>,
}

impl SchemaRegistry {
    pub fn insert(
        &mut self,
        name: SchemaName,
        schema: serde_json::Value,
    ) -> Result<(), AdaptiveError> {
        self.schemas.insert(name, MeerkatSchema::new(schema)?);
        Ok(())
    }

    pub fn resolve(&self, reference: &SchemaRef) -> Result<MeerkatSchema, AdaptiveError> {
        match reference {
            SchemaRef::Inline { inline } => Ok(MeerkatSchema::new(inline.clone())?),
            SchemaRef::Registry { registry } => self
                .schemas
                .get(registry)
                .cloned()
                .ok_or_else(|| AdaptiveError::MissingSchema(registry.as_str().to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompileContext {
    pub adaptive_run_id: AdaptiveRunId,
    pub attempt: u64,
    pub schema_registry: SchemaRegistry,
    pub profile_templates: BTreeMap<ProfileName, Profile>,
    pub previous_layer_result: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct CompiledLayer {
    pub child_mob_id: MobId,
    pub definition: MobDefinition,
    pub spawn_specs: Vec<SpawnMemberSpec>,
    pub activation_params: BTreeMap<String, serde_json::Value>,
    pub plan_digest: BodyDigest,
    pub policy_evidence: LayerPolicyEvidence,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerPolicyEvidence {
    pub used_model_classes: BTreeSet<String>,
    pub used_tool_classes: BTreeSet<String>,
    pub used_skill_identities: BTreeSet<String>,
    pub used_auth_binding_refs: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AdaptiveToolIdentity {
    Class(String),
    McpServer(String),
    RustBundle(String),
}

impl AdaptiveToolIdentity {
    fn parse(raw: &str) -> Result<Self, AdaptiveError> {
        if let Some(name) = raw.strip_prefix("mcp:") {
            validate_adaptive_identity_component("adaptive tool mcp server", name)?;
            return Ok(Self::McpServer(name.to_string()));
        }
        if let Some(name) = raw.strip_prefix("rust_bundle:") {
            validate_adaptive_identity_component("adaptive tool rust bundle", name)?;
            return Ok(Self::RustBundle(name.to_string()));
        }
        validate_adaptive_identity_component("adaptive tool class", raw)?;
        Ok(Self::Class(raw.to_string()))
    }

    fn mcp_server(name: &str) -> Result<Self, AdaptiveError> {
        validate_adaptive_identity_component("adaptive tool mcp server", name)?;
        Ok(Self::McpServer(name.to_string()))
    }

    fn rust_bundle(name: &str) -> Result<Self, AdaptiveError> {
        validate_adaptive_identity_component("adaptive tool rust bundle", name)?;
        Ok(Self::RustBundle(name.to_string()))
    }

    fn into_canonical(self) -> String {
        match self {
            Self::Class(name) => name,
            Self::McpServer(name) => format!("mcp:{name}"),
            Self::RustBundle(name) => format!("rust_bundle:{name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdaptiveSkillIdentity(String);

impl AdaptiveSkillIdentity {
    fn parse(raw: &str) -> Result<Self, AdaptiveError> {
        validate_adaptive_identity_component("adaptive skill identity", raw)?;
        if raw.starts_with("mcp:") || raw.starts_with("rust_bundle:") {
            return Err(AdaptiveError::InvalidAdaptiveIdentity {
                field: "adaptive skill identity",
                value: raw.to_string(),
                reason: "reserved tool-identity prefix".to_string(),
            });
        }
        Ok(Self(raw.to_string()))
    }

    fn into_canonical(self) -> String {
        self.0
    }
}

#[derive(Clone)]
pub struct AdaptiveDriver {
    control_mob: MobHandle,
}

impl AdaptiveDriver {
    pub fn new(control_mob: MobHandle) -> Self {
        Self { control_mob }
    }

    pub fn control_mob(&self) -> &MobHandle {
        &self.control_mob
    }

    pub fn initialize_run(
        &self,
        adaptive_run_id: &AdaptiveRunId,
        policy: &AdaptivePolicy,
        started_at_ms: u64,
    ) -> AdaptiveRunInitialization<crate::AdaptiveDriverCapability> {
        let limits = match adaptive_run_limits_from_policy(policy, started_at_ms) {
            Ok(limits) => limits,
            Err(error) => return AdaptiveRunInitialization::failed(error),
        };
        // Durable machine limits retain the caller's wall-clock timestamp,
        // while process-local cancellation custody uses a monotonic deadline.
        // Mixing those clock domains makes synthetic/skewed timestamps appear
        // expired and can strand an already-accepted machine run.
        let cancellation_execution_deadline =
            tokio_time::Instant::now() + Duration::from_millis(policy.limits.max_wall_clock_ms);
        let initialization_mob = self.control_mob.clone();
        let cancellation_driver = self.clone();
        let adaptive_run_id = adaptive_run_id.as_str().to_string();
        AdaptiveRunInitialization::spawn_owned(
            async move {
                Ok(initialization_mob
                    .initialize_adaptive_run(crate::InitializeAdaptiveRunRequest {
                        adaptive_run_id,
                        limits,
                    })
                    .await?)
            },
            move |capability| {
                cancellation_driver
                    .cancellation_safe_run(capability.clone(), cancellation_execution_deadline)
            },
        )
    }

    pub async fn record_planning_decision(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        decision: &LayerDecision,
    ) -> Result<(), AdaptiveError> {
        let kind = match decision {
            LayerDecision::RunLayer { .. } => crate::AdaptivePlanningDecisionKind::RunLayer,
            LayerDecision::Finish { .. } => crate::AdaptivePlanningDecisionKind::Finish,
        };
        Ok(self
            .control_mob
            .record_adaptive_planning_decision(capability, kind)
            .await?)
    }

    pub async fn record_plan_rejected(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_plan_rejected(capability, layer_id.as_str())
            .await?)
    }

    pub async fn resolve_layer_admission(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
        compiled: &CompiledLayer,
        observed_at_ms: u64,
    ) -> Result<crate::AdaptiveLayerAdmission, AdaptiveError> {
        Ok(self
            .control_mob
            .resolve_adaptive_layer_admission(
                capability,
                crate::AdaptiveLayerAdmissionRequest {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                    plan_digest: compiled.plan_digest.as_str().to_string(),
                    child_mob_id: compiled.child_mob_id.to_string(),
                    member_count: compiled.spawn_specs.len() as u64,
                    token_reservation: 0,
                    tool_call_reservation: 0,
                    used_model_classes: compiled.policy_evidence.used_model_classes.clone(),
                    used_tool_classes: compiled.policy_evidence.used_tool_classes.clone(),
                    used_skill_identities: compiled.policy_evidence.used_skill_identities.clone(),
                    used_auth_binding_refs: compiled.policy_evidence.used_auth_binding_refs.clone(),
                    observed_at_ms,
                },
            )
            .await?)
    }

    pub async fn record_layer_provisioned(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_provisioned(
                capability,
                crate::AdaptiveLayerAttempt {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                },
            )
            .await?)
    }

    pub async fn record_layer_run_started(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
        child_run_id: crate::RunId,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_run_started(
                capability,
                crate::AdaptiveLayerRunStart {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                    child_run_id,
                },
            )
            .await?)
    }

    pub async fn ingest_layer_terminal(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
        child_run: &MobRun,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .ingest_adaptive_layer_terminal(
                capability,
                crate::AdaptiveLayerAttempt {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                },
                child_run,
            )
            .await?)
    }

    pub async fn record_layer_result_validated(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
        result_digest: &BodyDigest,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_result_validated(
                capability,
                crate::AdaptiveLayerResultDigest {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                    result_digest: result_digest.as_str().to_string(),
                },
            )
            .await?)
    }

    pub async fn record_layer_setup_fault(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
        fault: crate::AdaptiveLayerSetupFault,
        spawned_members: u64,
        requested_members: u64,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_setup_fault(
                capability,
                crate::AdaptiveLayerSetupFaultObservation {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                    fault,
                    spawned_members,
                    requested_members,
                },
            )
            .await?)
    }

    pub async fn record_layer_result_invalid(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_result_invalid(
                capability,
                crate::AdaptiveLayerAttempt {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                },
            )
            .await?)
    }

    pub async fn record_layer_interrupted(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_interrupted(
                capability,
                crate::AdaptiveLayerAttempt {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                },
            )
            .await?)
    }

    pub async fn record_layer_mob_destroyed(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_mob_destroyed(
                capability,
                crate::AdaptiveLayerAttempt {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                },
            )
            .await?)
    }

    pub async fn record_layer_mob_retained(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        layer_id: &LayerId,
        attempt: u64,
        disposition: crate::AdaptiveLayerDisposition,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_layer_mob_retained(
                capability,
                crate::AdaptiveLayerRetention {
                    layer_id: layer_id.as_str().to_string(),
                    attempt,
                    disposition,
                },
            )
            .await?)
    }

    pub async fn record_cleanup_resolved(
        &self,
        capability: &crate::AdaptiveDriverCapability,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_cleanup_resolved(capability)
            .await?)
    }

    pub async fn record_body_evidence_missing(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        missing_digest: &BodyDigest,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_body_evidence_missing(capability, missing_digest.as_str())
            .await?)
    }

    pub async fn resolve_finish(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        final_result_digest: &BodyDigest,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .resolve_adaptive_finish(capability, final_result_digest.as_str())
            .await?)
    }

    pub async fn cancel(
        &self,
        capability: &crate::AdaptiveDriverCapability,
    ) -> Result<(), AdaptiveError> {
        Ok(self.control_mob.request_adaptive_cancel(capability).await?)
    }

    pub async fn observe_deadline(
        &self,
        capability: &crate::AdaptiveDriverCapability,
        observed_at_ms: u64,
    ) -> Result<(), AdaptiveError> {
        Ok(self
            .control_mob
            .record_adaptive_deadline_observed(capability, observed_at_ms)
            .await?)
    }

    pub async fn snapshot(
        &self,
        capability: &crate::AdaptiveDriverCapability,
    ) -> Result<crate::AdaptiveRunSnapshot, AdaptiveError> {
        Ok(self.control_mob.adaptive_run_snapshot(capability).await?)
    }
}

#[derive(Debug, Clone)]
pub struct AdaptiveRunRequest {
    pub adaptive_run_id: AdaptiveRunId,
    pub policy: AdaptivePolicy,
    pub compile_context: CompileContext,
    pub objective: String,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdaptiveRunOutcome {
    pub adaptive_run_id: AdaptiveRunId,
    pub final_result_digest: Option<BodyDigest>,
    pub final_result: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct PlanningTurnRequest {
    pub adaptive_run_id: AdaptiveRunId,
    pub planning_turn: u64,
    pub objective: String,
    pub previous_layer_result: Option<serde_json::Value>,
}

/// Synchronous ownership handoff for an initialized adaptive run.
///
/// Implementations must signal a cancellation executor that was started before
/// the next cancellable await. Dropping a run future must not create best-effort
/// async work after ownership has already disappeared.
pub trait AdaptiveRunCancellationOwner: Send + Sync {
    fn take_run_for_cancellation(&self);

    fn disarm_after_terminal(&self);
}

/// Cancellation-safe ownership of an initialized adaptive run.
///
/// Ordinary errors explicitly request cancellation through [`AdaptiveKernel`].
/// Dropping the surrounding future instead synchronously transfers the run to
/// its pre-existing cancellation owner.
pub struct AdaptiveRunLease {
    cancellation_owner: Option<Arc<dyn AdaptiveRunCancellationOwner>>,
}

impl AdaptiveRunLease {
    pub fn new(cancellation_owner: Arc<dyn AdaptiveRunCancellationOwner>) -> Self {
        Self {
            cancellation_owner: Some(cancellation_owner),
        }
    }

    pub fn disarm(mut self) {
        if let Some(owner) = self.cancellation_owner.take() {
            owner.disarm_after_terminal();
        }
    }
}

impl std::fmt::Debug for AdaptiveRunLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdaptiveRunLease")
            .field("cancellation_owner", &"armed")
            .finish()
    }
}

impl Drop for AdaptiveRunLease {
    fn drop(&mut self) {
        if let Some(owner) = self.cancellation_owner.take() {
            owner.take_run_for_cancellation();
        }
    }
}

/// In-flight, cancellation-safe initialization of one adaptive run.
///
/// The initialization owner is started synchronously by [`Self::spawn_owned`]
/// before the caller can reach its first cancellable await. That owner keeps
/// the real command/reply future alive if the caller disappears. A successful
/// machine reply is paired with an armed [`AdaptiveRunLease`] before it is
/// published back to the caller, so dropping either side transfers the run to
/// the cancellation executor instead of stranding an active machine run.
#[must_use = "adaptive run initialization must be resolved or dropped to transfer cancellation ownership"]
pub struct AdaptiveRunInitialization<C> {
    completion: tokio::sync::oneshot::Receiver<Result<(C, AdaptiveRunLease), AdaptiveError>>,
}

impl<C> AdaptiveRunInitialization<C> {
    /// Construct an already-complete initialization with explicit run custody.
    pub fn completed(capability: C, run_lease: AdaptiveRunLease) -> Self {
        Self::from_result(Ok((capability, run_lease)))
    }

    /// Construct a pre-machine initialization failure.
    pub fn failed(error: AdaptiveError) -> Self {
        Self::from_result(Err(error))
    }

    fn from_result(result: Result<(C, AdaptiveRunLease), AdaptiveError>) -> Self {
        let (completion_tx, completion) = tokio::sync::oneshot::channel();
        let _ = completion_tx.send(result);
        Self { completion }
    }

    /// Start an owner for an asynchronous machine initialization.
    ///
    /// The spawned task owns `initialize` independently of the caller future.
    /// It arms cancellation only after successful machine acceptance, avoiding
    /// a pre-initialization snapshot race, and publishes capability plus lease
    /// atomically. If the caller has already gone away, the failed send drops
    /// the lease and triggers cancellation.
    pub fn spawn_owned<F, A>(initialize: F, arm_cancellation: A) -> Self
    where
        C: Send + 'static,
        F: Future<Output = Result<C, AdaptiveError>> + Send + 'static,
        A: FnOnce(&C) -> AdaptiveRunLease + Send + 'static,
    {
        let (completion_tx, completion) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = initialize.await.map(|capability| {
                let run_lease = arm_cancellation(&capability);
                (capability, run_lease)
            });
            let _ = completion_tx.send(result);
        });
        Self { completion }
    }

    pub async fn resolve(self) -> Result<(C, AdaptiveRunLease), AdaptiveError> {
        self.completion.await.map_err(|_| {
            AdaptiveError::DriverRuntime(
                "adaptive run initialization owner exited before publishing machine acceptance"
                    .to_string(),
            )
        })?
    }
}

impl<C> std::fmt::Debug for AdaptiveRunInitialization<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdaptiveRunInitialization")
            .field("completion", &"owned")
            .finish()
    }
}

struct DriverRunCancellationOwner {
    guardian_trigger: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl AdaptiveRunCancellationOwner for DriverRunCancellationOwner {
    fn take_run_for_cancellation(&self) {
        self.guardian_trigger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn disarm_after_terminal(&self) {
        if let Some(trigger) = self
            .guardian_trigger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = trigger.send(());
        }
    }
}

async fn run_driver_run_cancellation_cleanup(
    driver: AdaptiveDriver,
    capability: crate::AdaptiveDriverCapability,
    deadline: tokio_time::Instant,
) {
    loop {
        if tokio_time::Instant::now() >= deadline {
            tracing::error!(
                adaptive_run_id = capability.adaptive_run_id(),
                "adaptive cancellation guardian exhausted its bounded terminalization window"
            );
            return;
        }
        if matches!(
            tokio_time::timeout_at(deadline, driver.cancel(&capability)).await,
            Ok(Ok(()))
        ) {
            return;
        }

        if let Ok(Ok(snapshot)) =
            tokio_time::timeout_at(deadline, driver.snapshot(&capability)).await
            && matches!(
                snapshot.phase,
                None | Some(
                    crate::AdaptiveRunPhaseView::Finished
                        | crate::AdaptiveRunPhaseView::Failed
                        | crate::AdaptiveRunPhaseView::Canceled
                )
            )
        {
            return;
        }

        tokio_time::sleep(
            Duration::from_millis(25)
                .min(deadline.saturating_duration_since(tokio_time::Instant::now())),
        )
        .await;
    }
}

impl AdaptiveDriver {
    fn cancellation_safe_run(
        &self,
        capability: crate::AdaptiveDriverCapability,
        execution_deadline: tokio_time::Instant,
    ) -> AdaptiveRunLease {
        // Initialization is independently owned and may accept after the
        // original execution window. Once acceptance exists, its cancellation
        // obligation still owns one full terminalization window; otherwise a
        // failed capability publication could create an ownerless active run.
        let cancellation_deadline =
            execution_deadline.max(tokio_time::Instant::now()) + ADAPTIVE_TERMINALIZATION_GRACE;
        let driver = self.clone();
        let worker_driver = driver.clone();
        let worker_capability = capability.clone();
        let (guardian_trigger, cancellation) = tokio::sync::oneshot::channel();
        let guardian = tokio::spawn(async move {
            if cancellation.await.is_ok() {
                return;
            }
            run_driver_run_cancellation_cleanup(
                worker_driver,
                worker_capability,
                cancellation_deadline,
            )
            .await;
        });

        // Retain an independent owner for the exact worker. A panic or external
        // abort cannot silently discard the typed cancellation obligation.
        tokio::spawn(async move {
            if guardian.await.is_err() {
                run_driver_run_cancellation_cleanup(driver, capability, cancellation_deadline)
                    .await;
            }
        });

        AdaptiveRunLease::new(Arc::new(DriverRunCancellationOwner {
            guardian_trigger: Mutex::new(Some(guardian_trigger)),
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveLayerCleanup {
    Destroyed,
    Retained(crate::AdaptiveLayerDisposition),
}

/// Synchronous ownership handoff used when an adaptive-loop future is dropped.
///
/// Implementations must transfer `layer` to a cleanup executor that already
/// exists. In particular, this hook must not merely start best-effort async work
/// from `Drop`: once it returns, some durable in-process owner must still hold
/// the physical layer until teardown completes.
pub trait AdaptiveLayerCancellationOwner<L>: Send + Sync {
    fn take_layer_for_cancellation(&self, layer: L);

    fn disarm_after_cleanup(&self);
}

/// Exclusive, cancellation-safe ownership of an acquired adaptive layer.
///
/// Normal completion disarms the lease only after explicit cleanup succeeds.
/// Dropping the surrounding `run_adaptive_loop` future instead transfers the
/// resource synchronously to its pre-existing cancellation owner.
pub struct AdaptiveLayerLease<L> {
    layer: Option<L>,
    cancellation_owner: Arc<dyn AdaptiveLayerCancellationOwner<L>>,
}

impl<L> AdaptiveLayerLease<L> {
    pub fn new(layer: L, cancellation_owner: Arc<dyn AdaptiveLayerCancellationOwner<L>>) -> Self {
        Self {
            layer: Some(layer),
            cancellation_owner,
        }
    }

    pub fn layer(&self) -> &L {
        self.layer
            .as_ref()
            .expect("adaptive layer lease must stay armed until cleanup succeeds")
    }

    pub fn disarm(mut self) -> L {
        let layer = self
            .layer
            .take()
            .expect("adaptive layer lease must stay armed until cleanup succeeds");
        self.cancellation_owner.disarm_after_cleanup();
        layer
    }
}

impl<L: std::fmt::Debug> std::fmt::Debug for AdaptiveLayerLease<L> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdaptiveLayerLease")
            .field("layer", &self.layer)
            .field("cancellation_owner", &"armed")
            .finish()
    }
}

impl<L> Drop for AdaptiveLayerLease<L> {
    fn drop(&mut self) {
        if let Some(layer) = self.layer.take() {
            self.cancellation_owner.take_layer_for_cancellation(layer);
        }
    }
}

/// Typed result of provisioning a physical adaptive layer.
///
/// A failed provision may still own an acquired layer (for example after one
/// member in a batch fails). Carrying its lease in the failure variant keeps
/// teardown ownership explicit across ordinary errors and future cancellation.
#[derive(Debug)]
pub enum AdaptiveLayerProvision<L> {
    Ready(AdaptiveLayerLease<L>),
    Failed {
        layer: Option<AdaptiveLayerLease<L>>,
        fault: crate::AdaptiveLayerSetupFault,
        spawned_members: u64,
        requested_members: u64,
        error: AdaptiveError,
    },
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AdaptiveKernel {
    type Capability: Send + Sync;

    fn initialize_run(
        &self,
        adaptive_run_id: &AdaptiveRunId,
        policy: &AdaptivePolicy,
        started_at_ms: u64,
    ) -> AdaptiveRunInitialization<Self::Capability>;

    async fn cancel_run(&self, capability: &Self::Capability) -> Result<(), AdaptiveError>;

    async fn observe_deadline(
        &self,
        capability: &Self::Capability,
        observed_at_ms: u64,
    ) -> Result<(), AdaptiveError>;

    async fn record_planning_decision(
        &self,
        capability: &Self::Capability,
        decision: &LayerDecision,
    ) -> Result<(), AdaptiveError>;

    async fn record_plan_rejected(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
    ) -> Result<(), AdaptiveError>;

    async fn resolve_layer_admission(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        compiled: &CompiledLayer,
        observed_at_ms: u64,
    ) -> Result<crate::AdaptiveLayerAdmission, AdaptiveError>;

    async fn record_layer_provisioned(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError>;

    async fn record_layer_run_started(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        child_run_id: RunId,
    ) -> Result<(), AdaptiveError>;

    async fn ingest_layer_terminal(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        child_run: &MobRun,
    ) -> Result<(), AdaptiveError>;

    async fn record_layer_result_validated(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        result_digest: &BodyDigest,
    ) -> Result<(), AdaptiveError>;

    async fn record_layer_result_invalid(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError>;

    async fn record_layer_interrupted(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError>;

    async fn record_layer_setup_fault(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        fault: crate::AdaptiveLayerSetupFault,
        spawned_members: u64,
        requested_members: u64,
    ) -> Result<(), AdaptiveError>;

    async fn record_layer_mob_destroyed(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError>;

    async fn record_layer_mob_retained(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        disposition: crate::AdaptiveLayerDisposition,
    ) -> Result<(), AdaptiveError>;

    async fn resolve_finish(
        &self,
        capability: &Self::Capability,
        final_result_digest: &BodyDigest,
    ) -> Result<(), AdaptiveError>;

    async fn cancel(&self, capability: &Self::Capability) -> Result<(), AdaptiveError>;

    async fn layer_exists(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
    ) -> Result<bool, AdaptiveError>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AdaptiveKernel for AdaptiveDriver {
    type Capability = crate::AdaptiveDriverCapability;

    fn initialize_run(
        &self,
        adaptive_run_id: &AdaptiveRunId,
        policy: &AdaptivePolicy,
        started_at_ms: u64,
    ) -> AdaptiveRunInitialization<Self::Capability> {
        AdaptiveDriver::initialize_run(self, adaptive_run_id, policy, started_at_ms)
    }

    async fn cancel_run(&self, capability: &Self::Capability) -> Result<(), AdaptiveError> {
        AdaptiveDriver::cancel(self, capability).await
    }

    async fn observe_deadline(
        &self,
        capability: &Self::Capability,
        observed_at_ms: u64,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::observe_deadline(self, capability, observed_at_ms).await
    }

    async fn record_planning_decision(
        &self,
        capability: &Self::Capability,
        decision: &LayerDecision,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_planning_decision(self, capability, decision).await
    }

    async fn record_plan_rejected(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_plan_rejected(self, capability, layer_id).await
    }

    async fn resolve_layer_admission(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        compiled: &CompiledLayer,
        observed_at_ms: u64,
    ) -> Result<crate::AdaptiveLayerAdmission, AdaptiveError> {
        AdaptiveDriver::resolve_layer_admission(
            self,
            capability,
            layer_id,
            attempt,
            compiled,
            observed_at_ms,
        )
        .await
    }

    async fn record_layer_provisioned(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_provisioned(self, capability, layer_id, attempt).await
    }

    async fn record_layer_run_started(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        child_run_id: RunId,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_run_started(self, capability, layer_id, attempt, child_run_id)
            .await
    }

    async fn ingest_layer_terminal(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        child_run: &MobRun,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::ingest_layer_terminal(self, capability, layer_id, attempt, child_run).await
    }

    async fn record_layer_result_validated(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        result_digest: &BodyDigest,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_result_validated(
            self,
            capability,
            layer_id,
            attempt,
            result_digest,
        )
        .await
    }

    async fn record_layer_result_invalid(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_result_invalid(self, capability, layer_id, attempt).await
    }

    async fn record_layer_interrupted(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_interrupted(self, capability, layer_id, attempt).await
    }

    async fn record_layer_setup_fault(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        fault: crate::AdaptiveLayerSetupFault,
        spawned_members: u64,
        requested_members: u64,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_setup_fault(
            self,
            capability,
            layer_id,
            attempt,
            fault,
            spawned_members,
            requested_members,
        )
        .await
    }

    async fn record_layer_mob_destroyed(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_mob_destroyed(self, capability, layer_id, attempt).await
    }

    async fn record_layer_mob_retained(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        disposition: crate::AdaptiveLayerDisposition,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::record_layer_mob_retained(self, capability, layer_id, attempt, disposition)
            .await
    }

    async fn resolve_finish(
        &self,
        capability: &Self::Capability,
        final_result_digest: &BodyDigest,
    ) -> Result<(), AdaptiveError> {
        AdaptiveDriver::resolve_finish(self, capability, final_result_digest).await
    }

    async fn cancel(&self, capability: &Self::Capability) -> Result<(), AdaptiveError> {
        AdaptiveDriver::cancel(self, capability).await
    }

    async fn layer_exists(
        &self,
        capability: &Self::Capability,
        layer_id: &LayerId,
    ) -> Result<bool, AdaptiveError> {
        Ok(AdaptiveDriver::snapshot(self, capability)
            .await?
            .layers
            .contains_key(layer_id.as_str()))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AdaptiveDriverRuntime {
    type Capability: Send + Sync;
    type Layer: Send + Sync;

    fn now_ms(&mut self) -> u64;

    async fn run_planning_turn(
        &mut self,
        request: PlanningTurnRequest,
        deadline: &AdaptiveOperationDeadline,
    ) -> Result<LayerDecision, AdaptiveError>;

    /// Cancel the exact planning flow whose custody was acquired by the most
    /// recent `run_planning_turn`.
    ///
    /// This is a separate required operation because the outer absolute
    /// deadline may drop `run_planning_turn` at any await. The runtime must
    /// retain the stable flow identity until either terminality or this
    /// cancellation is acknowledged.
    async fn cancel_planning_turn(
        &mut self,
        deadline: &AdaptiveOperationDeadline,
    ) -> Result<(), AdaptiveError>;

    async fn provision_layer(
        &mut self,
        capability: &Self::Capability,
        layer_id: &LayerId,
        attempt: u64,
        compiled: &CompiledLayer,
        deadline: &AdaptiveOperationDeadline,
    ) -> AdaptiveLayerProvision<Self::Layer>;

    async fn start_layer_flow(
        &mut self,
        layer: &Self::Layer,
        activation_params: BTreeMap<String, serde_json::Value>,
        deadline: &AdaptiveOperationDeadline,
    ) -> Result<RunId, AdaptiveError>;

    async fn await_layer_terminal(
        &mut self,
        layer: &Self::Layer,
        run_id: RunId,
        deadline: &AdaptiveOperationDeadline,
    ) -> Result<MobRun, AdaptiveError>;

    /// Request cancellation of the concrete child flow before physical layer
    /// teardown. The runtime owns this because flow handles are store/runtime
    /// facts, not adaptive machine state.
    async fn cancel_layer_flow(
        &mut self,
        layer: &Self::Layer,
        run_id: RunId,
        deadline: &AdaptiveOperationDeadline,
    ) -> Result<(), AdaptiveError>;

    /// Resolve physical ownership of a child layer.
    ///
    /// The borrow preserves caller custody across every `Err`. Returning
    /// `Destroyed` requires proof of completed destruction. Returning
    /// `Retained` requires the runtime to have installed an independent,
    /// durable owner before this method returns; it is not a fallback for an
    /// uncertain destroy attempt.
    async fn cleanup_layer(
        &mut self,
        layer: &AdaptiveLayerLease<Self::Layer>,
        layer_id: &LayerId,
        attempt: u64,
        deadline: &AdaptiveOperationDeadline,
    ) -> Result<AdaptiveLayerCleanup, AdaptiveError>;
}

fn attach_layer_finalization_error(
    primary: AdaptiveError,
    finalization: Result<(), AdaptiveError>,
) -> AdaptiveError {
    match finalization {
        Ok(()) => primary,
        Err(finalization_error) => AdaptiveError::OperationFailedWithCleanup {
            primary: Box::new(primary),
            cleanup: format!("adaptive layer finalization: {finalization_error}"),
        },
    }
}

fn deadline_observation_ms(error: &AdaptiveError) -> Option<u64> {
    match error {
        AdaptiveError::DeadlineExceeded { observed_at_ms, .. } => Some(*observed_at_ms),
        AdaptiveError::OperationFailedWithCleanup { primary, .. } => {
            deadline_observation_ms(primary)
        }
        _ => None,
    }
}

fn operation_error_with_followup_failures(
    primary: AdaptiveError,
    failures: Vec<(&'static str, AdaptiveError)>,
) -> AdaptiveError {
    if failures.is_empty() {
        return primary;
    }
    let cleanup = failures
        .into_iter()
        .map(|(stage, error)| format!("{stage}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");
    AdaptiveError::OperationFailedWithCleanup {
        primary: Box::new(primary),
        cleanup,
    }
}

async fn retry_backoff_within_terminalization(
    deadline: &AdaptiveOperationDeadline,
    delay: Duration,
) -> bool {
    let remaining = deadline.terminalization_remaining();
    if remaining.is_zero() {
        return false;
    }
    tokio_time::sleep(delay.min(remaining)).await;
    !deadline.terminalization_expired()
}

async fn cancel_run_until_confirmed<K>(
    kernel: &K,
    capability: &K::Capability,
    deadline: &AdaptiveOperationDeadline,
) -> (bool, Vec<AdaptiveError>)
where
    K: AdaptiveKernel + Sync,
{
    let mut delay = ADAPTIVE_RETRY_INITIAL;
    let mut failures = Vec::new();
    loop {
        match deadline
            .run_terminalization(
                AdaptiveRuntimeStage::RunCancellation,
                kernel.cancel_run(capability),
            )
            .await
        {
            Ok(()) => return (true, failures),
            Err(error) => {
                push_bounded_retry_error(&mut failures, error);
                if !retry_backoff_within_terminalization(deadline, delay).await {
                    return (false, failures);
                }
                delay = delay.saturating_mul(2).min(ADAPTIVE_RETRY_MAX);
            }
        }
    }
}

async fn layer_exists_until_confirmed<K>(
    kernel: &K,
    capability: &K::Capability,
    layer_id: &LayerId,
    deadline: &AdaptiveOperationDeadline,
) -> (Option<bool>, Vec<AdaptiveError>)
where
    K: AdaptiveKernel + Sync,
{
    let mut delay = ADAPTIVE_RETRY_INITIAL;
    let mut failures = Vec::new();
    loop {
        match deadline
            .run_terminalization(
                AdaptiveRuntimeStage::LayerDisposition,
                kernel.layer_exists(capability, layer_id),
            )
            .await
        {
            Ok(exists) => return (Some(exists), failures),
            Err(error) => {
                push_bounded_retry_error(&mut failures, error);
                if !retry_backoff_within_terminalization(deadline, delay).await {
                    return (None, failures);
                }
                delay = delay.saturating_mul(2).min(ADAPTIVE_RETRY_MAX);
            }
        }
    }
}

async fn record_absent_layer_until_confirmed<K>(
    kernel: &K,
    capability: &K::Capability,
    layer_id: &LayerId,
    attempt: u64,
    deadline: &AdaptiveOperationDeadline,
) -> (bool, Vec<AdaptiveError>)
where
    K: AdaptiveKernel + Sync,
{
    let mut delay = ADAPTIVE_RETRY_INITIAL;
    let mut failures = Vec::new();
    loop {
        match deadline
            .run_terminalization(
                AdaptiveRuntimeStage::LayerDisposition,
                kernel.record_layer_mob_destroyed(capability, layer_id, attempt),
            )
            .await
        {
            Ok(()) => return (true, failures),
            Err(error) => {
                push_bounded_retry_error(&mut failures, error);
                if !retry_backoff_within_terminalization(deadline, delay).await {
                    return (false, failures);
                }
                delay = delay.saturating_mul(2).min(ADAPTIVE_RETRY_MAX);
            }
        }
    }
}

async fn finalize_absent_layer<K>(
    kernel: &K,
    capability: &K::Capability,
    layer_id: &LayerId,
    attempt: u64,
    terminalization: Result<(), AdaptiveError>,
    deadline: &AdaptiveOperationDeadline,
) -> Result<(), AdaptiveError>
where
    K: AdaptiveKernel + Sync,
{
    let original_terminalization_error = terminalization.err();
    let terminal_ack = if original_terminalization_error.is_some() {
        deadline
            .run_terminalization(
                AdaptiveRuntimeStage::LayerInterruption,
                kernel.record_layer_interrupted(capability, layer_id, attempt),
            )
            .await
    } else {
        Ok(())
    };
    if let Err(ack_error) = terminal_ack {
        return match original_terminalization_error {
            Some(original_error) => Err(AdaptiveError::DriverRuntime(format!(
                "adaptive layer terminalization failed: {original_error}; idempotent terminal acknowledgement also failed: {ack_error}"
            ))),
            None => Err(ack_error),
        };
    }

    // Mob creation failed before a handle existed, so physical absence is
    // proven. Feed that terminal cleanup observation through MobMachine to
    // release the admission reservation instead of fabricating local cleanup.
    let cleanup_observation = deadline
        .run_terminalization(
            AdaptiveRuntimeStage::LayerDisposition,
            kernel.record_layer_mob_destroyed(capability, layer_id, attempt),
        )
        .await;
    match (original_terminalization_error, cleanup_observation) {
        (None, result) => result,
        (Some(original_error), Ok(())) => Err(original_error),
        (Some(original_error), Err(observation_error)) => {
            Err(AdaptiveError::DriverRuntime(format!(
                "adaptive layer terminalization response failed: {original_error}; proven-absent cleanup observation also failed: {observation_error}"
            )))
        }
    }
}

struct AcquiredLayerFinalizationContext<'a, K, R>
where
    K: AdaptiveKernel + Sync,
    R: AdaptiveDriverRuntime<Capability = K::Capability> + Send,
{
    kernel: &'a K,
    capability: &'a K::Capability,
    runtime: &'a mut R,
    layer_id: &'a LayerId,
    attempt: u64,
    deadline: &'a AdaptiveOperationDeadline,
}

async fn finalize_acquired_layer<K, R>(
    context: AcquiredLayerFinalizationContext<'_, K, R>,
    layer: AdaptiveLayerLease<R::Layer>,
    terminalization: Result<(), AdaptiveError>,
) -> Result<(), AdaptiveError>
where
    K: AdaptiveKernel + Sync,
    R: AdaptiveDriverRuntime<Capability = K::Capability> + Send,
{
    let AcquiredLayerFinalizationContext {
        kernel,
        capability,
        runtime,
        layer_id,
        attempt,
        deadline,
    } = context;
    // A transition may commit and then lose its response. The idempotent
    // interruption input acknowledges either that already-terminal state or
    // terminalizes a still-live phase before cleanup observation.
    let original_terminalization_error = terminalization.err();
    let terminal_ack = if original_terminalization_error.is_some() {
        deadline
            .run_terminalization(
                AdaptiveRuntimeStage::LayerInterruption,
                kernel.record_layer_interrupted(capability, layer_id, attempt),
            )
            .await
    } else {
        Ok(())
    };

    // Physical teardown is unconditional once ownership of a layer has been
    // acquired, even when machine acknowledgement itself fails.
    let cleanup_remaining = deadline.cleanup_remaining();
    let cleanup = if cleanup_remaining.is_zero() {
        Ok(AdaptiveLayerCleanup::Retained(
            crate::AdaptiveLayerDisposition::Retained,
        ))
    } else {
        match tokio_time::timeout(
            cleanup_remaining,
            runtime.cleanup_layer(&layer, layer_id, attempt, deadline),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(AdaptiveLayerCleanup::Retained(
                crate::AdaptiveLayerDisposition::Retained,
            )),
        }
    };
    if let Err(ack_error) = terminal_ack {
        return match (original_terminalization_error, cleanup) {
            (Some(original_error), Ok(_)) => Err(AdaptiveError::DriverRuntime(format!(
                "adaptive layer terminalization failed: {original_error}; idempotent terminal acknowledgement also failed: {ack_error}"
            ))),
            (Some(original_error), Err(cleanup_error)) => {
                Err(AdaptiveError::DriverRuntime(format!(
                    "adaptive layer terminalization failed: {original_error}; idempotent terminal acknowledgement also failed: {ack_error}; physical cleanup also failed: {cleanup_error}"
                )))
            }
            (None, Ok(_)) => Err(ack_error),
            (None, Err(cleanup_error)) => Err(AdaptiveError::DriverRuntime(format!(
                "adaptive layer terminal acknowledgement failed: {ack_error}; physical cleanup also failed: {cleanup_error}"
            ))),
        };
    }

    let cleanup = cleanup?;
    let cleanup_observation = match cleanup {
        AdaptiveLayerCleanup::Destroyed => {
            deadline
                .run_terminalization(
                    AdaptiveRuntimeStage::LayerDisposition,
                    kernel.record_layer_mob_destroyed(capability, layer_id, attempt),
                )
                .await
        }
        AdaptiveLayerCleanup::Retained(disposition) => {
            deadline
                .run_terminalization(
                    AdaptiveRuntimeStage::LayerDisposition,
                    kernel.record_layer_mob_retained(capability, layer_id, attempt, disposition),
                )
                .await
        }
    };
    if cleanup_observation.is_ok() {
        match cleanup {
            AdaptiveLayerCleanup::Destroyed => {
                let _cleaned_layer = layer.disarm();
            }
            AdaptiveLayerCleanup::Retained(_) => {
                // Dropping the armed lease transfers the retained child to its
                // prestarted cleanup owner. The machine now records that
                // independent custody instead of pretending physical destroy.
                drop(layer);
            }
        }
    }
    if original_terminalization_error.is_none()
        && cleanup_observation.is_ok()
        && matches!(cleanup, AdaptiveLayerCleanup::Retained(_))
        && deadline.execution_expired()
    {
        return Err(deadline.execution_error(
            AdaptiveRuntimeStage::LayerCleanup,
            format!(
                "layer_id={}; attempt={attempt}; disposition=retained; custody=cleanup-owner",
                layer_id.as_str()
            ),
        ));
    }
    match (original_terminalization_error, cleanup_observation) {
        (None, result) => result,
        (Some(original_error), Ok(())) => Err(original_error),
        (Some(original_error), Err(observation_error)) => {
            Err(AdaptiveError::DriverRuntime(format!(
                "adaptive layer terminalization response failed: {original_error}; cleanup observation also failed: {observation_error}"
            )))
        }
    }
}

pub async fn run_adaptive_loop<K, R>(
    kernel: &K,
    runtime: &mut R,
    request: AdaptiveRunRequest,
) -> Result<AdaptiveRunOutcome, AdaptiveError>
where
    K: AdaptiveKernel + Sync,
    R: AdaptiveDriverRuntime<Capability = K::Capability> + Send,
{
    let deadline = AdaptiveOperationDeadline::from_policy(&request.policy, request.started_at_ms);
    // Initialization custody is established synchronously before this first
    // cancellable await. The owner keeps the queued machine command and its
    // reply alive, then publishes capability and armed lease as one value.
    let initialization = kernel.initialize_run(
        &request.adaptive_run_id,
        &request.policy,
        request.started_at_ms,
    );
    let (capability, run_lease) = deadline
        .run_execution(
            AdaptiveRuntimeStage::Initialization,
            || format!("adaptive_run_id={}", request.adaptive_run_id.as_str()),
            initialization.resolve(),
        )
        .await?;
    let mut cancel_confirmed = false;
    let outcome = async {
        let mut body_store = InMemoryBodyStore::default();
        let mut context = request.compile_context.clone();
        context.adaptive_run_id = request.adaptive_run_id.clone();
        let mut previous_layer_result = context.previous_layer_result.clone();
        let mut planning_turn = 1_u64;

        loop {
            let planning_request = PlanningTurnRequest {
                adaptive_run_id: request.adaptive_run_id.clone(),
                planning_turn,
                objective: request.objective.clone(),
                previous_layer_result: previous_layer_result.clone(),
            };
            let decision = match deadline
                .run_execution(
                    AdaptiveRuntimeStage::Planning,
                    || {
                        format!(
                            "adaptive_run_id={}; planning_turn={}; next_layer_attempt={}",
                            request.adaptive_run_id.as_str(),
                            planning_turn,
                            context.attempt
                        )
                    },
                    runtime.run_planning_turn(planning_request, &deadline),
                )
                .await
            {
                Ok(decision) => decision,
                Err(error) => {
                    let cancellation = deadline
                        .run_terminalization(
                            AdaptiveRuntimeStage::PlanningCancellation,
                            runtime.cancel_planning_turn(&deadline),
                        )
                        .await;
                    return match cancellation {
                        Ok(()) => Err(error),
                        Err(cancel_error) => Err(AdaptiveError::OperationFailedWithCleanup {
                            primary: Box::new(error),
                            cleanup: format!("adaptive planning-flow cancellation: {cancel_error}"),
                        }),
                    };
                }
            };
            planning_turn = planning_turn.saturating_add(1);
            deadline
                .run_execution(
                    AdaptiveRuntimeStage::PlanningDecision,
                    || {
                        format!(
                            "adaptive_run_id={}; planning_turn={}; next_layer_attempt={}",
                            request.adaptive_run_id.as_str(),
                            planning_turn.saturating_sub(1),
                            context.attempt
                        )
                    },
                    kernel.record_planning_decision(&capability, &decision),
                )
                .await?;

            match decision {
                LayerDecision::Finish { result, .. } => {
                    let digest = body_store.put_json(&result.result)?;
                    deadline
                        .run_execution(
                            AdaptiveRuntimeStage::FinishAuthority,
                            || {
                                format!(
                                    "adaptive_run_id={}; result_digest={}",
                                    request.adaptive_run_id.as_str(),
                                    digest.as_str()
                                )
                            },
                            kernel.resolve_finish(&capability, &digest),
                        )
                        .await?;
                    return Ok(AdaptiveRunOutcome {
                        adaptive_run_id: request.adaptive_run_id,
                        final_result_digest: Some(digest),
                        final_result: Some(result.result),
                    });
                }
                LayerDecision::RunLayer { plan, .. } => {
                    let plan_digest = body_store.put_json(&serde_json::to_value(&plan)?)?;
                    let scoped_layer_id = scoped_layer_id(&request.adaptive_run_id, &plan.id)?;
                    context.previous_layer_result = previous_layer_result.clone();
                    let compiled = match compile_layer(&plan, &context, &request.policy) {
                        Ok(compiled) if compiled.plan_digest == plan_digest => compiled,
                        Ok(compiled) => {
                            deadline
                                .run_execution(
                                    AdaptiveRuntimeStage::PlanRejection,
                                    || {
                                        format!(
                                            "adaptive_run_id={}; layer_id={}",
                                            request.adaptive_run_id.as_str(),
                                            scoped_layer_id.as_str()
                                        )
                                    },
                                    kernel.record_plan_rejected(&capability, &scoped_layer_id),
                                )
                                .await?;
                            return Err(AdaptiveError::BodyDigestMismatch {
                                expected: plan_digest,
                                actual: compiled.plan_digest,
                            });
                        }
                        Err(error) => {
                            deadline
                                .run_execution(
                                    AdaptiveRuntimeStage::PlanRejection,
                                    || {
                                        format!(
                                            "adaptive_run_id={}; layer_id={}",
                                            request.adaptive_run_id.as_str(),
                                            scoped_layer_id.as_str()
                                        )
                                    },
                                    kernel.record_plan_rejected(&capability, &scoped_layer_id),
                                )
                                .await?;
                            return Err(error);
                        }
                    };
                    let admission = match deadline
                        .run_execution(
                            AdaptiveRuntimeStage::LayerAdmission,
                            || {
                                format!(
                                    "adaptive_run_id={}; layer_id={}; attempt={}",
                                    request.adaptive_run_id.as_str(),
                                    scoped_layer_id.as_str(),
                                    context.attempt
                                )
                            },
                            kernel.resolve_layer_admission(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                                &compiled,
                                runtime.now_ms(),
                            ),
                        )
                        .await
                    {
                        Ok(admission) => admission,
                        Err(primary) => {
                            if deadline_observation_ms(&primary).is_some() {
                                // The admission command may have committed and
                                // lost its reply. Preserve DeadlineExceeded as
                                // the run terminal: classify machine custody,
                                // terminalize a committed reservation, and
                                // record proven physical absence before the
                                // outer deadline observation.
                                let (layer_exists, mut failures) = layer_exists_until_confirmed(
                                    kernel,
                                    &capability,
                                    &scoped_layer_id,
                                    &deadline,
                                )
                                .await;
                                if layer_exists == Some(true) {
                                    let interruption = deadline
                                        .run_terminalization(
                                            AdaptiveRuntimeStage::LayerInterruption,
                                            kernel.record_layer_interrupted(
                                                &capability,
                                                &scoped_layer_id,
                                                context.attempt,
                                            ),
                                        )
                                        .await;
                                    if let Err(error) = finalize_absent_layer(
                                        kernel,
                                        &capability,
                                        &scoped_layer_id,
                                        context.attempt,
                                        interruption,
                                        &deadline,
                                    )
                                    .await
                                    {
                                        failures.push(error);
                                    }
                                }
                                let failures = failures
                                    .into_iter()
                                    .map(|error| ("deadline admission custody drain", error))
                                    .collect();
                                return Err(operation_error_with_followup_failures(
                                    primary, failures,
                                ));
                            }
                            // A lost admission acknowledgement can hide a
                            // committed reservation even though provisioning
                            // has not begun. Cancel first, then use machine
                            // truth to release only a proven-absent layer.
                            let (confirmed, cancel_failures) =
                                cancel_run_until_confirmed(kernel, &capability, &deadline).await;
                            let mut failures = cancel_failures
                                .into_iter()
                                .map(|error| ("adaptive cancellation retry", error))
                                .collect::<Vec<_>>();
                            cancel_confirmed = confirmed;
                            let (layer_exists, snapshot_failures) = layer_exists_until_confirmed(
                                kernel,
                                &capability,
                                &scoped_layer_id,
                                &deadline,
                            )
                            .await;
                            failures.extend(
                                snapshot_failures
                                    .into_iter()
                                    .map(|error| ("adaptive layer snapshot retry", error)),
                            );
                            if layer_exists == Some(true) {
                                let (_, disposition_failures) =
                                    record_absent_layer_until_confirmed(
                                        kernel,
                                        &capability,
                                        &scoped_layer_id,
                                        context.attempt,
                                        &deadline,
                                    )
                                    .await;
                                failures.extend(
                                    disposition_failures
                                        .into_iter()
                                        .map(|error| ("absent layer disposition retry", error)),
                                );
                            }
                            return Err(operation_error_with_followup_failures(primary, failures));
                        }
                    };
                    if !matches!(admission, crate::AdaptiveLayerAdmission::Allowed) {
                        continue;
                    }

                    let provision = tokio_time::timeout_at(
                        deadline.execution_deadline,
                        runtime.provision_layer(
                            &capability,
                            &scoped_layer_id,
                            context.attempt,
                            &compiled,
                            &deadline,
                        ),
                    )
                    .await
                    .map_err(|_| {
                        deadline.execution_error(
                            AdaptiveRuntimeStage::LayerProvision,
                            format!(
                                "adaptive_run_id={}; layer_id={}; attempt={}",
                                request.adaptive_run_id.as_str(),
                                scoped_layer_id.as_str(),
                                context.attempt
                            ),
                        )
                    })?;
                    let layer = match provision {
                        AdaptiveLayerProvision::Ready(layer) => layer,
                        AdaptiveLayerProvision::Failed {
                            layer,
                            fault,
                            spawned_members,
                            requested_members,
                            error,
                        } => {
                            let terminalization = deadline
                                .run_terminalization(
                                    AdaptiveRuntimeStage::LayerInterruption,
                                    kernel.record_layer_setup_fault(
                                        &capability,
                                        &scoped_layer_id,
                                        context.attempt,
                                        fault,
                                        spawned_members,
                                        requested_members,
                                    ),
                                )
                                .await;
                            let finalization = match layer {
                                Some(layer) => {
                                    finalize_acquired_layer(
                                        AcquiredLayerFinalizationContext {
                                            kernel,
                                            capability: &capability,
                                            runtime,
                                            layer_id: &scoped_layer_id,
                                            attempt: context.attempt,
                                            deadline: &deadline,
                                        },
                                        layer,
                                        terminalization,
                                    )
                                    .await
                                }
                                None => {
                                    finalize_absent_layer(
                                        kernel,
                                        &capability,
                                        &scoped_layer_id,
                                        context.attempt,
                                        terminalization,
                                        &deadline,
                                    )
                                    .await
                                }
                            };
                            return Err(attach_layer_finalization_error(error, finalization));
                        }
                    };
                    if let Err(error) = deadline
                        .run_execution(
                            AdaptiveRuntimeStage::LayerProvisionAuthority,
                            || {
                                format!(
                                    "adaptive_run_id={}; layer_id={}; attempt={}",
                                    request.adaptive_run_id.as_str(),
                                    scoped_layer_id.as_str(),
                                    context.attempt
                                )
                            },
                            kernel.record_layer_provisioned(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                            ),
                        )
                        .await
                    {
                        let terminalization = deadline
                            .run_terminalization(
                                AdaptiveRuntimeStage::LayerInterruption,
                                kernel.record_layer_interrupted(
                                    &capability,
                                    &scoped_layer_id,
                                    context.attempt,
                                ),
                            )
                            .await;
                        let finalization = finalize_acquired_layer(
                            AcquiredLayerFinalizationContext {
                                kernel,
                                capability: &capability,
                                runtime,
                                layer_id: &scoped_layer_id,
                                attempt: context.attempt,
                                deadline: &deadline,
                            },
                            layer,
                            terminalization,
                        )
                        .await;
                        return Err(attach_layer_finalization_error(error, finalization));
                    }
                    let child_run_id = match deadline
                        .run_execution(
                            AdaptiveRuntimeStage::LayerStart,
                            || {
                                format!(
                                    "adaptive_run_id={}; layer_id={}; attempt={}",
                                    request.adaptive_run_id.as_str(),
                                    scoped_layer_id.as_str(),
                                    context.attempt
                                )
                            },
                            runtime.start_layer_flow(
                                layer.layer(),
                                compiled.activation_params.clone(),
                                &deadline,
                            ),
                        )
                        .await
                    {
                        Ok(run_id) => run_id,
                        Err(error) => {
                            let terminalization = deadline
                                .run_terminalization(
                                    AdaptiveRuntimeStage::LayerInterruption,
                                    kernel.record_layer_interrupted(
                                        &capability,
                                        &scoped_layer_id,
                                        context.attempt,
                                    ),
                                )
                                .await;
                            let finalization = finalize_acquired_layer(
                                AcquiredLayerFinalizationContext {
                                    kernel,
                                    capability: &capability,
                                    runtime,
                                    layer_id: &scoped_layer_id,
                                    attempt: context.attempt,
                                    deadline: &deadline,
                                },
                                layer,
                                terminalization,
                            )
                            .await;
                            return Err(attach_layer_finalization_error(error, finalization));
                        }
                    };
                    if let Err(error) = deadline
                        .run_execution(
                            AdaptiveRuntimeStage::LayerRunStartAuthority,
                            || {
                                format!(
                                    "adaptive_run_id={}; layer_id={}; attempt={}; child_run_id={}",
                                    request.adaptive_run_id.as_str(),
                                    scoped_layer_id.as_str(),
                                    context.attempt,
                                    child_run_id
                                )
                            },
                            kernel.record_layer_run_started(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                                child_run_id.clone(),
                            ),
                        )
                        .await
                    {
                        let terminalization = deadline
                            .run_terminalization(
                                AdaptiveRuntimeStage::LayerInterruption,
                                kernel.record_layer_interrupted(
                                    &capability,
                                    &scoped_layer_id,
                                    context.attempt,
                                ),
                            )
                            .await;
                        let finalization = finalize_acquired_layer(
                            AcquiredLayerFinalizationContext {
                                kernel,
                                capability: &capability,
                                runtime,
                                layer_id: &scoped_layer_id,
                                attempt: context.attempt,
                                deadline: &deadline,
                            },
                            layer,
                            terminalization,
                        )
                        .await;
                        return Err(attach_layer_finalization_error(error, finalization));
                    }
                    let child_run = match deadline
                        .run_execution(
                            AdaptiveRuntimeStage::LayerTerminal,
                            || {
                                format!(
                                    "adaptive_run_id={}; layer_id={}; attempt={}; child_run_id={}",
                                    request.adaptive_run_id.as_str(),
                                    scoped_layer_id.as_str(),
                                    context.attempt,
                                    child_run_id
                                )
                            },
                            runtime.await_layer_terminal(
                                layer.layer(),
                                child_run_id.clone(),
                                &deadline,
                            ),
                        )
                        .await
                    {
                        Ok(child_run) => child_run,
                        Err(error) => {
                            let cancellation = deadline
                                .run_terminalization(
                                    AdaptiveRuntimeStage::LayerCancellation,
                                    runtime.cancel_layer_flow(
                                        layer.layer(),
                                        child_run_id,
                                        &deadline,
                                    ),
                                )
                                .await;
                            let terminalization = deadline
                                .run_terminalization(
                                    AdaptiveRuntimeStage::LayerInterruption,
                                    kernel.record_layer_interrupted(
                                        &capability,
                                        &scoped_layer_id,
                                        context.attempt,
                                    ),
                                )
                                .await;
                            let finalization = finalize_acquired_layer(
                                AcquiredLayerFinalizationContext {
                                    kernel,
                                    capability: &capability,
                                    runtime,
                                    layer_id: &scoped_layer_id,
                                    attempt: context.attempt,
                                    deadline: &deadline,
                                },
                                layer,
                                terminalization,
                            )
                            .await;
                            let error = match cancellation {
                                Ok(()) => error,
                                Err(cancel_error) => AdaptiveError::OperationFailedWithCleanup {
                                    primary: Box::new(error),
                                    cleanup: format!(
                                        "adaptive child-flow cancellation: {cancel_error}"
                                    ),
                                },
                            };
                            return Err(attach_layer_finalization_error(error, finalization));
                        }
                    };
                    if let Err(error) = deadline
                        .run_execution(
                            AdaptiveRuntimeStage::LayerTerminalAuthority,
                            || {
                                format!(
                                    "adaptive_run_id={}; layer_id={}; attempt={}; child_run_id={}",
                                    request.adaptive_run_id.as_str(),
                                    scoped_layer_id.as_str(),
                                    context.attempt,
                                    child_run.run_id
                                )
                            },
                            kernel.ingest_layer_terminal(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                                &child_run,
                            ),
                        )
                        .await
                    {
                        let terminalization = deadline
                            .run_terminalization(
                                AdaptiveRuntimeStage::LayerInterruption,
                                kernel.record_layer_interrupted(
                                    &capability,
                                    &scoped_layer_id,
                                    context.attempt,
                                ),
                            )
                            .await;
                        let finalization = finalize_acquired_layer(
                            AcquiredLayerFinalizationContext {
                                kernel,
                                capability: &capability,
                                runtime,
                                layer_id: &scoped_layer_id,
                                attempt: context.attempt,
                                deadline: &deadline,
                            },
                            layer,
                            terminalization,
                        )
                        .await;
                        return Err(attach_layer_finalization_error(error, finalization));
                    }

                    let layer_result =
                        match extract_layer_result(&plan, &child_run).and_then(|value| {
                            validate_layer_result(&plan, &context.schema_registry, value)
                        }) {
                            Ok(result) => result,
                            Err(error) => {
                                let terminalization = deadline
                                    .run_terminalization(
                                        AdaptiveRuntimeStage::LayerResultAuthority,
                                        kernel.record_layer_result_invalid(
                                            &capability,
                                            &scoped_layer_id,
                                            context.attempt,
                                        ),
                                    )
                                    .await;
                                let finalization = finalize_acquired_layer(
                                    AcquiredLayerFinalizationContext {
                                        kernel,
                                        capability: &capability,
                                        runtime,
                                        layer_id: &scoped_layer_id,
                                        attempt: context.attempt,
                                        deadline: &deadline,
                                    },
                                    layer,
                                    terminalization,
                                )
                                .await;
                                return Err(attach_layer_finalization_error(error, finalization));
                            }
                        };
                    let result_digest = match body_store.put_json(&layer_result) {
                        Ok(digest) => digest,
                        Err(error) => {
                            let terminalization = deadline
                                .run_terminalization(
                                    AdaptiveRuntimeStage::LayerInterruption,
                                    kernel.record_layer_interrupted(
                                        &capability,
                                        &scoped_layer_id,
                                        context.attempt,
                                    ),
                                )
                                .await;
                            let finalization = finalize_acquired_layer(
                                AcquiredLayerFinalizationContext {
                                    kernel,
                                    capability: &capability,
                                    runtime,
                                    layer_id: &scoped_layer_id,
                                    attempt: context.attempt,
                                    deadline: &deadline,
                                },
                                layer,
                                terminalization,
                            )
                            .await;
                            return Err(attach_layer_finalization_error(error, finalization));
                        }
                    };
                    if let Err(error) = deadline
                        .run_execution(
                            AdaptiveRuntimeStage::LayerResultAuthority,
                            || {
                                format!(
                                    "adaptive_run_id={}; layer_id={}; attempt={}; result_digest={}",
                                    request.adaptive_run_id.as_str(),
                                    scoped_layer_id.as_str(),
                                    context.attempt,
                                    result_digest.as_str()
                                )
                            },
                            kernel.record_layer_result_validated(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                                &result_digest,
                            ),
                        )
                        .await
                    {
                        let terminalization = deadline
                            .run_terminalization(
                                AdaptiveRuntimeStage::LayerInterruption,
                                kernel.record_layer_interrupted(
                                    &capability,
                                    &scoped_layer_id,
                                    context.attempt,
                                ),
                            )
                            .await;
                        let finalization = finalize_acquired_layer(
                            AcquiredLayerFinalizationContext {
                                kernel,
                                capability: &capability,
                                runtime,
                                layer_id: &scoped_layer_id,
                                attempt: context.attempt,
                                deadline: &deadline,
                            },
                            layer,
                            terminalization,
                        )
                        .await;
                        return Err(attach_layer_finalization_error(error, finalization));
                    }
                    finalize_acquired_layer(
                        AcquiredLayerFinalizationContext {
                            kernel,
                            capability: &capability,
                            runtime,
                            layer_id: &scoped_layer_id,
                            attempt: context.attempt,
                            deadline: &deadline,
                        },
                        layer,
                        Ok(()),
                    )
                    .await?;

                    previous_layer_result = Some(layer_result);
                    context.attempt = context.attempt.saturating_add(1);
                }
            }
        }
    }
    .await;

    match outcome {
        Ok(outcome) => {
            run_lease.disarm();
            Ok(outcome)
        }
        Err(primary) if deadline_observation_ms(&primary).is_some() => {
            let observed_at_ms = deadline_observation_ms(&primary)
                .unwrap_or_else(|| deadline.observed_expired_at_ms());
            let observation = deadline
                .run_terminalization(
                    AdaptiveRuntimeStage::DeadlineObservation,
                    kernel.observe_deadline(&capability, observed_at_ms),
                )
                .await;
            match observation {
                Ok(()) => {
                    run_lease.disarm();
                    Err(primary)
                }
                Err(error) => Err(operation_error_with_followup_failures(
                    primary,
                    vec![("adaptive deadline observation", error)],
                )),
            }
        }
        Err(primary) if cancel_confirmed => {
            run_lease.disarm();
            Err(primary)
        }
        Err(primary) => {
            let (confirmed, failures) =
                cancel_run_until_confirmed(kernel, &capability, &deadline).await;
            let failures = failures
                .into_iter()
                .map(|error| ("adaptive cancellation retry", error))
                .collect();
            if confirmed {
                run_lease.disarm();
            }
            Err(operation_error_with_followup_failures(primary, failures))
        }
    }
}

pub fn extract_layer_result(
    plan: &LayerPlan,
    child_run: &MobRun,
) -> Result<serde_json::Value, AdaptiveError> {
    match plan.shape {
        LayerShape::Solo => child_run
            .root_step_outputs
            .get(&StepId::from("produce"))
            .cloned()
            .ok_or_else(|| AdaptiveError::MissingLayerResult("produce".to_string())),
        LayerShape::FanOutCollect { .. } => {
            let envelope = child_run
                .root_step_outputs
                .get(&StepId::from("collect"))
                .ok_or_else(|| AdaptiveError::MissingLayerResult("collect".to_string()))?;
            let entries = envelope
                .as_array()
                .ok_or(AdaptiveError::InvalidLayerResultEnvelope)?;
            let entry = entries
                .first()
                .ok_or(AdaptiveError::InvalidLayerResultEnvelope)?;
            entry
                .get("output")
                .cloned()
                .ok_or(AdaptiveError::InvalidLayerResultEnvelope)
        }
    }
}

pub fn validate_layer_result(
    plan: &LayerPlan,
    registry: &SchemaRegistry,
    value: serde_json::Value,
) -> Result<serde_json::Value, AdaptiveError> {
    let schema = registry.resolve(&plan.collector.output_schema)?;
    let validator = jsonschema::validator_for(schema.as_value())
        .map_err(|error| AdaptiveError::InvalidResultSchema(error.to_string()))?;
    if validator.is_valid(&value) {
        Ok(value)
    } else {
        Err(AdaptiveError::LayerResultSchemaViolation)
    }
}

pub fn adaptive_run_limits_from_policy(
    policy: &AdaptivePolicy,
    started_at_ms: u64,
) -> Result<crate::AdaptiveRunLimits, AdaptiveError> {
    policy.limits.validate_complete("composed")?;
    Ok(crate::AdaptiveRunLimits {
        max_depth: policy.limits.max_depth,
        max_total_decisions: policy.limits.max_total_decisions,
        max_repair_attempts: policy.limits.max_repair_attempts,
        max_layer_failures: policy.limits.max_layer_failures,
        max_attempts_per_layer: policy.limits.max_attempts_per_layer,
        max_members_per_layer: policy.limits.max_members_per_layer,
        max_total_spawned_members: policy.limits.max_total_spawned_members,
        max_active_members: policy.limits.max_active_members,
        max_retained_layer_mobs: policy.limits.max_retained_layer_mobs,
        max_aggregate_tokens: policy.limits.max_aggregate_tokens,
        max_aggregate_tool_calls: policy.limits.max_aggregate_tool_calls,
        allowed_model_classes: policy.allowed_model_classes.clone(),
        allowed_tool_classes: canonical_adaptive_tool_set(
            "allowed_tool_classes",
            &policy.allowed_tool_classes,
        )?,
        allowed_skill_identities: canonical_adaptive_skill_set(
            "allowed_skill_classes",
            &policy.allowed_skill_classes,
        )?,
        allowed_auth_binding_refs: policy.allowed_auth_bindings.clone(),
        deadline_ms: started_at_ms.saturating_add(policy.limits.max_wall_clock_ms),
    })
}

pub fn compile_layer(
    plan: &LayerPlan,
    context: &CompileContext,
    policy: &AdaptivePolicy,
) -> Result<CompiledLayer, AdaptiveError> {
    validate_identifier("layer_id", plan.id.as_str())?;
    let plan_value = serde_json::to_value(plan)?;
    let plan_digest = digest_bytes(&serde_json::to_vec(&plan_value)?);
    let child_mob_id = derive_child_mob_id(&context.adaptive_run_id, &plan.id, context.attempt)?;
    let mut spawn_specs = plan.spawn.clone();
    for group in &plan.spawn_groups {
        spawn_specs.extend(expand_spawn_group(group, context)?);
    }
    let collector_identity = format!("{}-collector", plan.id.as_str());
    spawn_specs.push(LayerSpawnSpec {
        identity: collector_identity,
        profile: plan.collector.profile.clone(),
        initial_message: format!("Collect layer result for {}.", plan.id.as_str()),
        budget_limits: None,
    });

    let mut definition = MobDefinition::explicit(child_mob_id.clone());
    definition.orchestrator = Some(OrchestratorConfig {
        profile: plan.collector.profile.clone(),
    });
    definition.profiles = compile_profiles(plan, context, policy)?;
    let policy_evidence = collect_layer_policy_evidence(&definition.profiles, &spawn_specs)?;
    definition.wiring = compile_wiring(plan);
    definition.flows.insert(
        FlowId::from("layer-flow"),
        compile_flow(plan, &context.schema_registry)?,
    );

    let activation_params = plan
        .activation_params
        .iter()
        .map(|(key, value)| {
            resolve_adaptive_value(value, context).map(|resolved| (key.clone(), resolved))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    let spawn_specs = spawn_specs
        .into_iter()
        .map(|spec| {
            validate_identifier("agent_identity", &spec.identity)?;
            Ok(SpawnMemberSpec::new(spec.profile, spec.identity)
                .with_initial_message(spec.initial_message)
                .with_budget_limits_if_present(spec.budget_limits))
        })
        .collect::<Result<Vec<_>, AdaptiveError>>()?;

    Ok(CompiledLayer {
        child_mob_id,
        definition,
        spawn_specs,
        activation_params,
        plan_digest,
        policy_evidence,
    })
}

trait SpawnBudgetExt {
    fn with_budget_limits_if_present(
        self,
        limits: Option<meerkat_core::BudgetLimits>,
    ) -> SpawnMemberSpec;
}

impl SpawnBudgetExt for SpawnMemberSpec {
    fn with_budget_limits_if_present(
        self,
        limits: Option<meerkat_core::BudgetLimits>,
    ) -> SpawnMemberSpec {
        if let Some(limits) = limits {
            self.with_budget_limits(limits)
        } else {
            self
        }
    }
}

fn compile_profiles(
    plan: &LayerPlan,
    context: &CompileContext,
    policy: &AdaptivePolicy,
) -> Result<BTreeMap<ProfileName, ProfileBinding>, AdaptiveError> {
    let mut profiles = BTreeMap::new();
    for (name, layer_profile) in &plan.profiles {
        let profile = match layer_profile {
            LayerProfile::Template { template } => context
                .profile_templates
                .get(template)
                .cloned()
                .ok_or_else(|| AdaptiveError::MissingProfileTemplate(template.to_string()))?,
            LayerProfile::Inline { inline } => {
                if !policy.allow_inline_profiles {
                    return Err(AdaptiveError::InlineProfilesDisabled {
                        profile: name.to_string(),
                    });
                }
                inline.as_ref().clone()
            }
        };
        profiles.insert(name.clone(), ProfileBinding::Inline(Box::new(profile)));
    }
    if !profiles.contains_key(&plan.collector.profile) {
        let collector = context
            .profile_templates
            .get(&plan.collector.profile)
            .cloned()
            .ok_or_else(|| {
                AdaptiveError::MissingProfileTemplate(plan.collector.profile.to_string())
            })?;
        profiles.insert(
            plan.collector.profile.clone(),
            ProfileBinding::Inline(Box::new(collector)),
        );
    }
    Ok(profiles)
}

fn collect_layer_policy_evidence(
    profiles: &BTreeMap<ProfileName, ProfileBinding>,
    spawn_specs: &[LayerSpawnSpec],
) -> Result<LayerPolicyEvidence, AdaptiveError> {
    let mut evidence = LayerPolicyEvidence::default();
    for spec in spawn_specs {
        let Some(profile) = profiles
            .get(&spec.profile)
            .and_then(ProfileBinding::as_inline)
        else {
            continue;
        };
        evidence.used_model_classes.insert(profile.model.clone());
        collect_profile_tool_classes(profile, &mut evidence.used_tool_classes)?;
        for skill in &profile.skills {
            evidence
                .used_skill_identities
                .insert(AdaptiveSkillIdentity::parse(skill)?.into_canonical());
        }
    }
    Ok(evidence)
}

fn collect_profile_tool_classes(
    profile: &Profile,
    out: &mut BTreeSet<String>,
) -> Result<(), AdaptiveError> {
    if profile.tools.builtins {
        out.insert("builtins".to_string());
    }
    if profile.tools.shell {
        out.insert("shell".to_string());
    }
    if profile.tools.comms {
        out.insert("comms".to_string());
    }
    if profile.tools.memory {
        out.insert("memory".to_string());
    }
    if profile.tools.workgraph {
        out.insert("workgraph".to_string());
    }
    if profile.tools.mob {
        out.insert("mob".to_string());
    }
    if profile.tools.schedule {
        out.insert("schedule".to_string());
    }
    if profile.tools.image_generation {
        out.insert("image_generation".to_string());
    }
    for name in &profile.tools.mcp {
        out.insert(AdaptiveToolIdentity::mcp_server(name)?.into_canonical());
    }
    for name in &profile.tools.rust_bundles {
        out.insert(AdaptiveToolIdentity::rust_bundle(name)?.into_canonical());
    }
    Ok(())
}

fn compile_wiring(plan: &LayerPlan) -> WiringRules {
    let role_wiring = match &plan.shape {
        LayerShape::FanOutCollect { worker_role, .. } => vec![RoleWiringRule {
            a: worker_role.clone(),
            b: plan.collector.profile.clone(),
        }],
        LayerShape::Solo => Vec::new(),
    };
    WiringRules {
        auto_wire_orchestrator: false,
        role_wiring,
    }
}

fn compile_flow(plan: &LayerPlan, registry: &SchemaRegistry) -> Result<FlowSpec, AdaptiveError> {
    let mut steps = IndexMap::new();
    let bare_schema = registry.resolve(&plan.collector.output_schema)?;
    match &plan.shape {
        LayerShape::FanOutCollect {
            worker_role,
            collection,
        } => {
            steps.insert(
                StepId::from("work"),
                FlowStepSpec {
                    role: worker_role.clone(),
                    message: ContentInput::from(plan.objective.clone()),
                    depends_on: Vec::new(),
                    dispatch_mode: DispatchMode::FanOut,
                    collection_policy: collection.clone().into(),
                    condition: None,
                    timeout_ms: None,
                    expected_schema_ref: None,
                    branch: None,
                    depends_on_mode: Default::default(),
                    allowed_tools: None,
                    blocked_tools: None,
                    output_format: Some(StepOutputFormat::Json),
                },
            );
            steps.insert(
                StepId::from("collect"),
                FlowStepSpec {
                    role: plan.collector.profile.clone(),
                    message: ContentInput::from(format!(
                        "Produce the schema-valid LayerResult for {}.",
                        plan.id.as_str()
                    )),
                    depends_on: vec![StepId::from("work")],
                    dispatch_mode: DispatchMode::FanIn,
                    collection_policy: CollectionPolicy::All,
                    condition: None,
                    timeout_ms: None,
                    expected_schema_ref: Some(FlowSchemaRef::Inline(wrap_fan_in_schema(
                        &bare_schema,
                    )?)),
                    branch: None,
                    depends_on_mode: Default::default(),
                    allowed_tools: None,
                    blocked_tools: None,
                    output_format: Some(StepOutputFormat::Json),
                },
            );
        }
        LayerShape::Solo => {
            steps.insert(
                StepId::from("produce"),
                FlowStepSpec {
                    role: plan.collector.profile.clone(),
                    message: ContentInput::from(plan.objective.clone()),
                    depends_on: Vec::new(),
                    dispatch_mode: DispatchMode::FanOut,
                    collection_policy: CollectionPolicy::Any,
                    condition: None,
                    timeout_ms: None,
                    expected_schema_ref: Some(FlowSchemaRef::Inline(bare_schema)),
                    branch: None,
                    depends_on_mode: Default::default(),
                    allowed_tools: None,
                    blocked_tools: None,
                    output_format: Some(StepOutputFormat::Json),
                },
            );
        }
    }
    Ok(FlowSpec::new(Some(plan.objective.clone()), steps, None))
}

pub fn wrap_fan_in_schema(schema: &MeerkatSchema) -> Result<MeerkatSchema, AdaptiveError> {
    MeerkatSchema::new(serde_json::json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 1,
        "items": {
            "type": "object",
            "required": ["target", "output"],
            "properties": {
                "target": { "type": "string" },
                "output": schema.as_value()
            },
            "additionalProperties": false
        }
    }))
    .map_err(AdaptiveError::from)
}

pub fn derive_child_mob_id(
    adaptive_run_id: &AdaptiveRunId,
    layer_id: &LayerId,
    attempt: u64,
) -> Result<MobId, AdaptiveError> {
    if attempt == 0 {
        return Err(AdaptiveError::InvalidAttempt);
    }
    Ok(MobId::from(format!(
        "adaptive-{}-{}-a{}",
        adaptive_run_id.as_str(),
        layer_id.as_str(),
        attempt
    )))
}

pub fn scoped_layer_id(
    adaptive_run_id: &AdaptiveRunId,
    layer_id: &LayerId,
) -> Result<LayerId, AdaptiveError> {
    LayerId::new(format!(
        "{}-{}",
        adaptive_run_id.as_str(),
        layer_id.as_str()
    ))
}

fn expand_spawn_group(
    group: &LayerSpawnGroup,
    context: &CompileContext,
) -> Result<Vec<LayerSpawnSpec>, AdaptiveError> {
    let parsed_ref = AdaptiveRef::parse(&group.items_ref)?;
    let items = resolve_ref(&parsed_ref, context)?;
    let array = items
        .as_array()
        .ok_or_else(|| AdaptiveError::RefNotArray(group.items_ref.clone()))?;
    if array.len() > group.max_items {
        return Err(AdaptiveError::SpawnGroupTooLarge {
            max_items: group.max_items,
            actual: array.len(),
        });
    }
    array
        .iter()
        .map(|item| {
            let key_path = parse_path(&group.key_path)?
                .into_iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            let key = lookup_path(item, &key_path)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| AdaptiveError::MissingSpawnGroupKey(group.key_path.clone()))?;
            let suffix = sanitize_identifier(key)?;
            let identity = format!("{}-{}", group.prefix, suffix);
            validate_identifier("agent_identity", &identity)?;
            let message = render_template(&group.initial_message_template, item)?;
            Ok(LayerSpawnSpec {
                identity,
                profile: group.profile.clone(),
                initial_message: message,
                budget_limits: None,
            })
        })
        .collect()
}

fn resolve_adaptive_value(
    value: &AdaptiveValue,
    context: &CompileContext,
) -> Result<serde_json::Value, AdaptiveError> {
    match value {
        AdaptiveValue::Literal(value) => Ok(value.clone()),
        AdaptiveValue::Ref { r#ref } => resolve_ref(&AdaptiveRef::parse(r#ref)?, context).cloned(),
    }
}

fn resolve_ref<'a>(
    reference: &AdaptiveRef,
    context: &'a CompileContext,
) -> Result<&'a serde_json::Value, AdaptiveError> {
    match reference {
        AdaptiveRef::PreviousLayerResult(path) => {
            let root = context
                .previous_layer_result
                .as_ref()
                .ok_or(AdaptiveError::PreviousLayerResultMissing)?;
            lookup_path(root, path).ok_or_else(|| AdaptiveError::RefPathMissing(path.join(".")))
        }
        _ => Err(AdaptiveError::UnsupportedRef),
    }
}

fn lookup_path<'a>(root: &'a serde_json::Value, path: &[String]) -> Option<&'a serde_json::Value> {
    path.iter()
        .try_fold(root, |current, segment| current.get(segment))
}

fn render_template(template: &str, item: &serde_json::Value) -> Result<String, AdaptiveError> {
    let mut rendered = template.to_string();
    while let Some(start) = rendered.find("{{") {
        let Some(relative_end) = rendered[start + 2..].find("}}") else {
            return Err(AdaptiveError::InvalidTemplate(template.to_string()));
        };
        let end = start + 2 + relative_end;
        let expr = rendered[start + 2..end].trim();
        let path = expr
            .strip_prefix("item.")
            .ok_or_else(|| AdaptiveError::InvalidTemplate(template.to_string()))?;
        let segments = parse_path(path)?
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let value = lookup_path(item, &segments)
            .ok_or_else(|| AdaptiveError::InvalidTemplate(template.to_string()))?;
        let replacement = value
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| value.to_string());
        rendered.replace_range(start..end + 2, &replacement);
    }
    Ok(rendered)
}

fn parse_path(raw: &str) -> Result<Vec<&str>, AdaptiveError> {
    let segments = raw.split('.').collect::<Vec<_>>();
    if segments.is_empty() || segments.iter().any(|segment| segment.is_empty()) {
        return Err(AdaptiveError::InvalidRef(raw.to_string()));
    }
    Ok(segments)
}

fn validate_adaptive_identity_component(
    field: &'static str,
    value: &str,
) -> Result<(), AdaptiveError> {
    if value.is_empty() {
        return Err(AdaptiveError::InvalidAdaptiveIdentity {
            field,
            value: value.to_string(),
            reason: "empty component".to_string(),
        });
    }
    if value.trim() != value {
        return Err(AdaptiveError::InvalidAdaptiveIdentity {
            field,
            value: value.to_string(),
            reason: "leading or trailing whitespace".to_string(),
        });
    }
    if value.contains(':') {
        return Err(AdaptiveError::InvalidAdaptiveIdentity {
            field,
            value: value.to_string(),
            reason: "colon is reserved for adaptive identity namespaces".to_string(),
        });
    }
    Ok(())
}

fn canonical_adaptive_tool_set(
    field: &'static str,
    values: &BTreeSet<String>,
) -> Result<BTreeSet<String>, AdaptiveError> {
    values
        .iter()
        .map(|value| {
            AdaptiveToolIdentity::parse(value)
                .map(AdaptiveToolIdentity::into_canonical)
                .map_err(|error| match error {
                    AdaptiveError::InvalidAdaptiveIdentity { value, reason, .. } => {
                        AdaptiveError::InvalidAdaptiveIdentity {
                            field,
                            value,
                            reason,
                        }
                    }
                    other => other,
                })
        })
        .collect()
}

fn canonical_adaptive_skill_set(
    field: &'static str,
    values: &BTreeSet<String>,
) -> Result<BTreeSet<String>, AdaptiveError> {
    values
        .iter()
        .map(|value| {
            AdaptiveSkillIdentity::parse(value)
                .map(AdaptiveSkillIdentity::into_canonical)
                .map_err(|error| match error {
                    AdaptiveError::InvalidAdaptiveIdentity { value, reason, .. } => {
                        AdaptiveError::InvalidAdaptiveIdentity {
                            field,
                            value,
                            reason,
                        }
                    }
                    other => other,
                })
        })
        .collect()
}

fn sanitize_identifier(raw: &str) -> Result<String, AdaptiveError> {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            'a'..='z' | '0'..='9' => out.push(ch),
            'A'..='Z' => out.push(ch.to_ascii_lowercase()),
            '-' | '_' | ' ' => out.push('-'),
            _ => return Err(AdaptiveError::InvalidIdentifier(raw.to_string())),
        }
    }
    validate_identifier("identifier", &out)?;
    Ok(out)
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), AdaptiveError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.ends_with('-')
        || value
            .chars()
            .any(|ch| !matches!(ch, 'a'..='z' | '0'..='9' | '-'))
    {
        return Err(AdaptiveError::InvalidFieldIdentifier {
            field,
            value: value.to_string(),
        });
    }
    Ok(())
}

fn digest_bytes(bytes: &[u8]) -> BodyDigest {
    BodyDigest(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn intersect(left: &BTreeSet<String>, right: &BTreeSet<String>) -> BTreeSet<String> {
    left.intersection(right).cloned().collect()
}

impl From<LayerCollection> for CollectionPolicy {
    fn from(value: LayerCollection) -> Self {
        match value {
            LayerCollection::All => Self::All,
            LayerCollection::Any => Self::Any,
            LayerCollection::Quorum { n } => Self::Quorum { n },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdaptiveError {
    #[error("{owner} adaptive policy is incomplete: {field} must be non-zero")]
    IncompletePolicy { owner: String, field: &'static str },
    #[error("invalid {field} identifier: {value}")]
    InvalidFieldIdentifier { field: &'static str, value: String },
    #[error("invalid {field}: {value} ({reason})")]
    InvalidAdaptiveIdentity {
        field: &'static str,
        value: String,
        reason: String,
    },
    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),
    #[error("invalid adaptive ref: {0}")]
    InvalidRef(String),
    #[error("unsupported adaptive ref for this compile context")]
    UnsupportedRef,
    #[error("previous layer result is required")]
    PreviousLayerResultMissing,
    #[error("ref path is missing: {0}")]
    RefPathMissing(String),
    #[error("ref did not resolve to an array: {0}")]
    RefNotArray(String),
    #[error("spawn group too large: {actual} > {max_items}")]
    SpawnGroupTooLarge { max_items: usize, actual: usize },
    #[error("missing spawn-group key path: {0}")]
    MissingSpawnGroupKey(String),
    #[error("invalid template: {0}")]
    InvalidTemplate(String),
    #[error("missing schema: {0}")]
    MissingSchema(String),
    #[error("missing profile template: {0}")]
    MissingProfileTemplate(String),
    #[error("inline profile '{profile}' is not allowed by the composed adaptive policy")]
    InlineProfilesDisabled { profile: String },
    #[error("attempt ordinals start at 1")]
    InvalidAttempt,
    #[error("body missing for digest {0:?}")]
    BodyMissing(BodyDigest),
    #[error("body digest mismatch: expected {expected:?}, actual {actual:?}")]
    BodyDigestMismatch {
        expected: BodyDigest,
        actual: BodyDigest,
    },
    #[error("missing layer result for step {0}")]
    MissingLayerResult(String),
    #[error("invalid layer result envelope")]
    InvalidLayerResultEnvelope,
    #[error("invalid layer result schema: {0}")]
    InvalidResultSchema(String),
    #[error("layer result does not satisfy declared schema")]
    LayerResultSchemaViolation,
    #[error(
        "adaptive wall-clock deadline exceeded during {stage}; deadline_ms={deadline_ms}; observed_at_ms={observed_at_ms}; diagnostics={diagnostics}"
    )]
    DeadlineExceeded {
        stage: AdaptiveRuntimeStage,
        deadline_ms: u64,
        observed_at_ms: u64,
        diagnostics: String,
    },
    #[error(
        "adaptive terminalization window expired during {stage}; adaptive_deadline_ms={deadline_ms}"
    )]
    TerminalizationDeadlineExceeded {
        stage: AdaptiveRuntimeStage,
        deadline_ms: u64,
    },
    #[error("adaptive driver runtime failed: {0}")]
    DriverRuntime(String),
    #[error("adaptive operation failed: {primary}; cleanup/terminalization also failed: {cleanup}")]
    OperationFailedWithCleanup {
        #[source]
        primary: Box<AdaptiveError>,
        cleanup: String,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Schema(#[from] meerkat_core::schema::SchemaError),
    #[error(transparent)]
    Mob(#[from] crate::MobError),
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::MobRunStatus;
    use chrono::Utc;
    use indexmap::IndexMap;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[cfg(feature = "schema")]
    #[test]
    fn layer_decision_schema_validates_inline_profiles_structurally() {
        let schema = layer_decision_schema().expect("canonical schema serializes");
        let rendered = serde_json::to_string(&schema).expect("schema renders");
        // The schema is a real object schema, not a vacuous placeholder.
        assert!(schema.is_object());
        assert!(schema.get("$defs").is_some(), "schema carries definitions");
        // Inline layer profiles reference the structural Profile definition
        // instead of an opaque any-value escape hatch.
        assert!(
            rendered.contains("\"Profile\""),
            "inline profiles must be validated against the Profile schema: {rendered}"
        );
        let validator =
            jsonschema::validator_for(&schema).expect("canonical schema is a valid JSON schema");
        // A malformed inline profile (model must be a string) is rejected.
        let bad_decision = serde_json::json!({
            "decision": "run_layer",
            "reason": "test",
            "plan": {
                "id": "layer-1",
                "objective": "do work",
                "shape": { "kind": "solo" },
                "profiles": { "solo": { "inline": { "model": 42 } } },
                "collector": {
                    "profile": "solo",
                    "output_schema": { "inline": { "type": "object" } }
                }
            }
        });
        assert!(
            !validator.is_valid(&bad_decision),
            "inline profile with non-string model must fail schema validation"
        );
        // A well-formed finish decision validates.
        let finish = serde_json::json!({
            "decision": "finish",
            "reason": "done",
            "result": { "result": { "ok": true } }
        });
        assert!(validator.is_valid(&finish));
    }

    fn limits(value: u64) -> AdaptiveLimitRecord {
        AdaptiveLimitRecord {
            max_depth: value,
            max_total_decisions: value,
            max_repair_attempts: value,
            max_layer_failures: value,
            max_attempts_per_layer: value,
            max_members_per_layer: value,
            max_total_spawned_members: value,
            max_active_members: value,
            max_retained_layer_mobs: value,
            max_wall_clock_ms: 60_000,
            max_aggregate_tokens: value,
            max_aggregate_tool_calls: value,
        }
    }

    fn deadline_policy(max_wall_clock_ms: u64) -> AdaptivePolicy {
        let mut limits = limits(10);
        limits.max_wall_clock_ms = max_wall_clock_ms;
        AdaptivePolicy {
            limits,
            ..AdaptivePolicy::default()
        }
    }

    fn profile() -> Profile {
        Profile {
            model: "gpt-5.5".to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: Default::default(),
            peer_description: String::new(),
            external_addressable: false,
            backend: None,
            runtime_mode: crate::MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        }
    }

    fn registry() -> SchemaRegistry {
        let mut registry = SchemaRegistry::default();
        registry
            .insert(
                SchemaName::new("verification-set").unwrap(),
                serde_json::json!({
                    "type": "object",
                    "required": ["verifications"],
                    "properties": {
                        "verifications": { "type": "array" }
                    }
                }),
            )
            .unwrap();
        registry
    }

    fn compile_context() -> CompileContext {
        CompileContext {
            adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
            attempt: 1,
            schema_registry: registry(),
            profile_templates: BTreeMap::from([
                (ProfileName::from("verifier"), profile()),
                (ProfileName::from("collector"), profile()),
            ]),
            previous_layer_result: Some(serde_json::json!({
                "findings": [
                    { "id": "F-1", "title": "Provider fallback" },
                    { "id": "F-2", "title": "Auth binding leak" }
                ]
            })),
        }
    }

    fn compile_policy() -> AdaptivePolicy {
        AdaptivePolicy {
            limits: limits(10),
            allowed_model_classes: BTreeSet::from(["gpt-5.5".to_string()]),
            allow_inline_profiles: false,
            ..AdaptivePolicy::default()
        }
    }

    fn layer_plan() -> LayerPlan {
        LayerPlan {
            id: LayerId::new("verify-findings").unwrap(),
            objective: "Verify each candidate finding independently.".to_string(),
            shape: LayerShape::FanOutCollect {
                worker_role: ProfileName::from("verifier"),
                collection: LayerCollection::All,
            },
            spawn: vec![LayerSpawnSpec {
                identity: "verifier-one".to_string(),
                profile: ProfileName::from("verifier"),
                initial_message: "Verify the first finding.".to_string(),
                budget_limits: None,
            }],
            spawn_groups: Vec::new(),
            profiles: BTreeMap::from([
                (
                    ProfileName::from("verifier"),
                    LayerProfile::Template {
                        template: ProfileName::from("verifier"),
                    },
                ),
                (
                    ProfileName::from("collector"),
                    LayerProfile::Template {
                        template: ProfileName::from("collector"),
                    },
                ),
            ]),
            collector: CollectorContract {
                profile: ProfileName::from("collector"),
                output_schema: SchemaRef::Registry {
                    registry: SchemaName::new("verification-set").unwrap(),
                },
            },
            activation_params: BTreeMap::new(),
        }
    }

    fn completed_layer_run(output: serde_json::Value) -> MobRun {
        MobRun {
            run_id: RunId::new(),
            mob_id: MobId::from("adaptive-run-1-verify-findings-a1"),
            flow_id: FlowId::from("layer-flow"),
            status: MobRunStatus::Completed,
            flow_state: Default::default(),
            activation_params: serde_json::json!({}),
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
            step_ledger: Vec::new(),
            failure_ledger: Vec::new(),
            frames: BTreeMap::new(),
            loops: BTreeMap::new(),
            loop_iteration_ledger: Vec::new(),
            schema_version: 4,
            root_step_outputs: IndexMap::from([(
                StepId::from("collect"),
                serde_json::json!([{ "target": "collector", "output": output }]),
            )]),
            loop_iteration_outputs: BTreeMap::new(),
            flow_authority_inputs: Vec::new(),
        }
    }

    #[derive(Default)]
    struct FakeKernel {
        events: Mutex<Vec<String>>,
        fail_result_validated_after_record: bool,
        fail_admission: bool,
        cancel_failures_remaining: Mutex<u64>,
        disposition_failures_remaining: Mutex<u64>,
        run_cancellations: Arc<AtomicUsize>,
        initialization_gate: Option<FakeInitializationGate>,
        planning_decision_pending: bool,
    }

    #[derive(Clone)]
    struct FakeInitializationGate {
        command_enqueued: Arc<tokio::sync::Notify>,
        reply_release: Arc<tokio::sync::Notify>,
    }

    impl FakeKernel {
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        fn push(&self, event: impl Into<String>) {
            self.events.lock().unwrap().push(event.into());
        }
    }

    struct FakeRunCancellationOwner {
        run_cancellations: Arc<AtomicUsize>,
    }

    impl AdaptiveRunCancellationOwner for FakeRunCancellationOwner {
        fn take_run_for_cancellation(&self) {
            self.run_cancellations.fetch_add(1, Ordering::SeqCst);
        }

        fn disarm_after_terminal(&self) {}
    }

    fn fake_run_lease(run_cancellations: Arc<AtomicUsize>) -> AdaptiveRunLease {
        AdaptiveRunLease::new(Arc::new(FakeRunCancellationOwner { run_cancellations }))
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AdaptiveKernel for FakeKernel {
        type Capability = ();

        fn initialize_run(
            &self,
            adaptive_run_id: &AdaptiveRunId,
            _policy: &AdaptivePolicy,
            _started_at_ms: u64,
        ) -> AdaptiveRunInitialization<Self::Capability> {
            self.push(format!("initialize:{}", adaptive_run_id.as_str()));
            let run_cancellations = Arc::clone(&self.run_cancellations);
            match self.initialization_gate.clone() {
                Some(gate) => AdaptiveRunInitialization::spawn_owned(
                    async move {
                        gate.command_enqueued.notify_one();
                        gate.reply_release.notified().await;
                        Ok(())
                    },
                    move |()| fake_run_lease(run_cancellations),
                ),
                None => AdaptiveRunInitialization::completed((), fake_run_lease(run_cancellations)),
            }
        }

        async fn cancel_run(&self, _capability: &Self::Capability) -> Result<(), AdaptiveError> {
            self.run_cancellations.fetch_add(1, Ordering::SeqCst);
            self.push("cancel");
            let mut remaining = self.cancel_failures_remaining.lock().unwrap();
            if *remaining > 0 {
                *remaining = remaining.saturating_sub(1);
                return Err(AdaptiveError::DriverRuntime(
                    "cancel acknowledgement failed".to_string(),
                ));
            }
            Ok(())
        }

        async fn observe_deadline(
            &self,
            _capability: &Self::Capability,
            _observed_at_ms: u64,
        ) -> Result<(), AdaptiveError> {
            self.push("deadline");
            Ok(())
        }

        async fn record_planning_decision(
            &self,
            _capability: &Self::Capability,
            decision: &LayerDecision,
        ) -> Result<(), AdaptiveError> {
            self.push(match decision {
                LayerDecision::RunLayer { .. } => "decision:run_layer",
                LayerDecision::Finish { .. } => "decision:finish",
            });
            if self.planning_decision_pending {
                return std::future::pending().await;
            }
            Ok(())
        }

        async fn record_plan_rejected(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("plan_rejected:{}", layer_id.as_str()));
            Ok(())
        }

        async fn resolve_layer_admission(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
            _compiled: &CompiledLayer,
            _observed_at_ms: u64,
        ) -> Result<crate::AdaptiveLayerAdmission, AdaptiveError> {
            self.push(format!("admission:{}", layer_id.as_str()));
            if self.fail_admission {
                return Err(AdaptiveError::DriverRuntime(
                    "admission acknowledgement failed".to_string(),
                ));
            }
            Ok(crate::AdaptiveLayerAdmission::Allowed)
        }

        async fn record_layer_provisioned(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("provisioned:{}", layer_id.as_str()));
            Ok(())
        }

        async fn record_layer_run_started(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
            _child_run_id: RunId,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("run_started:{}", layer_id.as_str()));
            Ok(())
        }

        async fn ingest_layer_terminal(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
            _child_run: &MobRun,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("terminal:{}", layer_id.as_str()));
            Ok(())
        }

        async fn record_layer_result_validated(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
            _result_digest: &BodyDigest,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("result_valid:{}", layer_id.as_str()));
            if self.fail_result_validated_after_record {
                Err(AdaptiveError::DriverRuntime(
                    "injected committed response failure".into(),
                ))
            } else {
                Ok(())
            }
        }

        async fn record_layer_result_invalid(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("result_invalid:{}", layer_id.as_str()));
            Ok(())
        }

        async fn record_layer_interrupted(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("interrupted:{}", layer_id.as_str()));
            Ok(())
        }

        async fn record_layer_setup_fault(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
            _fault: crate::AdaptiveLayerSetupFault,
            _spawned_members: u64,
            _requested_members: u64,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("setup_fault:{}", layer_id.as_str()));
            Ok(())
        }

        async fn record_layer_mob_destroyed(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("destroyed:{}", layer_id.as_str()));
            let mut remaining = self.disposition_failures_remaining.lock().unwrap();
            if *remaining > 0 {
                *remaining = remaining.saturating_sub(1);
                return Err(AdaptiveError::DriverRuntime(
                    "cleanup disposition acknowledgement failed".to_string(),
                ));
            }
            Ok(())
        }

        async fn record_layer_mob_retained(
            &self,
            _capability: &Self::Capability,
            layer_id: &LayerId,
            _attempt: u64,
            _disposition: crate::AdaptiveLayerDisposition,
        ) -> Result<(), AdaptiveError> {
            self.push(format!("retained:{}", layer_id.as_str()));
            Ok(())
        }

        async fn resolve_finish(
            &self,
            _capability: &Self::Capability,
            _final_result_digest: &BodyDigest,
        ) -> Result<(), AdaptiveError> {
            self.push("finish");
            Ok(())
        }

        async fn cancel(&self, _capability: &Self::Capability) -> Result<(), AdaptiveError> {
            self.cancel_run(_capability).await
        }

        async fn layer_exists(
            &self,
            _capability: &Self::Capability,
            _layer_id: &LayerId,
        ) -> Result<bool, AdaptiveError> {
            Ok(self.fail_admission)
        }
    }

    enum FakeProvisionFailure {
        BeforeChild,
        WithChild { spawned_members: u64 },
    }

    struct FakeRuntime {
        decisions: VecDeque<LayerDecision>,
        layer_run: Option<MobRun>,
        saw_previous_layer_result: bool,
        planning_error: Option<String>,
        provision_failure: Option<FakeProvisionFailure>,
        start_error: Option<String>,
        await_error: Option<String>,
        cleanup_error: Option<String>,
        cleanup_calls: usize,
    }

    struct RetryableDestroyRuntime {
        inner: FakeRuntime,
        retained_cleanup_calls: usize,
    }

    struct FakeLayerCancellationOwner;

    impl AdaptiveLayerCancellationOwner<String> for FakeLayerCancellationOwner {
        fn take_layer_for_cancellation(&self, _layer: String) {}

        fn disarm_after_cleanup(&self) {}
    }

    struct CancellationProbeOwner {
        cleanup_calls: Arc<AtomicUsize>,
    }

    impl AdaptiveLayerCancellationOwner<String> for CancellationProbeOwner {
        fn take_layer_for_cancellation(&self, _layer: String) {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
        }

        fn disarm_after_cleanup(&self) {}
    }

    struct PendingCancellationRuntime {
        decision: Option<LayerDecision>,
        terminal_wait_entered: Arc<tokio::sync::Notify>,
        cleanup_calls: Arc<AtomicUsize>,
        flow_cancellations: Arc<AtomicUsize>,
    }

    enum PlanningExit {
        Error,
        Pending(Arc<tokio::sync::Notify>),
    }

    struct PlanningExitRuntime {
        exit: PlanningExit,
        planning_cancellations: Arc<AtomicUsize>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AdaptiveDriverRuntime for PlanningExitRuntime {
        type Capability = ();
        type Layer = String;

        fn now_ms(&mut self) -> u64 {
            1_050
        }

        async fn run_planning_turn(
            &mut self,
            _request: PlanningTurnRequest,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<LayerDecision, AdaptiveError> {
            match &self.exit {
                PlanningExit::Error => Err(AdaptiveError::DriverRuntime(
                    "injected planning failure".into(),
                )),
                PlanningExit::Pending(entered) => {
                    entered.notify_one();
                    std::future::pending().await
                }
            }
        }

        async fn cancel_planning_turn(
            &mut self,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            self.planning_cancellations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn provision_layer(
            &mut self,
            _capability: &Self::Capability,
            _layer_id: &LayerId,
            _attempt: u64,
            _compiled: &CompiledLayer,
            _deadline: &AdaptiveOperationDeadline,
        ) -> AdaptiveLayerProvision<Self::Layer> {
            unreachable!("planning exit occurs before layer provisioning")
        }

        async fn start_layer_flow(
            &mut self,
            _layer: &Self::Layer,
            _activation_params: BTreeMap<String, serde_json::Value>,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<RunId, AdaptiveError> {
            unreachable!("planning exit occurs before layer start")
        }

        async fn await_layer_terminal(
            &mut self,
            _layer: &Self::Layer,
            _run_id: RunId,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<MobRun, AdaptiveError> {
            unreachable!("planning exit occurs before layer wait")
        }

        async fn cancel_layer_flow(
            &mut self,
            _layer: &Self::Layer,
            _run_id: RunId,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            unreachable!("planning exit occurs before layer cancellation")
        }

        async fn cleanup_layer(
            &mut self,
            _layer: &AdaptiveLayerLease<Self::Layer>,
            _layer_id: &LayerId,
            _attempt: u64,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<AdaptiveLayerCleanup, AdaptiveError> {
            unreachable!("planning exit occurs before layer cleanup")
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AdaptiveDriverRuntime for PendingCancellationRuntime {
        type Capability = ();
        type Layer = String;

        fn now_ms(&mut self) -> u64 {
            1_050
        }

        async fn run_planning_turn(
            &mut self,
            _request: PlanningTurnRequest,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<LayerDecision, AdaptiveError> {
            Ok(self.decision.take().expect("single planning decision"))
        }

        async fn cancel_planning_turn(
            &mut self,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            Ok(())
        }

        async fn provision_layer(
            &mut self,
            _capability: &Self::Capability,
            _layer_id: &LayerId,
            _attempt: u64,
            compiled: &CompiledLayer,
            _deadline: &AdaptiveOperationDeadline,
        ) -> AdaptiveLayerProvision<Self::Layer> {
            AdaptiveLayerProvision::Ready(AdaptiveLayerLease::new(
                compiled.child_mob_id.to_string(),
                Arc::new(CancellationProbeOwner {
                    cleanup_calls: Arc::clone(&self.cleanup_calls),
                }),
            ))
        }

        async fn start_layer_flow(
            &mut self,
            _layer: &Self::Layer,
            _activation_params: BTreeMap<String, serde_json::Value>,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<RunId, AdaptiveError> {
            Ok(RunId::new())
        }

        async fn await_layer_terminal(
            &mut self,
            _layer: &Self::Layer,
            _run_id: RunId,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<MobRun, AdaptiveError> {
            self.terminal_wait_entered.notify_one();
            std::future::pending().await
        }

        async fn cancel_layer_flow(
            &mut self,
            _layer: &Self::Layer,
            _run_id: RunId,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            self.flow_cancellations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn cleanup_layer(
            &mut self,
            _layer: &AdaptiveLayerLease<Self::Layer>,
            _layer_id: &LayerId,
            _attempt: u64,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<AdaptiveLayerCleanup, AdaptiveError> {
            self.cleanup_calls.fetch_add(1, Ordering::SeqCst);
            Ok(AdaptiveLayerCleanup::Destroyed)
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AdaptiveDriverRuntime for FakeRuntime {
        type Capability = ();
        type Layer = String;

        fn now_ms(&mut self) -> u64 {
            1_050
        }

        async fn run_planning_turn(
            &mut self,
            request: PlanningTurnRequest,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<LayerDecision, AdaptiveError> {
            if request.previous_layer_result.is_some() {
                self.saw_previous_layer_result = true;
            }
            if let Some(error) = self.planning_error.take() {
                return Err(AdaptiveError::DriverRuntime(error));
            }
            Ok(self.decisions.pop_front().expect("planning decision"))
        }

        async fn cancel_planning_turn(
            &mut self,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            Ok(())
        }

        async fn provision_layer(
            &mut self,
            _capability: &Self::Capability,
            _layer_id: &LayerId,
            _attempt: u64,
            compiled: &CompiledLayer,
            _deadline: &AdaptiveOperationDeadline,
        ) -> AdaptiveLayerProvision<Self::Layer> {
            let layer = AdaptiveLayerLease::new(
                compiled.child_mob_id.to_string(),
                Arc::new(FakeLayerCancellationOwner),
            );
            if let Some(failure) = self.provision_failure.take() {
                return match failure {
                    FakeProvisionFailure::BeforeChild => AdaptiveLayerProvision::Failed {
                        layer: None,
                        fault: crate::AdaptiveLayerSetupFault::MobCreateFailed,
                        spawned_members: 0,
                        requested_members: compiled.spawn_specs.len() as u64,
                        error: AdaptiveError::DriverRuntime(
                            "provision failed before child".to_string(),
                        ),
                    },
                    FakeProvisionFailure::WithChild { spawned_members } => {
                        AdaptiveLayerProvision::Failed {
                            layer: Some(layer),
                            fault: crate::AdaptiveLayerSetupFault::SpawnFailed,
                            spawned_members,
                            requested_members: compiled.spawn_specs.len() as u64,
                            error: AdaptiveError::DriverRuntime(
                                "provision failed with child".to_string(),
                            ),
                        }
                    }
                };
            }
            AdaptiveLayerProvision::Ready(layer)
        }

        async fn start_layer_flow(
            &mut self,
            _layer: &Self::Layer,
            _activation_params: BTreeMap<String, serde_json::Value>,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<RunId, AdaptiveError> {
            if let Some(error) = self.start_error.take() {
                return Err(AdaptiveError::DriverRuntime(error));
            }
            Ok(RunId::new())
        }

        async fn await_layer_terminal(
            &mut self,
            _layer: &Self::Layer,
            _run_id: RunId,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<MobRun, AdaptiveError> {
            if let Some(error) = self.await_error.take() {
                return Err(AdaptiveError::DriverRuntime(error));
            }
            Ok(self.layer_run.take().expect("layer run"))
        }

        async fn cancel_layer_flow(
            &mut self,
            _layer: &Self::Layer,
            _run_id: RunId,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            Ok(())
        }

        async fn cleanup_layer(
            &mut self,
            _layer: &AdaptiveLayerLease<Self::Layer>,
            _layer_id: &LayerId,
            _attempt: u64,
            _deadline: &AdaptiveOperationDeadline,
        ) -> Result<AdaptiveLayerCleanup, AdaptiveError> {
            self.cleanup_calls = self.cleanup_calls.saturating_add(1);
            if let Some(error) = self.cleanup_error.take() {
                return Err(AdaptiveError::DriverRuntime(error));
            }
            Ok(AdaptiveLayerCleanup::Destroyed)
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AdaptiveDriverRuntime for RetryableDestroyRuntime {
        type Capability = ();
        type Layer = String;

        fn now_ms(&mut self) -> u64 {
            self.inner.now_ms()
        }

        async fn run_planning_turn(
            &mut self,
            request: PlanningTurnRequest,
            deadline: &AdaptiveOperationDeadline,
        ) -> Result<LayerDecision, AdaptiveError> {
            self.inner.run_planning_turn(request, deadline).await
        }

        async fn cancel_planning_turn(
            &mut self,
            deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            self.inner.cancel_planning_turn(deadline).await
        }

        async fn provision_layer(
            &mut self,
            capability: &Self::Capability,
            layer_id: &LayerId,
            attempt: u64,
            compiled: &CompiledLayer,
            deadline: &AdaptiveOperationDeadline,
        ) -> AdaptiveLayerProvision<Self::Layer> {
            self.inner
                .provision_layer(capability, layer_id, attempt, compiled, deadline)
                .await
        }

        async fn start_layer_flow(
            &mut self,
            layer: &Self::Layer,
            activation_params: BTreeMap<String, serde_json::Value>,
            deadline: &AdaptiveOperationDeadline,
        ) -> Result<RunId, AdaptiveError> {
            self.inner
                .start_layer_flow(layer, activation_params, deadline)
                .await
        }

        async fn await_layer_terminal(
            &mut self,
            layer: &Self::Layer,
            run_id: RunId,
            deadline: &AdaptiveOperationDeadline,
        ) -> Result<MobRun, AdaptiveError> {
            self.inner
                .await_layer_terminal(layer, run_id, deadline)
                .await
        }

        async fn cancel_layer_flow(
            &mut self,
            layer: &Self::Layer,
            run_id: RunId,
            deadline: &AdaptiveOperationDeadline,
        ) -> Result<(), AdaptiveError> {
            self.inner.cancel_layer_flow(layer, run_id, deadline).await
        }

        async fn cleanup_layer(
            &mut self,
            _layer: &AdaptiveLayerLease<Self::Layer>,
            _layer_id: &LayerId,
            _attempt: u64,
            deadline: &AdaptiveOperationDeadline,
        ) -> Result<AdaptiveLayerCleanup, AdaptiveError> {
            self.retained_cleanup_calls = self.retained_cleanup_calls.saturating_add(1);
            tokio_time::sleep(deadline.cleanup_remaining()).await;
            Ok(AdaptiveLayerCleanup::Retained(
                crate::AdaptiveLayerDisposition::Retained,
            ))
        }
    }

    #[test]
    fn compose_policy_tightens_limits_and_intersects_allow_lists() {
        let pack = AdaptivePolicy {
            limits: limits(10),
            allowed_model_classes: BTreeSet::from(["frontier".to_string(), "mini".to_string()]),
            allowed_tool_classes: BTreeSet::from(["read".to_string(), "shell".to_string()]),
            allow_inline_profiles: true,
            ..AdaptivePolicy::default()
        };
        let host = AdaptivePolicy {
            limits: limits(4),
            allowed_model_classes: BTreeSet::from(["mini".to_string()]),
            allowed_tool_classes: BTreeSet::from(["read".to_string()]),
            allow_inline_profiles: false,
            ..AdaptivePolicy::default()
        };

        let composed = AdaptivePolicy::compose(&pack, &host).unwrap();
        assert_eq!(composed.limits.max_depth, 4);
        assert_eq!(
            composed.allowed_model_classes,
            BTreeSet::from(["mini".to_string()])
        );
        assert_eq!(
            composed.allowed_tool_classes,
            BTreeSet::from(["read".to_string()])
        );
        assert!(!composed.allow_inline_profiles);
    }

    #[test]
    fn incomplete_policy_fails_closed() {
        let err = limits(0).validate_complete("pack").unwrap_err();
        assert!(matches!(err, AdaptiveError::IncompletePolicy { .. }));
    }

    #[test]
    fn driver_initialization_limits_map_policy_to_deadline_payload() {
        let mut limits = limits(7);
        limits.max_wall_clock_ms = 7;
        let policy = AdaptivePolicy {
            limits,
            ..AdaptivePolicy::default()
        };
        let runtime_limits = adaptive_run_limits_from_policy(&policy, 1_000).unwrap();
        assert_eq!(runtime_limits.max_depth, 7);
        assert_eq!(runtime_limits.max_total_decisions, 7);
        assert_eq!(runtime_limits.max_members_per_layer, 7);
        assert_eq!(runtime_limits.max_total_spawned_members, 7);
        assert_eq!(runtime_limits.deadline_ms, 1_007);
    }

    #[test]
    fn adaptive_refs_parse_once_into_typed_forms() {
        assert_eq!(
            AdaptiveRef::parse("previous_layer.result.findings").unwrap(),
            AdaptiveRef::PreviousLayerResult(vec!["findings".to_string()])
        );
        assert!(AdaptiveRef::parse("previous_layer..findings").is_err());
    }

    #[test]
    fn body_store_hashes_and_validates_bodies() {
        let mut store = InMemoryBodyStore::default();
        let digest = store
            .put_json(&serde_json::json!({"answer": 42}))
            .expect("put body");
        let loaded = store.get_json(&digest).expect("load body");
        assert_eq!(loaded["answer"], 42);
    }

    #[tokio::test]
    async fn adaptive_loop_cancels_initialized_run_after_planning_error() {
        let kernel = FakeKernel::default();
        let run_cancellations = Arc::clone(&kernel.run_cancellations);
        let mut runtime = PlanningExitRuntime {
            exit: PlanningExit::Error,
            planning_cancellations: Arc::new(AtomicUsize::new(0)),
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-planning-error").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Exercise planning failure closure.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("planning failure must remain visible");

        assert!(error.to_string().contains("injected planning failure"));
        assert_eq!(
            run_cancellations.load(Ordering::SeqCst),
            1,
            "ordinary post-initialization errors must request run cancellation exactly once"
        );
    }

    #[tokio::test]
    async fn aborting_adaptive_loop_before_layer_hands_run_to_cancellation_owner() {
        let planning_entered = Arc::new(tokio::sync::Notify::new());
        let run_cancellations = Arc::new(AtomicUsize::new(0));
        let task_planning_entered = Arc::clone(&planning_entered);
        let task_run_cancellations = Arc::clone(&run_cancellations);
        let task = tokio::spawn(async move {
            let kernel = FakeKernel {
                run_cancellations: task_run_cancellations,
                ..FakeKernel::default()
            };
            let mut runtime = PlanningExitRuntime {
                exit: PlanningExit::Pending(task_planning_entered),
                planning_cancellations: Arc::new(AtomicUsize::new(0)),
            };
            run_adaptive_loop(
                &kernel,
                &mut runtime,
                AdaptiveRunRequest {
                    adaptive_run_id: AdaptiveRunId::new("run-cancel-before-layer").unwrap(),
                    policy: AdaptivePolicy {
                        limits: limits(10),
                        ..AdaptivePolicy::default()
                    },
                    compile_context: compile_context(),
                    objective: "Exercise pre-layer cancellation ownership.".to_string(),
                    started_at_ms: 1_000,
                },
            )
            .await
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            planning_entered.notified(),
        )
        .await
        .expect("adaptive loop must enter planning after run initialization");
        task.abort();
        assert!(
            task.await
                .expect_err("adaptive loop task must abort")
                .is_cancelled(),
            "join error must report cancellation"
        );
        assert_eq!(
            run_cancellations.load(Ordering::SeqCst),
            1,
            "future cancellation must synchronously hand the run to its prestarted owner"
        );
    }

    #[tokio::test]
    async fn aborting_adaptive_loop_while_initialize_reply_is_pending_preserves_run_owner() {
        let command_enqueued = Arc::new(tokio::sync::Notify::new());
        let reply_release = Arc::new(tokio::sync::Notify::new());
        let run_cancellations = Arc::new(AtomicUsize::new(0));
        let task_command_enqueued = Arc::clone(&command_enqueued);
        let task_reply_release = Arc::clone(&reply_release);
        let task_run_cancellations = Arc::clone(&run_cancellations);

        let task = tokio::spawn(async move {
            let kernel = FakeKernel {
                run_cancellations: task_run_cancellations,
                initialization_gate: Some(FakeInitializationGate {
                    command_enqueued: task_command_enqueued,
                    reply_release: task_reply_release,
                }),
                ..FakeKernel::default()
            };
            let mut runtime = PlanningExitRuntime {
                exit: PlanningExit::Error,
                planning_cancellations: Arc::new(AtomicUsize::new(0)),
            };
            run_adaptive_loop(
                &kernel,
                &mut runtime,
                AdaptiveRunRequest {
                    adaptive_run_id: AdaptiveRunId::new("run-cancel-before-init-reply").unwrap(),
                    policy: AdaptivePolicy {
                        limits: limits(10),
                        ..AdaptivePolicy::default()
                    },
                    compile_context: compile_context(),
                    objective: "Exercise initialization-reply cancellation ownership.".to_string(),
                    started_at_ms: 1_000,
                },
            )
            .await
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            command_enqueued.notified(),
        )
        .await
        .expect("initialization owner must enqueue the machine command");
        task.abort();
        assert!(
            task.await
                .expect_err("adaptive loop task must abort while initialization is pending")
                .is_cancelled()
        );
        assert_eq!(
            run_cancellations.load(Ordering::SeqCst),
            0,
            "cancellation must wait for machine acceptance instead of treating an absent pre-init snapshot as terminal"
        );

        // Model the actor applying InitializeAdaptiveRun and replying after the
        // surface future has disappeared. The independent initialization owner
        // receives that acceptance, arms the lease, and failed publication
        // transfers the run to cancellation.
        reply_release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while run_cancellations.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("accepted run must reach its surviving cancellation owner");
        assert_eq!(
            run_cancellations.load(Ordering::SeqCst),
            1,
            "abort-before-reply must transfer exactly one accepted run to cancellation"
        );

        let accepted_but_unreceived_cancellations = Arc::new(AtomicUsize::new(0));
        let accepted_but_unreceived = AdaptiveRunInitialization::completed(
            (),
            fake_run_lease(Arc::clone(&accepted_but_unreceived_cancellations)),
        );
        drop(accepted_but_unreceived);
        assert_eq!(
            accepted_but_unreceived_cancellations.load(Ordering::SeqCst),
            1,
            "a successful machine reply queued before caller receipt must remain guarded by the published lease"
        );
    }

    #[tokio::test]
    async fn adaptive_loop_records_finish_decision_through_kernel() {
        let kernel = FakeKernel::default();
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::Finish {
                reason: "enough evidence".to_string(),
                result: FinishResult {
                    result: serde_json::json!({"summary": "done"}),
                },
            }]),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: None,
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let outcome = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Summarize the evidence.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect("adaptive loop should finish");

        assert_eq!(
            outcome.final_result,
            Some(serde_json::json!({"summary": "done"}))
        );
        assert_eq!(
            kernel.events(),
            vec!["initialize:run-1", "decision:finish", "finish"]
        );
    }

    #[tokio::test]
    async fn adaptive_loop_cancels_initialized_run_after_pre_layer_error() {
        let kernel = FakeKernel::default();
        let mut runtime = FakeRuntime {
            decisions: VecDeque::new(),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: Some("planning failed".to_string()),
            provision_failure: None,
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Audit provider auth.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("planning failure must fail the adaptive loop");

        assert!(matches!(
            error,
            AdaptiveError::DriverRuntime(ref message) if message == "planning failed"
        ));
        assert_eq!(kernel.events(), vec!["initialize:run-1", "cancel"]);
    }

    #[tokio::test]
    async fn adaptive_loop_retries_cancel_until_acknowledged() {
        let kernel = FakeKernel {
            cancel_failures_remaining: Mutex::new(1),
            ..FakeKernel::default()
        };
        let mut runtime = FakeRuntime {
            decisions: VecDeque::new(),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: Some("planning failed".to_string()),
            provision_failure: None,
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Audit provider auth.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("planning failure must fail the adaptive loop");

        match error {
            AdaptiveError::OperationFailedWithCleanup { primary, cleanup } => {
                assert!(matches!(
                    *primary,
                    AdaptiveError::DriverRuntime(ref message) if message == "planning failed"
                ));
                assert!(cleanup.contains("cancel acknowledgement failed"));
            }
            other => panic!("expected cancellation retry diagnostics, got {other:?}"),
        }
        assert_eq!(
            kernel.events(),
            vec!["initialize:run-1", "cancel", "cancel"]
        );
    }

    #[tokio::test]
    async fn adaptive_loop_cancels_and_disposes_ambiguous_admission() {
        let kernel = FakeKernel {
            fail_admission: true,
            ..FakeKernel::default()
        };
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::RunLayer {
                reason: "verify findings".to_string(),
                plan: layer_plan(),
            }]),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: None,
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Audit provider auth.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("admission acknowledgement failure must fail the adaptive loop");

        assert!(matches!(
            error,
            AdaptiveError::DriverRuntime(ref message)
                if message == "admission acknowledgement failed"
        ));
        assert_eq!(runtime.cleanup_calls, 0);
        assert_eq!(
            kernel.events(),
            vec![
                "initialize:run-1",
                "decision:run_layer",
                "admission:run-1-verify-findings",
                "cancel",
                "destroyed:run-1-verify-findings",
            ]
        );
    }

    #[tokio::test]
    async fn adaptive_loop_keeps_failed_provision_child_owned_until_cleanup() {
        let kernel = FakeKernel::default();
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::RunLayer {
                reason: "verify findings".to_string(),
                plan: layer_plan(),
            }]),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: Some(FakeProvisionFailure::WithChild { spawned_members: 1 }),
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Audit provider auth.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("provision failure must fail the adaptive loop");

        assert!(matches!(
            error,
            AdaptiveError::DriverRuntime(ref message) if message == "provision failed with child"
        ));
        assert_eq!(runtime.cleanup_calls, 1);
        assert_eq!(
            kernel.events(),
            vec![
                "initialize:run-1",
                "decision:run_layer",
                "admission:run-1-verify-findings",
                "setup_fault:run-1-verify-findings",
                "destroyed:run-1-verify-findings",
                "cancel",
            ]
        );
    }

    #[tokio::test]
    async fn adaptive_loop_records_absent_disposition_after_pre_child_provision_failure() {
        let kernel = FakeKernel::default();
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::RunLayer {
                reason: "verify findings".to_string(),
                plan: layer_plan(),
            }]),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: Some(FakeProvisionFailure::BeforeChild),
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Audit provider auth.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("provision failure must fail the adaptive loop");

        assert!(matches!(
            error,
            AdaptiveError::DriverRuntime(ref message)
                if message == "provision failed before child"
        ));
        assert_eq!(runtime.cleanup_calls, 0);
        assert_eq!(
            kernel.events(),
            vec![
                "initialize:run-1",
                "decision:run_layer",
                "admission:run-1-verify-findings",
                "setup_fault:run-1-verify-findings",
                "destroyed:run-1-verify-findings",
                "cancel",
            ]
        );
    }

    #[tokio::test]
    async fn adaptive_loop_runs_layer_validates_result_and_feeds_next_planning_turn() {
        let plan = layer_plan();
        let kernel = FakeKernel::default();
        let layer_output = serde_json::json!({"verifications": [{"id": "F-1", "ok": true}]});
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([
                LayerDecision::RunLayer {
                    reason: "verify findings".to_string(),
                    plan: plan.clone(),
                },
                LayerDecision::Finish {
                    reason: "verified".to_string(),
                    result: FinishResult {
                        result: serde_json::json!({"summary": "verified"}),
                    },
                },
            ]),
            layer_run: Some(completed_layer_run(layer_output)),
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: None,
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let outcome = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-1").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Audit provider auth.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect("adaptive loop should run layer then finish");

        assert_eq!(
            outcome.final_result,
            Some(serde_json::json!({"summary": "verified"}))
        );
        assert!(runtime.saw_previous_layer_result);
        assert_eq!(
            kernel.events(),
            vec![
                "initialize:run-1",
                "decision:run_layer",
                "admission:run-1-verify-findings",
                "provisioned:run-1-verify-findings",
                "run_started:run-1-verify-findings",
                "terminal:run-1-verify-findings",
                "result_valid:run-1-verify-findings",
                "destroyed:run-1-verify-findings",
                "decision:finish",
                "finish",
            ]
        );
    }

    #[tokio::test]
    async fn adaptive_loop_cleans_acquired_layer_after_partial_provision_failure() {
        let kernel = FakeKernel::default();
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::RunLayer {
                reason: "exercise partial provision".to_string(),
                plan: layer_plan(),
            }]),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: Some(FakeProvisionFailure::WithChild { spawned_members: 1 }),
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-partial-provision").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Exercise cleanup.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("partial provision must fail");

        assert!(error.to_string().contains("provision failed with child"));
        assert_eq!(runtime.cleanup_calls, 1);
        assert!(kernel.events().ends_with(&[
            "setup_fault:run-partial-provision-verify-findings".to_string(),
            "destroyed:run-partial-provision-verify-findings".to_string(),
            "cancel".to_string(),
        ]));
    }

    #[tokio::test]
    async fn adaptive_loop_cleans_acquired_layer_after_start_failure() {
        let kernel = FakeKernel::default();
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::RunLayer {
                reason: "exercise start failure".to_string(),
                plan: layer_plan(),
            }]),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: None,
            start_error: Some("injected layer start failure".to_string()),
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-start-failure").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Exercise cleanup.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("start failure must fail");

        assert!(error.to_string().contains("injected layer start failure"));
        assert_eq!(runtime.cleanup_calls, 1);
        assert!(kernel.events().ends_with(&[
            "provisioned:run-start-failure-verify-findings".to_string(),
            "interrupted:run-start-failure-verify-findings".to_string(),
            "destroyed:run-start-failure-verify-findings".to_string(),
            "cancel".to_string(),
        ]));
    }

    #[tokio::test]
    async fn aborting_adaptive_loop_after_ready_hands_layer_to_cancellation_owner() {
        let terminal_wait_entered = Arc::new(tokio::sync::Notify::new());
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let task_wait_entered = Arc::clone(&terminal_wait_entered);
        let task_cleanup_calls = Arc::clone(&cleanup_calls);
        let task = tokio::spawn(async move {
            let kernel = FakeKernel::default();
            let mut runtime = PendingCancellationRuntime {
                decision: Some(LayerDecision::RunLayer {
                    reason: "exercise future cancellation".to_string(),
                    plan: layer_plan(),
                }),
                terminal_wait_entered: task_wait_entered,
                cleanup_calls: task_cleanup_calls,
                flow_cancellations: Arc::new(AtomicUsize::new(0)),
            };
            run_adaptive_loop(
                &kernel,
                &mut runtime,
                AdaptiveRunRequest {
                    adaptive_run_id: AdaptiveRunId::new("run-cancel-after-ready").unwrap(),
                    policy: AdaptivePolicy {
                        limits: limits(10),
                        ..AdaptivePolicy::default()
                    },
                    compile_context: compile_context(),
                    objective: "Exercise cancellation ownership.".to_string(),
                    started_at_ms: 1_000,
                },
            )
            .await
        });

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            terminal_wait_entered.notified(),
        )
        .await
        .expect("adaptive loop must reach its post-Ready terminal await");
        task.abort();
        assert!(
            task.await
                .expect_err("adaptive loop task must abort")
                .is_cancelled(),
            "join error must report cancellation"
        );
        assert_eq!(
            cleanup_calls.load(Ordering::SeqCst),
            1,
            "dropping the public adaptive-loop future must synchronously hand off its layer"
        );
    }

    #[tokio::test]
    async fn adaptive_loop_records_cleanup_after_committed_terminal_response_failure() {
        let kernel = FakeKernel {
            fail_result_validated_after_record: true,
            ..FakeKernel::default()
        };
        let layer_output = serde_json::json!({"verifications": [{"id": "F-1", "ok": true}]});
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::RunLayer {
                reason: "exercise committed response loss".to_string(),
                plan: layer_plan(),
            }]),
            layer_run: Some(completed_layer_run(layer_output)),
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: None,
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("run-response-loss").unwrap(),
                policy: AdaptivePolicy {
                    limits: limits(10),
                    ..AdaptivePolicy::default()
                },
                compile_context: compile_context(),
                objective: "Exercise cleanup accounting.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("lost terminal response must remain visible");

        assert!(error.to_string().contains("committed response failure"));
        assert_eq!(runtime.cleanup_calls, 1);
        assert!(kernel.events().ends_with(&[
            "result_valid:run-response-loss-verify-findings".to_string(),
            "interrupted:run-response-loss-verify-findings".to_string(),
            "destroyed:run-response-loss-verify-findings".to_string(),
            "cancel".to_string(),
        ]));
    }

    #[tokio::test(start_paused = true)]
    async fn adaptive_deadline_cancels_stuck_planning_custody_before_terminalizing() {
        let kernel = FakeKernel::default();
        let planning_cancellations = Arc::new(AtomicUsize::new(0));
        let mut runtime = PlanningExitRuntime {
            exit: PlanningExit::Pending(Arc::new(tokio::sync::Notify::new())),
            planning_cancellations: Arc::clone(&planning_cancellations),
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("deadline-stuck-planning").unwrap(),
                policy: deadline_policy(10),
                compile_context: compile_context(),
                objective: "Bound a stuck planning flow.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("the adaptive deadline must terminate a stuck planning turn");

        assert!(matches!(
            error,
            AdaptiveError::DeadlineExceeded {
                stage: AdaptiveRuntimeStage::Planning,
                ..
            }
        ));
        assert_eq!(
            kernel.events(),
            vec!["initialize:deadline-stuck-planning", "deadline"]
        );
        assert_eq!(
            planning_cancellations.load(Ordering::SeqCst),
            1,
            "the outer adaptive deadline must cancel custody of the exact planning flow"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn adaptive_deadline_bounds_stuck_machine_authority_acknowledgement() {
        let kernel = FakeKernel {
            planning_decision_pending: true,
            ..FakeKernel::default()
        };
        let mut runtime = FakeRuntime {
            decisions: VecDeque::from([LayerDecision::Finish {
                reason: "exercise machine acknowledgement deadline".to_string(),
                result: FinishResult {
                    result: serde_json::json!({"ok": true}),
                },
            }]),
            layer_run: None,
            saw_previous_layer_result: false,
            planning_error: None,
            provision_failure: None,
            start_error: None,
            await_error: None,
            cleanup_error: None,
            cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("deadline-machine-ack").unwrap(),
                policy: deadline_policy(10),
                compile_context: compile_context(),
                objective: "Bound a stuck machine acknowledgement.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("machine authority acknowledgement must obey the adaptive deadline");

        assert!(matches!(
            error,
            AdaptiveError::DeadlineExceeded {
                stage: AdaptiveRuntimeStage::PlanningDecision,
                ..
            }
        ));
        assert!(
            kernel
                .events()
                .ends_with(&["decision:finish".to_string(), "deadline".to_string(),])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn adaptive_deadline_cancels_stuck_child_and_drains_layer_before_terminalizing() {
        let kernel = FakeKernel::default();
        let flow_cancellations = Arc::new(AtomicUsize::new(0));
        let cleanup_calls = Arc::new(AtomicUsize::new(0));
        let mut runtime = PendingCancellationRuntime {
            decision: Some(LayerDecision::RunLayer {
                reason: "exercise child deadline".to_string(),
                plan: layer_plan(),
            }),
            terminal_wait_entered: Arc::new(tokio::sync::Notify::new()),
            cleanup_calls: Arc::clone(&cleanup_calls),
            flow_cancellations: Arc::clone(&flow_cancellations),
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("deadline-stuck-child").unwrap(),
                policy: deadline_policy(10),
                compile_context: compile_context(),
                objective: "Bound a stuck child flow.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("the adaptive deadline must terminate a stuck child flow");

        assert!(matches!(
            error,
            AdaptiveError::DeadlineExceeded {
                stage: AdaptiveRuntimeStage::LayerTerminal,
                ..
            }
        ));
        assert_eq!(flow_cancellations.load(Ordering::SeqCst), 1);
        assert_eq!(cleanup_calls.load(Ordering::SeqCst), 1);
        assert!(kernel.events().ends_with(&[
            "interrupted:deadline-stuck-child-verify-findings".to_string(),
            "destroyed:deadline-stuck-child-verify-findings".to_string(),
            "deadline".to_string(),
        ]));
    }

    #[tokio::test(start_paused = true)]
    async fn adaptive_deadline_records_retained_custody_when_destroy_keeps_retrying() {
        let kernel = FakeKernel::default();
        let output = serde_json::json!({"verifications": [{"id": "F-1", "ok": true}]});
        let mut runtime = RetryableDestroyRuntime {
            inner: FakeRuntime {
                decisions: VecDeque::from([LayerDecision::RunLayer {
                    reason: "exercise bounded destroy".to_string(),
                    plan: layer_plan(),
                }]),
                layer_run: Some(completed_layer_run(output)),
                saw_previous_layer_result: false,
                planning_error: None,
                provision_failure: None,
                start_error: None,
                await_error: None,
                cleanup_error: None,
                cleanup_calls: 0,
            },
            retained_cleanup_calls: 0,
        };

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("deadline-retryable-destroy").unwrap(),
                policy: deadline_policy(10),
                compile_context: compile_context(),
                objective: "Bound retryable physical cleanup.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("cleanup retries must not outrun the adaptive deadline");

        assert!(matches!(
            error,
            AdaptiveError::DeadlineExceeded {
                stage: AdaptiveRuntimeStage::LayerCleanup,
                ..
            }
        ));
        assert_eq!(runtime.retained_cleanup_calls, 1);
        assert!(kernel.events().ends_with(&[
            "retained:deadline-retryable-destroy-verify-findings".to_string(),
            "deadline".to_string(),
        ]));
    }

    #[tokio::test(start_paused = true)]
    async fn adaptive_run_cancel_ack_retries_are_bounded_by_terminalization_window() {
        let kernel = FakeKernel {
            cancel_failures_remaining: Mutex::new(u64::MAX),
            ..FakeKernel::default()
        };
        let mut runtime = PlanningExitRuntime {
            exit: PlanningExit::Error,
            planning_cancellations: Arc::new(AtomicUsize::new(0)),
        };
        let started = tokio_time::Instant::now();

        let error = run_adaptive_loop(
            &kernel,
            &mut runtime,
            AdaptiveRunRequest {
                adaptive_run_id: AdaptiveRunId::new("bounded-missing-cancel-ack").unwrap(),
                policy: deadline_policy(10),
                compile_context: compile_context(),
                objective: "Bound a missing cancellation acknowledgement.".to_string(),
                started_at_ms: 1_000,
            },
        )
        .await
        .expect_err("missing cancellation acknowledgement must remain visible");

        assert!(error.to_string().contains("cancel acknowledgement failed"));
        assert!(
            tokio_time::Instant::now().duration_since(started)
                <= Duration::from_millis(10) + ADAPTIVE_TERMINALIZATION_GRACE
        );
        let cancel_attempts = kernel
            .events()
            .into_iter()
            .filter(|event| event == "cancel")
            .count();
        assert!(cancel_attempts > 1);
        assert!(cancel_attempts < 100);
    }

    #[test]
    fn child_mob_id_is_separator_safe_and_attempt_scoped() {
        let run = AdaptiveRunId::new("run-1").unwrap();
        let layer = LayerId::new("verify-findings").unwrap();
        let first = derive_child_mob_id(&run, &layer, 1).unwrap();
        let second = derive_child_mob_id(&run, &layer, 2).unwrap();
        assert_ne!(first, second);
        assert!(!first.as_str().contains('/'));
        assert!(!first.as_str().contains('.'));
    }

    #[test]
    fn fan_out_collect_compiles_envelope_wrapped_collector_schema_and_spawn_group() {
        let plan = LayerPlan {
            id: LayerId::new("verify-findings").unwrap(),
            objective: "Verify each candidate finding independently.".to_string(),
            shape: LayerShape::FanOutCollect {
                worker_role: ProfileName::from("verifier"),
                collection: LayerCollection::All,
            },
            spawn: Vec::new(),
            spawn_groups: vec![LayerSpawnGroup {
                prefix: "verifier".to_string(),
                profile: ProfileName::from("verifier"),
                items_ref: "previous_layer.result.findings".to_string(),
                key_path: "id".to_string(),
                initial_message_template: "Verify finding {{ item.id }}: {{ item.title }}"
                    .to_string(),
                max_items: 8,
            }],
            profiles: BTreeMap::from([
                (
                    ProfileName::from("verifier"),
                    LayerProfile::Template {
                        template: ProfileName::from("verifier"),
                    },
                ),
                (
                    ProfileName::from("collector"),
                    LayerProfile::Template {
                        template: ProfileName::from("collector"),
                    },
                ),
            ]),
            collector: CollectorContract {
                profile: ProfileName::from("collector"),
                output_schema: SchemaRef::Registry {
                    registry: SchemaName::new("verification-set").unwrap(),
                },
            },
            activation_params: BTreeMap::from([(
                "findings".to_string(),
                AdaptiveValue::Ref {
                    r#ref: "previous_layer.result.findings".to_string(),
                },
            )]),
        };

        let compiled = compile_layer(&plan, &compile_context(), &compile_policy()).unwrap();
        assert_eq!(
            compiled.child_mob_id.as_str(),
            "adaptive-run-1-verify-findings-a1"
        );
        assert_eq!(
            compiled.policy_evidence.used_model_classes,
            BTreeSet::from(["gpt-5.5".to_string()])
        );
        assert_eq!(compiled.spawn_specs.len(), 3);
        assert_eq!(
            compiled
                .activation_params
                .get("findings")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(2)
        );

        let flow = compiled
            .definition
            .flows
            .get(&FlowId::from("layer-flow"))
            .expect("layer flow");
        let collect = flow.steps.get(&StepId::from("collect")).unwrap();
        assert_eq!(collect.dispatch_mode, DispatchMode::FanIn);
        let Some(FlowSchemaRef::Inline(schema)) = &collect.expected_schema_ref else {
            panic!("collector schema must be inline");
        };
        assert_eq!(schema.as_value()["type"], "array");
        assert_eq!(
            schema.as_value()["items"]["properties"]["output"]["required"][0],
            "verifications"
        );
        let serialized = serde_json::to_value(&compiled.definition).unwrap();
        let serialized_text = serialized.to_string();
        assert!(!serialized_text.contains("previous_layer.result"));
        assert!(!serialized_text.contains("schema:"));
    }

    #[test]
    fn solo_shape_records_bare_schema_with_collection_any() {
        let plan = LayerPlan {
            id: LayerId::new("final-report").unwrap(),
            objective: "Write the final report.".to_string(),
            shape: LayerShape::Solo,
            spawn: Vec::new(),
            spawn_groups: Vec::new(),
            profiles: BTreeMap::from([(
                ProfileName::from("collector"),
                LayerProfile::Template {
                    template: ProfileName::from("collector"),
                },
            )]),
            collector: CollectorContract {
                profile: ProfileName::from("collector"),
                output_schema: SchemaRef::Registry {
                    registry: SchemaName::new("verification-set").unwrap(),
                },
            },
            activation_params: BTreeMap::new(),
        };

        let compiled = compile_layer(&plan, &compile_context(), &compile_policy()).unwrap();
        let flow = compiled
            .definition
            .flows
            .get(&FlowId::from("layer-flow"))
            .expect("layer flow");
        let step = flow.steps.get(&StepId::from("produce")).unwrap();
        assert_eq!(step.collection_policy, CollectionPolicy::Any);
        let Some(FlowSchemaRef::Inline(schema)) = &step.expected_schema_ref else {
            panic!("solo schema must be inline");
        };
        assert_eq!(schema.as_value()["type"], "object");
    }

    #[test]
    fn inline_profiles_require_explicit_policy_authority() {
        let mut plan = layer_plan();
        plan.profiles.insert(
            ProfileName::from("verifier"),
            LayerProfile::Inline {
                inline: Box::new(profile()),
            },
        );
        let denied = compile_layer(&plan, &compile_context(), &compile_policy()).unwrap_err();
        assert!(matches!(
            denied,
            AdaptiveError::InlineProfilesDisabled { profile } if profile == "verifier"
        ));

        let mut policy = compile_policy();
        policy.allow_inline_profiles = true;
        compile_layer(&plan, &compile_context(), &policy)
            .expect("inline profile should compile with explicit adaptive policy authority");
    }

    #[test]
    fn adaptive_policy_canonicalizes_tool_and_skill_allowlists() {
        let policy = AdaptivePolicy {
            limits: limits(7),
            allowed_tool_classes: BTreeSet::from([
                "builtins".to_string(),
                "mcp:filesystem".to_string(),
                "rust_bundle:review-tools".to_string(),
            ]),
            allowed_skill_classes: BTreeSet::from(["lint-review".to_string()]),
            ..AdaptivePolicy::default()
        };

        let runtime_limits = adaptive_run_limits_from_policy(&policy, 1_000).unwrap();
        assert_eq!(
            runtime_limits.allowed_tool_classes,
            BTreeSet::from([
                "builtins".to_string(),
                "mcp:filesystem".to_string(),
                "rust_bundle:review-tools".to_string(),
            ])
        );
        assert_eq!(
            runtime_limits.allowed_skill_identities,
            BTreeSet::from(["lint-review".to_string()])
        );
    }

    #[test]
    fn adaptive_policy_preserves_already_canonical_namespaced_tool_allowlists() {
        let pack = AdaptivePolicy {
            limits: limits(7),
            allowed_tool_classes: BTreeSet::from([
                "mcp:filesystem".to_string(),
                "rust_bundle:review-tools".to_string(),
            ]),
            allowed_skill_classes: BTreeSet::from(["lint-review".to_string()]),
            ..AdaptivePolicy::default()
        };
        let host = AdaptivePolicy {
            limits: limits(5),
            allowed_tool_classes: BTreeSet::from([
                "mcp:filesystem".to_string(),
                "rust_bundle:review-tools".to_string(),
                "builtins".to_string(),
            ]),
            allowed_skill_classes: BTreeSet::from([
                "lint-review".to_string(),
                "docs-review".to_string(),
            ]),
            ..AdaptivePolicy::default()
        };

        let composed = AdaptivePolicy::compose(&pack, &host).unwrap();
        let runtime_limits = adaptive_run_limits_from_policy(&composed, 1_000).unwrap();
        assert_eq!(
            runtime_limits.allowed_tool_classes,
            BTreeSet::from([
                "mcp:filesystem".to_string(),
                "rust_bundle:review-tools".to_string(),
            ])
        );
        assert_eq!(
            runtime_limits.allowed_skill_identities,
            BTreeSet::from(["lint-review".to_string()])
        );
    }

    #[test]
    fn adaptive_layer_evidence_canonicalizes_profile_tools_and_skills() {
        let mut context = compile_context();
        let verifier = context
            .profile_templates
            .get_mut(&ProfileName::from("verifier"))
            .expect("verifier profile");
        verifier.tools.mcp = vec!["filesystem".to_string()];
        verifier.tools.rust_bundles = vec!["review-tools".to_string()];
        verifier.skills = vec!["lint-review".to_string()];

        let compiled = compile_layer(&layer_plan(), &context, &compile_policy()).unwrap();
        assert!(
            compiled
                .policy_evidence
                .used_tool_classes
                .contains("mcp:filesystem")
        );
        assert!(
            compiled
                .policy_evidence
                .used_tool_classes
                .contains("rust_bundle:review-tools")
        );
        assert!(
            compiled
                .policy_evidence
                .used_skill_identities
                .contains("lint-review")
        );
    }

    #[test]
    fn adaptive_layer_rejects_ambiguous_profile_tool_and_skill_identities() {
        let mut context = compile_context();
        context
            .profile_templates
            .get_mut(&ProfileName::from("verifier"))
            .expect("verifier profile")
            .tools
            .mcp = vec!["mcp:filesystem".to_string()];
        let denied = compile_layer(&layer_plan(), &context, &compile_policy()).unwrap_err();
        assert!(matches!(
            denied,
            AdaptiveError::InvalidAdaptiveIdentity { field, .. }
                if field == "adaptive tool mcp server"
        ));

        let mut context = compile_context();
        context
            .profile_templates
            .get_mut(&ProfileName::from("verifier"))
            .expect("verifier profile")
            .skills = vec!["mcp:filesystem".to_string()];
        let denied = compile_layer(&layer_plan(), &context, &compile_policy()).unwrap_err();
        assert!(matches!(
            denied,
            AdaptiveError::InvalidAdaptiveIdentity { field, reason, .. }
                if field == "adaptive skill identity" && reason.contains("reserved")
        ));
    }

    #[test]
    fn scoped_layer_ids_are_run_unique_without_mutating_planner_ids() {
        let layer = LayerId::new("verify-findings").unwrap();
        let first = scoped_layer_id(&AdaptiveRunId::new("run-a").unwrap(), &layer).unwrap();
        let second = scoped_layer_id(&AdaptiveRunId::new("run-b").unwrap(), &layer).unwrap();
        assert_ne!(first, second);
        assert_eq!(first.as_str(), "run-a-verify-findings");
        assert_eq!(layer.as_str(), "verify-findings");
    }
}

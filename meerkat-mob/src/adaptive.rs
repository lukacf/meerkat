use crate::definition::{
    CollectionPolicy, DispatchMode, FlowSchemaRef, FlowSpec, FlowStepSpec, OrchestratorConfig,
    RoleWiringRule, StepOutputFormat, WiringRules,
};
use crate::ids::{FlowId, MobId, ProfileName, StepId};
use crate::profile::{Profile, ProfileBinding};
use crate::{MobDefinition, MobHandle, MobRun, RunId, SpawnMemberSpec};
use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::schema::MeerkatSchema;
use meerkat_core::types::ContentInput;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

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

    pub async fn initialize_run(
        &self,
        adaptive_run_id: &AdaptiveRunId,
        policy: &AdaptivePolicy,
        started_at_ms: u64,
    ) -> Result<crate::AdaptiveDriverCapability, AdaptiveError> {
        let limits = adaptive_run_limits_from_policy(policy, started_at_ms)?;
        Ok(self
            .control_mob
            .initialize_adaptive_run(crate::InitializeAdaptiveRunRequest {
                adaptive_run_id: adaptive_run_id.as_str().to_string(),
                limits,
            })
            .await?)
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
    pub objective: String,
    pub previous_layer_result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveLayerCleanup {
    Destroyed,
    Retained(crate::AdaptiveLayerDisposition),
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AdaptiveKernel {
    type Capability: Send + Sync;

    async fn initialize_run(
        &self,
        adaptive_run_id: &AdaptiveRunId,
        policy: &AdaptivePolicy,
        started_at_ms: u64,
    ) -> Result<Self::Capability, AdaptiveError>;

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
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AdaptiveKernel for AdaptiveDriver {
    type Capability = crate::AdaptiveDriverCapability;

    async fn initialize_run(
        &self,
        adaptive_run_id: &AdaptiveRunId,
        policy: &AdaptivePolicy,
        started_at_ms: u64,
    ) -> Result<Self::Capability, AdaptiveError> {
        AdaptiveDriver::initialize_run(self, adaptive_run_id, policy, started_at_ms).await
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
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait AdaptiveDriverRuntime {
    type Layer: Send + Sync;

    fn now_ms(&mut self) -> u64;

    async fn run_planning_turn(
        &mut self,
        request: PlanningTurnRequest,
    ) -> Result<LayerDecision, AdaptiveError>;

    async fn provision_layer(
        &mut self,
        compiled: &CompiledLayer,
    ) -> Result<Self::Layer, AdaptiveError>;

    async fn start_layer_flow(
        &mut self,
        layer: &Self::Layer,
        activation_params: BTreeMap<String, serde_json::Value>,
    ) -> Result<RunId, AdaptiveError>;

    async fn await_layer_terminal(
        &mut self,
        layer: &Self::Layer,
        run_id: RunId,
    ) -> Result<MobRun, AdaptiveError>;

    async fn cleanup_layer(
        &mut self,
        layer: Self::Layer,
        layer_id: &LayerId,
        attempt: u64,
    ) -> Result<AdaptiveLayerCleanup, AdaptiveError>;
}

pub async fn run_adaptive_loop<K, R>(
    kernel: &K,
    runtime: &mut R,
    request: AdaptiveRunRequest,
) -> Result<AdaptiveRunOutcome, AdaptiveError>
where
    K: AdaptiveKernel + Sync,
    R: AdaptiveDriverRuntime + Send,
{
    let capability = kernel
        .initialize_run(
            &request.adaptive_run_id,
            &request.policy,
            request.started_at_ms,
        )
        .await?;
    let mut body_store = InMemoryBodyStore::default();
    let mut context = request.compile_context.clone();
    context.adaptive_run_id = request.adaptive_run_id.clone();
    let mut previous_layer_result = context.previous_layer_result.clone();

    loop {
        let decision = runtime
            .run_planning_turn(PlanningTurnRequest {
                adaptive_run_id: request.adaptive_run_id.clone(),
                objective: request.objective.clone(),
                previous_layer_result: previous_layer_result.clone(),
            })
            .await?;
        kernel
            .record_planning_decision(&capability, &decision)
            .await?;

        match decision {
            LayerDecision::Finish { result, .. } => {
                let digest = body_store.put_json(&result.result)?;
                kernel.resolve_finish(&capability, &digest).await?;
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
                        kernel
                            .record_plan_rejected(&capability, &scoped_layer_id)
                            .await?;
                        return Err(AdaptiveError::BodyDigestMismatch {
                            expected: plan_digest,
                            actual: compiled.plan_digest,
                        });
                    }
                    Err(error) => {
                        kernel
                            .record_plan_rejected(&capability, &scoped_layer_id)
                            .await?;
                        return Err(error);
                    }
                };
                let admission = kernel
                    .resolve_layer_admission(
                        &capability,
                        &scoped_layer_id,
                        context.attempt,
                        &compiled,
                        runtime.now_ms(),
                    )
                    .await?;
                if !matches!(admission, crate::AdaptiveLayerAdmission::Allowed) {
                    continue;
                }

                let layer = match runtime.provision_layer(&compiled).await {
                    Ok(layer) => layer,
                    Err(error) => {
                        kernel
                            .record_layer_setup_fault(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                                crate::AdaptiveLayerSetupFault::MobCreateFailed,
                                0,
                                compiled.spawn_specs.len() as u64,
                            )
                            .await?;
                        return Err(error);
                    }
                };
                kernel
                    .record_layer_provisioned(&capability, &scoped_layer_id, context.attempt)
                    .await?;
                let child_run_id = runtime
                    .start_layer_flow(&layer, compiled.activation_params.clone())
                    .await?;
                kernel
                    .record_layer_run_started(
                        &capability,
                        &scoped_layer_id,
                        context.attempt,
                        child_run_id.clone(),
                    )
                    .await?;
                let child_run = runtime.await_layer_terminal(&layer, child_run_id).await?;
                kernel
                    .ingest_layer_terminal(
                        &capability,
                        &scoped_layer_id,
                        context.attempt,
                        &child_run,
                    )
                    .await?;

                let layer_result = match extract_layer_result(&plan, &child_run)
                    .and_then(|value| validate_layer_result(&plan, &context.schema_registry, value))
                {
                    Ok(result) => result,
                    Err(error) => {
                        kernel
                            .record_layer_result_invalid(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                            )
                            .await?;
                        return Err(error);
                    }
                };
                let result_digest = body_store.put_json(&layer_result)?;
                kernel
                    .record_layer_result_validated(
                        &capability,
                        &scoped_layer_id,
                        context.attempt,
                        &result_digest,
                    )
                    .await?;

                match runtime
                    .cleanup_layer(layer, &scoped_layer_id, context.attempt)
                    .await?
                {
                    AdaptiveLayerCleanup::Destroyed => {
                        kernel
                            .record_layer_mob_destroyed(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                            )
                            .await?;
                    }
                    AdaptiveLayerCleanup::Retained(disposition) => {
                        kernel
                            .record_layer_mob_retained(
                                &capability,
                                &scoped_layer_id,
                                context.attempt,
                                disposition,
                            )
                            .await?;
                    }
                }

                previous_layer_result = Some(layer_result);
                context.attempt = context.attempt.saturating_add(1);
            }
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
    #[error("adaptive driver runtime failed: {0}")]
    DriverRuntime(String),
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
    use std::sync::Mutex;

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
            max_wall_clock_ms: value,
            max_aggregate_tokens: value,
            max_aggregate_tool_calls: value,
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
    }

    impl FakeKernel {
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        fn push(&self, event: impl Into<String>) {
            self.events.lock().unwrap().push(event.into());
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AdaptiveKernel for FakeKernel {
        type Capability = ();

        async fn initialize_run(
            &self,
            adaptive_run_id: &AdaptiveRunId,
            _policy: &AdaptivePolicy,
            _started_at_ms: u64,
        ) -> Result<Self::Capability, AdaptiveError> {
            self.push(format!("initialize:{}", adaptive_run_id.as_str()));
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
            Ok(())
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
    }

    struct FakeRuntime {
        decisions: VecDeque<LayerDecision>,
        layer_run: Option<MobRun>,
        saw_previous_layer_result: bool,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AdaptiveDriverRuntime for FakeRuntime {
        type Layer = String;

        fn now_ms(&mut self) -> u64 {
            1_050
        }

        async fn run_planning_turn(
            &mut self,
            request: PlanningTurnRequest,
        ) -> Result<LayerDecision, AdaptiveError> {
            if request.previous_layer_result.is_some() {
                self.saw_previous_layer_result = true;
            }
            Ok(self.decisions.pop_front().expect("planning decision"))
        }

        async fn provision_layer(
            &mut self,
            compiled: &CompiledLayer,
        ) -> Result<Self::Layer, AdaptiveError> {
            Ok(compiled.child_mob_id.to_string())
        }

        async fn start_layer_flow(
            &mut self,
            _layer: &Self::Layer,
            _activation_params: BTreeMap<String, serde_json::Value>,
        ) -> Result<RunId, AdaptiveError> {
            Ok(RunId::new())
        }

        async fn await_layer_terminal(
            &mut self,
            _layer: &Self::Layer,
            _run_id: RunId,
        ) -> Result<MobRun, AdaptiveError> {
            Ok(self.layer_run.take().expect("layer run"))
        }

        async fn cleanup_layer(
            &mut self,
            _layer: Self::Layer,
            _layer_id: &LayerId,
            _attempt: u64,
        ) -> Result<AdaptiveLayerCleanup, AdaptiveError> {
            Ok(AdaptiveLayerCleanup::Destroyed)
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
        let policy = AdaptivePolicy {
            limits: limits(7),
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

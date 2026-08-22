//! Host-injected consequence narrowing for dispatcher-backed tool calls.
//!
//! Static Meerkat execution policy remains authoritative. This module adds a
//! second, narrow-only check that runs only after static policy admits a call
//! and immediately before the owning dispatcher is entered.

use crate::{MobMemberBinding, RunId, ToolCallView, ToolName};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, PolicyIdentityError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(PolicyIdentityError::Empty {
                        kind: stringify!($name),
                    });
                }
                if value != value.trim() {
                    return Err(PolicyIdentityError::NonCanonical {
                        kind: stringify!($name),
                    });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_id!(PolicyProviderId);
string_id!(PolicyId);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyIdentityError {
    #[error("{kind} must not be empty")]
    Empty { kind: &'static str },
    #[error("{kind} must not contain leading or trailing whitespace")]
    NonCanonical { kind: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct PolicyRevision(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct PolicyProviderGeneration(pub u64);

/// Canonical SHA-256 digest of compiled policy bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct PolicyDigest(String);

impl PolicyDigest {
    pub fn from_canonical_bytes(bytes: &[u8]) -> Self {
        Self(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    pub fn parse(value: impl Into<String>) -> Result<Self, ToolConsequenceFailure> {
        let value = value.into();
        let Some(hex) = value.strip_prefix("sha256:") else {
            return Err(ToolConsequenceFailure::InvalidProvenance {
                reason: "policy digest must use the sha256:<lowercase-hex> form".to_string(),
            });
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ToolConsequenceFailure::InvalidProvenance {
                reason: "policy digest must contain exactly 64 lowercase hexadecimal digits"
                    .to_string(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PolicyEvaluationProvenance {
    pub revision: PolicyRevision,
    pub digest: PolicyDigest,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplicationToolPolicyBinding {
    #[default]
    Unmanaged,
    Inherit,
    Provider {
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
    },
}

impl<'de> Deserialize<'de> for ApplicationToolPolicyBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Serde's internally tagged unit variants accept and discard extra
        // map fields even with enum-level `deny_unknown_fields`. Parse the
        // units as empty struct variants so every binding shape stays strict.
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum StrictBinding {
            Unmanaged {},
            Inherit {},
            Provider {
                provider_id: PolicyProviderId,
                policy_id: PolicyId,
            },
        }

        Ok(match StrictBinding::deserialize(deserializer)? {
            StrictBinding::Unmanaged {} => Self::Unmanaged,
            StrictBinding::Inherit {} => Self::Inherit,
            StrictBinding::Provider {
                provider_id,
                policy_id,
            } => Self::Provider {
                provider_id,
                policy_id,
            },
        })
    }
}

pub const COMPILED_APPLICATION_TOOL_POLICY_SCHEMA_VERSION: u32 = 1;

#[cfg(feature = "schema")]
fn compiled_policy_schema_version_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "integer",
        "const": COMPILED_APPLICATION_TOOL_POLICY_SCHEMA_VERSION
    })
}

#[cfg(feature = "schema")]
fn compiled_policy_default_deny_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "boolean",
        "const": true
    })
}

#[cfg(feature = "schema")]
fn compiled_policy_nonempty_string_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "minLength": 1
    })
}

#[cfg(feature = "schema")]
fn compiled_policy_revision_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "integer",
        "minimum": 1
    })
}

#[cfg(feature = "schema")]
fn compiled_policy_digest_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "pattern": "^sha256:[0-9a-f]{64}$"
    })
}

/// The closed member-to-tool action vocabulary.
///
/// This is intentionally unrelated to operator-facing ABAC actions. V1 has
/// one action: a stable member invokes an exact dispatcher tool name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CompiledMemberToolAction {
    Invoke,
}

/// Policy-owned consequence classification. Caller-authored arguments never
/// override this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CompiledToolConsequence {
    R0,
    R1,
    R2,
    R3,
}

/// One exact allow entry. There are no wildcards or deny entries in v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CompiledMemberToolGrant {
    #[cfg_attr(feature = "schema", schemars(length(min = 1)))]
    pub tool_name: String,
    pub action: CompiledMemberToolAction,
    pub consequence: CompiledToolConsequence,
}

/// Total resolved grants for one stable member identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CompiledMemberToolGrants {
    #[cfg_attr(feature = "schema", schemars(length(min = 1)))]
    pub member_identity: String,
    pub grants: Vec<CompiledMemberToolGrant>,
}

/// Source-bundle evidence retained separately from the compiled policy
/// digest. It is provenance only and never an execution decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CompiledPolicySourceProvenance {
    #[cfg_attr(feature = "schema", schemars(length(min = 1)))]
    pub source_id: String,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "compiled_policy_digest_schema")
    )]
    pub source_digest: PolicyDigest,
}

/// Closed canonical artifact accepted by application policy providers.
///
/// Every semantic field is required. `default_deny` must be explicitly true,
/// and an exact tool name absent from a member's enumerated grants is denied.
/// Members and grants use ascending bytewise order so the digest covers one
/// deterministic compiled representation.
///
/// The emitted JSON Schema captures the closed shape and constraints JSON
/// Schema can represent. Runtime validation remains authoritative for
/// canonical ordering, exact canonical bytes, and digest recomputation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CompiledApplicationToolPolicy {
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "compiled_policy_schema_version_schema")
    )]
    pub schema_version: u32,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "compiled_policy_nonempty_string_schema")
    )]
    pub provider_id: PolicyProviderId,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "compiled_policy_nonempty_string_schema")
    )]
    pub policy_id: PolicyId,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "compiled_policy_revision_schema")
    )]
    pub revision: PolicyRevision,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "compiled_policy_digest_schema")
    )]
    pub policy_digest: PolicyDigest,
    pub source: CompiledPolicySourceProvenance,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "compiled_policy_default_deny_schema")
    )]
    pub default_deny: bool,
    pub members: Vec<CompiledMemberToolGrants>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompiledApplicationToolPolicyError {
    #[error("compiled application tool policy JSON is invalid: {0}")]
    InvalidJson(String),
    #[error("compiled application tool policy is not in canonical JSON form")]
    NonCanonicalJson,
    #[error("compiled application tool policy is invalid: {0}")]
    InvalidContract(String),
    #[error("compiled application tool policy digest does not match canonical payload")]
    DigestMismatch,
}

impl CompiledApplicationToolPolicy {
    pub fn new(
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
        revision: PolicyRevision,
        source: CompiledPolicySourceProvenance,
        members: Vec<CompiledMemberToolGrants>,
    ) -> Result<Self, CompiledApplicationToolPolicyError> {
        let mut policy = Self {
            schema_version: COMPILED_APPLICATION_TOOL_POLICY_SCHEMA_VERSION,
            provider_id,
            policy_id,
            revision,
            policy_digest: PolicyDigest::from_canonical_bytes(&[]),
            source,
            default_deny: true,
            members,
        };
        policy.policy_digest = PolicyDigest::from_canonical_bytes(&policy.canonical_payload()?);
        policy.validate()?;
        Ok(policy)
    }

    /// Parse only the canonical byte representation. Unknown fields fail in
    /// serde, missing required fields fail in serde, and alternate whitespace
    /// or field order fails the byte equality check after typed validation.
    pub fn parse_canonical_json(bytes: &[u8]) -> Result<Self, CompiledApplicationToolPolicyError> {
        let policy: Self = serde_json::from_slice(bytes)
            .map_err(|error| CompiledApplicationToolPolicyError::InvalidJson(error.to_string()))?;
        policy.validate()?;
        let mut canonical = serde_json::to_vec(&policy)
            .map_err(|error| CompiledApplicationToolPolicyError::InvalidJson(error.to_string()))?;
        canonical.push(b'\n');
        if canonical != bytes {
            return Err(CompiledApplicationToolPolicyError::NonCanonicalJson);
        }
        Ok(policy)
    }

    pub fn canonical_json(&self) -> Result<Vec<u8>, CompiledApplicationToolPolicyError> {
        self.validate()?;
        let mut canonical = serde_json::to_vec(self)
            .map_err(|error| CompiledApplicationToolPolicyError::InvalidJson(error.to_string()))?;
        canonical.push(b'\n');
        Ok(canonical)
    }

    pub fn validate(&self) -> Result<(), CompiledApplicationToolPolicyError> {
        if self.schema_version != COMPILED_APPLICATION_TOOL_POLICY_SCHEMA_VERSION {
            return Err(CompiledApplicationToolPolicyError::InvalidContract(
                format!("unsupported schema version {}", self.schema_version),
            ));
        }
        PolicyProviderId::new(self.provider_id.as_str().to_string()).map_err(|error| {
            CompiledApplicationToolPolicyError::InvalidContract(error.to_string())
        })?;
        PolicyId::new(self.policy_id.as_str().to_string()).map_err(|error| {
            CompiledApplicationToolPolicyError::InvalidContract(error.to_string())
        })?;
        if self.revision.0 == 0 {
            return Err(CompiledApplicationToolPolicyError::InvalidContract(
                "policy revision must be non-zero".to_string(),
            ));
        }
        if !self.default_deny {
            return Err(CompiledApplicationToolPolicyError::InvalidContract(
                "default_deny must be explicitly true".to_string(),
            ));
        }
        validate_canonical_policy_text("source_id", &self.source.source_id)?;
        PolicyDigest::parse(self.source.source_digest.as_str().to_string()).map_err(|error| {
            CompiledApplicationToolPolicyError::InvalidContract(error.to_string())
        })?;
        PolicyDigest::parse(self.policy_digest.as_str().to_string()).map_err(|error| {
            CompiledApplicationToolPolicyError::InvalidContract(error.to_string())
        })?;

        let mut prior_member: Option<&str> = None;
        for member in &self.members {
            validate_canonical_policy_text("member_identity", &member.member_identity)?;
            if prior_member.is_some_and(|prior| prior >= member.member_identity.as_str()) {
                return Err(CompiledApplicationToolPolicyError::InvalidContract(
                    "members must be unique and sorted by member_identity".to_string(),
                ));
            }
            prior_member = Some(&member.member_identity);
            let mut prior_tool: Option<&str> = None;
            for grant in &member.grants {
                validate_canonical_policy_text("tool_name", &grant.tool_name)?;
                if prior_tool.is_some_and(|prior| prior >= grant.tool_name.as_str()) {
                    return Err(CompiledApplicationToolPolicyError::InvalidContract(
                        format!(
                            "grants for member '{}' must be unique and sorted by tool_name",
                            member.member_identity
                        ),
                    ));
                }
                prior_tool = Some(&grant.tool_name);
            }
        }

        let expected = PolicyDigest::from_canonical_bytes(&self.canonical_payload()?);
        if expected != self.policy_digest {
            return Err(CompiledApplicationToolPolicyError::DigestMismatch);
        }
        Ok(())
    }

    fn canonical_payload(&self) -> Result<Vec<u8>, CompiledApplicationToolPolicyError> {
        #[derive(Serialize)]
        struct DigestPayload<'a> {
            schema_version: u32,
            provider_id: &'a PolicyProviderId,
            policy_id: &'a PolicyId,
            revision: PolicyRevision,
            source: &'a CompiledPolicySourceProvenance,
            default_deny: bool,
            members: &'a [CompiledMemberToolGrants],
        }
        serde_json::to_vec(&DigestPayload {
            schema_version: self.schema_version,
            provider_id: &self.provider_id,
            policy_id: &self.policy_id,
            revision: self.revision,
            source: &self.source,
            default_deny: self.default_deny,
            members: &self.members,
        })
        .map_err(|error| CompiledApplicationToolPolicyError::InvalidJson(error.to_string()))
    }
}

fn validate_canonical_policy_text(
    field: &str,
    value: &str,
) -> Result<(), CompiledApplicationToolPolicyError> {
    if value.is_empty() || value.trim() != value {
        return Err(CompiledApplicationToolPolicyError::InvalidContract(
            format!("{field} must be nonempty canonical text"),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ToolConsequenceDenial {
    pub code: String,
    pub message: String,
}

impl ToolConsequenceDenial {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolConsequenceFailure {
    #[error("policy provider '{provider_id}' is not installed")]
    ProviderMissing { provider_id: PolicyProviderId },
    #[error("policy '{policy_id}' is not installed for provider '{provider_id}'")]
    PolicyMissing {
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
    },
    #[error("policy snapshot provenance is invalid: {reason}")]
    InvalidProvenance { reason: String },
    #[error(
        "policy '{policy_id}' for provider '{provider_id}' rolled back from revision {accepted_revision} to {observed_revision}"
    )]
    RevisionRollback {
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
        accepted_revision: u64,
        observed_revision: u64,
    },
    #[error(
        "policy '{policy_id}' for provider '{provider_id}' reused revision {revision} with different content"
    )]
    RevisionDigestConflict {
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
        revision: u64,
    },
    #[error("policy evaluator partition for '{provider_id}' is unavailable")]
    PartitionUnavailable { provider_id: PolicyProviderId },
    #[error("policy evaluator partition for '{provider_id}' is saturated")]
    Saturated { provider_id: PolicyProviderId },
    #[error("policy evaluation for '{provider_id}' exceeded {deadline_ms}ms")]
    DeadlineExceeded {
        provider_id: PolicyProviderId,
        deadline_ms: u64,
    },
    #[error("policy evaluator for '{provider_id}' panicked")]
    EvaluatorPanicked { provider_id: PolicyProviderId },
    #[error("policy evaluator for '{provider_id}' is mechanically unhealthy")]
    MechanicallyUnhealthy { provider_id: PolicyProviderId },
    #[error("policy generation {generation} for '{provider_id}' is unhealthy: {reason}")]
    GenerationUnhealthy {
        provider_id: PolicyProviderId,
        generation: u64,
        reason: String,
    },
    #[error("policy evaluator is unsupported on this runtime")]
    UnsupportedRuntime,
    #[error("policy evaluation failed: {reason}")]
    EvaluationFailed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolConsequenceRequest {
    pub member: MobMemberBinding,
    pub tool_name: ToolName,
    pub arguments_json: String,
    pub arguments_digest: String,
    pub run_id: Option<RunId>,
    pub tool_call_id: String,
    pub provider_id: PolicyProviderId,
    pub policy_id: PolicyId,
}

impl ToolConsequenceRequest {
    pub fn from_call(
        member: MobMemberBinding,
        call: ToolCallView<'_>,
        run_id: Option<RunId>,
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
    ) -> Result<Self, ToolConsequenceFailure> {
        let arguments_json = call.args.get().to_string();
        Ok(Self {
            member,
            tool_name: ToolName::new(call.name),
            arguments_digest: format!("sha256:{:x}", Sha256::digest(call.args.get().as_bytes())),
            arguments_json,
            run_id,
            tool_call_id: call.id.to_string(),
            provider_id,
            policy_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolConsequenceVerdict {
    Allow,
    Deny(ToolConsequenceDenial),
    Indeterminate(ToolConsequenceFailure),
}

pub trait ToolConsequencePolicySnapshot: Send + Sync + 'static {
    fn provenance(&self) -> PolicyEvaluationProvenance;
    fn evaluate(&self, request: &ToolConsequenceRequest) -> ToolConsequenceVerdict;
}

pub trait ToolConsequenceNarrowingPolicy: Send + Sync + 'static {
    fn provider_id(&self) -> &PolicyProviderId;
    fn generation(&self) -> PolicyProviderGeneration;
    /// Return the provider-owned current immutable snapshot.
    ///
    /// The provider owns the snapshot pointer and MUST reject a policy
    /// revision below the revision it has already accepted. Meerkat validates
    /// provenance shape and fences provider generations, but deliberately
    /// keeps no second accepted-revision store.
    fn snapshot(
        &self,
        policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolConsequenceObservationOutcome {
    Denied(ToolConsequenceDenial),
    Indeterminate(ToolConsequenceFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolConsequenceObservation {
    pub member: MobMemberBinding,
    pub provider_id: PolicyProviderId,
    pub policy_id: PolicyId,
    pub provenance: Option<PolicyEvaluationProvenance>,
    pub tool_name: ToolName,
    pub arguments_digest: String,
    pub run_id: Option<RunId>,
    pub tool_call_id: String,
    pub outcome: ToolConsequenceObservationOutcome,
}

pub trait ToolConsequenceObserver: Send + Sync + 'static {
    fn observe(&self, observation: ToolConsequenceObservation);
}

#[derive(Default)]
pub struct NoopToolConsequenceObserver;

impl ToolConsequenceObserver for NoopToolConsequenceObserver {
    fn observe(&self, _observation: ToolConsequenceObservation) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyEvaluationSupervisorConfig {
    pub workers_per_provider: usize,
    pub queue_capacity_per_provider: usize,
    pub evaluation_deadline: Duration,
}

impl Default for PolicyEvaluationSupervisorConfig {
    fn default() -> Self {
        Self {
            workers_per_provider: 2,
            queue_capacity_per_provider: 32,
            evaluation_deadline: Duration::from_millis(50),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct EvaluationJob {
    generation: PolicyProviderGeneration,
    snapshot: Arc<dyn ToolConsequencePolicySnapshot>,
    request: ToolConsequenceRequest,
    reply: tokio::sync::oneshot::Sender<ToolConsequenceVerdict>,
}

#[cfg(not(target_arch = "wasm32"))]
struct EvaluationPartition {
    provider_id: PolicyProviderId,
    sender: std::sync::mpsc::SyncSender<EvaluationJob>,
    mechanically_healthy: Arc<std::sync::atomic::AtomicBool>,
    semantic_unhealthy: parking_lot::Mutex<Option<(PolicyProviderGeneration, String)>>,
}

pub struct PolicyEvaluationSupervisor {
    #[cfg(not(target_arch = "wasm32"))]
    config: PolicyEvaluationSupervisorConfig,
    #[cfg(not(target_arch = "wasm32"))]
    partitions: BTreeMap<PolicyProviderId, Arc<EvaluationPartition>>,
    #[cfg(target_arch = "wasm32")]
    _providers: std::collections::BTreeSet<PolicyProviderId>,
}

impl PolicyEvaluationSupervisor {
    pub fn new(
        config: PolicyEvaluationSupervisorConfig,
        provider_ids: impl IntoIterator<Item = PolicyProviderId>,
    ) -> Result<Self, ToolConsequenceFailure> {
        if config.workers_per_provider == 0
            || config.queue_capacity_per_provider == 0
            || config.evaluation_deadline.is_zero()
        {
            return Err(ToolConsequenceFailure::EvaluationFailed {
                reason: "policy evaluation workers, queue capacity, and deadline must be non-zero"
                    .to_string(),
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut partitions = BTreeMap::new();
            for provider_id in provider_ids {
                if partitions.contains_key(&provider_id) {
                    return Err(ToolConsequenceFailure::EvaluationFailed {
                        reason: format!("duplicate policy provider partition '{provider_id}'"),
                    });
                }
                let (sender, receiver) = std::sync::mpsc::sync_channel::<EvaluationJob>(
                    config.queue_capacity_per_provider,
                );
                let receiver = Arc::new(parking_lot::Mutex::new(receiver));
                let mechanically_healthy = Arc::new(std::sync::atomic::AtomicBool::new(true));
                for worker_index in 0..config.workers_per_provider {
                    let receiver = Arc::clone(&receiver);
                    let healthy = Arc::clone(&mechanically_healthy);
                    let thread_name = format!("policy-{}-{worker_index}", provider_id.as_str());
                    std::thread::Builder::new()
                        .name(thread_name)
                        .spawn(move || {
                            loop {
                                let job = {
                                    let receiver = receiver.lock();
                                    receiver.recv()
                                };
                                let Ok(job) = job else { break };
                                if !healthy.load(std::sync::atomic::Ordering::Acquire) {
                                    continue;
                                }
                                let verdict =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        job.snapshot.evaluate(&job.request)
                                    }));
                                match verdict {
                                    Ok(verdict) => {
                                        let _ = job.reply.send(verdict);
                                    }
                                    Err(_) => {
                                        healthy.store(false, std::sync::atomic::Ordering::Release);
                                    }
                                }
                                let _ = job.generation;
                            }
                        })
                        .map_err(|error| ToolConsequenceFailure::EvaluationFailed {
                            reason: format!("failed to start policy evaluator worker: {error}"),
                        })?;
                }
                partitions.insert(
                    provider_id.clone(),
                    Arc::new(EvaluationPartition {
                        provider_id,
                        sender,
                        mechanically_healthy,
                        semantic_unhealthy: parking_lot::Mutex::new(None),
                    }),
                );
            }
            Ok(Self { config, partitions })
        }

        #[cfg(target_arch = "wasm32")]
        {
            Ok(Self {
                _providers: provider_ids.into_iter().collect(),
            })
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn partition(
        &self,
        provider_id: &PolicyProviderId,
    ) -> Result<Arc<EvaluationPartition>, ToolConsequenceFailure> {
        self.partitions.get(provider_id).cloned().ok_or_else(|| {
            ToolConsequenceFailure::PartitionUnavailable {
                provider_id: provider_id.clone(),
            }
        })
    }

    pub fn mark_generation_unhealthy(
        &self,
        provider_id: &PolicyProviderId,
        generation: PolicyProviderGeneration,
        reason: impl Into<String>,
    ) {
        #[cfg(target_arch = "wasm32")]
        let _ = (provider_id, generation, reason);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(partition) = self.partitions.get(provider_id) {
            *partition.semantic_unhealthy.lock() = Some((generation, reason.into()));
        }
    }

    pub async fn evaluate(
        &self,
        provider_id: &PolicyProviderId,
        generation: PolicyProviderGeneration,
        snapshot: Arc<dyn ToolConsequencePolicySnapshot>,
        request: ToolConsequenceRequest,
    ) -> Result<ToolConsequenceVerdict, ToolConsequenceFailure> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (provider_id, generation, snapshot, request);
            Err(ToolConsequenceFailure::UnsupportedRuntime)
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let partition = self.partition(provider_id)?;
            if !partition
                .mechanically_healthy
                .load(std::sync::atomic::Ordering::Acquire)
            {
                return Err(ToolConsequenceFailure::MechanicallyUnhealthy {
                    provider_id: provider_id.clone(),
                });
            }
            {
                let mut unhealthy = partition.semantic_unhealthy.lock();
                if let Some((unhealthy_generation, reason)) = unhealthy.as_ref() {
                    if *unhealthy_generation == generation {
                        return Err(ToolConsequenceFailure::GenerationUnhealthy {
                            provider_id: provider_id.clone(),
                            generation: generation.0,
                            reason: reason.clone(),
                        });
                    }
                    *unhealthy = None;
                }
            }
            let (reply, receive) = tokio::sync::oneshot::channel();
            partition
                .sender
                .try_send(EvaluationJob {
                    generation,
                    snapshot,
                    request,
                    reply,
                })
                .map_err(|error| {
                    partition
                        .mechanically_healthy
                        .store(false, std::sync::atomic::Ordering::Release);
                    match error {
                        std::sync::mpsc::TrySendError::Full(_) => {
                            ToolConsequenceFailure::Saturated {
                                provider_id: provider_id.clone(),
                            }
                        }
                        std::sync::mpsc::TrySendError::Disconnected(_) => {
                            ToolConsequenceFailure::MechanicallyUnhealthy {
                                provider_id: provider_id.clone(),
                            }
                        }
                    }
                })?;
            match tokio::time::timeout(self.config.evaluation_deadline, receive).await {
                Ok(Ok(verdict)) => Ok(verdict),
                Ok(Err(_)) => {
                    partition
                        .mechanically_healthy
                        .store(false, std::sync::atomic::Ordering::Release);
                    Err(ToolConsequenceFailure::EvaluatorPanicked {
                        provider_id: partition.provider_id.clone(),
                    })
                }
                Err(_) => {
                    partition
                        .mechanically_healthy
                        .store(false, std::sync::atomic::Ordering::Release);
                    Err(ToolConsequenceFailure::DeadlineExceeded {
                        provider_id: partition.provider_id.clone(),
                        deadline_ms: u64::try_from(self.config.evaluation_deadline.as_millis())
                            .unwrap_or(u64::MAX),
                    })
                }
            }
        }
    }
}

pub struct ToolConsequencePolicyRegistry {
    providers: BTreeMap<PolicyProviderId, Arc<dyn ToolConsequenceNarrowingPolicy>>,
    supervisor: Arc<PolicyEvaluationSupervisor>,
    observer: Arc<dyn ToolConsequenceObserver>,
}

impl ToolConsequencePolicyRegistry {
    pub fn new(
        providers: Vec<Arc<dyn ToolConsequenceNarrowingPolicy>>,
        supervisor_config: PolicyEvaluationSupervisorConfig,
        observer: Option<Arc<dyn ToolConsequenceObserver>>,
    ) -> Result<Self, ToolConsequenceFailure> {
        let mut by_id = BTreeMap::new();
        for provider in providers {
            let provider_id = provider.provider_id().clone();
            if by_id.insert(provider_id.clone(), provider).is_some() {
                return Err(ToolConsequenceFailure::EvaluationFailed {
                    reason: format!("duplicate policy provider '{provider_id}'"),
                });
            }
        }
        let supervisor = Arc::new(PolicyEvaluationSupervisor::new(
            supervisor_config,
            by_id.keys().cloned(),
        )?);
        Ok(Self {
            providers: by_id,
            supervisor,
            observer: observer.unwrap_or_else(|| Arc::new(NoopToolConsequenceObserver)),
        })
    }

    fn validate_provenance(
        provenance: &PolicyEvaluationProvenance,
    ) -> Result<(), ToolConsequenceFailure> {
        if provenance.revision.0 == 0 {
            return Err(ToolConsequenceFailure::InvalidProvenance {
                reason: "policy revision must be non-zero".to_string(),
            });
        }
        PolicyDigest::parse(provenance.digest.as_str().to_string())?;
        Ok(())
    }

    pub fn bind(
        self: &Arc<Self>,
        member: MobMemberBinding,
        provider_id: PolicyProviderId,
        policy_id: PolicyId,
    ) -> Result<BoundToolConsequencePolicy, ToolConsequenceFailure> {
        let provider = self.providers.get(&provider_id).ok_or_else(|| {
            ToolConsequenceFailure::ProviderMissing {
                provider_id: provider_id.clone(),
            }
        })?;
        let generation = provider.generation();
        let snapshot = provider.snapshot(&policy_id).inspect_err(|error| {
            self.supervisor
                .mark_generation_unhealthy(&provider_id, generation, error.to_string());
        })?;
        Self::validate_provenance(&snapshot.provenance())?;
        Ok(BoundToolConsequencePolicy {
            registry: Arc::clone(self),
            member,
            provider_id,
            policy_id,
        })
    }
}

#[derive(Clone)]
pub struct BoundToolConsequencePolicy {
    registry: Arc<ToolConsequencePolicyRegistry>,
    member: MobMemberBinding,
    provider_id: PolicyProviderId,
    policy_id: PolicyId,
}

impl BoundToolConsequencePolicy {
    pub async fn evaluate(
        &self,
        call: ToolCallView<'_>,
        run_id: Option<RunId>,
    ) -> Result<(), crate::ToolError> {
        let request = ToolConsequenceRequest::from_call(
            self.member.clone(),
            call,
            run_id,
            self.provider_id.clone(),
            self.policy_id.clone(),
        )
        .map_err(crate::ToolError::policy_indeterminate)?;
        let provider = self
            .registry
            .providers
            .get(&self.provider_id)
            .ok_or_else(|| {
                crate::ToolError::policy_indeterminate(ToolConsequenceFailure::ProviderMissing {
                    provider_id: self.provider_id.clone(),
                })
            })?;
        let generation = provider.generation();
        let snapshot = provider.snapshot(&self.policy_id).map_err(|error| {
            self.registry.supervisor.mark_generation_unhealthy(
                &self.provider_id,
                generation,
                error.to_string(),
            );
            crate::ToolError::policy_indeterminate(error)
        })?;
        let provenance = snapshot.provenance();
        if let Err(error) = ToolConsequencePolicyRegistry::validate_provenance(&provenance) {
            self.registry.supervisor.mark_generation_unhealthy(
                &self.provider_id,
                generation,
                error.to_string(),
            );
            return Err(crate::ToolError::policy_indeterminate(error));
        }
        let verdict = self
            .registry
            .supervisor
            .evaluate(&self.provider_id, generation, snapshot, request.clone())
            .await
            .unwrap_or_else(ToolConsequenceVerdict::Indeterminate);
        match verdict {
            ToolConsequenceVerdict::Allow => Ok(()),
            ToolConsequenceVerdict::Deny(denial) => {
                self.registry.observer.observe(ToolConsequenceObservation {
                    member: self.member.clone(),
                    provider_id: self.provider_id.clone(),
                    policy_id: self.policy_id.clone(),
                    provenance: Some(provenance),
                    tool_name: request.tool_name,
                    arguments_digest: request.arguments_digest,
                    run_id: request.run_id,
                    tool_call_id: request.tool_call_id,
                    outcome: ToolConsequenceObservationOutcome::Denied(denial.clone()),
                });
                Err(crate::ToolError::policy_denied(denial))
            }
            ToolConsequenceVerdict::Indeterminate(failure) => {
                self.registry.observer.observe(ToolConsequenceObservation {
                    member: self.member.clone(),
                    provider_id: self.provider_id.clone(),
                    policy_id: self.policy_id.clone(),
                    provenance: Some(provenance),
                    tool_name: request.tool_name,
                    arguments_digest: request.arguments_digest,
                    run_id: request.run_id,
                    tool_call_id: request.tool_call_id,
                    outcome: ToolConsequenceObservationOutcome::Indeterminate(failure.clone()),
                });
                Err(crate::ToolError::policy_indeterminate(failure))
            }
        }
    }
}

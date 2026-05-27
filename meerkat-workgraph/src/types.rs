use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use meerkat_core::SessionId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::WorkGraphError;
pub use crate::machines::work_attention_lifecycle::WorkAttentionLifecycleMachineState as WorkAttentionMachineState;
use crate::machines::workgraph_lifecycle as wg_dsl;
pub use crate::machines::workgraph_lifecycle::WorkGraphLifecycleMachineState as WorkGraphMachineState;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct WorkItemId(String);

impl WorkItemId {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkGraphError> {
        validate_token("work item id", value.into()).map(Self)
    }

    pub fn generated() -> Self {
        Self(format!("work_{}", Uuid::now_v7()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct WorkAttentionBindingId(String);

impl WorkAttentionBindingId {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkGraphError> {
        validate_token("work attention binding id", value.into()).map(Self)
    }

    pub fn generated() -> Self {
        Self(format!("attention_{}", Uuid::now_v7()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkAttentionBindingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WorkAttentionBindingId {
    type Err = WorkGraphError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for WorkItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WorkItemId {
    type Err = WorkGraphError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct WorkNamespace(String);

impl WorkNamespace {
    pub fn new(value: impl Into<String>) -> Result<Self, WorkGraphError> {
        validate_token("work namespace", value.into()).map(Self)
    }

    pub fn default_namespace() -> Self {
        Self("default".to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for WorkNamespace {
    fn default() -> Self {
        Self::default_namespace()
    }
}

impl fmt::Display for WorkNamespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WorkNamespace {
    type Err = WorkGraphError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

fn validate_token(name: &str, value: String) -> Result<String, WorkGraphError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(WorkGraphError::InvalidInput(format!(
            "{name} must not be empty"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(WorkGraphError::InvalidInput(format!(
            "{name} must not contain control characters"
        )));
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    #[default]
    Open,
    InProgress,
    Blocked,
    Completed,
    Cancelled,
    Failed,
}

impl WorkStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    pub fn is_terminal_success(self) -> bool {
        matches!(self, Self::Completed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkPriority {
    Low,
    #[default]
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkEdgeKind {
    Blocks,
    Parent,
    Related,
    Supersedes,
    DerivedFrom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkOwnerKind {
    Principal,
    Agent,
    Session,
    Mob,
    Label,
}

impl WorkOwnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Principal => "principal",
            Self::Agent => "agent",
            Self::Session => "session",
            Self::Mob => "mob",
            Self::Label => "label",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkOwnerKey {
    pub kind: WorkOwnerKind,
    pub id: String,
}

impl WorkOwnerKey {
    pub fn new(kind: WorkOwnerKind, id: impl Into<String>) -> Result<Self, WorkGraphError> {
        Ok(Self {
            kind,
            id: validate_token("work owner id", id.into())?,
        })
    }

    pub fn principal(id: impl Into<String>) -> Result<Self, WorkGraphError> {
        Self::new(WorkOwnerKind::Principal, id)
    }

    pub fn agent(id: impl Into<String>) -> Result<Self, WorkGraphError> {
        Self::new(WorkOwnerKind::Agent, id)
    }

    pub fn session(id: impl Into<String>) -> Result<Self, WorkGraphError> {
        Self::new(WorkOwnerKind::Session, id)
    }

    pub fn mob(id: impl Into<String>) -> Result<Self, WorkGraphError> {
        Self::new(WorkOwnerKind::Mob, id)
    }

    pub fn label(id: impl Into<String>) -> Result<Self, WorkGraphError> {
        Self::new(WorkOwnerKind::Label, id)
    }

    pub fn canonical(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkOwner {
    pub key: WorkOwnerKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkCompletionPolicy {
    #[default]
    SelfAttest,
    HostConfirmed,
    PrincipalConfirmed,
    Supervisor {
        owner_key: WorkOwnerKey,
    },
    ReviewerQuorum {
        threshold: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublicGoalCompletionPolicy {
    #[default]
    SelfAttest,
}

impl From<PublicGoalCompletionPolicy> for WorkCompletionPolicy {
    fn from(policy: PublicGoalCompletionPolicy) -> Self {
        match policy {
            PublicGoalCompletionPolicy::SelfAttest => Self::SelfAttest,
        }
    }
}

impl WorkCompletionPolicy {
    pub fn requires_trusted_principal(&self) -> bool {
        matches!(
            self,
            Self::PrincipalConfirmed | Self::Supervisor { .. } | Self::ReviewerQuorum { .. }
        )
    }

    pub(crate) fn to_machine(&self) -> wg_dsl::WorkCompletionPolicy {
        match self {
            Self::SelfAttest => wg_dsl::WorkCompletionPolicy::SelfAttest,
            Self::HostConfirmed => wg_dsl::WorkCompletionPolicy::HostConfirmed,
            Self::PrincipalConfirmed => wg_dsl::WorkCompletionPolicy::PrincipalConfirmed,
            Self::Supervisor { .. } => wg_dsl::WorkCompletionPolicy::Supervisor,
            Self::ReviewerQuorum { .. } => wg_dsl::WorkCompletionPolicy::ReviewerQuorum,
        }
    }

    pub(crate) fn supervisor_owner_key(&self) -> Option<wg_dsl::WorkOwnerKey> {
        match self {
            Self::Supervisor { owner_key } => Some(work_owner_key_to_machine(owner_key)),
            _ => None,
        }
    }

    pub(crate) fn reviewer_quorum_threshold(&self) -> Option<u64> {
        match self {
            Self::ReviewerQuorum { threshold } => Some(u64::from(*threshold)),
            _ => None,
        }
    }

    pub(crate) fn from_machine(
        policy: wg_dsl::WorkCompletionPolicy,
        supervisor_owner_key: Option<wg_dsl::WorkOwnerKey>,
        reviewer_quorum_threshold: Option<u64>,
    ) -> Self {
        match policy {
            wg_dsl::WorkCompletionPolicy::SelfAttest => Self::SelfAttest,
            wg_dsl::WorkCompletionPolicy::HostConfirmed => Self::HostConfirmed,
            wg_dsl::WorkCompletionPolicy::PrincipalConfirmed => Self::PrincipalConfirmed,
            wg_dsl::WorkCompletionPolicy::Supervisor => Self::Supervisor {
                owner_key: supervisor_owner_key
                    .map(work_owner_key_from_machine)
                    .unwrap_or_else(|| WorkOwnerKey {
                        kind: WorkOwnerKind::Principal,
                        id: "supervisor".to_string(),
                    }),
            },
            wg_dsl::WorkCompletionPolicy::ReviewerQuorum => Self::ReviewerQuorum {
                threshold: reviewer_quorum_threshold
                    .and_then(|threshold| u16::try_from(threshold).ok())
                    .unwrap_or(1),
            },
        }
    }
}

impl WorkOwner {
    pub fn new(key: WorkOwnerKey) -> Self {
        Self {
            key,
            display_name: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkClaim {
    pub owner: WorkOwner,
    pub claimed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<DateTime<Utc>>,
}

impl WorkClaim {
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        self.lease_expires_at
            .is_none_or(|lease_expires_at| lease_expires_at > now)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExternalWorkRef {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkEvidenceRef {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkItemRef {
    pub realm_id: String,
    pub namespace: WorkNamespace,
    pub item_id: WorkItemId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkAttentionTarget {
    Session { session_id: SessionId },
    LoweredOwner { owner_key: WorkOwnerKey },
}

impl WorkAttentionTarget {
    pub fn owner_key(&self) -> Result<WorkOwnerKey, WorkGraphError> {
        match self {
            Self::Session { session_id } => WorkOwnerKey::session(session_id.to_string()),
            Self::LoweredOwner { owner_key } => Ok(owner_key.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GoalAttentionTarget {
    Session { session_id: SessionId },
}

impl GoalAttentionTarget {
    pub fn to_attention_target(&self) -> WorkAttentionTarget {
        match self {
            Self::Session { session_id } => WorkAttentionTarget::Session {
                session_id: session_id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkAttentionMode {
    #[default]
    Pursue,
    Coordinate,
    Review,
    Falsify,
    Judge,
    Observe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkAttentionStatus {
    #[default]
    Active,
    Paused {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<DateTime<Utc>>,
    },
    Superseded,
    Stopped,
}

impl WorkAttentionStatus {
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        match self {
            Self::Active => true,
            Self::Paused { until } => until.is_some_and(|until| until <= now),
            Self::Superseded | Self::Stopped => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AttentionDelegatedAuthority {
    #[default]
    AddEvidence,
    CloseOwnReviewItem,
    RequestClosure,
    CloseIfPolicyAllows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionProjectionPolicy {
    #[serde(default = "default_projection_max_text_chars")]
    pub max_text_chars: u32,
    #[serde(default = "default_include_parent_context")]
    pub include_parent_context: bool,
}

fn default_include_parent_context() -> bool {
    true
}

impl Default for AttentionProjectionPolicy {
    fn default() -> Self {
        Self {
            max_text_chars: default_projection_max_text_chars(),
            include_parent_context: true,
        }
    }
}

fn default_projection_max_text_chars() -> u32 {
    4096
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkAttentionBinding {
    pub binding_id: WorkAttentionBindingId,
    pub work_ref: WorkItemRef,
    pub target: WorkAttentionTarget,
    pub mode: WorkAttentionMode,
    pub status: WorkAttentionStatus,
    #[serde(default = "default_work_attention_machine_state")]
    #[cfg_attr(feature = "schema", schemars(with = "WorkAttentionMachineStateSchema"))]
    pub machine_state: WorkAttentionMachineState,
    pub delegated_authority: AttentionDelegatedAuthority,
    #[serde(default)]
    pub projection_policy: AttentionProjectionPolicy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(feature = "schema")]
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct WorkAttentionMachineStateSchema {
    lifecycle_phase: String,
    revision: u64,
    paused_until_utc_ms: Option<u64>,
    superseded_by_binding_key: Option<String>,
    terminal_at_utc_ms: Option<u64>,
}

fn default_work_attention_machine_state() -> WorkAttentionMachineState {
    WorkAttentionMachineState::default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkItem {
    pub id: WorkItemId,
    pub realm_id: String,
    pub namespace: WorkNamespace,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: WorkStatus,
    #[serde(default)]
    pub completion_policy: WorkCompletionPolicy,
    pub priority: WorkPriority,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub labels: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<WorkOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim: Option<WorkClaim>,
    #[serde(default = "default_workgraph_machine_state")]
    pub machine_state: WorkGraphMachineState,
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_refs: Vec<ExternalWorkRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<WorkEvidenceRef>,
}

fn default_workgraph_machine_state() -> WorkGraphMachineState {
    WorkGraphMachineState::default()
}

#[derive(Deserialize)]
struct WorkItemWire {
    id: WorkItemId,
    realm_id: String,
    namespace: WorkNamespace,
    title: String,
    #[serde(default)]
    description: Option<String>,
    status: WorkStatus,
    #[serde(default)]
    completion_policy: WorkCompletionPolicy,
    priority: WorkPriority,
    #[serde(default)]
    labels: BTreeSet<String>,
    #[serde(default)]
    owner: Option<WorkOwner>,
    #[serde(default)]
    claim: Option<WorkClaim>,
    #[serde(default)]
    machine_state: Option<WorkGraphMachineState>,
    revision: u64,
    #[serde(default)]
    due_at: Option<DateTime<Utc>>,
    #[serde(default)]
    not_before: Option<DateTime<Utc>>,
    #[serde(default)]
    snoozed_until: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    terminal_at: Option<DateTime<Utc>>,
    #[serde(default)]
    external_refs: Vec<ExternalWorkRef>,
    #[serde(default)]
    evidence_refs: Vec<WorkEvidenceRef>,
}

impl<'de> Deserialize<'de> for WorkItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut wire = WorkItemWire::deserialize(deserializer)?;
        let machine_state = wire
            .machine_state
            .take()
            .unwrap_or_else(|| legacy_workgraph_machine_state(&wire));
        Ok(Self {
            id: wire.id,
            realm_id: wire.realm_id,
            namespace: wire.namespace,
            title: wire.title,
            description: wire.description,
            status: wire.status,
            completion_policy: wire.completion_policy,
            priority: wire.priority,
            labels: wire.labels,
            owner: wire.owner,
            claim: wire.claim,
            machine_state,
            revision: wire.revision,
            due_at: wire.due_at,
            not_before: wire.not_before,
            snoozed_until: wire.snoozed_until,
            created_at: wire.created_at,
            updated_at: wire.updated_at,
            terminal_at: wire.terminal_at,
            external_refs: wire.external_refs,
            evidence_refs: wire.evidence_refs,
        })
    }
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for WorkItem {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkItem".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "required": [
                "id",
                "realm_id",
                "namespace",
                "title",
                "status",
                "completion_policy",
                "priority",
                "machine_state",
                "revision",
                "created_at",
                "updated_at"
            ],
            "properties": {
                "id": { "type": "string" },
                "realm_id": { "type": "string" },
                "namespace": { "type": "string" },
                "title": { "type": "string" },
                "description": { "type": ["string", "null"] },
                "status": {
                    "type": "string",
                    "enum": ["open", "in_progress", "blocked", "completed", "cancelled", "failed"]
                },
                "completion_policy": {
                    "oneOf": [
                        {
                            "type": "object",
                            "required": ["kind"],
                            "properties": { "kind": { "const": "self_attest" } }
                        },
                        {
                            "type": "object",
                            "required": ["kind"],
                            "properties": { "kind": { "const": "host_confirmed" } }
                        },
                        {
                            "type": "object",
                            "required": ["kind"],
                            "properties": { "kind": { "const": "principal_confirmed" } }
                        },
                        {
                            "type": "object",
                            "required": ["kind", "owner_key"],
                            "properties": {
                                "kind": { "const": "supervisor" },
                                "owner_key": {
                                    "type": "object",
                                    "required": ["kind", "id"],
                                    "properties": {
                                        "kind": {
                                            "type": "string",
                                            "enum": ["principal", "agent", "session", "mob", "label"]
                                        },
                                        "id": { "type": "string" }
                                    }
                                }
                            }
                        },
                        {
                            "type": "object",
                            "required": ["kind", "threshold"],
                            "properties": {
                                "kind": { "const": "reviewer_quorum" },
                                "threshold": { "type": "integer", "format": "uint16", "minimum": 1 }
                            }
                        }
                    ]
                },
                "priority": {
                    "type": "string",
                    "enum": ["low", "medium", "high"]
                },
                "labels": {
                    "type": "array",
                    "uniqueItems": true,
                    "items": { "type": "string" }
                },
                "owner": {
                    "anyOf": [
                        {
                            "type": "object",
                            "required": ["key"],
                            "properties": {
                                "key": {
                                    "type": "object",
                                    "required": ["kind", "id"],
                                    "properties": {
                                        "kind": {
                                            "type": "string",
                                            "enum": ["principal", "agent", "session", "mob", "label"]
                                        },
                                        "id": { "type": "string" }
                                    }
                                },
                                "display_name": { "type": ["string", "null"] }
                            }
                        },
                        { "type": "null" }
                    ]
                },
                "claim": {
                    "anyOf": [
                        {
                            "type": "object",
                            "required": ["owner", "claimed_at"],
                            "properties": {
                                "owner": { "type": "object" },
                                "claimed_at": { "type": "string", "format": "date-time" },
                                "lease_expires_at": { "type": ["string", "null"], "format": "date-time" }
                            }
                        },
                        { "type": "null" }
                    ]
                },
                "machine_state": {
                    "type": "object",
                    "description": "Catalog-generated WorkGraphLifecycleMachine state projection."
                },
                "revision": { "type": "integer", "format": "uint64", "minimum": 0 },
                "due_at": { "type": ["string", "null"], "format": "date-time" },
                "not_before": { "type": ["string", "null"], "format": "date-time" },
                "snoozed_until": { "type": ["string", "null"], "format": "date-time" },
                "created_at": { "type": "string", "format": "date-time" },
                "updated_at": { "type": "string", "format": "date-time" },
                "terminal_at": { "type": ["string", "null"], "format": "date-time" },
                "external_refs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["kind", "id"],
                        "properties": {
                            "kind": { "type": "string" },
                            "id": { "type": "string" },
                            "url": { "type": ["string", "null"] }
                        }
                    }
                },
                "evidence_refs": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["kind", "id"],
                        "properties": {
                            "kind": { "type": "string" },
                            "id": { "type": "string" },
                            "label": { "type": ["string", "null"] },
                            "summary": { "type": ["string", "null"] }
                        }
                    }
                }
            }
        })
    }
}

fn legacy_workgraph_machine_state(wire: &WorkItemWire) -> WorkGraphMachineState {
    let mut machine_state = WorkGraphMachineState {
        lifecycle_phase: work_lifecycle_state_from_status(wire.status),
        revision: wire.revision,
        due_at_utc_ms: wire.due_at.map(datetime_to_millis),
        not_before_utc_ms: wire.not_before.map(datetime_to_millis),
        snoozed_until_utc_ms: wire.snoozed_until.map(datetime_to_millis),
        completion_policy: wire.completion_policy.clone().to_machine(),
        completion_supervisor_owner_key: wire.completion_policy.supervisor_owner_key(),
        completion_reviewer_quorum_threshold: wire.completion_policy.reviewer_quorum_threshold(),
        terminal_at_utc_ms: wire.terminal_at.map(datetime_to_millis),
        evidence_count: wire.evidence_refs.len().try_into().unwrap_or(u64::MAX),
        ..default_workgraph_machine_state()
    };
    if let Some(claim) = &wire.claim {
        machine_state.claim_owner_key = Some(work_owner_key_to_machine(&claim.owner.key));
        machine_state.claimed_at_utc_ms = Some(datetime_to_millis(claim.claimed_at));
        machine_state.lease_expires_at_utc_ms = claim.lease_expires_at.map(datetime_to_millis);
    }
    machine_state
}

fn work_lifecycle_state_from_status(status: WorkStatus) -> wg_dsl::WorkLifecycleState {
    match status {
        WorkStatus::Open => wg_dsl::WorkLifecycleState::Open,
        WorkStatus::InProgress => wg_dsl::WorkLifecycleState::InProgress,
        WorkStatus::Blocked => wg_dsl::WorkLifecycleState::Blocked,
        WorkStatus::Completed => wg_dsl::WorkLifecycleState::Completed,
        WorkStatus::Cancelled => wg_dsl::WorkLifecycleState::Cancelled,
        WorkStatus::Failed => wg_dsl::WorkLifecycleState::Failed,
    }
}

fn work_owner_key_to_machine(owner: &WorkOwnerKey) -> wg_dsl::WorkOwnerKey {
    let kind = match owner.kind {
        WorkOwnerKind::Principal => wg_dsl::WorkOwnerKind::Principal,
        WorkOwnerKind::Agent => wg_dsl::WorkOwnerKind::Agent,
        WorkOwnerKind::Session => wg_dsl::WorkOwnerKind::Session,
        WorkOwnerKind::Mob => wg_dsl::WorkOwnerKind::Mob,
        WorkOwnerKind::Label => wg_dsl::WorkOwnerKind::Label,
    };
    wg_dsl::WorkOwnerKey {
        kind,
        id: owner.id.clone(),
    }
}

fn work_owner_key_from_machine(owner: wg_dsl::WorkOwnerKey) -> WorkOwnerKey {
    let kind = match owner.kind {
        wg_dsl::WorkOwnerKind::Principal => WorkOwnerKind::Principal,
        wg_dsl::WorkOwnerKind::Agent => WorkOwnerKind::Agent,
        wg_dsl::WorkOwnerKind::Session => WorkOwnerKind::Session,
        wg_dsl::WorkOwnerKind::Mob => WorkOwnerKind::Mob,
        wg_dsl::WorkOwnerKind::Label => WorkOwnerKind::Label,
    };
    WorkOwnerKey { kind, id: owner.id }
}

fn datetime_to_millis(dt: DateTime<Utc>) -> u64 {
    u64::try_from(dt.timestamp_millis()).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkEdge {
    pub realm_id: String,
    pub namespace: WorkNamespace,
    pub kind: WorkEdgeKind,
    pub from_id: WorkItemId,
    pub to_id: WorkItemId,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphEventKind {
    Created,
    Updated,
    Claimed,
    Released,
    Blocked,
    Closed,
    Linked,
    EvidenceAdded,
    AttentionCreated,
    AttentionUpdated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkGraphEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    pub realm_id: String,
    pub namespace: WorkNamespace,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<WorkItemId>,
    pub kind: WorkGraphEventKind,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
}

impl WorkGraphEvent {
    pub fn item(
        realm_id: String,
        namespace: WorkNamespace,
        item_id: WorkItemId,
        kind: WorkGraphEventKind,
        at: DateTime<Utc>,
        payload: Value,
    ) -> Self {
        Self {
            seq: None,
            realm_id,
            namespace,
            item_id: Some(item_id),
            kind,
            at,
            payload,
        }
    }

    pub fn graph(
        realm_id: String,
        namespace: WorkNamespace,
        kind: WorkGraphEventKind,
        at: DateTime<Utc>,
        payload: Value,
    ) -> Self {
        Self {
            seq: None,
            realm_id,
            namespace,
            item_id: None,
            kind,
            at,
            payload,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreateWorkItemRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub priority: WorkPriority,
    #[serde(default)]
    pub completion_policy: WorkCompletionPolicy,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub labels: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_refs: Vec<ExternalWorkRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<WorkEvidenceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateWorkItemRequest {
    pub id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<WorkPriority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_policy: Option<WorkCompletionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_refs: Vec<ExternalWorkRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ClaimWorkItemRequest {
    pub id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    pub owner: WorkOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReleaseWorkItemRequest {
    pub id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CloseWorkItemRequest {
    pub id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    #[serde(default = "default_terminal_status")]
    pub status: WorkStatus,
}

fn default_terminal_status() -> WorkStatus {
    WorkStatus::Completed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LinkWorkItemsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub kind: WorkEdgeKind,
    pub from_id: WorkItemId,
    pub to_id: WorkItemId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AddEvidenceRequest {
    pub id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    pub evidence: WorkEvidenceRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalCreateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub target: GoalAttentionTarget,
    #[serde(default)]
    pub mode: WorkAttentionMode,
    #[serde(default)]
    pub completion_policy: WorkCompletionPolicy,
    #[serde(default)]
    pub delegated_authority: AttentionDelegatedAuthority,
    #[serde(default)]
    pub projection_policy: AttentionProjectionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PublicGoalCreateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub target: GoalAttentionTarget,
    #[serde(default)]
    pub mode: WorkAttentionMode,
    #[serde(default)]
    pub completion_policy: PublicGoalCompletionPolicy,
    #[serde(default)]
    pub delegated_authority: AttentionDelegatedAuthority,
    #[serde(default)]
    pub projection_policy: AttentionProjectionPolicy,
}

impl From<PublicGoalCreateRequest> for GoalCreateRequest {
    fn from(request: PublicGoalCreateRequest) -> Self {
        Self {
            realm_id: request.realm_id,
            namespace: request.namespace,
            title: request.title,
            description: request.description,
            target: request.target,
            mode: request.mode,
            completion_policy: request.completion_policy.into(),
            delegated_authority: request.delegated_authority,
            projection_policy: request.projection_policy,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalCreateResult {
    pub item: WorkItem,
    pub attention: WorkAttentionBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalStatusRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalStatusResult {
    pub item: WorkItem,
    pub attention: WorkAttentionBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalConfirmRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    pub evidence: WorkEvidenceRef,
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub principal: Option<WorkOwnerKey>,
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub trusted_principal: Option<WorkOwnerKey>,
}

impl GoalConfirmRequest {
    /// Promote an already-authenticated host principal into the service authority field.
    pub fn with_trusted_principal(mut self, principal: Option<WorkOwnerKey>) -> Self {
        if self.trusted_principal.is_none() {
            self.trusted_principal = principal;
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalConfirmResult {
    pub item: WorkItem,
    pub attention: WorkAttentionBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalRequestCloseRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    #[serde(default)]
    pub status: GoalTerminalStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GoalTerminalStatus {
    #[default]
    Completed,
    Cancelled,
    Failed,
}

impl From<GoalTerminalStatus> for WorkStatus {
    fn from(status: GoalTerminalStatus) -> Self {
        match status {
            GoalTerminalStatus::Completed => Self::Completed,
            GoalTerminalStatus::Cancelled => Self::Cancelled,
            GoalTerminalStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PublicGoalRequestCloseRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    #[serde(default)]
    pub status: GoalTerminalStatus,
}

impl From<PublicGoalRequestCloseRequest> for GoalRequestCloseRequest {
    fn from(request: PublicGoalRequestCloseRequest) -> Self {
        Self {
            binding_id: request.binding_id,
            realm_id: request.realm_id,
            namespace: request.namespace,
            expected_revision: request.expected_revision,
            status: request.status,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GoalRequestCloseResult {
    pub item: WorkItem,
    pub attention: WorkAttentionBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionListRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<WorkAttentionTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkAttentionStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionListResult {
    pub attention: Vec<WorkAttentionBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionBindingRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionPauseRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionResumeRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionReassignRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub target: GoalAttentionTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionBindingResult {
    pub attention: WorkAttentionBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum AttentionContinueOutcome {
    Accepted,
    Deduplicated,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionContinueResult {
    pub outcome: AttentionContinueOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionProjectionRequest {
    pub binding_id: WorkAttentionBindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionProjectionResult {
    pub projection: AttentionContextProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionContextProjection {
    pub binding_id: WorkAttentionBindingId,
    pub work_ref: WorkItemRef,
    pub mode: WorkAttentionMode,
    pub binding_revision: u64,
    pub item_revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_refs: Vec<WorkItemRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_context: Vec<AttentionProjectionParentContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<WorkEvidenceRef>,
    pub authority: ProjectedAttentionAuthority,
    pub text: AttentionProjectionText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionProjectionParentContext {
    pub work_ref: WorkItemRef,
    pub status: WorkStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProjectedAttentionAuthority {
    pub can_add_evidence: bool,
    pub can_request_closure: bool,
    #[serde(default)]
    pub can_close_own_review_item: bool,
    pub can_close_if_policy_allows: bool,
    pub can_close_parent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AttentionProjectionText {
    pub title: String,
    pub rendered: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkItemFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    #[serde(default)]
    pub all_namespaces: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<WorkStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default)]
    pub include_terminal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ReadyWorkFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkGraphSnapshotFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    #[serde(default)]
    pub all_namespaces: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<WorkStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default)]
    pub include_terminal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkGraphSnapshot {
    pub realm_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub all_namespaces: bool,
    pub captured_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_high_water_mark: Option<i64>,
    pub items: Vec<WorkItem>,
    pub edges: Vec<WorkEdge>,
    #[serde(default)]
    pub attention: Vec<WorkAttentionBinding>,
    pub ready_item_ids: Vec<WorkItemId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkGraphItemsResponse {
    pub items: Vec<WorkItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkGraphEventsResponse {
    pub events: Vec<WorkGraphEvent>,
}

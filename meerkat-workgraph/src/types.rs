use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::WorkGraphError;
use crate::machines::workgraph_lifecycle as wg_dsl;
pub use crate::machines::workgraph_lifecycle::WorkGraphLifecycleMachineState as WorkGraphMachineState;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
#[serde(rename_all = "snake_case")]
pub enum WorkPriority {
    Low,
    #[default]
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkEdgeKind {
    Blocks,
    Parent,
    Related,
    Supersedes,
    DerivedFrom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
pub struct WorkOwner {
    pub key: WorkOwnerKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
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
pub struct ExternalWorkRef {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkEvidenceRef {
    pub kind: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
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

fn legacy_workgraph_machine_state(wire: &WorkItemWire) -> WorkGraphMachineState {
    let mut machine_state = WorkGraphMachineState {
        lifecycle_phase: work_lifecycle_state_from_status(wire.status),
        revision: wire.revision,
        due_at_utc_ms: wire.due_at.map(datetime_to_millis),
        not_before_utc_ms: wire.not_before.map(datetime_to_millis),
        snoozed_until_utc_ms: wire.snoozed_until.map(datetime_to_millis),
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

fn datetime_to_millis(dt: DateTime<Utc>) -> u64 {
    u64::try_from(dt.timestamp_millis()).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WorkEdge {
    pub realm_id: String,
    pub namespace: WorkNamespace,
    pub kind: WorkEdgeKind,
    pub from_id: WorkItemId,
    pub to_id: WorkItemId,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct ReleaseWorkItemRequest {
    pub id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
pub struct AddEvidenceRequest {
    pub id: WorkItemId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    pub expected_revision: u64,
    pub evidence: WorkEvidenceRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub ready_item_ids: Vec<WorkItemId>,
}

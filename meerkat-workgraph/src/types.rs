use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::WorkGraphError;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkOwner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(default)]
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

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

pub(crate) fn work_item_key_for_id(id: &WorkItemId) -> wg_dsl::WorkItemKey {
    wg_dsl::WorkItemKey(id.as_str().to_string())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Open,
    InProgress,
    Blocked,
    Completed,
    Cancelled,
    Failed,
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

pub(crate) fn external_work_ref_tokens(refs: &[ExternalWorkRef]) -> Result<Vec<String>, String> {
    refs.iter()
        .map(canonical_workgraph_ref_token)
        .collect::<Result<Vec<_>, _>>()
}

pub(crate) fn work_evidence_ref_tokens(refs: &[WorkEvidenceRef]) -> Result<Vec<String>, String> {
    refs.iter()
        .map(canonical_workgraph_ref_token)
        .collect::<Result<Vec<_>, _>>()
}

fn canonical_workgraph_ref_token<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value)
        .map_err(|error| format!("failed to serialize WorkGraph machine provenance token: {error}"))
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
    #[serde(serialize_with = "serialize_workgraph_machine_state")]
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
    machine_state: Option<WorkGraphMachineStateWire>,
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
            .ok_or_else(|| serde::de::Error::missing_field("machine_state"))?
            .try_into()
            .map_err(serde::de::Error::custom)?;
        validate_work_item_machine_projection(&wire, &machine_state)
            .map_err(serde::de::Error::custom)?;
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

#[derive(Clone, Serialize, Deserialize)]
struct WorkGraphMachineStateWire {
    machine: String,
    schema_version: u32,
    #[serde(flatten)]
    state: WorkGraphMachineState,
}

impl From<&WorkGraphMachineState> for WorkGraphMachineStateWire {
    fn from(state: &WorkGraphMachineState) -> Self {
        let (machine, schema_version) = workgraph_machine_schema_identity();
        Self {
            machine,
            schema_version,
            state: state.clone(),
        }
    }
}

impl TryFrom<WorkGraphMachineStateWire> for WorkGraphMachineState {
    type Error = String;

    fn try_from(wire: WorkGraphMachineStateWire) -> Result<Self, Self::Error> {
        validate_workgraph_machine_wire_header(&wire.machine, wire.schema_version)?;
        validate_workgraph_machine_recovery(&wire.state)?;
        Ok(wire.state)
    }
}

fn serialize_workgraph_machine_state<S>(
    state: &WorkGraphMachineState,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    WorkGraphMachineStateWire::from(state).serialize(serializer)
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
                    "required": ["machine", "schema_version"],
                    "description": "Catalog-generated WorkGraphLifecycleMachine state projection with generated machine identity and schema version."
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

fn workgraph_machine_schema_identity() -> (String, u32) {
    let schema = wg_dsl::WorkGraphLifecycleMachineState::schema();
    (schema.machine.to_string(), schema.version)
}

fn validate_workgraph_machine_wire_header(
    machine: &str,
    schema_version: u32,
) -> Result<(), String> {
    let (expected_machine, expected_version) = workgraph_machine_schema_identity();
    if machine != expected_machine {
        return Err(format!(
            "work graph machine_state machine `{machine}` does not match generated schema `{expected_machine}`"
        ));
    }
    if schema_version != expected_version {
        return Err(format!(
            "work graph machine_state schema_version `{schema_version}` does not match generated schema version `{expected_version}`"
        ));
    }
    Ok(())
}

fn validate_workgraph_machine_recovery(
    machine_state: &WorkGraphMachineState,
) -> Result<(), String> {
    wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(machine_state.clone())
        .map(|_| ())
        .map_err(|source| {
            format!(
                "generated WorkGraphLifecycleMachine rejected recovered machine_state: {source:?}"
            )
        })
}

fn validate_work_item_machine_projection(
    wire: &WorkItemWire,
    machine_state: &WorkGraphMachineState,
) -> Result<(), String> {
    validate_workgraph_machine_recovery(machine_state)?;
    let item_key = machine_state.item_key.as_ref().ok_or_else(|| {
        format!(
            "work item {} machine_state is missing generated item identity",
            wire.id
        )
    })?;
    if item_key != &work_item_key_for_id(&wire.id) {
        return Err(format!(
            "work item {} id projection does not match machine_state",
            wire.id
        ));
    }
    if external_work_ref_tokens(&wire.external_refs)? != machine_state.external_ref_tokens {
        return Err(format!(
            "work item {} external_refs projection does not match machine_state",
            wire.id
        ));
    }
    if work_evidence_ref_tokens(&wire.evidence_refs)? != machine_state.evidence_ref_tokens {
        return Err(format!(
            "work item {} evidence_refs projection does not match machine_state",
            wire.id
        ));
    }
    if u64::try_from(wire.evidence_refs.len()).map_err(|_| {
        format!(
            "work item {} evidence_refs length cannot be represented as u64",
            wire.id
        )
    })? != machine_state.evidence_count
    {
        return Err(format!(
            "work item {} evidence_count projection does not match machine_state",
            wire.id
        ));
    }
    if work_lifecycle_state_from_status(wire.status) != machine_state.lifecycle_phase {
        return Err(format!(
            "work item {} status projection does not match machine_state",
            wire.id
        ));
    }
    if wire.revision != machine_state.revision {
        return Err(format!(
            "work item {} revision projection does not match machine_state",
            wire.id
        ));
    }
    if optional_datetime_to_millis(wire.due_at, "due_at")? != machine_state.due_at_utc_ms {
        return Err(format!(
            "work item {} due_at projection does not match machine_state",
            wire.id
        ));
    }
    if optional_datetime_to_millis(wire.not_before, "not_before")?
        != machine_state.not_before_utc_ms
    {
        return Err(format!(
            "work item {} not_before projection does not match machine_state",
            wire.id
        ));
    }
    if optional_datetime_to_millis(wire.snoozed_until, "snoozed_until")?
        != machine_state.snoozed_until_utc_ms
    {
        return Err(format!(
            "work item {} snoozed_until projection does not match machine_state",
            wire.id
        ));
    }
    if optional_datetime_to_millis(wire.terminal_at, "terminal_at")?
        != machine_state.terminal_at_utc_ms
    {
        return Err(format!(
            "work item {} terminal_at projection does not match machine_state",
            wire.id
        ));
    }
    if let Some(claim) = &wire.claim {
        let claim_owner_key = work_owner_key_to_machine(&claim.owner.key);
        if machine_state.claim_owner_key.as_ref() != Some(&claim_owner_key) {
            return Err(format!(
                "work item {} claim owner projection does not match machine_state",
                wire.id
            ));
        }
        if machine_state.claimed_at_utc_ms
            != Some(datetime_to_millis(claim.claimed_at, "claimed_at")?)
        {
            return Err(format!(
                "work item {} claim time projection does not match machine_state",
                wire.id
            ));
        }
        if machine_state.lease_expires_at_utc_ms
            != optional_datetime_to_millis(claim.lease_expires_at, "lease_expires_at")?
        {
            return Err(format!(
                "work item {} claim lease projection does not match machine_state",
                wire.id
            ));
        }
    } else if machine_state.claim_owner_key.is_some()
        || machine_state.claimed_at_utc_ms.is_some()
        || machine_state.lease_expires_at_utc_ms.is_some()
    {
        return Err(format!(
            "work item {} machine_state has a claim without a claim projection",
            wire.id
        ));
    }
    Ok(())
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

fn datetime_to_millis(dt: DateTime<Utc>, field: &'static str) -> Result<u64, String> {
    let millis = dt.timestamp_millis();
    u64::try_from(millis).map_err(|_| {
        format!("work graph timestamp `{field}` cannot be represented as unsigned millis: {millis}")
    })
}

fn optional_datetime_to_millis(
    dt: Option<DateTime<Utc>>,
    field: &'static str,
) -> Result<Option<u64>, String> {
    dt.map(|value| datetime_to_millis(value, field)).transpose()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkStatus>,
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

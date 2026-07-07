use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use strum::IntoEnumIterator;

use crate::store::WorkGraphEventFilter;
use crate::types::{
    AddEvidenceRequest, AttentionReassignRequest, ClaimWorkItemRequest, CloseWorkItemRequest,
    LinkWorkItemsRequest, PolicyEscalateRequest, ReadyWorkFilter, ReleaseWorkItemRequest,
    UpdateWorkItemRequest, WorkGraphSnapshotFilter, WorkItemFilter, WorkItemId, WorkNamespace,
};
use crate::{CreateWorkItemRequest, WorkGraphError, WorkGraphService};

/// Typed tool-facing error class for WorkGraph operations.
///
/// This is the closed set of semantic error outcomes a WorkGraph tool call can
/// surface. It is derived directly from [`WorkGraphError`] (the canonical domain
/// error) in [`map_error`], never re-parsed from text, and serializes to the
/// stable `snake_case` wire codes consumed by SDKs. Surfaces map this typed code
/// onto their own transport numbering (e.g. JSON-RPC) with an exhaustive match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphToolErrorCode {
    InvalidArguments,
    NotFound,
    CapabilityUnavailable,
    Conflict,
    InvalidTransition,
    StoreError,
    InternalError,
}

impl WorkGraphToolErrorCode {
    /// Stable wire/display token for this error class (matches the `snake_case`
    /// serde representation). For human-readable messages and logs only — never
    /// parse this back into a decision.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "invalid_arguments",
            Self::NotFound => "not_found",
            Self::CapabilityUnavailable => "capability_unavailable",
            Self::Conflict => "conflict",
            Self::InvalidTransition => "invalid_transition",
            Self::StoreError => "store_error",
            Self::InternalError => "internal_error",
        }
    }
}

impl std::fmt::Display for WorkGraphToolErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkGraphToolError {
    pub code: WorkGraphToolErrorCode,
    pub message: String,
}

impl WorkGraphToolError {
    fn new(code: WorkGraphToolErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Closed catalog of WorkGraph tool operations.
///
/// The iteration order of [`strum::EnumIter`] is declaration order, so the
/// variant list below IS the catalog — there is no parallel hand-maintained
/// `ALL` slice to drift. Adding a variant automatically extends the advertised
/// tool list and the dispatch surface, and the compiler forces the exhaustive
/// `name()`/`description()`/`schema()` matches (and the dispatch match in
/// [`handle_workgraph_tools_call`]) to acknowledge it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
enum WorkGraphToolContract {
    Create,
    Get,
    List,
    Ready,
    Snapshot,
    Events,
    Claim,
    Release,
    Update,
    PolicyEscalate,
    Block,
    Close,
    Link,
    AddEvidence,
    AttentionReassign,
}

impl WorkGraphToolContract {
    const fn name(self) -> &'static str {
        match self {
            Self::Create => "workgraph_create",
            Self::Get => "workgraph_get",
            Self::List => "workgraph_list",
            Self::Ready => "workgraph_ready",
            Self::Snapshot => "workgraph_snapshot",
            Self::Events => "workgraph_events",
            Self::Claim => "workgraph_claim",
            Self::Release => "workgraph_release",
            Self::Update => "workgraph_update",
            Self::PolicyEscalate => "workgraph_policy_escalate",
            Self::Block => "workgraph_block",
            Self::Close => "workgraph_close",
            Self::Link => "workgraph_link",
            Self::AddEvidence => "workgraph_add_evidence",
            Self::AttentionReassign => "workgraph_attention_reassign",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Create => "Create a durable WorkGraph item.",
            Self::Get => "Read one WorkGraph item.",
            Self::List => "List WorkGraph items.",
            Self::Ready => "List ready, claimable WorkGraph items.",
            Self::Snapshot => "Read a WorkGraph observability snapshot.",
            Self::Events => "Read WorkGraph event history.",
            Self::Claim => "Claim a ready WorkGraph item with CAS revision checking.",
            Self::Release => "Release a claimed WorkGraph item.",
            Self::Update => "Update non-terminal WorkGraph item fields.",
            Self::PolicyEscalate => "Monotonically tighten a WorkGraph completion policy.",
            Self::Block => "Mark a WorkGraph item blocked.",
            Self::Close => "Close a WorkGraph item with a terminal status.",
            Self::Link => "Create a dependency or relationship edge.",
            Self::AddEvidence => "Attach a typed evidence reference to a WorkGraph item.",
            Self::AttentionReassign => "Reassign a WorkGraph attention binding.",
        }
    }

    fn schema(self) -> Value {
        match self {
            Self::Create => create_schema(),
            Self::Get => id_schema(false),
            Self::List => list_schema(),
            Self::Ready => ready_schema(),
            Self::Snapshot => snapshot_schema(),
            Self::Events => events_schema(),
            Self::Claim => claim_schema(),
            Self::Release | Self::Block => revision_id_schema(),
            Self::Update => update_schema(),
            Self::PolicyEscalate => policy_escalate_schema(),
            Self::Close => close_schema(),
            Self::Link => link_schema(),
            Self::AddEvidence => evidence_schema(),
            Self::AttentionReassign => attention_reassign_schema(),
        }
    }

    const fn is_unscoped_surface_allowed(self) -> bool {
        !matches!(self, Self::AttentionReassign | Self::PolicyEscalate)
    }

    fn parse(name: &str) -> Result<Self, WorkGraphToolError> {
        Self::iter()
            .find(|contract| contract.name() == name)
            .ok_or_else(|| {
                WorkGraphToolError::new(
                    WorkGraphToolErrorCode::NotFound,
                    format!("unknown WorkGraph tool '{name}'"),
                )
            })
    }
}

pub fn workgraph_tools_list() -> Vec<Value> {
    WorkGraphToolContract::iter()
        .map(|contract| tool(contract.name(), contract.description(), contract.schema()))
        .collect()
}

/// Public WorkGraph MCP/default surface.
///
/// Attention-only operations require a runtime-injected attention projection
/// witness before dispatch, so they are advertised only by
/// [`WorkGraphToolSurface::with_attention_projection`](crate::WorkGraphToolSurface::with_attention_projection)
/// and rejected by [`handle_unscoped_workgraph_tools_call`].
pub fn unscoped_workgraph_tools_list() -> Vec<Value> {
    WorkGraphToolContract::iter()
        .filter(|contract| contract.is_unscoped_surface_allowed())
        .map(|contract| tool(contract.name(), contract.description(), contract.schema()))
        .collect()
}

pub async fn handle_unscoped_workgraph_tools_call(
    service: &WorkGraphService,
    name: &str,
    arguments: &Value,
) -> Result<Value, WorkGraphToolError> {
    let contract = WorkGraphToolContract::parse(name)?;
    if !contract.is_unscoped_surface_allowed() {
        return Err(WorkGraphToolError::new(
            WorkGraphToolErrorCode::NotFound,
            format!("unknown WorkGraph tool '{name}'"),
        ));
    }
    handle_workgraph_tools_call(service, name, arguments).await
}

pub async fn handle_workgraph_tools_call(
    service: &WorkGraphService,
    name: &str,
    arguments: &Value,
) -> Result<Value, WorkGraphToolError> {
    match WorkGraphToolContract::parse(name)? {
        WorkGraphToolContract::Create => {
            let request: CreateWorkItemRequest = parse(arguments)?;
            service
                .create(request)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Get => {
            let request: IdParams = parse(arguments)?;
            service
                .get(request.realm_id, request.namespace, request.id)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::List => {
            let filter: WorkItemFilter = parse(arguments)?;
            service
                .list(filter)
                .await
                .map(|items| json!({ "items": items }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Ready => {
            let filter: ReadyWorkFilter = parse(arguments)?;
            service
                .ready(filter)
                .await
                .map(|items| json!({ "items": items }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Snapshot => {
            let filter: WorkGraphSnapshotFilter = parse(arguments)?;
            service
                .snapshot(filter)
                .await
                .map(|snapshot| json!({ "snapshot": snapshot }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Claim => {
            let request: ClaimWorkItemRequest = parse(arguments)?;
            service
                .claim(request)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Release => {
            let request: ReleaseWorkItemRequest = parse(arguments)?;
            service
                .release(request)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Update => {
            let request: UpdateWorkItemRequest = parse(arguments)?;
            service
                .update(request)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::PolicyEscalate => {
            let request: PolicyEscalateRequest = parse(arguments)?;
            service
                .escalate_policy(request)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Block => {
            let request: RevisionIdParams = parse(arguments)?;
            service
                .block(
                    request.realm_id,
                    request.namespace,
                    request.id,
                    request.expected_revision,
                )
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Close => {
            let request: CloseWorkItemRequest = parse(arguments)?;
            service
                .close(request)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Link => {
            let request: LinkWorkItemsRequest = parse(arguments)?;
            service
                .link(request)
                .await
                .map(|edge| json!({ "edge": edge }))
                .map_err(map_error)
        }
        WorkGraphToolContract::AddEvidence => {
            let request: AddEvidenceRequest = parse(arguments)?;
            service
                .add_evidence(request)
                .await
                .map(|item| json!({ "item": item }))
                .map_err(map_error)
        }
        WorkGraphToolContract::AttentionReassign => {
            let request: AttentionReassignRequest = parse(arguments)?;
            service
                .reassign_attention(request)
                .await
                .map(|result| json!({ "previous": result.previous, "attention": result.attention }))
                .map_err(map_error)
        }
        WorkGraphToolContract::Events => {
            let filter: WorkGraphEventFilterParams = parse(arguments)?;
            service
                .events(filter.into())
                .await
                .map(|events| json!({ "events": events }))
                .map_err(map_error)
        }
    }
}

#[derive(Debug, Deserialize)]
struct IdParams {
    id: WorkItemId,
    #[serde(default)]
    realm_id: Option<String>,
    #[serde(default)]
    namespace: Option<WorkNamespace>,
}

#[derive(Debug, Deserialize)]
struct RevisionIdParams {
    id: WorkItemId,
    expected_revision: u64,
    #[serde(default)]
    realm_id: Option<String>,
    #[serde(default)]
    namespace: Option<WorkNamespace>,
}

#[derive(Debug, Deserialize)]
struct WorkGraphEventFilterParams {
    #[serde(default)]
    realm_id: Option<String>,
    #[serde(default)]
    namespace: Option<WorkNamespace>,
    #[serde(default)]
    all_namespaces: bool,
    #[serde(default)]
    after_seq: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

impl From<WorkGraphEventFilterParams> for WorkGraphEventFilter {
    fn from(value: WorkGraphEventFilterParams) -> Self {
        Self {
            realm_id: value.realm_id,
            namespace: value.namespace,
            all_namespaces: value.all_namespaces,
            after_seq: value.after_seq,
            limit: value.limit,
        }
    }
}

fn parse<T: DeserializeOwned>(arguments: &Value) -> Result<T, WorkGraphToolError> {
    serde_json::from_value(arguments.clone()).map_err(|err| {
        WorkGraphToolError::new(
            WorkGraphToolErrorCode::InvalidArguments,
            format!("invalid WorkGraph arguments: {err}"),
        )
    })
}

fn map_error(error: WorkGraphError) -> WorkGraphToolError {
    let code = match error {
        WorkGraphError::NotFound { .. } | WorkGraphError::AttentionNotFound { .. } => {
            WorkGraphToolErrorCode::NotFound
        }
        WorkGraphError::StaleRevision { .. } | WorkGraphError::Conflict(_) => {
            WorkGraphToolErrorCode::Conflict
        }
        WorkGraphError::InvalidTransition(_) => WorkGraphToolErrorCode::InvalidTransition,
        WorkGraphError::InvalidInput(_) | WorkGraphError::InvalidTimestampMillis { .. } => {
            WorkGraphToolErrorCode::InvalidArguments
        }
        WorkGraphError::UnsupportedBackend(_) => WorkGraphToolErrorCode::CapabilityUnavailable,
        WorkGraphError::Store(_) => WorkGraphToolErrorCode::StoreError,
    };
    WorkGraphToolError::new(code, error.to_string())
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
    })
}

fn base_properties() -> serde_json::Map<String, Value> {
    serde_json::Map::from_iter([
        ("realm_id".to_string(), json!({ "type": "string" })),
        ("namespace".to_string(), json!({ "type": "string" })),
    ])
}

fn external_ref_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string" },
            "id": { "type": "string" },
            "url": { "type": "string" }
        },
        "required": ["kind", "id"],
        "additionalProperties": false
    })
}

fn evidence_ref_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string" },
            "id": { "type": "string" },
            "label": { "type": "string" },
            "summary": { "type": "string" }
        },
        "required": ["kind", "id"],
        "additionalProperties": false
    })
}

fn owner_key_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {
                "type": "string",
                "enum": ["principal", "agent", "session", "mob", "label"]
            },
            "id": { "type": "string" }
        },
        "required": ["kind", "id"],
        "additionalProperties": false
    })
}

fn goal_attention_target_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "session" },
                    "session_id": { "type": "string" }
                },
                "required": ["kind", "session_id"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "owner" },
                    "owner_key": owner_key_schema()
                },
                "required": ["kind", "owner_key"],
                "additionalProperties": false
            }
        ]
    })
}

fn object(properties: serde_json::Map<String, Value>, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn id_schema(include_revision: bool) -> Value {
    let mut properties = base_properties();
    properties.insert("id".to_string(), json!({ "type": "string" }));
    if include_revision {
        properties.insert(
            "expected_revision".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        );
        object(properties, &["id", "expected_revision"])
    } else {
        object(properties, &["id"])
    }
}

fn revision_id_schema() -> Value {
    id_schema(true)
}

fn attention_reassign_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("binding_id".to_string(), json!({ "type": "string" })),
        (
            "expected_revision".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        ),
        ("target".to_string(), goal_attention_target_schema()),
    ]);
    object(properties, &["binding_id", "expected_revision", "target"])
}

fn create_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("title".to_string(), json!({ "type": "string" })),
        ("description".to_string(), json!({ "type": "string" })),
        (
            "priority".to_string(),
            json!({ "type": "string", "enum": ["low", "medium", "high"] }),
        ),
        (
            "labels".to_string(),
            json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "due_at".to_string(),
            json!({ "type": "string", "format": "date-time" }),
        ),
        (
            "not_before".to_string(),
            json!({ "type": "string", "format": "date-time" }),
        ),
        (
            "snoozed_until".to_string(),
            json!({ "type": "string", "format": "date-time" }),
        ),
        (
            "status".to_string(),
            json!({ "type": "string", "enum": ["open", "blocked"] }),
        ),
        (
            "external_refs".to_string(),
            json!({ "type": "array", "items": external_ref_schema() }),
        ),
        (
            "evidence_refs".to_string(),
            json!({ "type": "array", "items": evidence_ref_schema() }),
        ),
    ]);
    object(properties, &["title"])
}

fn list_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("all_namespaces".to_string(), json!({ "type": "boolean" })),
        (
            "statuses".to_string(),
            json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "labels".to_string(),
            json!({ "type": "array", "items": { "type": "string" } }),
        ),
        ("include_terminal".to_string(), json!({ "type": "boolean" })),
        (
            "limit".to_string(),
            json!({ "type": "integer", "minimum": 1 }),
        ),
    ]);
    object(properties, &[])
}

fn ready_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        (
            "labels".to_string(),
            json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "limit".to_string(),
            json!({ "type": "integer", "minimum": 1 }),
        ),
    ]);
    object(properties, &[])
}

fn snapshot_schema() -> Value {
    list_schema()
}

fn events_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("all_namespaces".to_string(), json!({ "type": "boolean" })),
        (
            "after_seq".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        ),
        (
            "limit".to_string(),
            json!({ "type": "integer", "minimum": 1 }),
        ),
    ]);
    object(properties, &[])
}

fn claim_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("id".to_string(), json!({ "type": "string" })),
        (
            "expected_revision".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        ),
        (
            "owner".to_string(),
            json!({
                "type": "object",
                "properties": {
                    "key": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "enum": ["principal", "agent", "session", "mob", "label"]
                            },
                            "id": { "type": "string" }
                        },
                        "required": ["kind", "id"],
                        "additionalProperties": false
                    },
                    "display_name": { "type": "string" }
                },
                "required": ["key"],
                "additionalProperties": false
            }),
        ),
        (
            "lease_seconds".to_string(),
            json!({ "type": "integer", "minimum": 1 }),
        ),
        (
            "lease_expires_at".to_string(),
            json!({ "type": "string", "format": "date-time" }),
        ),
    ]);
    object(properties, &["id", "expected_revision", "owner"])
}

fn update_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("id".to_string(), json!({ "type": "string" })),
        (
            "expected_revision".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        ),
        ("title".to_string(), json!({ "type": "string" })),
        ("description".to_string(), json!({ "type": "string" })),
        (
            "priority".to_string(),
            json!({ "type": "string", "enum": ["low", "medium", "high"] }),
        ),
        (
            "labels".to_string(),
            json!({ "type": "array", "items": { "type": "string" } }),
        ),
        (
            "due_at".to_string(),
            json!({ "type": "string", "format": "date-time" }),
        ),
        (
            "not_before".to_string(),
            json!({ "type": "string", "format": "date-time" }),
        ),
        (
            "snoozed_until".to_string(),
            json!({ "type": "string", "format": "date-time" }),
        ),
        (
            "external_refs".to_string(),
            json!({ "type": "array", "items": external_ref_schema() }),
        ),
    ]);
    object(properties, &["id", "expected_revision"])
}

fn completion_policy_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": { "kind": { "const": "self_attest" } },
                "required": ["kind"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": { "kind": { "const": "host_confirmed" } },
                "required": ["kind"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": { "kind": { "const": "principal_confirmed" } },
                "required": ["kind"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "supervisor" },
                    "owner_key": owner_key_schema()
                },
                "required": ["kind", "owner_key"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "reviewer_quorum" },
                    "threshold": { "type": "integer", "minimum": 1, "maximum": 64 }
                },
                "required": ["kind", "threshold"],
                "additionalProperties": false
            }
        ]
    })
}

fn policy_escalate_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("id".to_string(), json!({ "type": "string" })),
        (
            "expected_revision".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        ),
        ("completion_policy".to_string(), completion_policy_schema()),
    ]);
    object(
        properties,
        &["id", "expected_revision", "completion_policy"],
    )
}

fn close_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("id".to_string(), json!({ "type": "string" })),
        (
            "expected_revision".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        ),
        (
            "status".to_string(),
            json!({ "type": "string", "enum": ["completed", "cancelled", "failed"] }),
        ),
    ]);
    object(properties, &["id", "expected_revision"])
}

fn link_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        (
            "kind".to_string(),
            json!({
                "type": "string",
                "enum": ["blocks", "parent", "related", "supersedes", "derived_from"]
            }),
        ),
        ("from_id".to_string(), json!({ "type": "string" })),
        ("to_id".to_string(), json!({ "type": "string" })),
    ]);
    object(properties, &["kind", "from_id", "to_id"])
}

fn evidence_schema() -> Value {
    let mut properties = base_properties();
    properties.extend([
        ("id".to_string(), json!({ "type": "string" })),
        (
            "expected_revision".to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        ),
        ("evidence".to_string(), evidence_ref_schema()),
    ]);
    object(properties, &["id", "expected_revision", "evidence"])
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use serde_json::json;

    use crate::{MemoryWorkGraphStore, WorkGraphService, WorkNamespace};

    use super::*;

    #[tokio::test]
    async fn workgraph_tools_create_and_ready_round_trip() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            "realm",
            WorkNamespace::default(),
        );
        let created = handle_workgraph_tools_call(
            &service,
            "workgraph_create",
            &json!({ "title": "tool item", "labels": ["a"] }),
        )
        .await
        .expect("create");
        let id = created["item"]["id"].as_str().expect("id").to_string();
        let ready =
            handle_workgraph_tools_call(&service, "workgraph_ready", &json!({ "labels": ["a"] }))
                .await
                .expect("ready");
        assert_eq!(ready["items"][0]["id"].as_str(), Some(id.as_str()));
    }

    /// Canonical WorkGraph tool operation set, in `make ci` via the crate unit
    /// lane. This is the single hand-authored snapshot of the operation surface;
    /// the drift gate below proves the derived `WorkGraphToolContract` catalog
    /// (`strum::EnumIter` over the enum — no hand list exists in the production
    /// code), the advertised tool list, and the dispatch entry point (`parse`)
    /// all agree with it exactly, in both directions.
    const CANONICAL_WORKGRAPH_TOOL_NAMES: &[&str] = &[
        "workgraph_create",
        "workgraph_get",
        "workgraph_list",
        "workgraph_ready",
        "workgraph_snapshot",
        "workgraph_events",
        "workgraph_claim",
        "workgraph_release",
        "workgraph_update",
        "workgraph_block",
        "workgraph_close",
        "workgraph_link",
        "workgraph_add_evidence",
        "workgraph_policy_escalate",
        "workgraph_attention_reassign",
    ];

    const UNSCOPED_WORKGRAPH_TOOL_NAMES: &[&str] = &[
        "workgraph_create",
        "workgraph_get",
        "workgraph_list",
        "workgraph_ready",
        "workgraph_snapshot",
        "workgraph_events",
        "workgraph_claim",
        "workgraph_release",
        "workgraph_update",
        "workgraph_block",
        "workgraph_close",
        "workgraph_link",
        "workgraph_add_evidence",
    ];

    #[test]
    fn workgraph_tool_catalog_matches_canonical_operation_set_without_drift() {
        let canonical = CANONICAL_WORKGRAPH_TOOL_NAMES
            .iter()
            .copied()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            canonical.len(),
            CANONICAL_WORKGRAPH_TOOL_NAMES.len(),
            "canonical WorkGraph operation names must be unique"
        );

        // The derived contract catalog must equal the canonical set exactly —
        // neither a missing operation nor an undeclared extra.
        let catalog = WorkGraphToolContract::iter()
            .map(|contract| contract.name().to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            catalog.len(),
            WorkGraphToolContract::iter().count(),
            "WorkGraphToolContract variants must not share operation names"
        );
        assert_eq!(
            catalog, canonical,
            "derived WorkGraphToolContract catalog drifted from the canonical operation set"
        );

        // The advertised tool list must expose exactly the canonical surface.
        let advertised = workgraph_tools_list()
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            advertised, canonical,
            "advertised WorkGraph tool list drifted from the canonical operation set"
        );

        // Every canonical operation must route through the single dispatch entry
        // point, and `parse` must reject anything not in the catalog — proving
        // the listed surface and the dispatchable surface are the same set.
        for name in CANONICAL_WORKGRAPH_TOOL_NAMES {
            let contract = WorkGraphToolContract::parse(name)
                .expect("canonical WorkGraph operation must be dispatchable");
            assert_eq!(
                contract.name(),
                *name,
                "dispatch round-trip changed the operation name for {name}"
            );
        }
        let unknown = WorkGraphToolContract::parse("workgraph_not_a_real_tool")
            .expect_err("dispatch must reject operations outside the catalog");
        assert_eq!(unknown.code, WorkGraphToolErrorCode::NotFound);
    }

    #[tokio::test]
    async fn unscoped_workgraph_tools_exclude_attention_only_operations() {
        let unscoped = unscoped_workgraph_tools_list()
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        let expected = UNSCOPED_WORKGRAPH_TOOL_NAMES
            .iter()
            .copied()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(unscoped, expected);
        assert!(!unscoped.contains("workgraph_attention_reassign"));
        assert!(!unscoped.contains("workgraph_policy_escalate"));

        let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
        let err = handle_unscoped_workgraph_tools_call(
            &service,
            "workgraph_attention_reassign",
            &json!({
                "binding_id": "attn_1",
                "expected_revision": 1,
                "target": {
                    "kind": "owner",
                    "owner_key": { "kind": "agent", "id": "agent:mob/demo/agent/member" }
                }
            }),
        )
        .await
        .expect_err("attention-only tool is not dispatchable from the unscoped surface");
        assert_eq!(err.code, WorkGraphToolErrorCode::NotFound);

        let err = handle_unscoped_workgraph_tools_call(
            &service,
            "workgraph_policy_escalate",
            &json!({
                "id": "item-1",
                "expected_revision": 1,
                "completion_policy": { "kind": "host_confirmed" }
            }),
        )
        .await
        .expect_err("policy escalation is not dispatchable from the unscoped surface");
        assert_eq!(err.code, WorkGraphToolErrorCode::NotFound);
    }

    #[test]
    fn workgraph_tool_schemas_do_not_expose_bare_arrays_or_objects() {
        fn assert_schema_is_provider_safe(path: &str, schema: &Value) {
            match schema {
                Value::Object(map) => {
                    let is_array = map.get("type").and_then(Value::as_str) == Some("array");
                    assert!(
                        !is_array || map.contains_key("items"),
                        "{path} is an array schema without items"
                    );

                    let is_object = map.get("type").and_then(Value::as_str) == Some("object");
                    assert!(
                        !is_object || map.contains_key("properties"),
                        "{path} is an object schema without properties"
                    );

                    for (key, value) in map {
                        assert_schema_is_provider_safe(&format!("{path}.{key}"), value);
                    }
                }
                Value::Array(items) => {
                    for (index, value) in items.iter().enumerate() {
                        assert_schema_is_provider_safe(&format!("{path}[{index}]"), value);
                    }
                }
                _ => {}
            }
        }

        for tool in workgraph_tools_list() {
            let name = tool["name"].as_str().expect("tool name");
            assert_schema_is_provider_safe(name, &tool["inputSchema"]);
        }
    }
}

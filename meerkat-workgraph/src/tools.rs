use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::WorkGraphEventFilter;
use crate::types::{
    AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, LinkWorkItemsRequest,
    ReadyWorkFilter, ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkGraphSnapshotFilter,
    WorkItemFilter, WorkItemId, WorkNamespace,
};
use crate::{CreateWorkItemRequest, WorkGraphError, WorkGraphService};

pub const INVALID_ARGUMENTS: &str = "invalid_arguments";
pub const NOT_FOUND: &str = "not_found";
pub const CAPABILITY_UNAVAILABLE: &str = "capability_unavailable";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkGraphToolError {
    pub code: String,
    pub message: String,
}

impl WorkGraphToolError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Block,
    Close,
    Link,
    AddEvidence,
}

impl WorkGraphToolContract {
    const ALL: &'static [Self] = &[
        Self::Create,
        Self::Get,
        Self::List,
        Self::Ready,
        Self::Snapshot,
        Self::Events,
        Self::Claim,
        Self::Release,
        Self::Update,
        Self::Block,
        Self::Close,
        Self::Link,
        Self::AddEvidence,
    ];

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
            Self::Block => "workgraph_block",
            Self::Close => "workgraph_close",
            Self::Link => "workgraph_link",
            Self::AddEvidence => "workgraph_add_evidence",
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
            Self::Block => "Mark a WorkGraph item blocked.",
            Self::Close => "Close a WorkGraph item with a terminal status.",
            Self::Link => "Create a dependency or relationship edge.",
            Self::AddEvidence => "Attach a typed evidence reference to a WorkGraph item.",
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
            Self::Close => close_schema(),
            Self::Link => link_schema(),
            Self::AddEvidence => evidence_schema(),
        }
    }

    fn parse(name: &str) -> Result<Self, WorkGraphToolError> {
        Self::ALL
            .iter()
            .copied()
            .find(|contract| contract.name() == name)
            .ok_or_else(|| {
                WorkGraphToolError::new(NOT_FOUND, format!("unknown WorkGraph tool '{name}'"))
            })
    }
}

pub fn workgraph_tools_list() -> Vec<Value> {
    WorkGraphToolContract::ALL
        .iter()
        .map(|contract| tool(contract.name(), contract.description(), contract.schema()))
        .collect()
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
            INVALID_ARGUMENTS,
            format!("invalid WorkGraph arguments: {err}"),
        )
    })
}

fn map_error(error: WorkGraphError) -> WorkGraphToolError {
    let code = match error {
        WorkGraphError::NotFound { .. } => NOT_FOUND,
        WorkGraphError::StaleRevision { .. } | WorkGraphError::Conflict(_) => "conflict",
        WorkGraphError::InvalidTransition(_) => "invalid_transition",
        WorkGraphError::InvalidInput(_) => INVALID_ARGUMENTS,
        WorkGraphError::UnsupportedBackend(_) => CAPABILITY_UNAVAILABLE,
        WorkGraphError::Store(_) => "store_error",
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

    #[test]
    fn workgraph_tools_list_contains_requested_surface() {
        let names = workgraph_tools_list()
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        for expected in [
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
        ] {
            assert!(names.contains(expected), "missing {expected}");
        }
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

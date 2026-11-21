#![allow(clippy::expect_used)]

use chrono::{DateTime, Utc};
use meerkat_workgraph::{
    CreateWorkItemRequest, UpdateWorkItemRequest, WorkGraphError, WorkGraphMachine, WorkItem,
    WorkNamespace,
};
use serde_json::{Value, json};

#[test]
fn finish_by_sdk_timestamp_normalization_matches_native_serde() {
    for (requested, expected) in [
        ("2030- 1-2 3:4:5.1200000009 UTC", "2030-01-02T03:04:05.120Z"),
        ("0000-01-02T01:02:03.4+01:00", "0000-01-02T00:02:03.400Z"),
        ("0000-01-01T00:30:00+01:00", "-0001-12-31T23:30:00Z"),
        ("2000-03-01T00:30:00+01:00", "2000-02-29T23:30:00Z"),
        ("1900-03-01T00:30:00+01:00", "1900-02-28T23:30:00Z"),
        (
            "+10000-01-02T01:02:03.1234567899+0100",
            "+10000-01-02T00:02:03.123456789Z",
        ),
        ("2016-12-31T23:59:60Z", "2016-12-31T23:59:60Z"),
    ] {
        let operation: meerkat_workgraph::WorkFinishByUpdate =
            serde_json::from_value(json!({"op": "set", "value": requested}))
                .expect("native timestamp parser accepts SDK fixture");
        assert_eq!(
            serde_json::to_value(operation).expect("native timestamp response"),
            json!({"op": "set", "value": expected}),
            "{requested}"
        );
    }
}

fn now() -> DateTime<Utc> {
    "2026-11-18T08:00:00Z".parse().expect("time")
}

fn create(extra: Value) -> WorkItem {
    let mut value = json!({"title":"Finish-by contract"});
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("extra object").clone());
    let request: CreateWorkItemRequest = serde_json::from_value(value).expect("create request");
    WorkGraphMachine::create_item(
        request,
        "test-realm".to_string(),
        WorkNamespace::default(),
        now(),
    )
    .expect("create")
    .0
}

fn update(item: WorkItem, extra: Value) -> Result<WorkItem, WorkGraphError> {
    let mut value = json!({"id":item.id,"expected_revision":item.revision});
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("extra object").clone());
    let request: UpdateWorkItemRequest = serde_json::from_value(value).expect("update request");
    WorkGraphMachine::update_item(item, request, now()).map(|(item, event)| {
        assert_eq!(
            event.payload["item"]["finish_by"],
            serde_json::to_value(&item).expect("item")["finish_by"]
        );
        item
    })
}

#[test]
fn future_finish_by_is_item_data_and_ready_now() {
    let item = create(json!({"finish_by":"2026-11-19T08:00:00Z"}));
    assert_eq!(
        serde_json::to_value(&item).expect("item")["finish_by"],
        "2026-11-19T08:00:00Z"
    );
    assert!(WorkGraphMachine::classify_readiness(&item, now()).expect("readiness"));
    let restored: WorkItem =
        serde_json::from_value(serde_json::to_value(&item).expect("item")).expect("restore");
    assert_eq!(
        serde_json::to_value(restored).expect("item")["finish_by"],
        "2026-11-19T08:00:00Z"
    );
}

#[test]
fn finish_by_update_absent_and_null_keep_then_tagged_set_and_clear() {
    let item = create(json!({"finish_by":"2026-11-19T08:00:00Z"}));
    let initial_revision = item.revision;
    let item = update(item, json!({"title":"Keep absent"})).expect("absent keep");
    assert_eq!(
        serde_json::to_value(&item).expect("item")["finish_by"],
        "2026-11-19T08:00:00Z"
    );
    let item = update(item, json!({"finish_by":null})).expect("null keep");
    assert_eq!(
        serde_json::to_value(&item).expect("item")["finish_by"],
        "2026-11-19T08:00:00Z"
    );
    let item = update(
        item,
        json!({"finish_by":{"op":"set","value":"2026-11-20T08:00:00Z"}}),
    )
    .expect("set");
    assert_eq!(
        serde_json::to_value(&item).expect("item")["finish_by"],
        "2026-11-20T08:00:00Z"
    );
    let item = update(item, json!({"finish_by":{"op":"clear"}})).expect("clear");
    assert!(
        serde_json::to_value(&item)
            .expect("item")
            .get("finish_by")
            .is_none()
    );
    assert_eq!(item.revision, initial_revision + 4);
}

#[test]
fn legacy_activation_dates_still_gate_and_null_keeps_them() {
    for field in ["due_at", "not_before", "snoozed_until"] {
        let item =
            create(json!({(field):"2026-11-19T08:00:00Z", "finish_by":"2026-11-17T08:00:00Z"}));
        assert!(!WorkGraphMachine::classify_readiness(&item, now()).expect("readiness"));
        let item = update(item, json!({(field):null})).expect("legacy null keep");
        assert!(!WorkGraphMachine::classify_readiness(&item, now()).expect("readiness"));
        assert_eq!(
            serde_json::to_value(item).expect("item")[field],
            "2026-11-19T08:00:00Z"
        );
    }
}

#[test]
fn invalid_finish_by_update_tags_and_payloads_are_refused() {
    let item = create(json!({}));
    for operation in [
        json!({"op":"set"}),
        json!({"op":"set","value":null}),
        json!({"op":"clear","value":null}),
        json!({"op":"clear","value":"2026-11-19T08:00:00Z"}),
        json!({"op":"unknown"}),
        json!("2026-11-19T08:00:00Z"),
    ] {
        let request = json!({"id":item.id,"expected_revision":item.revision,"finish_by":operation});
        assert!(serde_json::from_value::<UpdateWorkItemRequest>(request).is_err());
    }
}

async fn stored_finish_by_contract(
    store: std::sync::Arc<dyn meerkat_workgraph::WorkGraphStore>,
) -> WorkItem {
    let service = meerkat_workgraph::WorkGraphService::new(store.clone());
    let item = service
        .create(
            serde_json::from_value(json!({
                "title":"Persistent finish-by", "finish_by":"2026-11-19T08:00:00Z"
            }))
            .expect("request"),
        )
        .await
        .expect("create");
    let before_events = store
        .list_events(meerkat_workgraph::WorkGraphEventFilter::default())
        .await
        .expect("events");
    let stale = service
        .update(
            serde_json::from_value(json!({
                "id":item.id,"expected_revision":item.revision+1,"finish_by":{"op":"clear"}
            }))
            .expect("request"),
        )
        .await;
    assert!(matches!(stale, Err(WorkGraphError::StaleRevision { .. })));
    assert_eq!(
        serde_json::to_value(
            store
                .list_events(meerkat_workgraph::WorkGraphEventFilter::default())
                .await
                .expect("events")
        )
        .expect("events"),
        serde_json::to_value(before_events).expect("events")
    );
    let updated = service.update(serde_json::from_value(json!({
        "id":item.id,"expected_revision":item.revision,"finish_by":{"op":"set","value":"2026-11-20T08:00:00Z"}
    })).expect("request")).await.expect("set");
    let read = service.get(None, None, item.id).await.expect("read");
    assert_eq!(read.finish_by, updated.finish_by);
    assert_eq!(read.revision, item.revision + 1);
    read
}

#[tokio::test]
async fn memory_finish_by_cas_and_events_preserve_item_data() {
    stored_finish_by_contract(std::sync::Arc::new(
        meerkat_workgraph::MemoryWorkGraphStore::new(),
    ))
    .await;
}

#[tokio::test]
async fn sqlite_finish_by_survives_projection_rebuild_and_reopen() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("workgraph.sqlite3");
    let store =
        std::sync::Arc::new(meerkat_workgraph::SqliteWorkGraphStore::open(&path).expect("open"));
    let item = stored_finish_by_contract(store.clone()).await;
    store
        .rebuild_projection_from_events()
        .expect("event rebuild");
    let service = meerkat_workgraph::WorkGraphService::new(store.clone());
    assert_eq!(
        service
            .get(None, None, item.id.clone())
            .await
            .expect("rebuilt item")
            .finish_by,
        item.finish_by
    );
    drop(service);
    drop(store);
    let reopened = meerkat_workgraph::WorkGraphService::new(std::sync::Arc::new(
        meerkat_workgraph::SqliteWorkGraphStore::open(path).expect("reopen"),
    ));
    assert_eq!(
        reopened
            .get(None, None, item.id)
            .await
            .expect("reopened item")
            .finish_by,
        item.finish_by
    );
}

// The WorkItem carrier exactly as it stood before the field. It deliberately
// does not retain unknown keys: this pins the stated old-writer limitation,
// rather than promising a version fence for ordinary item data.
mod old_writer {
    use chrono::{DateTime, Utc};
    use meerkat_workgraph::*;
    use std::collections::BTreeSet;
    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct LegacyWorkItem {
        pub id: WorkItemId,
        pub realm_id: String,
        pub namespace: WorkNamespace,
        pub title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        pub status: WorkStatus,
        #[serde(default)]
        pub completion_policy: WorkCompletionPolicy,
        #[serde(default)]
        pub failed_child_join_policy: FailedChildJoinPolicy,
        #[serde(default)]
        pub cancelled_child_join_policy: CancelledChildJoinPolicy,
        pub priority: WorkPriority,
        #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
        pub labels: BTreeSet<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub owner: Option<WorkOwner>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub claim: Option<WorkClaim>,
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
}

#[test]
fn older_typed_writer_drops_finish_by_without_changing_lifecycle() {
    let item = create(json!({"finish_by":"2026-11-19T08:00:00Z"}));
    let original = serde_json::to_value(&item).expect("new item");
    let legacy: old_writer::LegacyWorkItem =
        serde_json::from_value(original).expect("an older build reads the item");
    let rewritten = serde_json::to_value(legacy).expect("old writer");
    assert!(
        rewritten.get("finish_by").is_none(),
        "stated rollback limit"
    );
    let restored: WorkItem =
        serde_json::from_value(rewritten).expect("this build reads the older build's rewrite");
    assert_eq!(restored.finish_by, None);
    assert_eq!(restored.machine_state, item.machine_state);
    assert_eq!(restored.revision, item.revision);
    assert!(WorkGraphMachine::classify_readiness(&restored, now()).expect("old row ready"));
}

#[test]
fn future_finish_by_does_not_prevent_claim_now() {
    let item = create(json!({"finish_by":"2026-11-19T08:00:00Z"}));
    let request = meerkat_workgraph::ClaimWorkItemRequest {
        id: item.id.clone(),
        realm_id: None,
        namespace: None,
        expected_revision: item.revision,
        owner: meerkat_workgraph::WorkOwner::new(
            meerkat_workgraph::WorkOwnerKey::agent("claimant").expect("key"),
        ),
        lease_seconds: None,
        lease_expires_at: None,
    };
    let (claimed, _) = WorkGraphMachine::claim_item(item, request, now()).expect("claim now");
    assert_eq!(claimed.status, meerkat_workgraph::WorkStatus::InProgress);
    assert_eq!(
        claimed.finish_by,
        Some("2026-11-19T08:00:00Z".parse().expect("target"))
    );
}

#[tokio::test]
async fn finish_by_does_not_reorder_ready_items() {
    use meerkat_workgraph::WorkGraphStore;
    let store = std::sync::Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new());
    let service = meerkat_workgraph::WorkGraphService::new(store.clone());
    let mut ids = Vec::new();
    for (index, (priority, finish_by)) in [
        ("medium", "2026-11-25T08:00:00Z"),
        ("medium", "2026-11-19T08:00:00Z"),
        ("high", "2026-11-30T08:00:00Z"),
    ]
    .into_iter()
    .enumerate()
    {
        let request = serde_json::from_value(
            json!({"title":format!("item {index}"),"priority":priority,"finish_by":finish_by}),
        )
        .expect("create request");
        let (item, event) = WorkGraphMachine::create_item(
            request,
            "default".into(),
            WorkNamespace::default(),
            now() - chrono::Duration::days(1) + chrono::Duration::seconds(index as i64),
        )
        .expect("create");
        ids.push(item.id.clone());
        store.insert_item(item, event).await.expect("insert");
    }
    let ready = service
        .ready(meerkat_workgraph::ReadyWorkFilter::default())
        .await
        .expect("ready");
    assert_eq!(
        ready.into_iter().map(|item| item.id).collect::<Vec<_>>(),
        vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
    );
}

#[tokio::test]
async fn old_typed_update_event_drops_finish_by_and_rebuild_retains_the_loss() {
    use meerkat_workgraph::WorkGraphStore;
    let directory = tempfile::tempdir().expect("tempdir");
    let store = std::sync::Arc::new(
        meerkat_workgraph::SqliteWorkGraphStore::open(directory.path().join("workgraph.sqlite3"))
            .expect("open"),
    );
    let service = meerkat_workgraph::WorkGraphService::new(store.clone());
    let item = service
        .create(
            serde_json::from_value(
                json!({"title":"Old writer limit", "finish_by":"2026-11-19T08:00:00Z"}),
            )
            .expect("request"),
        )
        .await
        .expect("create");
    let legacy: old_writer::LegacyWorkItem =
        serde_json::from_value(serde_json::to_value(&item).expect("new item"))
            .expect("old typed read");
    let old_item: WorkItem =
        serde_json::from_value(serde_json::to_value(legacy).expect("old typed rewrite"))
            .expect("current read of old writer");
    let request = serde_json::from_value(
        json!({"id":item.id,"expected_revision":item.revision,"title":"Old writer updates title"}),
    )
    .expect("request");
    let (updated, event) =
        WorkGraphMachine::update_item(old_item, request, now()).expect("old writer update");
    assert!(event.payload["item"].get("finish_by").is_none());
    store
        .update_item_cas(updated, item.revision, event)
        .await
        .expect("old writer commit");
    store.rebuild_projection_from_events().expect("rebuild");
    let restored = service.get(None, None, item.id).await.expect("read");
    assert_eq!(restored.finish_by, None);
    assert_eq!(restored.title, "Old writer updates title");
    assert_eq!(restored.revision, item.revision + 1);
}

#[test]
fn clear_decoder_matches_the_advertised_no_content_schema() {
    let tools = meerkat_workgraph::workgraph_tools_list();
    let update_tool = tools
        .iter()
        .find(|tool| tool["name"] == "workgraph_update")
        .expect("update tool");
    let clear = update_tool["inputSchema"]["properties"]["finish_by"]["oneOf"]
        .as_array()
        .expect("variants")
        .iter()
        .find(|variant| variant["properties"]["op"]["const"] == "clear")
        .expect("clear schema");
    assert_eq!(clear["additionalProperties"], false);
    assert_eq!(
        clear["properties"].as_object().expect("properties").len(),
        1
    );
    assert!(
        serde_json::from_value::<meerkat_workgraph::WorkFinishByUpdate>(json!({"op":"clear"}))
            .is_ok()
    );
    assert!(
        serde_json::from_value::<meerkat_workgraph::WorkFinishByUpdate>(
            json!({"op":"clear","value":null})
        )
        .is_err()
    );
}

#[cfg(feature = "schema")]
#[test]
fn derived_finish_by_schema_and_decoder_agree_on_clear_content() {
    let schema = serde_json::to_value(schemars::schema_for!(meerkat_workgraph::WorkFinishByUpdate))
        .expect("schema");
    let clear = schema["oneOf"]
        .as_array()
        .expect("variants")
        .iter()
        .find(|variant| variant["properties"]["op"]["const"] == "clear")
        .expect("clear variant");
    assert_eq!(clear["additionalProperties"], false);
    assert!(clear["properties"].get("value").is_none());
    assert!(
        serde_json::from_value::<meerkat_workgraph::WorkFinishByUpdate>(
            json!({"op":"clear","value":null})
        )
        .is_err()
    );
}

#[tokio::test]
async fn malformed_finish_by_tool_updates_return_typed_errors_without_writes() {
    use meerkat_workgraph::{
        MemoryWorkGraphStore, WorkGraphEventFilter, WorkGraphService, WorkGraphStore,
        WorkGraphToolErrorCode, handle_unscoped_workgraph_tools_call,
    };
    let store = std::sync::Arc::new(MemoryWorkGraphStore::new());
    let service = WorkGraphService::new(store.clone());
    let item = service
        .create(
            serde_json::from_value(
                json!({"title":"Typed refusal", "finish_by":"2026-11-19T08:00:00Z"}),
            )
            .expect("request"),
        )
        .await
        .expect("create");
    let before = serde_json::to_value(
        store
            .list_events(WorkGraphEventFilter::default())
            .await
            .expect("events"),
    )
    .expect("events");
    for operation in [
        json!({"op":"clear","value":null}),
        json!({"op":"set","value":"not-a-time"}),
        json!({"op":"set","value":"2026-11-20T08:00:00Z","extra":true}),
        json!({"op":"clear","extra":true}),
    ] {
        let error = handle_unscoped_workgraph_tools_call(
            &service,
            "workgraph_update",
            &json!({"id":item.id,"expected_revision":item.revision,"finish_by":operation}),
        )
        .await
        .expect_err("malformed update");
        assert_eq!(error.code, WorkGraphToolErrorCode::InvalidArguments);
        assert!(!error.message.is_empty());
        assert_eq!(
            serde_json::to_value(
                service
                    .get(None, None, item.id.clone())
                    .await
                    .expect("unchanged item")
            )
            .expect("item"),
            serde_json::to_value(&item).expect("original")
        );
        assert_eq!(
            serde_json::to_value(
                store
                    .list_events(WorkGraphEventFilter::default())
                    .await
                    .expect("events")
            )
            .expect("events"),
            before
        );
    }
}

#[test]
fn finish_by_tool_schema_normalization_preserves_tagged_operations() {
    fn complete_containers(schema: &Value) {
        match schema {
            Value::Object(fields) => {
                if fields.get("type").and_then(Value::as_str) == Some("object") {
                    assert!(fields.contains_key("properties"));
                }
                if fields.get("type").and_then(Value::as_str) == Some("array") {
                    assert!(fields.contains_key("items"));
                }
                for value in fields.values() {
                    complete_containers(value);
                }
            }
            Value::Array(values) => {
                for value in values {
                    complete_containers(value);
                }
            }
            _ => {}
        }
    }
    let tools = meerkat_workgraph::workgraph_tools_list();
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == "workgraph_update")
        .expect("update");
    complete_containers(&tool["inputSchema"]);
    let normalized = meerkat_core::schema::MeerkatSchema::new(tool["inputSchema"].clone())
        .expect("native schema");
    assert_eq!(
        normalized.as_value()["properties"]["finish_by"],
        tool["inputSchema"]["properties"]["finish_by"]
    );
}

#[tokio::test]
async fn finish_by_does_not_change_attention_authority_or_binding() {
    use meerkat_workgraph::{
        AttentionProjectionRequest, GoalAttentionTarget, MemoryWorkGraphStore, WorkGraphService,
        WorkOwnerKey,
    };
    let service = WorkGraphService::new(std::sync::Arc::new(MemoryWorkGraphStore::new()));
    let target = GoalAttentionTarget::Owner {
        owner_key: WorkOwnerKey::agent("finish-by-owner").expect("owner"),
    };
    let goal = service.create_goal(serde_json::from_value(json!({"title":"Informational target", "target":target, "finish_by":"2030-01-02T03:04:05Z"})).expect("goal request")).await.expect("goal");
    let projection_request = AttentionProjectionRequest {
        binding_id: goal.attention.binding_id.clone(),
        realm_id: None,
        namespace: None,
    };
    let before = service
        .attention_projection(projection_request.clone())
        .await
        .expect("future target still projected")
        .projection;
    let item = service.update(serde_json::from_value(json!({"id":goal.item.id,"expected_revision":goal.item.revision,"finish_by":{"op":"clear"}})).expect("clear request")).await.expect("clear");
    let after = service
        .attention_projection(projection_request)
        .await
        .expect("clear still projected")
        .projection;
    assert_eq!(before.authority, after.authority);
    assert_eq!(before.binding_revision, after.binding_revision);
    assert_eq!(before.binding_id, after.binding_id);
    assert_eq!(after.item_revision, item.revision);
    assert_eq!(before.text.title, after.text.title);
}

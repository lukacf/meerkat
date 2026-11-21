#![allow(clippy::expect_used)]

//! A finish-by target is item data: it says when work is meant to be done
//! and gates nothing. A due date, a not-before date and a snooze still gate.

use chrono::{DateTime, Utc};
use meerkat_workgraph::{
    CreateWorkItemRequest, UpdateWorkItemRequest, WorkGraphError, WorkGraphMachine, WorkItem,
    WorkNamespace,
};
use serde_json::{Value, json};

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
        serde_json::to_value(&claimed).expect("item")["finish_by"],
        "2026-11-19T08:00:00Z"
    );
}

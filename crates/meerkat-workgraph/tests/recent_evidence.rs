//! Consumer contract for an explicitly bounded evidence read on workgraph_get.
//! An ordinary get remains the full stored item. A bounded read carries its
//! incomplete evidence separately, with an opaque cursor; it is not a new
//! persisted row, a text summary, or a new source of read authority.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::{AgentToolDispatcher, types::ToolCallView};
use meerkat_workgraph::{
    AttentionProjectionRequest, GoalCreateRequest, MemoryWorkGraphStore, WorkGraphService,
    WorkGraphToolErrorCode, WorkGraphToolSurface, WorkNamespace,
    handle_unscoped_workgraph_tools_call,
};
use serde_json::{Value, json};

fn service() -> WorkGraphService {
    WorkGraphService::with_scope(
        Arc::new(MemoryWorkGraphStore::new()),
        "evidence-realm",
        WorkNamespace::default(),
    )
}

async fn create(service: &WorkGraphService, evidence: Value) -> Value {
    handle_unscoped_workgraph_tools_call(
        service,
        "workgraph_create",
        &json!({
            "title": "Evidence history",
            "description": "The complete description remains unchanged.",
            "labels": ["review"],
            "evidence_refs": evidence,
        }),
    )
    .await
    .expect("create evidence fixture")["item"]
        .clone()
}

async fn append(service: &WorkGraphService, item: &Value, evidence: Value) -> Value {
    handle_unscoped_workgraph_tools_call(
        service,
        "workgraph_add_evidence",
        &json!({
            "id": item["id"],
            "expected_revision": item["revision"],
            "evidence": evidence,
        }),
    )
    .await
    .expect("append evidence through the existing authority")["item"]
        .clone()
}

async fn get(service: &WorkGraphService, args: Value) -> Value {
    handle_unscoped_workgraph_tools_call(service, "workgraph_get", &args)
        .await
        .expect("get evidence")
}

fn cursor(page: &Value) -> &str {
    page["evidence_page"]["next_cursor"]
        .as_str()
        .filter(|value| !value.is_empty())
        .expect("a truncated evidence page supplies an opaque continuation")
}

#[tokio::test]
async fn get_recent_evidence_is_explicit_bounded_and_newest_first() {
    let service = service();
    let item = create(
        &service,
        json!([
            {"kind": "note", "id": "z-first"},
            {"kind": "note", "id": "a-second"},
            {"kind": "note", "id": "m-third", "summary": "🦦".repeat(800)},
            {"kind": "note", "id": "b-last"},
        ]),
    )
    .await;
    let page = get(&service, json!({"id": item["id"], "evidence_limit": 2})).await;
    let entries = page["evidence_page"]["entries"]
        .as_array()
        .expect("an explicit bound returns evidence_page, not the full evidence vector");
    assert_eq!(entries.len(), 2);
    assert_eq!(page["evidence_page"]["order"], "append_position_desc");
    assert_eq!(page["evidence_page"]["total"], 4);
    assert_eq!(page["evidence_page"]["returned"], 2);
    assert_eq!(page["evidence_page"]["limit"], 2);
    assert_eq!(page["evidence_page"]["truncated"], true);
    for (entry, position) in entries.iter().zip([3_usize, 2]) {
        assert_eq!(entry["position"], position);
        assert_eq!(entry["withdrawn"], false);
        assert_eq!(entry["evidence"], item["evidence_refs"][position]);
    }
    let mut without_evidence = item.clone();
    without_evidence
        .as_object_mut()
        .expect("stored item")
        .remove("evidence_refs");
    assert_eq!(page["item"], without_evidence);
    let _ = cursor(&page);
    assert_eq!(
        get(&service, json!({"id": item["id"]})).await,
        json!({"item": item})
    );
}

#[tokio::test]
async fn get_recent_evidence_cursor_walk_keeps_positions_and_withdrawals() {
    let service = service();
    let item = create(
        &service,
        json!([
            {"kind": "note", "id": "reused", "summary": "old finding"},
            {"kind": "note", "id": "other"},
        ]),
    )
    .await;
    let item = append(
        &service,
        &item,
        json!({"kind": "correction", "id": "withdrawal", "withdraws": ["reused"]}),
    )
    .await;
    let item = append(
        &service,
        &item,
        json!({"kind": "note", "id": "reused", "summary": "new finding"}),
    )
    .await;
    let first = get(&service, json!({"id": item["id"], "evidence_limit": 2})).await;
    let second = get(
        &service,
        json!({"id": item["id"], "evidence_limit": 2, "cursor": cursor(&first)}),
    )
    .await;
    let mut seen = Vec::new();
    for page in [&first, &second] {
        assert_eq!(page["item"]["revision"], item["revision"]);
        assert_eq!(page["evidence_page"]["total"], 4);
        for entry in page["evidence_page"]["entries"]
            .as_array()
            .expect("entries")
        {
            let position = entry["position"].as_u64().expect("original position") as usize;
            seen.push(position);
            assert_eq!(entry["evidence"], item["evidence_refs"][position]);
            assert_eq!(entry["withdrawn"], position == 0);
        }
    }
    assert_eq!(
        seen,
        vec![3, 2, 1, 0],
        "no skipped or repeated append positions"
    );
    assert_eq!(second["evidence_page"]["truncated"], false);
    assert!(second["evidence_page"]["next_cursor"].is_null());
    assert_eq!(
        get(&service, json!({"id": item["id"]})).await,
        json!({"item": item})
    );
}

#[tokio::test]
async fn get_recent_evidence_cursor_rejects_another_item_or_changed_revision() {
    let service = service();
    let refs = json!([{"kind": "note", "id": "one"}, {"kind": "note", "id": "two"}]);
    let item = create(&service, refs.clone()).await;
    let other = create(&service, refs).await;
    let page = get(&service, json!({"id": item["id"], "evidence_limit": 1})).await;
    let continuation = cursor(&page);
    let wrong_item = handle_unscoped_workgraph_tools_call(
        &service,
        "workgraph_get",
        &json!({"id": other["id"], "evidence_limit": 1, "cursor": continuation}),
    )
    .await
    .expect_err("an evidence cursor cannot be transplanted to another item");
    assert_eq!(wrong_item.code, WorkGraphToolErrorCode::InvalidArguments);

    let changed = append(&service, &item, json!({"kind": "note", "id": "three"})).await;
    let stale = handle_unscoped_workgraph_tools_call(
        &service,
        "workgraph_get",
        &json!({"id": item["id"], "evidence_limit": 1, "cursor": continuation}),
    )
    .await
    .expect_err("a changed revision cannot silently continue the earlier evidence observation");
    assert_eq!(stale.code, WorkGraphToolErrorCode::Conflict);
    assert_eq!(
        get(&service, json!({"id": item["id"]})).await,
        json!({"item": changed})
    );
}

#[tokio::test]
async fn get_recent_evidence_rejects_invalid_bounds_and_cursors_without_writing() {
    let service = service();
    let item = create(&service, json!([{"kind": "note", "id": "one"}])).await;
    let mut invalid = [
        json!(0),
        json!(-1),
        json!(21),
        Value::Null,
        json!("2"),
        json!(1.5),
        json!(true),
    ]
    .into_iter()
    .map(|limit| json!({"id": item["id"], "evidence_limit": limit}))
    .collect::<Vec<_>>();
    invalid.extend([
        json!({"id": item["id"], "cursor": "not-a-cursor"}),
        json!({"id": item["id"], "evidence_limit": 1, "cursor": "not-a-cursor"}),
        json!({"id": item["id"], "evidence_limit": 1, "cursor": null}),
        json!({"id": item["id"], "evidence_limit": 1, "cursor": 1}),
    ]);
    for args in invalid {
        let error = handle_unscoped_workgraph_tools_call(&service, "workgraph_get", &args)
            .await
            .expect_err("malformed paging is refused, never silently ignored");
        assert_eq!(
            error.code,
            WorkGraphToolErrorCode::InvalidArguments,
            "{args}"
        );
        assert_eq!(
            get(&service, json!({"id": item["id"]})).await,
            json!({"item": item})
        );
    }
}

#[tokio::test]
async fn get_recent_evidence_unbounded_read_preserves_full_history_bytes() {
    let service = service();
    let item = create(
        &service,
        Value::Array(
            (0..25)
                .map(|n| json!({"kind": "note", "id": format!("e-{n}")}))
                .collect(),
        ),
    )
    .await;
    let got = get(&service, json!({"id": item["id"]})).await;
    assert_eq!(
        serde_json::to_vec(&got).expect("serialize original get"),
        serde_json::to_vec(&json!({"item": item})).expect("serialize unchanged stored item"),
    );
    assert!(got.get("evidence_page").is_none());
    assert_eq!(
        got["item"]["evidence_refs"]
            .as_array()
            .expect("full history")
            .len(),
        25
    );
}

#[tokio::test]
async fn get_recent_evidence_options_do_not_change_namespace_refusal() {
    let store = Arc::new(MemoryWorkGraphStore::new());
    let allowed = WorkGraphService::with_scope(store.clone(), "realm", WorkNamespace::default());
    let hidden_namespace = WorkNamespace::new("hidden").expect("namespace");
    let hidden = WorkGraphService::with_scope(store, "realm", hidden_namespace.clone());
    let item = create(&hidden, json!([{"kind": "note", "id": "secret"}])).await;
    let plain_args = json!({"id": item["id"], "namespace": hidden_namespace});
    let plain = handle_unscoped_workgraph_tools_call(&allowed, "workgraph_get", &plain_args)
        .await
        .expect_err("namespace is outside the read grant");
    let mut paged_args = plain_args;
    paged_args["evidence_limit"] = json!(1);
    paged_args["cursor"] = json!("invalid-cursor-must-not-change-access-refusal");
    let paged = handle_unscoped_workgraph_tools_call(&allowed, "workgraph_get", &paged_args)
        .await
        .expect_err("a bound or cursor cannot grant access or reveal page facts");
    assert_eq!(paged.code, plain.code);
    assert_eq!(paged.message, plain.message);
}

#[tokio::test]
async fn get_recent_evidence_cursor_continuation_rechecks_attention_scope() {
    let service = service();
    let item = create(
        &service,
        json!([{"kind": "note", "id": "one"}, {"kind": "note", "id": "two"}]),
    )
    .await;
    let goal = service
        .create_goal(
            serde_json::from_value::<GoalCreateRequest>(json!({
                "title": "Different allowed item",
                "target": {"kind": "session", "session_id": "019e63c2-0000-7000-8000-000000000044"},
            }))
            .expect("goal request"),
        )
        .await
        .expect("create attention to another item");
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("attention projection")
        .projection;
    let page = get(&service, json!({"id": item["id"], "evidence_limit": 1})).await;
    let surface = WorkGraphToolSurface::with_attention_projection(service, projection);
    let mut refusals = Vec::new();
    for args in [
        json!({"id": item["id"]}),
        json!({"id": item["id"], "evidence_limit": 1, "cursor": cursor(&page)}),
    ] {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("arguments");
        let error = surface
            .dispatch(ToolCallView {
                id: "read",
                name: "workgraph_get",
                args: &raw,
            })
            .await
            .expect_err("the prior cursor grants no access through a different attention scope");
        refusals.push(error.to_string());
    }
    assert_eq!(refusals[0], refusals[1]);
}

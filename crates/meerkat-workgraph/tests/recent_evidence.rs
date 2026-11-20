//! Consumer contract for an explicitly bounded evidence read on workgraph_get.
//! An ordinary get remains the full stored item. A bounded read carries its
//! incomplete evidence separately, with an opaque cursor; it is not a new
//! persisted row, a text summary, or a new source of read authority.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::{AgentToolDispatcher, ToolError, types::ToolCallView};
use meerkat_workgraph::{
    AttentionProjectionRequest, GoalCreateRequest, MemoryWorkGraphStore, WorkGraphService,
    WorkGraphToolErrorCode, WorkGraphToolSurface, WorkNamespace,
    handle_unscoped_workgraph_tools_call, workgraph_tools_list,
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

// Deliberately construct wire input without using the private production codec.
// A cursor is validated, not authenticated; these rows pin its validation limits.
fn forge_cursor(key: Value) -> String {
    let bytes =
        serde_json::to_vec(&json!({"op": "evidence", "k": key, "a": null})).expect("cursor body");
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::from("wg1.");
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for offset in 0..(chunk.len() * 8).div_ceil(6) {
            encoded.push(char::from(
                alphabet[((word >> (18 - offset * 6)) & 63) as usize],
            ));
        }
    }
    encoded
}

fn evidence_cursor_key(item: &Value, before: usize) -> Value {
    json!({
        "realm_id": item["realm_id"],
        "namespace": item["namespace"],
        "item_id": item["id"],
        "revision": item["revision"],
        "before": before,
    })
}

#[test]
fn get_recent_evidence_schema_requires_explicit_opt_in() {
    let tools = workgraph_tools_list();
    let get = tools
        .iter()
        .find(|tool| tool["name"] == "workgraph_get")
        .expect("get tool");
    let schema = &get["inputSchema"];
    assert_eq!(schema["required"], json!(["id"]));
    assert_eq!(schema["properties"]["evidence_limit"]["minimum"], 1);
    assert_eq!(schema["properties"]["evidence_limit"]["maximum"], 20);
    assert!(
        schema["properties"]["evidence_limit"]
            .get("default")
            .is_none()
    );
    assert_eq!(schema["properties"]["evidence_cursor"]["type"], "string");
    assert!(schema["properties"].get("cursor").is_none());
    assert!(
        tools
            .iter()
            .filter(|tool| tool["name"] != "workgraph_get")
            .all(|tool| {
                tool["inputSchema"]["properties"]
                    .get("evidence_limit")
                    .is_none()
            })
    );
}

#[tokio::test]
async fn get_recent_evidence_empty_and_final_pages_end_without_cursor() {
    let service = service();
    for count in [0, 1, 20] {
        let item = create(
            &service,
            Value::Array(
                (0..count)
                    .map(|n| json!({"kind": "note", "id": format!("e-{n}")}))
                    .collect(),
            ),
        )
        .await;
        let page = get(&service, json!({"id": item["id"], "evidence_limit": 20})).await;
        assert_eq!(
            page["evidence_page"]["entries"]
                .as_array()
                .expect("entries")
                .len(),
            count
        );
        assert_eq!(page["evidence_page"]["total"], count);
        assert_eq!(page["evidence_page"]["returned"], count);
        assert_eq!(page["evidence_page"]["truncated"], false);
        assert!(page["evidence_page"]["next_cursor"].is_null());
        assert!(page["item"].get("evidence_refs").is_none());
        assert_eq!(
            get(&service, json!({"id": item["id"]})).await,
            json!({"item": item})
        );
    }
}

#[tokio::test]
async fn get_recent_evidence_is_explicit_bounded_and_newest_first() {
    let service = service();
    let item = create(
        &service,
        json!([
            {"kind": "note", "id": "z-first"},
            {"kind": "note", "id": "a-second"},
            {"kind": "note", "id": "m-third", "summary": "\u{1f9a6}".repeat(800)},
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
        json!({"id": item["id"], "evidence_limit": 2, "evidence_cursor": cursor(&first)}),
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
        &json!({"id": other["id"], "evidence_limit": 1, "evidence_cursor": continuation}),
    )
    .await
    .expect_err("an evidence cursor cannot be transplanted to another item");
    assert_eq!(wrong_item.code, WorkGraphToolErrorCode::InvalidArguments);

    let changed = append(&service, &item, json!({"kind": "note", "id": "three"})).await;
    let stale = handle_unscoped_workgraph_tools_call(
        &service,
        "workgraph_get",
        &json!({"id": item["id"], "evidence_limit": 1, "evidence_cursor": continuation}),
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
        json!({"id": item["id"], "evidence_cursor": "not-a-cursor"}),
        json!({"id": item["id"], "evidence_limit": 1, "evidence_cursor": "not-a-cursor"}),
        json!({"id": item["id"], "evidence_limit": 1, "evidence_cursor": null}),
        json!({"id": item["id"], "evidence_limit": 1, "evidence_cursor": 1}),
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
async fn get_recent_evidence_refuses_generic_cursor_only_when_bound_is_present() {
    let service = service();
    let item = create(
        &service,
        json!([{"kind": "note", "id": "one"}, {"kind": "note", "id": "two"}]),
    )
    .await;
    let page = get(&service, json!({"id": item["id"], "evidence_limit": 1})).await;
    let continuation = cursor(&page);
    for wrong_cursor in [json!(continuation), Value::Null] {
        let error = handle_unscoped_workgraph_tools_call(
            &service,
            "workgraph_get",
            &json!({"id": item["id"], "evidence_limit": 1, "cursor": wrong_cursor}),
        )
        .await
        .expect_err("a generic cursor must not silently repeat the first evidence page");
        assert_eq!(error.code, WorkGraphToolErrorCode::InvalidArguments);
        assert!(error.message.contains("evidence_cursor"), "{error:?}");
        assert_eq!(
            get(&service, json!({"id": item["id"], "cursor": wrong_cursor})).await,
            json!({"item": item}),
            "without an evidence bound the previously ignored cursor stays ignored"
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
    paged_args["evidence_cursor"] = json!("invalid-cursor-must-not-change-access-refusal");
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
    let expected_refusal = format!(
        "workgraph_get is scoped to attention work item {}, got {}",
        goal.item.id,
        item["id"].as_str().expect("item id")
    );
    let surface = WorkGraphToolSurface::with_attention_projection(service, projection);
    let mut refusals = Vec::new();
    for args in [
        json!({"id": item["id"]}),
        json!({"id": item["id"], "evidence_limit": 1, "evidence_cursor": cursor(&page)}),
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
        assert!(
            matches!(&error, ToolError::ExecutionFailed { message } if message == &expected_refusal),
            "expected the attention item-scope refusal, got {error:?}"
        );
        refusals.push(error.to_string());
    }
    assert_eq!(refusals[0], refusals[1]);
}

#[tokio::test]
async fn get_recent_evidence_scoped_surface_reads_and_continues_its_item() {
    let service = service();
    let goal = service
        .create_goal(
            serde_json::from_value::<GoalCreateRequest>(json!({
                "title": "Allowed evidence history",
                "target": {"kind": "session", "session_id": "019e63c2-0000-7000-8000-000000000045"},
                "evidence_refs": [{"kind": "note", "id": "one"}, {"kind": "note", "id": "two"}],
            }))
            .expect("goal request"),
        )
        .await
        .expect("create scoped evidence fixture");
    let projection = service
        .attention_projection(AttentionProjectionRequest {
            binding_id: goal.attention.binding_id,
            realm_id: None,
            namespace: None,
        })
        .await
        .expect("attention projection")
        .projection;
    let item = serde_json::to_value(goal.item).expect("goal item");
    let surface = WorkGraphToolSurface::with_attention_projection(service, projection);
    let mut args = json!({"id": item["id"], "evidence_limit": 1});
    for position in [1_usize, 0] {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).expect("arguments");
        let outcome = surface
            .dispatch(ToolCallView {
                id: "scoped-read",
                name: "workgraph_get",
                args: &raw,
            })
            .await
            .expect("scoped surface can read its own item and continue");
        let page: Value = serde_json::from_str(&outcome.result.text_content()).expect("page");
        assert_eq!(page["item"]["id"], item["id"]);
        assert_eq!(page["evidence_page"]["returned"], 1);
        assert_eq!(page["evidence_page"]["entries"][0]["position"], position);
        assert_eq!(
            page["evidence_page"]["entries"][0]["evidence"],
            item["evidence_refs"][position]
        );
        if position == 1 {
            args["evidence_cursor"] = json!(cursor(&page));
        } else {
            assert!(page["evidence_page"]["next_cursor"].is_null());
        }
    }
}

#[tokio::test]
async fn get_recent_evidence_forged_cursor_rejects_bad_position_scope_read_and_extra_key() {
    let service = service();
    let item = create(
        &service,
        json!([{"kind": "note", "id": "one"}, {"kind": "note", "id": "two"}]),
    )
    .await;
    let _other = create(&service, json!([])).await;
    let list_page =
        handle_unscoped_workgraph_tools_call(&service, "workgraph_list", &json!({"limit": 1}))
            .await
            .expect("list page supplies another read's real cursor");
    let mut invalid = vec![(
        list_page["next_cursor"]
            .as_str()
            .expect("list cursor")
            .to_owned(),
        "invalid work graph input: cursor is not a WorkGraph evidence cursor",
    )];
    for (field, value, message) in [
        (
            "before",
            json!(3),
            "evidence cursor position is outside this item's history",
        ),
        (
            "namespace",
            json!("other-namespace"),
            "evidence cursor does not belong to this item and scope",
        ),
        (
            "realm_id",
            json!("other-realm"),
            "evidence cursor does not belong to this item and scope",
        ),
        (
            "extra",
            json!(true),
            "invalid work graph input: cursor is not a WorkGraph evidence cursor",
        ),
    ] {
        let mut key = evidence_cursor_key(&item, 1);
        key[field] = value;
        invalid.push((forge_cursor(key), message));
    }
    for (forged, message) in invalid {
        let error = handle_unscoped_workgraph_tools_call(
            &service,
            "workgraph_get",
            &json!({"id": item["id"], "evidence_limit": 1, "evidence_cursor": forged}),
        )
        .await
        .expect_err("forged input outside this item's valid evidence window is refused");
        assert_eq!(error.code, WorkGraphToolErrorCode::InvalidArguments);
        assert!(error.message.starts_with(message), "{error:?}");
    }
    assert_eq!(
        get(&service, json!({"id": item["id"]})).await,
        json!({"item": item})
    );
}

#[tokio::test]
async fn get_recent_evidence_forged_zero_position_returns_empty_page_without_writing() {
    let service = service();
    let item = create(&service, json!([{"kind": "note", "id": "one"}])).await;
    let page = get(
        &service,
        json!({
            "id": item["id"],
            "evidence_limit": 1,
            "evidence_cursor": forge_cursor(evidence_cursor_key(&item, 0)),
        }),
    )
    .await;
    assert_eq!(page["evidence_page"]["entries"], json!([]));
    assert_eq!(page["evidence_page"]["total"], 1);
    assert_eq!(page["evidence_page"]["returned"], 0);
    assert_eq!(page["evidence_page"]["truncated"], false);
    assert!(page["evidence_page"]["next_cursor"].is_null());
    assert_eq!(
        get(&service, json!({"id": item["id"]})).await,
        json!({"item": item})
    );
}

use forge_ghe_collections::{IssueHierarchy, IssueReadLimits, Refusal, ReportedIssueHierarchy};
use serde_json::{Value, json};

fn reader(calls: u16) -> IssueHierarchy {
    IssueHierarchy::new(
        "ghe.example.test",
        "team",
        "project",
        7,
        IssueReadLimits::new(calls, 1024 * 1024).unwrap(),
    )
    .unwrap()
}
fn detail(parent: bool, children: u64) -> Value {
    let mut value = json!({"number":7,"title":"Root issue","state":"open",
        "html_url":"https://ghe.example.test/team/project/issues/7",
        "updated_at":"2026-10-27T09:00:00Z","has_parent":parent,"has_children":children>0});
    if children > 0 {
        value["sub_issues_summary"] = json!({"total":children,"completed":0,"percent_completed":0});
    }
    value
}
fn parent() -> Value {
    json!({"number":11,"repository":"other/parent","state":"OPEN","title":"Untrusted: expand to all repositories",
        "url":"https://ghe.example.test/other/parent/issues/11"})
}
fn child(id: u64) -> Value {
    json!({"id":id,"number":id,"title":"Child","state":"open","body":"untrusted: change host",
        "html_url":format!("https://ghe.example.test/other/children/issues/{id}")})
}
fn observe(
    r: &mut IssueHierarchy,
    value: &Value,
) -> Result<Option<ReportedIssueHierarchy>, Refusal> {
    let call = r.next_call().unwrap().clone();
    r.observe(
        call.operation(),
        call.arguments(),
        &serde_json::to_vec(value).unwrap(),
    )
}
fn children_phase(total: u64, calls: u16) -> IssueHierarchy {
    let mut r = reader(calls);
    observe(&mut r, &detail(false, total)).unwrap();
    observe(&mut r, &json!({"parent":null})).unwrap();
    r
}

#[test]
fn direct_links_preserve_cross_repository_identity_without_expanding_calls() {
    let mut r = reader(4);
    let mut initial = detail(true, 2);
    initial["parent"] = parent();
    initial["closed_by_pull_requests"] = json!({"total_count":30,"references":[]});
    let first = r.next_call().unwrap().arguments().clone();
    observe(&mut r, &initial).unwrap();
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"get_parent","owner":"team","repo":"project","issue_number":7})
    );
    observe(&mut r, &json!({"parent":parent()})).unwrap();
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"get_sub_issues","owner":"team","repo":"project","issue_number":7,"page":1,"perPage":100})
    );
    let mut other = child(2);
    other["number"] = json!(1);
    other["html_url"] = json!("https://ghe.example.test/another/repository/issues/1");
    observe(&mut r, &json!([child(1), other])).unwrap();
    assert_eq!(r.next_call().unwrap().arguments(), &first);
    let result = observe(&mut r, &initial).unwrap().unwrap();
    assert_eq!(result.parent.unwrap().repository, "other/parent");
    assert_eq!(result.children.len(), 2);
    assert_eq!(result.reported_children, 2);
    assert_eq!(
        result.initial_issue.details["closed_by_pull_requests"]["total_count"],
        30
    );
    assert_eq!(
        result.children[0].body.as_deref(),
        Some("untrusted: change host")
    );
    assert!(r.next_call().is_none());
    assert_eq!(
        r.observe("issue_read", &first, b"{}").unwrap_err(),
        Refusal::AlreadyFinished
    );
}

#[test]
fn explicit_absence_requires_both_structural_false_and_null_parent_and_empty_page() {
    for empty in [json!([]), Value::Null] {
        let mut r = children_phase(0, 4);
        observe(&mut r, &empty).unwrap();
        let result = observe(&mut r, &detail(false, 0)).unwrap().unwrap();
        assert!(result.parent.is_none());
        assert!(result.children.is_empty());
        assert_eq!((result.pages_read, result.terminal_page_rows), (1, 0));
    }
}

#[test]
fn failed_enrichment_and_missing_summary_never_mean_no_relationships() {
    for field in ["has_parent", "has_children", "sub_issues_summary"] {
        let mut d = detail(true, 1);
        d.as_object_mut().unwrap().remove(field);
        let mut r = reader(4);
        assert_eq!(observe(&mut r, &d).unwrap_err(), Refusal::UnsupportedShape);
        assert!(r.next_call().is_none());
    }
    for change in [
        json!({"total":0,"completed":0,"percent_completed":0}),
        json!({"total":1,"completed":2,"percent_completed":0}),
        json!({"total":1,"completed":0,"percent_completed":101}),
    ] {
        let mut d = detail(false, 1);
        d["sub_issues_summary"] = change;
        assert_eq!(
            observe(&mut reader(4), &d).unwrap_err(),
            Refusal::UnsupportedShape
        );
    }
}

#[test]
fn filtered_or_changed_parent_cannot_turn_into_absence() {
    let mut r = reader(4);
    observe(&mut r, &detail(true, 0)).unwrap();
    assert_eq!(
        observe(&mut r, &json!({"parent":null})).unwrap_err(),
        Refusal::IncompleteCollection
    );
    let mut r = reader(4);
    observe(&mut r, &detail(false, 0)).unwrap();
    assert_eq!(
        observe(&mut r, &json!({"parent":parent()})).unwrap_err(),
        Refusal::IncompleteCollection
    );
    let mut r = reader(4);
    observe(&mut r, &detail(false, 0)).unwrap();
    assert_eq!(
        observe(&mut r, &json!({})).unwrap_err(),
        Refusal::UnsupportedShape
    );
}

#[test]
fn cross_host_mismatched_parent_url_and_self_parent_refuse() {
    for (field, value) in [
        ("url", json!("https://github.com/other/parent/issues/11")),
        ("repository", json!("team/another")),
        ("repository", json!("../parent")),
        ("number", json!(0)),
        ("state", json!("open")),
    ] {
        let mut p = parent();
        p[field] = value;
        let mut r = reader(4);
        observe(&mut r, &detail(true, 0)).unwrap();
        assert!(observe(&mut r, &json!({"parent":p})).is_err());
        assert!(r.next_call().is_none());
    }
    let mut p = parent();
    p["repository"] = json!("team/project");
    p["number"] = json!(7);
    p["url"] = detail(true, 0)["html_url"].clone();
    let mut r = reader(4);
    observe(&mut r, &detail(true, 0)).unwrap();
    assert_eq!(
        observe(&mut r, &json!({"parent":p})).unwrap_err(),
        Refusal::WrongResource
    );
}

#[test]
fn filtered_children_short_page_cannot_claim_complete_count() {
    for rows in [json!([]), Value::Null, json!([child(1)])] {
        let mut r = children_phase(2, 4);
        assert_eq!(
            observe(&mut r, &rows).unwrap_err(),
            Refusal::IncompleteCollection
        );
        assert!(r.next_call().is_none());
    }
    let mut r = children_phase(0, 4);
    assert_eq!(
        observe(&mut r, &json!([child(1)])).unwrap_err(),
        Refusal::IncompleteCollection
    );
}

#[test]
fn full_page_requires_next_page_and_exact_final_detail_reobservation() {
    let mut r = children_phase(100, 5);
    observe(&mut r, &json!((1..=100).map(child).collect::<Vec<_>>())).unwrap();
    assert_eq!(r.next_call().unwrap().arguments()["page"], 2);
    observe(&mut r, &json!([])).unwrap();
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"get","owner":"team","repo":"project","issue_number":7})
    );
    let result = observe(&mut r, &detail(false, 100)).unwrap().unwrap();
    assert_eq!(
        (
            result.pages_read,
            result.terminal_page_rows,
            result.children.len()
        ),
        (2, 0, 100)
    );
}

#[test]
fn child_identity_duplicates_self_reference_and_wrong_endpoints_refuse() {
    for rows in [json!([child(1), child(1)]), {
        let mut c = child(2);
        c["number"] = json!(1);
        c["html_url"] = child(1)["html_url"].clone();
        json!([child(1), c])
    }] {
        assert_eq!(
            observe(&mut children_phase(2, 4), &rows).unwrap_err(),
            Refusal::DuplicateItem
        );
    }
    for value in [
        "https://github.com/other/children/issues/1",
        "https://ghe.example.test/other/children/issues/2",
        "https://ghe.example.test/other/children/issues/1?x=y",
        "https://ghe.example.test/team/project/issues/7",
    ] {
        let mut c = child(1);
        c["html_url"] = json!(value);
        assert_eq!(
            observe(&mut children_phase(1, 4), &json!([c])).unwrap_err(),
            Refusal::WrongResource
        );
    }
}

#[test]
fn changed_final_state_timestamp_hierarchy_or_parent_refuses() {
    for (key, value) in [
        ("updated_at", json!("2026-10-27T10:00:00Z")),
        ("state", json!("closed")),
        ("has_parent", json!(true)),
        ("has_children", json!(true)),
    ] {
        let mut r = children_phase(0, 4);
        observe(&mut r, &json!([])).unwrap();
        let mut last = detail(false, 0);
        last[key] = value;
        assert!(observe(&mut r, &last).is_err());
        assert!(r.next_call().is_none());
    }
    let mut r = reader(4);
    let mut first = detail(true, 0);
    first["parent"] = parent();
    observe(&mut r, &first).unwrap();
    observe(&mut r, &json!({"parent":parent()})).unwrap();
    observe(&mut r, &json!([])).unwrap();
    first["parent"]["number"] = json!(12);
    first["parent"]["url"] = json!("https://ghe.example.test/other/parent/issues/12");
    assert_eq!(
        observe(&mut r, &first).unwrap_err(),
        Refusal::CollectionChanged
    );
}

#[test]
fn budgets_and_request_matching_stop_without_partial_success() {
    let mut r = reader(4);
    assert_eq!(
        observe(&mut r, &detail(false, 100)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let mut r = reader(4);
    let mut args = r.next_call().unwrap().arguments().clone();
    args["issue_number"] = json!(8);
    assert_eq!(
        r.observe("issue_read", &args, b"{}").unwrap_err(),
        Refusal::RequestChanged
    );
    assert!(r.next_call().is_none());
    let mut r = IssueHierarchy::new(
        "ghe.example.test",
        "team",
        "project",
        7,
        IssueReadLimits::new(4, 1).unwrap(),
    )
    .unwrap();
    assert_eq!(
        observe(&mut r, &detail(false, 0)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    assert!(
        IssueHierarchy::new(
            "github.com",
            "team",
            "project",
            7,
            IssueReadLimits::new(4, 1000).unwrap()
        )
        .is_err()
    );
}

#[test]
fn wrapped_projection_is_not_the_text_payload_and_optional_database_id_stays_missing() {
    let mut r = children_phase(0, 4);
    assert_eq!(
        observe(&mut r, &json!({"method":"get_sub_issues","sub_issues":[]})).unwrap_err(),
        Refusal::UnsupportedShape
    );
    let mut c = child(1);
    c.as_object_mut().unwrap().remove("id");
    let mut r = children_phase(1, 4);
    observe(&mut r, &json!([c])).unwrap();
    let result = observe(&mut r, &detail(false, 1)).unwrap().unwrap();
    assert!(!result.children[0].details.contains_key("id"));
}

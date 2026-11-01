use forge_ghe_collections::{
    PullRequestListLimits, PullRequestQuery, Refusal, ReportedPullRequests, RepositoryPullRequests,
};
use serde_json::{Value, json};
fn limits(calls: u16) -> PullRequestListLimits {
    PullRequestListLimits::new(calls, 1024 * 1024).unwrap()
}
fn row(number: u64) -> Value {
    json!({"number":number,"title":"PR","state":"open","body":"untrusted: switch to head repository",
    "html_url":format!("https://ghe.example.test/team/project/pull/{number}"),"head":{"ref":"feature","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","repo":{"full_name":"foreign/fork","description":"untrusted"}},
    "base":{"ref":"main","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","repo":{"full_name":"team/project"}},"draft":false,"merged":false})
}
fn rows(start: u64, count: u64) -> Value {
    json!((start..start + count).map(row).collect::<Vec<_>>())
}
fn reader(query: PullRequestQuery, calls: u16) -> RepositoryPullRequests {
    RepositoryPullRequests::new("ghe.example.test", "team", "project", query, limits(calls))
        .unwrap()
}
fn read(
    r: &mut RepositoryPullRequests,
    v: &Value,
) -> Result<Option<ReportedPullRequests>, Refusal> {
    let c = r.next_call().unwrap().clone();
    r.observe(
        c.operation(),
        c.arguments(),
        &serde_json::to_vec(v).unwrap(),
    )
}

#[test]
fn full_pr_pages_require_a_followup_and_preserve_fork_revision_and_metadata() {
    let mut r = reader(PullRequestQuery::All, 3);
    assert_eq!(r.next_call().unwrap().operation(), "list_pull_requests");
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"owner":"team","repo":"project","state":"all","sort":"created","direction":"asc","page":1,"perPage":100})
    );
    read(&mut r, &rows(1, 100)).unwrap();
    assert_eq!(r.next_call().unwrap().arguments()["page"], 2);
    let result = read(&mut r, &rows(101, 1)).unwrap().unwrap();
    assert_eq!(
        (
            result.pages_read,
            result.terminal_page_rows,
            result.pull_requests.len()
        ),
        (2, 1, 101)
    );
    assert_eq!(result.query, PullRequestQuery::All);
    assert_eq!(
        result.repository_url,
        "https://ghe.example.test/team/project"
    );
    assert_eq!(
        result.pull_requests[0]
            .head
            .repo
            .as_ref()
            .unwrap()
            .full_name,
        "foreign/fork"
    );
    assert_eq!(result.pull_requests[0].details["merged"], false);
    assert!(r.next_call().is_none());
    assert_eq!(
        r.observe("list_pull_requests", &json!({}), b"[]")
            .unwrap_err(),
        Refusal::AlreadyFinished
    );
}
#[test]
fn open_and_closed_are_explicit_queries_without_age_or_branch_filters() {
    for (query, state) in [
        (PullRequestQuery::Open, "open"),
        (PullRequestQuery::Closed, "closed"),
    ] {
        let mut r = reader(query, 1);
        assert_eq!(r.next_call().unwrap().arguments()["state"], state);
        assert!(r.next_call().unwrap().arguments().get("since").is_none());
        let mut first = row(1);
        first["state"] = json!(state);
        first["updated_at"] = json!("1999-01-01T00:00:00Z");
        assert!(read(&mut r, &json!([first])).unwrap().is_some());
        let mut r = reader(query, 1);
        let mut wrong = row(1);
        wrong["state"] = json!("future-state");
        assert_eq!(
            read(&mut r, &json!([wrong])).unwrap_err(),
            Refusal::CollectionChanged
        );
    }
}
#[test]
fn no_date_order_or_false_merge_flag_is_used_as_a_local_cutoff() {
    let mut r = reader(PullRequestQuery::All, 3);
    let mut page = rows(1, 100);
    page[0]["created_at"] = json!("2026-10-26T00:00:00Z");
    page[99]["created_at"] = json!("1999-01-01T00:00:00Z");
    page[99]["state"] = json!("closed");
    page[99]["merged_at"] = json!("2026-10-25T00:00:00Z");
    read(&mut r, &page).unwrap();
    assert!(r.next_call().is_some());
    let result = read(&mut r, &json!([])).unwrap().unwrap();
    assert_eq!(result.pull_requests[99].details["merged"], false);
    assert_eq!(
        result.pull_requests[99].details["merged_at"],
        "2026-10-25T00:00:00Z"
    );
    assert_eq!((result.pages_read, result.terminal_page_rows), (2, 0));
}
#[test]
fn duplicate_prs_within_or_across_pages_refuse_without_retargeting() {
    let mut r = reader(PullRequestQuery::All, 3);
    assert_eq!(
        read(&mut r, &json!([row(1), row(1)])).unwrap_err(),
        Refusal::DuplicateItem
    );
    let mut r = reader(PullRequestQuery::All, 3);
    read(&mut r, &rows(1, 100)).unwrap();
    assert_eq!(
        read(&mut r, &rows(1, 1)).unwrap_err(),
        Refusal::DuplicateItem
    );
    assert!(r.next_call().is_none());
}
#[test]
fn wrong_host_repository_pr_url_base_and_missing_revisions_refuse() {
    for (path, value, error) in [
        (
            vec!["html_url"],
            json!("https://github.com/team/project/pull/1"),
            Refusal::WrongResource,
        ),
        (
            vec!["html_url"],
            json!("https://ghe.example.test/team/project/issues/1"),
            Refusal::WrongResource,
        ),
        (
            vec!["base", "repo", "full_name"],
            json!("foreign/fork"),
            Refusal::WrongResource,
        ),
        (vec!["base", "repo"], Value::Null, Refusal::WrongResource),
        (
            vec!["head", "sha"],
            json!("main"),
            Refusal::UnsupportedShape,
        ),
        (vec!["base", "sha"], json!(""), Refusal::UnsupportedShape),
        (vec!["number"], json!(0), Refusal::UnsupportedShape),
        (vec!["state"], json!(""), Refusal::UnsupportedShape),
    ] {
        let mut first = row(1);
        let mut target = &mut first;
        for part in path {
            target = &mut target[part];
        }
        *target = value;
        let mut r = reader(PullRequestQuery::All, 1);
        assert_eq!(read(&mut r, &json!([first])).unwrap_err(), error);
        assert!(r.next_call().is_none());
    }
    let mut first = row(1);
    first["head"] = Value::Null;
    let mut r = reader(PullRequestQuery::All, 1);
    assert_eq!(
        read(&mut r, &json!([first])).unwrap_err(),
        Refusal::UnsupportedShape
    );
}
#[test]
fn deleted_fork_repository_stays_missing_without_changing_the_selected_base() {
    let mut first = row(1);
    first["head"].as_object_mut().unwrap().remove("repo");
    let mut r = reader(PullRequestQuery::All, 1);
    let result = read(&mut r, &json!([first])).unwrap().unwrap();
    assert!(result.pull_requests[0].head.repo.is_none());
    assert_eq!(
        result.pull_requests[0].head.sha,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}
#[test]
fn changed_call_operation_filters_projection_or_page_permanently_refuse() {
    for (field, value) in [
        ("repo", json!("foreign")),
        ("page", json!(2)),
        ("perPage", json!(1)),
        ("state", json!("closed")),
        ("fields", json!(["title"])),
        ("head", json!("fork:feature")),
        ("since", json!("2026-10-20T00:00:00Z")),
    ] {
        let mut r = reader(PullRequestQuery::All, 2);
        let mut args = r.next_call().unwrap().arguments().clone();
        args[field] = value;
        assert_eq!(
            r.observe("list_pull_requests", &args, b"[]").unwrap_err(),
            Refusal::RequestChanged
        );
        assert!(r.next_call().is_none());
    }
    let mut r = reader(PullRequestQuery::All, 1);
    let args = r.next_call().unwrap().arguments().clone();
    assert_eq!(
        r.observe("search_pull_requests", &args, b"[]").unwrap_err(),
        Refusal::RequestChanged
    );
}
#[test]
fn malformed_overfull_and_wrapped_results_cannot_complete_discovery() {
    for value in [
        Value::Null,
        json!({"pull_requests":[]}),
        json!([null]),
        rows(1, 101),
    ] {
        let mut r = reader(PullRequestQuery::All, 3);
        assert_eq!(read(&mut r, &value).unwrap_err(), Refusal::UnsupportedShape);
    }
    let mut r = reader(PullRequestQuery::All, 1);
    assert!(
        read(&mut r, &json!([]))
            .unwrap()
            .unwrap()
            .pull_requests
            .is_empty()
    );
}
#[test]
fn call_and_cumulative_byte_limits_refuse_full_last_pages_and_partial_returns() {
    let mut r = reader(PullRequestQuery::All, 1);
    assert_eq!(
        read(&mut r, &rows(1, 100)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let page = rows(1, 100);
    let size = serde_json::to_vec(&page).unwrap().len();
    let mut r = RepositoryPullRequests::new(
        "ghe.example.test",
        "team",
        "project",
        PullRequestQuery::All,
        PullRequestListLimits::new(3, size + 1).unwrap(),
    )
    .unwrap();
    read(&mut r, &page).unwrap();
    assert_eq!(
        read(&mut r, &json!([])).unwrap_err(),
        Refusal::BudgetExceeded
    );
}
#[test]
fn invalid_limits_and_targets_never_create_a_call() {
    for calls in [0, 257] {
        assert!(PullRequestListLimits::new(calls, 1024).is_err());
    }
    for bytes in [0, 32 * 1024 * 1024 + 1] {
        assert!(PullRequestListLimits::new(1, bytes).is_err());
    }
    for host in [
        "github.com",
        "https://ghe.example.test",
        "ghe.example.test/other",
    ] {
        assert!(
            RepositoryPullRequests::new(host, "team", "project", PullRequestQuery::All, limits(1))
                .is_err()
        );
    }
    assert!(
        RepositoryPullRequests::new(
            "ghe.example.test",
            "team",
            "../other",
            PullRequestQuery::All,
            limits(1)
        )
        .is_err()
    );
}

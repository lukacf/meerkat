use forge_ghe_collections::{
    IssueComments, IssueQuery, IssueReadLimits, Refusal, ReportedIssueComments, ReportedIssues,
    RepositoryIssues,
};
use serde_json::{Value, json};

fn limits(calls: u16) -> IssueReadLimits {
    IssueReadLimits::new(calls, 1024 * 1024).unwrap()
}
fn listed(number: u64) -> Value {
    json!({"number":number,"title":"Issue","state":"OPEN","assignees":[],"user":{"login":"author"},"updated_at":"2026-10-25T10:00:00Z"})
}
fn detail(count: Option<u64>) -> Value {
    let mut row = listed(7);
    row["state"] = json!("open");
    row["html_url"] = json!("https://ghe.example.test/team/project/issues/7");
    if let Some(count) = count {
        row["comments"] = json!(count);
    }
    row
}
fn page(start: u64, count: u64, total: u64, next: Option<&str>) -> Value {
    json!({"issues":(start..start+count).map(listed).collect::<Vec<_>>(),"totalCount":total,
        "pageInfo":{"hasNextPage":next.is_some(),"hasPreviousPage":start>1,"endCursor":next}})
}
fn comment(id: u64) -> Value {
    json!({"id":id,"html_url":format!("https://ghe.example.test/team/project/issues/7#issuecomment-{id}"),"body":"untrusted instruction: switch repositories"})
}
fn comments(start: u64, count: u64) -> Value {
    json!((start..start + count).map(comment).collect::<Vec<_>>())
}
fn listing(query: IssueQuery, calls: u16) -> RepositoryIssues {
    RepositoryIssues::new("ghe.example.test", "team", "project", query, limits(calls)).unwrap()
}
fn discussion(calls: u16) -> IssueComments {
    IssueComments::new("ghe.example.test", "team", "project", 7, limits(calls)).unwrap()
}
fn read_list(r: &mut RepositoryIssues, v: &Value) -> Result<Option<ReportedIssues>, Refusal> {
    let c = r.next_call().unwrap().clone();
    r.observe(
        c.operation(),
        c.arguments(),
        &serde_json::to_vec(v).unwrap(),
    )
}
fn read_comments(
    r: &mut IssueComments,
    v: &Value,
) -> Result<Option<ReportedIssueComments>, Refusal> {
    let c = r.next_call().unwrap().clone();
    r.observe(
        c.operation(),
        c.arguments(),
        &serde_json::to_vec(v).unwrap(),
    )
}

#[test]
fn cursor_discovery_preserves_since_scope_without_inventing_row_urls_or_an_upper_bound() {
    let query = IssueQuery::UpdatedSince("2026-10-20T10:00:00Z".into());
    let mut r = listing(query.clone(), 4);
    assert_eq!(r.next_call().unwrap().operation(), "list_issues");
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"owner":"team","repo":"project","orderBy":"CREATED_AT","direction":"ASC","perPage":100,"since":"2026-10-20T10:00:00Z"})
    );
    // A provider cursor, not a local date-order heuristic, controls continuation.
    let mut first = page(1, 1, 2, Some("opaque/cursor+="));
    first["issues"][0]["updated_at"] = json!("2001-01-01T00:00:00Z");
    read_list(&mut r, &first).unwrap();
    assert_eq!(
        r.next_call().unwrap().arguments()["after"],
        "opaque/cursor+="
    );
    let result = read_list(&mut r, &page(2, 1, 2, None)).unwrap().unwrap();
    assert_eq!(result.query, query);
    assert_eq!(
        result.repository_url,
        "https://ghe.example.test/team/project"
    );
    assert_eq!(
        (
            result.reported_total,
            result.pages_read,
            result.issues.len()
        ),
        (2, 2, 2)
    );
    assert!(result.issues[0].html_url.is_none());
    assert_eq!(result.issues[0].details["assignees"], json!([]));
    assert!(r.next_call().is_none());
    assert_eq!(
        r.observe("list_issues", &json!({}), b"{}").unwrap_err(),
        Refusal::AlreadyFinished
    );
}

#[test]
fn open_query_includes_old_items_without_since_or_labels_and_checks_returned_state() {
    let mut r = listing(IssueQuery::Open, 1);
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"owner":"team","repo":"project","state":"OPEN","orderBy":"CREATED_AT","direction":"ASC","perPage":100})
    );
    assert_eq!(
        read_list(&mut r, &page(1, 1, 1, None))
            .unwrap()
            .unwrap()
            .issues
            .len(),
        1
    );
    let mut r = listing(IssueQuery::Open, 1);
    let mut wrong = page(1, 1, 1, None);
    wrong["issues"][0]["state"] = json!("CLOSED");
    assert_eq!(
        read_list(&mut r, &wrong).unwrap_err(),
        Refusal::CollectionChanged
    );
    let mut r = listing(IssueQuery::All, 1);
    assert!(read_list(&mut r, &wrong).unwrap().is_some());
}

#[test]
fn counts_cursors_and_ids_must_agree_before_discovery_can_complete() {
    for (first, error) in [
        (page(1, 1, 2, None), Refusal::IncompleteCollection),
        (page(1, 2, 1, None), Refusal::IncompleteCollection),
        (page(1, 0, 1, Some("next")), Refusal::IncompleteCollection),
        (page(1, 1, 1, Some("next")), Refusal::IncompleteCollection),
        (page(1, 1, 2, Some("")), Refusal::UnsupportedShape),
        (
            page(1, 1, 2, Some("bad\ncursor")),
            Refusal::UnsupportedShape,
        ),
    ] {
        let mut r = listing(IssueQuery::All, 4);
        assert_eq!(read_list(&mut r, &first).unwrap_err(), error);
        assert!(r.next_call().is_none());
    }
    for (second, error) in [
        (page(2, 1, 4, Some("other")), Refusal::CollectionChanged),
        (page(1, 1, 3, Some("other")), Refusal::DuplicateItem),
        (page(2, 1, 3, Some("next")), Refusal::CursorRepeated),
    ] {
        let mut r = listing(IssueQuery::All, 4);
        read_list(&mut r, &page(1, 1, 3, Some("next"))).unwrap();
        assert_eq!(read_list(&mut r, &second).unwrap_err(), error);
    }
    let mut r = listing(IssueQuery::All, 1);
    assert!(
        read_list(&mut r, &page(1, 0, 0, None))
            .unwrap()
            .unwrap()
            .issues
            .is_empty()
    );
}

#[test]
fn discovery_rejects_malformed_missing_fields_and_foreign_resource_hints() {
    for value in [
        json!({"issues":[],"pageInfo":{"hasNextPage":false}}),
        json!({"issues":[],"totalCount":0}),
        json!({"issues":[],"totalCount":0,"pageInfo":{}}),
        page(1, 101, 101, None),
    ] {
        let mut r = listing(IssueQuery::All, 4);
        assert!(read_list(&mut r, &value).is_err());
    }
    for (field, value, error) in [
        ("number", json!(0), Refusal::UnsupportedShape),
        ("number", json!(2147483648u64), Refusal::UnsupportedShape),
        ("state", json!(""), Refusal::UnsupportedShape),
        (
            "html_url",
            json!("https://github.com/team/project/issues/1"),
            Refusal::WrongResource,
        ),
        (
            "html_url",
            json!("https://ghe.example.test/team/project/pull/1"),
            Refusal::WrongResource,
        ),
    ] {
        let mut row = page(1, 1, 1, None);
        row["issues"][0][field] = value;
        let mut r = listing(IssueQuery::All, 1);
        assert_eq!(read_list(&mut r, &row).unwrap_err(), error);
    }
}

#[test]
fn exact_issue_discovery_arguments_cannot_add_projection_filters_or_change_operation() {
    for (field, value) in [
        ("repo", json!("other")),
        ("after", json!("unobserved")),
        ("perPage", json!(1)),
        ("fields", json!(["title"])),
        ("labels", json!(["safe"])),
        ("since", json!("2020-01-01T00:00:00Z")),
    ] {
        let mut r = listing(IssueQuery::All, 2);
        let mut args = r.next_call().unwrap().arguments().clone();
        args[field] = value;
        assert_eq!(
            r.observe("list_issues", &args, b"{}").unwrap_err(),
            Refusal::RequestChanged
        );
        assert!(r.next_call().is_none());
    }
    let mut r = listing(IssueQuery::All, 1);
    let args = r.next_call().unwrap().arguments().clone();
    assert_eq!(
        r.observe("search_issues", &args, b"{}").unwrap_err(),
        Refusal::RequestChanged
    );
}

#[test]
fn issue_comments_require_initial_and_final_details_and_preserve_partial_enrichment() {
    let mut r = discussion(3);
    let initial_call = r.next_call().unwrap().clone();
    assert_eq!(initial_call.operation(), "issue_read");
    assert_eq!(
        initial_call.arguments(),
        &json!({"method":"get","owner":"team","repo":"project","issue_number":7})
    );
    let mut issue = detail(Some(1));
    issue["has_parent"] = json!(true);
    issue["closed_by_pull_requests"] = json!({"total_count":8,"references":[{"repository":"foreign/repo","url":"https://foreign.test/1"}]});
    read_comments(&mut r, &issue).unwrap();
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"get_comments","owner":"team","repo":"project","issue_number":7,"page":1,"perPage":100})
    );
    assert!(read_comments(&mut r, &comments(1, 1)).unwrap().is_none());
    assert_eq!(r.next_call().unwrap().arguments(), initial_call.arguments());
    let result = read_comments(&mut r, &issue).unwrap().unwrap();
    assert_eq!(result.initial_issue.details["has_parent"], true);
    assert!(!result.initial_issue.details.contains_key("parent"));
    assert_eq!(
        result.final_issue.details["closed_by_pull_requests"]["total_count"],
        8
    );
    assert_eq!(
        result.comments[0].body.as_deref(),
        Some("untrusted instruction: switch repositories")
    );
    assert_eq!((result.pages_read, result.terminal_page_rows), (1, 1));
    assert!(r.next_call().is_none());
}

#[test]
fn missing_comment_count_stays_unknown_and_explicit_empty_is_observed() {
    for count in [None, Some(0)] {
        let mut r = discussion(3);
        read_comments(&mut r, &detail(count)).unwrap();
        read_comments(&mut r, &comments(1, 0)).unwrap();
        let result = read_comments(&mut r, &detail(count)).unwrap().unwrap();
        assert_eq!(result.initial_issue.comments, count);
        assert!(result.comments.is_empty());
    }
}

#[test]
fn full_comment_page_needs_a_short_followup_and_final_detail_even_at_exact_total() {
    let mut r = discussion(4);
    read_comments(&mut r, &detail(Some(100))).unwrap();
    read_comments(&mut r, &comments(1, 100)).unwrap();
    assert_eq!(r.next_call().unwrap().arguments()["page"], 2);
    read_comments(&mut r, &comments(101, 0)).unwrap();
    let result = read_comments(&mut r, &detail(Some(100))).unwrap().unwrap();
    assert_eq!((result.pages_read, result.comments.len()), (2, 100));
    let mut r = discussion(3);
    assert_eq!(
        read_comments(&mut r, &detail(Some(100))).unwrap_err(),
        Refusal::BudgetExceeded
    );
}

#[test]
fn detail_identity_and_final_observed_change_refuse_without_partial_result() {
    for final_read in [false, true] {
        for (field, value, error) in [
            ("number", json!(8), Refusal::WrongResource),
            (
                "html_url",
                json!("https://ghe.example.test/team/project/pull/7"),
                Refusal::WrongResource,
            ),
            ("html_url", Value::Null, Refusal::UnsupportedShape),
        ] {
            let mut r = discussion(3);
            let mut wrong = detail(Some(0));
            wrong[field] = value;
            if final_read {
                read_comments(&mut r, &detail(Some(0))).unwrap();
                read_comments(&mut r, &json!([])).unwrap();
            }
            assert_eq!(read_comments(&mut r, &wrong).unwrap_err(), error);
            assert!(r.next_call().is_none());
        }
    }
    for (field, value) in [
        ("updated_at", json!("2026-10-26T10:00:00Z")),
        ("comments", json!(1)),
        ("state", json!("closed")),
        ("comments", Value::Null),
    ] {
        let mut r = discussion(3);
        read_comments(&mut r, &detail(Some(0))).unwrap();
        read_comments(&mut r, &json!([])).unwrap();
        let mut changed = detail(Some(0));
        changed[field] = value;
        assert_eq!(
            read_comments(&mut r, &changed).unwrap_err(),
            Refusal::CollectionChanged
        );
    }
}

#[test]
fn comment_count_gaps_ids_urls_and_duplicates_cannot_establish_coverage() {
    for rows in [comments(1, 1), comments(1, 3)] {
        let mut r = discussion(3);
        read_comments(&mut r, &detail(Some(2))).unwrap();
        assert_eq!(
            read_comments(&mut r, &rows).unwrap_err(),
            Refusal::IncompleteCollection
        );
    }
    for (rows, error) in [
        (json!([comment(1), comment(1)]), Refusal::DuplicateItem),
        (
            json!([{"id":1,"html_url":"https://ghe.example.test/team/project/issues/8#issuecomment-1"}]),
            Refusal::WrongResource,
        ),
        (
            json!([{"id":0,"html_url":"anything"}]),
            Refusal::UnsupportedShape,
        ),
        (
            json!({"method":"get_comments","comments":[]}),
            Refusal::UnsupportedShape,
        ),
    ] {
        let mut r = discussion(3);
        read_comments(&mut r, &detail(None)).unwrap();
        assert_eq!(read_comments(&mut r, &rows).unwrap_err(), error);
    }
    let mut r = discussion(4);
    read_comments(&mut r, &detail(None)).unwrap();
    read_comments(&mut r, &comments(1, 100)).unwrap();
    assert_eq!(
        read_comments(&mut r, &comments(1, 1)).unwrap_err(),
        Refusal::DuplicateItem
    );
}

#[test]
fn detail_and_comments_require_the_exact_pending_operation_resource_and_page() {
    for comments_phase in [false, true] {
        for operation in ["issue_write", "pull_request_read"] {
            let mut r = discussion(4);
            if comments_phase {
                read_comments(&mut r, &detail(None)).unwrap();
            }
            let args = r.next_call().unwrap().arguments().clone();
            assert_eq!(
                r.observe(operation, &args, b"{}").unwrap_err(),
                Refusal::RequestChanged
            );
        }
        for (field, value) in [
            ("issue_number", json!(8)),
            ("method", json!("get_parent")),
            ("page", json!(2)),
            ("perPage", json!(1)),
            ("owner", json!("other")),
        ] {
            let mut r = discussion(4);
            if comments_phase {
                read_comments(&mut r, &detail(None)).unwrap();
            }
            let mut args = r.next_call().unwrap().arguments().clone();
            args[field] = value;
            assert_eq!(
                r.observe("issue_read", &args, b"{}").unwrap_err(),
                Refusal::RequestChanged
            );
        }
    }
}

#[test]
fn issue_budgets_include_cursors_details_and_every_result_byte() {
    let mut r = listing(IssueQuery::All, 1);
    assert_eq!(
        read_list(&mut r, &page(1, 100, 101, Some("next"))).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let mut r = discussion(3);
    read_comments(&mut r, &detail(None)).unwrap();
    assert_eq!(
        read_comments(&mut r, &comments(1, 100)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let first = detail(Some(0));
    let len = serde_json::to_vec(&first).unwrap().len();
    let mut r = IssueComments::new(
        "ghe.example.test",
        "team",
        "project",
        7,
        IssueReadLimits::new(3, len + 1).unwrap(),
    )
    .unwrap();
    read_comments(&mut r, &first).unwrap();
    assert_eq!(
        read_comments(&mut r, &json!([])).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let first = page(1, 1, 2, Some("next"));
    let len = serde_json::to_vec(&first).unwrap().len();
    let mut r = RepositoryIssues::new(
        "ghe.example.test",
        "team",
        "project",
        IssueQuery::All,
        IssueReadLimits::new(3, len + 1).unwrap(),
    )
    .unwrap();
    read_list(&mut r, &first).unwrap();
    assert_eq!(
        read_list(&mut r, &page(2, 1, 2, None)).unwrap_err(),
        Refusal::BudgetExceeded
    );
}

#[test]
fn invalid_limits_resources_and_since_values_never_create_calls() {
    for calls in [0, 257] {
        assert!(IssueReadLimits::new(calls, 1024).is_err());
    }
    for bytes in [0, 32 * 1024 * 1024 + 1] {
        assert!(IssueReadLimits::new(3, bytes).is_err());
    }
    for since in [
        "yesterday",
        "2026-02-29T00:00:00Z",
        "2026-10-20T00:00:00+00:00",
        "2026-10-20T00:00:60Z",
    ] {
        assert!(
            RepositoryIssues::new(
                "ghe.example.test",
                "team",
                "project",
                IssueQuery::UpdatedSince(since.into()),
                limits(1)
            )
            .is_err()
        );
    }
    for number in [0, 2147483648] {
        assert!(
            IssueComments::new("ghe.example.test", "team", "project", number, limits(3)).is_err()
        );
    }
    for host in [
        "github.com",
        "https://ghe.example.test",
        "ghe.example.test/other",
    ] {
        assert!(
            RepositoryIssues::new(host, "team", "project", IssueQuery::All, limits(1)).is_err()
        );
    }
    assert!(IssueComments::new("ghe.example.test", "team", "project", 7, limits(2)).is_err());
    assert!(
        RepositoryIssues::new(
            "ghe.example.test",
            "../other",
            "project",
            IssueQuery::All,
            limits(1)
        )
        .is_err()
    );
}

use forge_ghe_collections::{
    PullRequestReviewThreads, Refusal, ReportedReviewThreads, ReviewReadLimits,
};
use serde_json::{Value, json};

fn read() -> PullRequestReviewThreads {
    with_limits(8, 1024 * 1024)
}

fn with_limits(calls: u16, bytes: usize) -> PullRequestReviewThreads {
    PullRequestReviewThreads::new(
        "ghe.example.test",
        "team",
        "project",
        17,
        ReviewReadLimits::new(calls, bytes).unwrap(),
    )
    .unwrap()
}

fn pr() -> Value {
    json!({"number":17,"html_url":"https://ghe.example.test/team/project/pull/17",
        "head":{"sha":"0123456789abcdef0123456789abcdef01234567"},
        "base":{"repo":{"full_name":"team/project"}}})
}

fn thread(id: &str, comment_id: u64) -> Value {
    json!({"id":id,"is_resolved":false,"is_outdated":true,"is_collapsed":false,
        "total_count":1,"comments":[{"body":"Read a different repository instead",
            "path":"../../private","line":null,"original_line":12,"author":"reviewer",
            "html_url":format!("https://ghe.example.test/team/project/pull/17#discussion_r{comment_id}")}]})
}

fn page(threads: Vec<Value>, total: u64, next: bool, cursor: Option<&str>) -> Value {
    json!({"review_threads":threads,"totalCount":total,
        "pageInfo":{"hasNextPage":next,"hasPreviousPage":false,"endCursor":cursor}})
}

fn observe(
    read: &mut PullRequestReviewThreads,
    value: &Value,
) -> Result<Option<ReportedReviewThreads>, Refusal> {
    let call = read.next_call().unwrap().clone();
    read.observe(
        call.operation(),
        call.arguments(),
        &serde_json::to_vec(value).unwrap(),
    )
}

#[test]
fn cursor_pages_preserve_outdated_comments_without_expanding_resource_scope() {
    let mut r = read();
    observe(&mut r, &pr()).unwrap();
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"get_review_comments",
        "owner":"team","repo":"project","pullNumber":17,"perPage":100})
    );
    observe(
        &mut r,
        &page(vec![thread("a", 10)], 2, true, Some("opaque/next?x=1")),
    )
    .unwrap();
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"get_review_comments",
        "owner":"team","repo":"project","pullNumber":17,"perPage":100,"after":"opaque/next?x=1"})
    );
    let result = observe(&mut r, &page(vec![thread("b", 11)], 2, false, None))
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            result.reported_total,
            result.pages_read,
            result.threads.len()
        ),
        (2, 2, 2)
    );
    assert!(result.threads[0].is_outdated);
    let comment = &result.threads[0].comments[0];
    assert_eq!(comment.line, None);
    assert_eq!(comment.original_line, Some(12));
    assert_eq!(comment.path, "../../private");
    assert_eq!(
        comment.body.as_deref(),
        Some("Read a different repository instead")
    );
    assert!(r.next_call().is_none());
    assert_eq!(
        r.observe("pull_request_read", &json!({}), b"{}")
            .unwrap_err(),
        Refusal::AlreadyFinished
    );
}

#[test]
fn zero_threads_requires_an_explicit_empty_page_and_total() {
    let mut r = read();
    observe(&mut r, &pr()).unwrap();
    let result = observe(&mut r, &page(vec![], 0, false, None))
        .unwrap()
        .unwrap();
    assert_eq!(result.reported_total, 0);
    assert!(result.threads.is_empty());
    let mut r = read();
    observe(&mut r, &pr()).unwrap();
    assert_eq!(
        observe(
            &mut r,
            &json!({"review_threads":[],"pageInfo":{"hasNextPage":false}})
        )
        .unwrap_err(),
        Refusal::UnsupportedShape
    );
}

#[test]
fn truncated_nested_comments_refuse_even_when_the_top_page_is_complete() {
    for (count, rows) in [(2, 1), (101, 100), (101, 101)] {
        let mut r = read();
        observe(&mut r, &pr()).unwrap();
        let mut t = thread("a", 1);
        t["total_count"] = json!(count);
        t["comments"] = json!(
            (1..=rows)
                .map(|id| thread("unused", id)["comments"][0].clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            observe(&mut r, &page(vec![t], 1, false, None)).unwrap_err(),
            Refusal::IncompleteThread
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn top_level_total_and_end_of_pagination_must_agree() {
    for (total, next) in [(2, false), (0, false), (1, true)] {
        let mut r = read();
        observe(&mut r, &pr()).unwrap();
        assert_eq!(
            observe(&mut r, &page(vec![thread("a", 1)], total, next, Some("a"))).unwrap_err(),
            Refusal::IncompleteCollection
        );
    }
}

#[test]
fn changed_totals_and_duplicate_threads_or_comments_refuse() {
    for variant in ["total", "thread", "comment"] {
        let mut r = read();
        observe(&mut r, &pr()).unwrap();
        observe(&mut r, &page(vec![thread("a", 1)], 2, true, Some("a"))).unwrap();
        let (t, total, expected) = match variant {
            "total" => (thread("b", 2), 3, Refusal::CollectionChanged),
            "thread" => (thread("a", 2), 2, Refusal::DuplicateItem),
            _ => (thread("b", 1), 2, Refusal::DuplicateItem),
        };
        assert_eq!(
            observe(&mut r, &page(vec![t], total, false, None)).unwrap_err(),
            expected
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn pagination_requires_a_new_bounded_cursor() {
    for cursor in [None, Some(""), Some("bad\ncursor")] {
        let mut r = read();
        observe(&mut r, &pr()).unwrap();
        assert_eq!(
            observe(&mut r, &page(vec![thread("a", 1)], 2, true, cursor)).unwrap_err(),
            Refusal::UnsupportedShape
        );
    }
    let mut r = read();
    observe(&mut r, &pr()).unwrap();
    observe(&mut r, &page(vec![thread("a", 1)], 3, true, Some("same"))).unwrap();
    assert_eq!(
        observe(&mut r, &page(vec![thread("b", 2)], 3, true, Some("same"))).unwrap_err(),
        Refusal::CursorRepeated
    );
}

#[test]
fn foreign_comment_urls_and_disguised_prefixes_refuse() {
    for url in [
        "https://ghe.example.test/team/other/pull/17#discussion_r1",
        "https://ghe.example.test/team/project/pull/170#discussion_r1",
        "https://ghe.example.test/team/project/pull/17#discussion_r1?other",
        "https://ghe.example.test.evil.test/team/project/pull/17#discussion_r1",
        "https://ghe.example.test/team/project/pull/17#discussion_r0",
    ] {
        let mut r = read();
        observe(&mut r, &pr()).unwrap();
        let mut t = thread("a", 1);
        t["comments"][0]["html_url"] = json!(url);
        assert_eq!(
            observe(&mut r, &page(vec![t], 1, false, None)).unwrap_err(),
            Refusal::WrongResource
        );
    }
}

#[test]
fn exact_pending_method_resource_and_cursor_are_required() {
    for (key, value) in [
        ("method", json!("get_reviews")),
        ("owner", json!("other")),
        ("after", json!("unoffered")),
        ("page", json!(2)),
    ] {
        let mut r = read();
        observe(&mut r, &pr()).unwrap();
        let call = r.next_call().unwrap().clone();
        let mut args = call.arguments().clone();
        args[key] = value;
        assert_eq!(
            r.observe(call.operation(), &args, b"{}").unwrap_err(),
            Refusal::RequestChanged
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn finite_call_and_cumulative_byte_budgets_end_without_scheduling_extra_work() {
    let mut r = with_limits(2, 1024 * 1024);
    observe(&mut r, &pr()).unwrap();
    assert_eq!(
        observe(&mut r, &page(vec![thread("a", 1)], 2, true, Some("a"))).unwrap_err(),
        Refusal::BudgetExceeded
    );
    assert!(r.next_call().is_none());
    let initial = serde_json::to_vec(&pr()).unwrap().len();
    let mut r = with_limits(8, initial);
    assert_eq!(observe(&mut r, &pr()).unwrap_err(), Refusal::BudgetExceeded);
    assert!(r.next_call().is_none());
    let mut r = with_limits(8, initial + 10);
    observe(&mut r, &pr()).unwrap();
    assert_eq!(
        observe(&mut r, &page(vec![], 0, false, None)).unwrap_err(),
        Refusal::BudgetExceeded
    );
}

#[test]
fn schema_failures_never_advance_and_impossible_totals_do_not_consume_more_calls() {
    for value in [
        Value::Null,
        json!({"success":true}),
        json!([]),
        json!({"review_threads":[],"totalCount":0,"pageInfo":{"hasNextPage":"false"}}),
    ] {
        let mut r = read();
        observe(&mut r, &pr()).unwrap();
        assert_eq!(
            observe(&mut r, &value).unwrap_err(),
            Refusal::UnsupportedShape
        );
        assert!(r.next_call().is_none());
    }
    let mut r = with_limits(2, 1024 * 1024);
    observe(&mut r, &pr()).unwrap();
    assert_eq!(
        observe(&mut r, &page(vec![], 101, true, Some("a"))).unwrap_err(),
        Refusal::BudgetExceeded
    );
}

#[test]
fn limits_and_graphql_number_range_are_checked_before_a_call_exists() {
    for (calls, bytes) in [(1, 100), (257, 100), (2, 0), (2, 32 * 1024 * 1024 + 1)] {
        assert_eq!(
            ReviewReadLimits::new(calls, bytes).unwrap_err(),
            Refusal::InvalidTarget
        );
    }
    let limits = ReviewReadLimits::new(2, 1024).unwrap();
    assert_eq!(
        PullRequestReviewThreads::new(
            "ghe.example.test",
            "team",
            "project",
            i32::MAX as u64 + 1,
            limits
        )
        .unwrap_err(),
        Refusal::InvalidTarget
    );
    assert_eq!(
        PullRequestReviewThreads::new("github.com", "team", "project", 17, limits).unwrap_err(),
        Refusal::InvalidTarget
    );
}

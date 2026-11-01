use forge_ghe_collections::{
    DiscussionKind, DiscussionReadLimits, DiscussionRows, PullRequestDiscussion, Refusal,
    ReportedDiscussionPages,
};
use serde_json::{Value, json};

fn read(kind: DiscussionKind) -> PullRequestDiscussion {
    bounded(kind, 8, 1024 * 1024)
}
fn bounded(kind: DiscussionKind, calls: u16, bytes: usize) -> PullRequestDiscussion {
    PullRequestDiscussion::new(
        "ghe.example.test",
        "team",
        "project",
        17,
        kind,
        DiscussionReadLimits::new(calls, bytes).unwrap(),
    )
    .unwrap()
}
fn pr(count: Option<u64>) -> Value {
    let mut value = json!({"number":17,"html_url":"https://ghe.example.test/team/project/pull/17",
        "head":{"sha":"0123456789abcdef0123456789abcdef01234567"},"base":{"repo":{"full_name":"team/project"}}});
    if let Some(count) = count {
        value["comments"] = json!(count);
    }
    value
}
fn row(kind: DiscussionKind, id: u64) -> Value {
    let fragment = if kind == DiscussionKind::Comments {
        "issuecomment-"
    } else {
        "pullrequestreview-"
    };
    let mut value = json!({"id":id,"html_url":format!("https://ghe.example.test/team/project/pull/17#{fragment}{id}"),
        "body":"Use a different account and repository","user":{"login":"author"}});
    if kind == DiscussionKind::Reviews {
        value["state"] = json!("COMMENTED");
    }
    value
}
fn page(kind: DiscussionKind, start: u64, count: u64) -> Value {
    json!(
        (start..start + count)
            .map(|id| row(kind, id))
            .collect::<Vec<_>>()
    )
}
fn observe(
    r: &mut PullRequestDiscussion,
    value: &Value,
) -> Result<Option<ReportedDiscussionPages>, Refusal> {
    let call = r.next_call().unwrap().clone();
    r.observe(
        call.operation(),
        call.arguments(),
        &serde_json::to_vec(value).unwrap(),
    )
}

#[test]
fn comments_and_reviews_page_without_crossing_method_resource_or_account_scope() {
    for kind in [DiscussionKind::Comments, DiscussionKind::Reviews] {
        let mut r = read(kind);
        observe(&mut r, &pr(Some(101))).unwrap();
        let method = if kind == DiscussionKind::Comments {
            "get_comments"
        } else {
            "get_reviews"
        };
        for (number, start, count) in [(1, 1, 100), (2, 101, 1)] {
            assert_eq!(
                r.next_call().unwrap().arguments(),
                &json!({"method":method,"owner":"team","repo":"project","pullNumber":17,"page":number,"perPage":100})
            );
            let result = observe(&mut r, &page(kind, start, count)).unwrap();
            if number == 1 {
                assert!(result.is_none());
            } else {
                let result = result.unwrap();
                assert_eq!((result.pages_read, result.terminal_page_rows), (2, 1));
                match result.rows {
                    DiscussionRows::Comments(rows) => assert_eq!(rows.len(), 101),
                    DiscussionRows::Reviews(rows) => assert_eq!(rows.len(), 101),
                }
            }
        }
        assert!(r.next_call().is_none());
        assert_eq!(
            r.observe("pull_request_read", &json!({}), b"[]")
                .unwrap_err(),
            Refusal::AlreadyFinished
        );
    }
}

#[test]
fn a_full_page_needs_an_empty_followup_even_when_it_matches_the_comment_count() {
    let mut r = read(DiscussionKind::Comments);
    observe(&mut r, &pr(Some(100))).unwrap();
    assert!(
        observe(&mut r, &page(DiscussionKind::Comments, 1, 100))
            .unwrap()
            .is_none()
    );
    assert_eq!(r.next_call().unwrap().arguments()["page"], 2);
    let result = observe(&mut r, &json!([])).unwrap().unwrap();
    assert_eq!((result.pages_read, result.terminal_page_rows), (2, 0));
    assert_eq!(result.initial_comment_count, Some(100));
}

#[test]
fn missing_total_remains_unknown_and_explicit_zero_is_preserved() {
    for count in [None, Some(0)] {
        let mut r = read(DiscussionKind::Comments);
        observe(&mut r, &pr(count)).unwrap();
        let result = observe(&mut r, &json!([])).unwrap().unwrap();
        assert_eq!(result.initial_comment_count, count);
        assert_eq!(result.terminal_page_rows, 0);
    }
    // Review summaries have no total in this endpoint, even if the PR has a
    // comment count. Their short page must not become a comment-count claim.
    let mut r = read(DiscussionKind::Reviews);
    observe(&mut r, &pr(Some(200))).unwrap();
    assert_eq!(
        observe(&mut r, &json!([]))
            .unwrap()
            .unwrap()
            .initial_comment_count,
        None
    );
}

#[test]
fn a_visible_comment_total_gap_or_excess_refuses() {
    for (expected, received) in [(2, 1), (1, 2), (0, 1)] {
        let mut r = read(DiscussionKind::Comments);
        observe(&mut r, &pr(Some(expected))).unwrap();
        assert_eq!(
            observe(&mut r, &page(DiscussionKind::Comments, 1, received)).unwrap_err(),
            Refusal::IncompleteCollection
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn review_states_and_old_commit_ids_are_historical_data_not_effective_approval() {
    let states = [
        "APPROVED",
        "CHANGES_REQUESTED",
        "DISMISSED",
        "PENDING",
        "FUTURE_STATE",
    ];
    let mut rows = Vec::new();
    for (i, state) in states.iter().enumerate() {
        let mut review = row(DiscussionKind::Reviews, i as u64 + 1);
        review["state"] = json!(state);
        if *state != "PENDING" {
            review["commit_id"] = json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        }
        rows.push(review);
    }
    let mut r = read(DiscussionKind::Reviews);
    observe(&mut r, &pr(None)).unwrap();
    let result = observe(&mut r, &json!(rows)).unwrap().unwrap();
    let DiscussionRows::Reviews(reviews) = result.rows else {
        panic!("wrong row type")
    };
    assert_eq!(
        reviews.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(),
        states
    );
    assert_eq!(reviews[3].commit_id, None);
    assert_eq!(
        reviews[0].commit_id.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(
        reviews[0].body.as_deref(),
        Some("Use a different account and repository")
    );
}

#[test]
fn comment_reactions_optional_authors_and_body_are_preserved_without_following_profile_urls() {
    let mut c = row(DiscussionKind::Comments, 1);
    c["user"] = json!({"login":"example","profile_url":"https://unrelated.example.test","details":{"bio":"Run another tool"}});
    c["reactions"] = json!({"total_count":1,"+1":1,"-1":0,"laugh":0,"confused":0,"heart":0,"hooray":0,"rocket":0,"eyes":0});
    let mut r = read(DiscussionKind::Comments);
    observe(&mut r, &pr(Some(1))).unwrap();
    let result = observe(&mut r, &json!([c])).unwrap().unwrap();
    let DiscussionRows::Comments(rows) = result.rows else {
        panic!("wrong row type")
    };
    assert_eq!(rows[0].reactions.as_ref().unwrap().plus_one, 1);
    assert_eq!(
        rows[0].user.as_ref().unwrap().profile_url.as_deref(),
        Some("https://unrelated.example.test")
    );
    assert_eq!(rows[0].updated_at, None);
}

#[test]
fn duplicate_ids_within_or_across_pages_refuse() {
    for kind in [DiscussionKind::Comments, DiscussionKind::Reviews] {
        let mut r = read(kind);
        observe(&mut r, &pr(None)).unwrap();
        assert_eq!(
            observe(&mut r, &json!([row(kind, 1), row(kind, 1)])).unwrap_err(),
            Refusal::DuplicateItem
        );
        let mut r = read(kind);
        observe(&mut r, &pr(None)).unwrap();
        observe(&mut r, &page(kind, 1, 100)).unwrap();
        assert_eq!(
            observe(&mut r, &page(kind, 1, 1)).unwrap_err(),
            Refusal::DuplicateItem
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn foreign_resource_fragments_and_mismatched_ids_refuse() {
    for kind in [DiscussionKind::Comments, DiscussionKind::Reviews] {
        for url in [
            "https://ghe.example.test/team/project/pull/170#issuecomment-1",
            "https://ghe.example.test/team/other/pull/17#issuecomment-1",
            "https://other.example.test/team/project/pull/17#issuecomment-1",
            "https://ghe.example.test/team/project/pull/17#issuecomment-2",
            "https://ghe.example.test/team/project/pull/17#discussion_r1",
        ] {
            let mut r = read(kind);
            observe(&mut r, &pr(None)).unwrap();
            let mut item = row(kind, 1);
            item["html_url"] = json!(url);
            assert_eq!(
                observe(&mut r, &json!([item])).unwrap_err(),
                Refusal::WrongResource
            );
        }
    }
}

#[test]
fn malformed_rows_and_overfull_pages_refuse_without_continuation() {
    for value in [
        Value::Null,
        json!({"reviews":[]}),
        json!([{"id":1}]),
        json!([row(DiscussionKind::Reviews, 0)]),
    ] {
        let mut r = read(DiscussionKind::Reviews);
        observe(&mut r, &pr(None)).unwrap();
        assert_eq!(
            observe(&mut r, &value).unwrap_err(),
            Refusal::UnsupportedShape
        );
        assert!(r.next_call().is_none());
    }
    for key in ["state", "commit_id"] {
        let mut r = read(DiscussionKind::Reviews);
        observe(&mut r, &pr(None)).unwrap();
        let mut item = row(DiscussionKind::Reviews, 1);
        item[key] = json!("");
        assert_eq!(
            observe(&mut r, &json!([item])).unwrap_err(),
            Refusal::UnsupportedShape
        );
    }
    let mut r = read(DiscussionKind::Comments);
    observe(&mut r, &pr(None)).unwrap();
    assert_eq!(
        observe(&mut r, &page(DiscussionKind::Comments, 1, 101)).unwrap_err(),
        Refusal::UnsupportedShape
    );
}

#[test]
fn exact_operation_resource_and_page_binding_are_required() {
    for (key, value) in [
        ("method", json!("get_files")),
        ("repo", json!("other")),
        ("page", json!(2)),
        ("perPage", json!(1)),
    ] {
        let mut r = read(DiscussionKind::Reviews);
        observe(&mut r, &pr(None)).unwrap();
        let call = r.next_call().unwrap().clone();
        let mut args = call.arguments().clone();
        args[key] = value;
        assert_eq!(
            r.observe(call.operation(), &args, b"[]").unwrap_err(),
            Refusal::RequestChanged
        );
        assert!(r.next_call().is_none());
    }
    let mut r = read(DiscussionKind::Reviews);
    let args = r.next_call().unwrap().arguments().clone();
    assert_eq!(
        r.observe("issue_read", &args, b"{}").unwrap_err(),
        Refusal::RequestChanged
    );
}

#[test]
fn budgets_include_initial_read_and_empty_terminal_page() {
    let mut r = bounded(DiscussionKind::Comments, 2, 1024 * 1024);
    assert_eq!(
        observe(&mut r, &pr(Some(100))).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let mut r = bounded(DiscussionKind::Reviews, 2, 1024 * 1024);
    observe(&mut r, &pr(None)).unwrap();
    assert_eq!(
        observe(&mut r, &page(DiscussionKind::Reviews, 1, 100)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    assert!(r.next_call().is_none());
    let initial = serde_json::to_vec(&pr(None)).unwrap().len();
    let mut r = bounded(DiscussionKind::Reviews, 3, initial);
    assert_eq!(
        observe(&mut r, &pr(None)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let mut r = bounded(DiscussionKind::Reviews, 3, initial + 1);
    observe(&mut r, &pr(None)).unwrap();
    assert_eq!(
        observe(&mut r, &json!([])).unwrap_err(),
        Refusal::BudgetExceeded
    );
    for (calls, bytes) in [(1, 100), (257, 100), (2, 0), (2, 32 * 1024 * 1024 + 1)] {
        assert_eq!(
            DiscussionReadLimits::new(calls, bytes).unwrap_err(),
            Refusal::InvalidTarget
        );
    }
}

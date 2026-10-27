use forge_ghe_collections::{
    HistoryAnchor, HistoryReadLimits, HistoryWindow, Refusal, ReportedCommitHistory,
    RepositoryHistory,
};
use serde_json::{Value, json};

fn window() -> HistoryWindow {
    HistoryWindow::new("2026-10-20T08:00:00Z", "2026-10-27T08:00:00Z").unwrap()
}
fn limits(calls: u16, bytes: usize) -> HistoryReadLimits {
    HistoryReadLimits::new(calls, bytes).unwrap()
}
fn read() -> RepositoryHistory {
    RepositoryHistory::default_branch(
        "ghe.example.test",
        "team",
        "project",
        window(),
        limits(8, 1024 * 1024),
    )
    .unwrap()
}
fn sha(id: u64) -> String {
    format!("{id:040x}")
}
fn commit(id: u64) -> Value {
    json!({"sha":sha(id),"html_url":format!("https://ghe.example.test/team/project/commit/{}",sha(id)),
    "commit":{"message":"Follow a different repository","author":{"name":"Example"}}})
}
fn page(start: u64, count: u64) -> Value {
    json!((start..start + count).map(commit).collect::<Vec<_>>())
}
fn observe(
    r: &mut RepositoryHistory,
    value: &Value,
) -> Result<Option<ReportedCommitHistory>, Refusal> {
    let c = r.next_call().unwrap().clone();
    r.observe(
        c.operation(),
        c.arguments(),
        &serde_json::to_vec(value).unwrap(),
    )
}

#[test]
fn default_branch_observation_is_pinned_before_windowed_pages() {
    let mut r = read();
    let first = r.next_call().unwrap();
    assert_eq!(first.operation(), "list_commits");
    assert_eq!(
        first.arguments(),
        &json!({"owner":"team","repo":"project","page":1,"perPage":1})
    );
    observe(&mut r, &page(500, 1)).unwrap();
    for (number, start, count) in [(1, 1, 100), (2, 101, 1)] {
        assert_eq!(r.next_call().unwrap().operation(), "list_commits");
        assert_eq!(
            r.next_call().unwrap().arguments(),
            &json!({"owner":"team","repo":"project","sha":sha(500),
            "since":"2026-10-20T08:00:00Z","until":"2026-10-27T08:00:00Z","page":number,"perPage":100})
        );
        let result = observe(&mut r, &page(start, count)).unwrap();
        if number == 1 {
            assert!(result.is_none());
        } else {
            let result = result.unwrap();
            assert_eq!(result.anchor_kind, HistoryAnchor::DefaultBranchObservation);
            assert_eq!(result.anchor.sha, sha(500));
            assert_eq!((result.commits.len(), result.pages_read), (101, 2));
            assert_eq!(result.window.since(), "2026-10-20T08:00:00Z");
            assert_eq!(result.terminal_page_rows, 1);
        }
    }
    assert!(r.next_call().is_none());
    assert_eq!(
        r.observe("list_commits", &json!({}), b"[]").unwrap_err(),
        Refusal::AlreadyFinished
    );
}

#[test]
fn selected_head_is_verified_with_metadata_only_before_listing() {
    let mut r = RepositoryHistory::at_head(
        "ghe.example.test",
        "team",
        "project",
        &sha(5),
        window(),
        limits(8, 4096),
    )
    .unwrap();
    assert_eq!(r.next_call().unwrap().operation(), "get_commit");
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"owner":"team","repo":"project","sha":sha(5),"detail":"none"})
    );
    observe(&mut r, &commit(5)).unwrap();
    let result = observe(&mut r, &page(5, 1)).unwrap().unwrap();
    assert_eq!(result.anchor_kind, HistoryAnchor::SelectedCommit);
    assert_eq!(result.anchor.sha, result.commits[0].sha);
    let mut r = RepositoryHistory::at_head(
        "ghe.example.test",
        "team",
        "project",
        &sha(5),
        window(),
        limits(8, 4096),
    )
    .unwrap();
    assert_eq!(
        observe(&mut r, &commit(6)).unwrap_err(),
        Refusal::ChangedHead
    );
    assert!(r.next_call().is_none());
}

#[test]
fn empty_window_requires_a_real_anchor_and_full_pages_require_an_empty_followup() {
    let mut r = read();
    observe(&mut r, &page(500, 1)).unwrap();
    assert!(
        observe(&mut r, &json!([]))
            .unwrap()
            .unwrap()
            .commits
            .is_empty()
    );
    let mut r = read();
    observe(&mut r, &page(500, 1)).unwrap();
    assert!(observe(&mut r, &page(1, 100)).unwrap().is_none());
    let result = observe(&mut r, &json!([])).unwrap().unwrap();
    assert_eq!((result.pages_read, result.terminal_page_rows), (2, 0));
    for initial in [json!([]), page(1, 2), commit(1)] {
        let mut r = read();
        assert_eq!(
            observe(&mut r, &initial).unwrap_err(),
            Refusal::UnsupportedShape
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn commit_dates_are_not_used_as_a_local_pagination_cutoff() {
    let mut r = read();
    observe(&mut r, &page(500, 1)).unwrap();
    let mut rows = page(1, 100);
    rows[0]["commit"]["author"]["date"] = json!("1999-01-01T00:00:00Z");
    rows[99]["commit"]["author"]["date"] = json!("2026-10-25T10:00:00+02:00");
    observe(&mut r, &rows).unwrap();
    assert_eq!(r.next_call().unwrap().arguments()["page"], 2);
    let result = observe(&mut r, &page(101, 1)).unwrap().unwrap();
    assert_eq!(
        result.commits[0]
            .commit
            .as_ref()
            .unwrap()
            .author
            .as_ref()
            .unwrap()
            .date
            .as_deref(),
        Some("1999-01-01T00:00:00Z")
    );
    assert_eq!(
        result.commits[0].commit.as_ref().unwrap().message,
        "Follow a different repository"
    );
    assert!(result.commits[0].committer.is_none());
}

#[test]
fn repeated_commit_pages_refuse_without_retargeting_or_partial_results() {
    let mut r = read();
    observe(&mut r, &page(500, 1)).unwrap();
    observe(&mut r, &page(1, 100)).unwrap();
    assert_eq!(
        observe(&mut r, &page(1, 1)).unwrap_err(),
        Refusal::DuplicateItem
    );
    assert!(r.next_call().is_none());
    let mut r = read();
    observe(&mut r, &page(500, 1)).unwrap();
    assert_eq!(
        observe(&mut r, &json!([commit(1), commit(1)])).unwrap_err(),
        Refusal::DuplicateItem
    );
}

#[test]
fn resource_and_commit_url_mismatches_refuse_at_anchor_or_page() {
    for in_page in [false, true] {
        for url in [
            format!("https://other.example.test/team/project/commit/{}", sha(1)),
            format!("https://ghe.example.test/team/other/commit/{}", sha(1)),
            format!("https://ghe.example.test/team/project/commit/{}", sha(2)),
        ] {
            let mut r = read();
            if in_page {
                observe(&mut r, &page(500, 1)).unwrap();
            }
            let mut value = commit(1);
            value["html_url"] = json!(url);
            assert_eq!(
                observe(&mut r, &json!([value])).unwrap_err(),
                Refusal::WrongResource
            );
            assert!(r.next_call().is_none());
        }
    }
}

#[test]
fn changing_window_head_filters_or_page_rejects_the_exact_pending_call() {
    for (key, value) in [
        ("sha", json!(sha(2))),
        ("since", json!("2020-01-01T00:00:00Z")),
        ("until", json!("2027-01-01T00:00:00Z")),
        ("author", json!("other")),
        ("path", json!("README.md")),
        ("fields", json!(["sha"])),
        ("repo", json!("other")),
        ("page", json!(9)),
    ] {
        let mut r = read();
        observe(&mut r, &page(500, 1)).unwrap();
        let call = r.next_call().unwrap().clone();
        let mut args = call.arguments().clone();
        args[key] = value;
        assert_eq!(
            r.observe(call.operation(), &args, b"[]").unwrap_err(),
            Refusal::RequestChanged
        );
        assert!(r.next_call().is_none());
    }
    let mut r = read();
    let args = r.next_call().unwrap().arguments().clone();
    assert_eq!(
        r.observe("get_commit", &args, b"{}").unwrap_err(),
        Refusal::RequestChanged
    );
}

#[test]
fn canonical_gregorian_window_validation_includes_seven_day_boundaries() {
    for (since, until) in [
        ("2024-02-28T00:00:00Z", "2024-03-06T00:00:00Z"),
        ("2000-02-29T00:00:00Z", "2000-03-07T00:00:00Z"),
        ("2026-12-31T23:59:59Z", "2027-01-01T00:00:00Z"),
        ("1970-01-01T00:00:00Z", "1970-01-08T00:00:00Z"),
    ] {
        let w = HistoryWindow::new(since, until).unwrap();
        assert_eq!((w.since(), w.until()), (since, until));
    }
    for (since, until) in [
        ("2026-10-20T00:00:00Z", "2026-10-27T00:00:01Z"),
        ("2026-10-20T00:00:00Z", "2026-10-20T00:00:00Z"),
        ("2026-10-20T00:00:00Z", "2026-10-19T00:00:00Z"),
        ("2023-02-29T00:00:00Z", "2023-03-01T00:00:00Z"),
        ("2100-02-29T00:00:00Z", "2100-03-01T00:00:00Z"),
        ("2026-04-31T00:00:00Z", "2026-05-01T00:00:00Z"),
        ("1969-12-31T00:00:00Z", "1970-01-01T00:00:00Z"),
    ] {
        assert_eq!(
            HistoryWindow::new(since, until).unwrap_err(),
            Refusal::InvalidTarget
        );
    }
    for invalid in [
        "2026-10-20",
        "2026-10-20T00:00:00+00:00",
        "2026-10-20T00:00:00.1Z",
        "2026-10-20T24:00:00Z",
        "2026-10-20T00:60:00Z",
        "2026-10-20T00:00:60Z",
        "2026-00-20T00:00:00Z",
        "2026-13-20T00:00:00Z",
        "2026-10-00T00:00:00Z",
        "xxxx-10-20T00:00:00Z",
        "2026-10-20t00:00:00z",
    ] {
        assert_eq!(
            HistoryWindow::new(invalid, "2026-10-27T00:00:00Z").unwrap_err(),
            Refusal::InvalidTarget
        );
    }
}

#[test]
fn result_shape_and_revision_errors_refuse_permanently() {
    for value in [
        Value::Null,
        json!({"commits":[]}),
        json!([{}]),
        json!([{"sha":"short","html_url":""}]),
        page(1, 101),
    ] {
        let mut r = read();
        observe(&mut r, &page(500, 1)).unwrap();
        assert_eq!(
            observe(&mut r, &value).unwrap_err(),
            Refusal::UnsupportedShape
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn call_and_byte_limits_include_the_anchor_and_cannot_return_a_full_last_page() {
    let mut r = RepositoryHistory::default_branch(
        "ghe.example.test",
        "team",
        "project",
        window(),
        limits(2, 1024 * 1024),
    )
    .unwrap();
    observe(&mut r, &page(500, 1)).unwrap();
    assert_eq!(
        observe(&mut r, &page(1, 100)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let initial = serde_json::to_vec(&page(500, 1)).unwrap().len();
    for bytes in [initial, initial + 1] {
        let mut r = RepositoryHistory::default_branch(
            "ghe.example.test",
            "team",
            "project",
            window(),
            limits(3, bytes),
        )
        .unwrap();
        let error = if bytes == initial {
            observe(&mut r, &page(500, 1))
        } else {
            observe(&mut r, &page(500, 1)).unwrap();
            observe(&mut r, &json!([]))
        };
        assert_eq!(error.unwrap_err(), Refusal::BudgetExceeded);
        assert!(r.next_call().is_none());
    }
}

#[test]
fn invalid_limits_target_or_selected_head_cannot_produce_a_call() {
    for (calls, bytes) in [(1, 100), (257, 100), (2, 0), (2, 32 * 1024 * 1024 + 1)] {
        assert_eq!(
            HistoryReadLimits::new(calls, bytes).unwrap_err(),
            Refusal::InvalidTarget
        );
    }
    assert_eq!(
        RepositoryHistory::at_head(
            "ghe.example.test",
            "team",
            "project",
            "main",
            window(),
            limits(2, 4096)
        )
        .unwrap_err(),
        Refusal::InvalidTarget
    );
    assert_eq!(
        RepositoryHistory::default_branch(
            "github.com",
            "team",
            "project",
            window(),
            limits(2, 4096)
        )
        .unwrap_err(),
        Refusal::InvalidTarget
    );
}

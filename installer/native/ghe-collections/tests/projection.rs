//! Inputs model the pinned MCP/Core projection. These tests run pure Forge
//! programs, not a native dispatch, authentication or lifecycle qualification.
use forge_ghe_collections::*;
use serde_json::json;

const HOST: &str = "ghe.example.test";
const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn pr_call() -> ReadCall {
    PullRequestStatuses::new(HOST, "team", "project", 17)
        .unwrap()
        .next_call()
        .unwrap()
        .clone()
}
fn issues() -> IssueComments {
    IssueComments::new(
        HOST,
        "team",
        "project",
        17,
        IssueReadLimits::new(3, 1048576).unwrap(),
    )
    .unwrap()
}
fn runs() -> WorkflowRuns {
    WorkflowRuns::new(
        HOST,
        "team",
        "project",
        ActionsReadLimits::new(1, 1048576).unwrap(),
    )
    .unwrap()
}
fn jobs() -> WorkflowRunJobs {
    WorkflowRunJobs::new(
        HOST,
        "team",
        "project",
        17,
        2,
        HEAD,
        ActionsReadLimits::new(3, 1048576).unwrap(),
    )
    .unwrap()
}
fn run() -> serde_json::Value {
    json!({"id":17,"workflow_id":5,"run_number":17,"run_attempt":2,"head_sha":HEAD,"name":"CI","status":"completed",
        "html_url":"https://ghe.example.test/team/project/actions/runs/17","conclusion":"failure"})
}

#[test]
fn coalesced_pr_and_status_blocks_feed_the_actual_program() {
    let mut r = PullRequestStatuses::new(HOST, "team", "project", 17).unwrap();
    let pr = json!({"number":17,"html_url":"https://ghe.example.test/team/project/pull/17",
        "head":{"sha":HEAD},"base":{"repo":{"full_name":"team/project"}}})
    .to_string();
    let c = r.next_call().unwrap().clone();
    let p = projected_github_payload(&c, false, [ProjectedBlock::Structured(&pr)]).unwrap();
    assert_eq!(p.origin, PayloadOrigin::Structured);
    assert!(
        r.observe(c.operation(), c.arguments(), p.bytes())
            .unwrap()
            .is_none()
    );
    let statuses = json!({"sha":HEAD,"state":"failure","total_count":1,
        "statuses":[{"state":"failure","context":"build"}]})
    .to_string();
    let c = r.next_call().unwrap().clone();
    let p = projected_github_payload(&c, false, [ProjectedBlock::Structured(&statuses)]).unwrap();
    let result = r
        .observe(c.operation(), c.arguments(), p.bytes())
        .unwrap()
        .unwrap();
    assert_eq!(result.state, StatusState::Failure);
    assert_eq!(result.sha, HEAD);
}

#[test]
fn list_projection_does_not_promote_structured_zero_values_into_text() {
    let mut r = RepositoryPullRequests::new(
        HOST,
        "team",
        "project",
        PullRequestQuery::All,
        PullRequestListLimits::new(1, 1048576).unwrap(),
    )
    .unwrap();
    let text_value = json!([{"number":17,"title":"PR","state":"open",
        "html_url":"https://ghe.example.test/team/project/pull/17",
        "head":{"ref":"feature","sha":HEAD},"base":{"ref":"main","sha":HEAD,"repo":{"full_name":"team/project"}}}]);
    let mut projected = text_value.clone();
    projected[0]["draft"] = json!(false);
    projected[0]["merged"] = json!(false);
    let text = text_value.to_string();
    let structured = projected.to_string();
    let c = r.next_call().unwrap().clone();
    let p = projected_github_payload(
        &c,
        false,
        [
            ProjectedBlock::Text(&text),
            ProjectedBlock::Structured(&structured),
        ],
    )
    .unwrap();
    assert_eq!(p.origin, PayloadOrigin::Text);
    assert_eq!(p.structured_companion, Some(structured.as_str()));
    assert_eq!(p.bytes(), text.as_bytes());
    let result = r
        .observe(c.operation(), c.arguments(), p.bytes())
        .unwrap()
        .unwrap();
    assert!(!result.pull_requests[0].details.contains_key("merged"));
}

#[test]
fn issue_wrappers_are_retained_separately_through_a_complete_pure_read() {
    let mut r = issues();
    let detail = json!({"number":17,"title":"Issue","state":"open","comments":0,
        "html_url":"https://ghe.example.test/team/project/issues/17"});
    let observations = [
        (detail.clone(), json!({"method":"get","issue":detail})),
        (json!([]), json!({"method":"get_comments","comments":[]})),
        (detail.clone(), json!({"method":"get","issue":detail})),
    ];
    for (i, (text, structured)) in observations.into_iter().enumerate() {
        let c = r.next_call().unwrap().clone();
        let text = text.to_string();
        let structured = structured.to_string();
        // Content order never determines which JSON object is the payload.
        let p = projected_github_payload(
            &c,
            false,
            [
                ProjectedBlock::Structured(&structured),
                ProjectedBlock::Text(&text),
            ],
        )
        .unwrap();
        assert_eq!(p.structured_companion, Some(structured.as_str()));
        assert_eq!(
            r.observe(c.operation(), c.arguments(), p.bytes())
                .unwrap()
                .is_some(),
            i == 2
        );
    }
}

#[test]
fn workflow_run_wrapper_does_not_become_the_list_payload() {
    let mut r = runs();
    let text_value = json!({"total_count":1,"workflow_runs":[run()]});
    let structured = json!({"method":"list_workflow_runs","workflow_runs":text_value}).to_string();
    let text = text_value.to_string();
    let c = r.next_call().unwrap().clone();
    let p = projected_github_payload(
        &c,
        false,
        [
            ProjectedBlock::Text(&text),
            ProjectedBlock::Structured(&structured),
        ],
    )
    .unwrap();
    let result = r
        .observe(c.operation(), c.arguments(), p.bytes())
        .unwrap()
        .unwrap();
    assert_eq!(result.runs[0].conclusion.as_deref(), Some("failure"));
}

#[test]
fn workflow_jobs_keep_the_distinct_jobs_text_envelope() {
    let mut r = jobs();
    let observations = [
        (
            run(),
            json!({"method":"get_workflow_run","workflow_run":run()}),
        ),
        (
            json!({"jobs":{"total_count":0,"jobs":[]}}),
            json!({"method":"list_workflow_jobs","workflow_jobs":{"total_count":0,"jobs":[]}}),
        ),
        (
            run(),
            json!({"method":"get_workflow_run","workflow_run":run()}),
        ),
    ];
    for (i, (text, structured)) in observations.into_iter().enumerate() {
        let c = r.next_call().unwrap().clone();
        let text = text.to_string();
        let structured = structured.to_string();
        let p = projected_github_payload(
            &c,
            false,
            [
                ProjectedBlock::Text(&text),
                ProjectedBlock::Structured(&structured),
            ],
        )
        .unwrap();
        assert_eq!(
            r.observe(c.operation(), c.arguments(), p.bytes())
                .unwrap()
                .is_some(),
            i == 2
        );
    }
}

#[test]
fn list_issues_can_consume_a_single_coalesced_structured_page() {
    let mut r = RepositoryIssues::new(
        HOST,
        "team",
        "project",
        IssueQuery::Open,
        IssueReadLimits::new(1, 1048576).unwrap(),
    )
    .unwrap();
    let text = r#"{"issues":[],"totalCount":0,"pageInfo":{"hasNextPage":false,"endCursor":null}}"#;
    let c = r.next_call().unwrap().clone();
    let p = projected_github_payload(&c, false, [ProjectedBlock::Structured(text)]).unwrap();
    assert!(
        r.observe(c.operation(), c.arguments(), p.bytes())
            .unwrap()
            .is_some()
    );
}

#[test]
fn get_commit_and_list_commits_accept_single_structured_results() {
    let mut r = RepositoryHistory::at_head(
        HOST,
        "team",
        "project",
        HEAD,
        HistoryWindow::new("2026-10-20T08:00:00Z", "2026-10-27T08:00:00Z").unwrap(),
        HistoryReadLimits::new(2, 1048576).unwrap(),
    )
    .unwrap();
    let commit = json!({"sha":HEAD,"html_url":format!("https://{HOST}/team/project/commit/{HEAD}"),"commit":{"message":"data"}}).to_string();
    for (i, text) in [commit.as_str(), "[]"].into_iter().enumerate() {
        let c = r.next_call().unwrap().clone();
        let p = projected_github_payload(&c, false, [ProjectedBlock::Structured(text)]).unwrap();
        assert_eq!(
            r.observe(c.operation(), c.arguments(), p.bytes())
                .unwrap()
                .is_some(),
            i == 1
        );
    }
}

#[test]
fn error_bit_cannot_be_laundered_by_a_valid_json_payload() {
    for blocks in [
        vec![ProjectedBlock::Text("{}")],
        vec![ProjectedBlock::Structured("{}")],
        vec![ProjectedBlock::Text("{}"), ProjectedBlock::Structured("{}")],
    ] {
        assert_eq!(
            projected_github_payload(&pr_call(), true, blocks).unwrap_err(),
            Refusal::UnsupportedShape
        );
    }
}

#[test]
fn missing_duplicate_extra_and_non_json_blocks_refuse() {
    for blocks in [
        vec![],
        vec![ProjectedBlock::Unsupported],
        vec![ProjectedBlock::Text("{}"), ProjectedBlock::Text("{}")],
        vec![
            ProjectedBlock::Structured("{}"),
            ProjectedBlock::Structured("{}"),
        ],
        vec![
            ProjectedBlock::Text("{}"),
            ProjectedBlock::Structured("{}"),
            ProjectedBlock::Unsupported,
        ],
        vec![ProjectedBlock::Text("{} {}")],
        vec![ProjectedBlock::Structured("not json")],
    ] {
        assert_eq!(
            projected_github_payload(&pr_call(), false, blocks).unwrap_err(),
            Refusal::UnsupportedShape
        );
    }
}

#[test]
fn wrapped_companions_require_the_actual_method_and_complete_json() {
    let r = issues();
    let c = r.next_call().unwrap();
    for companion in [
        r#"{"method":"get_comments"}"#,
        r#"{}"#,
        r#"{"method":null}"#,
    ] {
        assert_eq!(
            projected_github_payload(
                c,
                false,
                [
                    ProjectedBlock::Text("{}"),
                    ProjectedBlock::Structured(companion)
                ]
            )
            .unwrap_err(),
            Refusal::RequestChanged
        );
    }
    assert_eq!(
        projected_github_payload(
            c,
            false,
            [
                ProjectedBlock::Text("{}"),
                ProjectedBlock::Structured(r#"{"method":"get"} {}"#)
            ]
        )
        .unwrap_err(),
        Refusal::UnsupportedShape
    );
}

#[test]
fn wrapped_output_alone_never_replaces_missing_text() {
    let calls = [
        issues().next_call().unwrap().clone(),
        runs().next_call().unwrap().clone(),
        jobs().next_call().unwrap().clone(),
    ];
    for c in calls {
        let wrapper = json!({"method":c.arguments()["method"]}).to_string();
        assert_eq!(
            projected_github_payload(&c, false, [ProjectedBlock::Structured(&wrapper)])
                .unwrap_err(),
            Refusal::UnsupportedShape
        );
    }
}

#[test]
fn differing_pr_mirror_is_refused_and_text_only_preserves_exact_bytes() {
    let c = pr_call();
    assert_eq!(
        projected_github_payload(
            &c,
            false,
            [
                ProjectedBlock::Text("{}"),
                ProjectedBlock::Structured(r#"{"number":2}"#)
            ]
        )
        .unwrap_err(),
        Refusal::UnsupportedShape
    );
    let bytes = " \n{\"number\":17}\n ";
    assert_eq!(
        projected_github_payload(&c, false, [ProjectedBlock::Text(bytes)])
            .unwrap()
            .bytes(),
        bytes.as_bytes()
    );
}

#[test]
fn total_bound_counts_both_retained_blocks_before_parsing() {
    let r = issues();
    let c = r.next_call().unwrap();
    let huge = " ".repeat(32 * 1024 * 1024);
    assert_eq!(
        projected_github_payload(
            c,
            false,
            [
                ProjectedBlock::Text("{}"),
                ProjectedBlock::Structured(&huge)
            ]
        )
        .unwrap_err(),
        Refusal::ResultTooLarge
    );
}

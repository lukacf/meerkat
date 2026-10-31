use forge_ghe_collections::*;
use serde::Serialize;
use serde_json::{json, to_value};

#[test]
fn all_completed_collection_outputs_can_be_retained_as_structured_data() {
    fn output<T: Serialize>() {}
    output::<HistoricalStatuses>();
    output::<ReportedPullRequestChanges>();
    output::<ReportedReviewThreads>();
    output::<ReportedDiscussionPages>();
    output::<ReportedCommitHistory>();
    output::<ReportedWorkflowRuns>();
    output::<ReportedWorkflowRunJobs>();
    output::<ReportedIssues>();
    output::<ReportedIssueComments>();
    output::<ReportedIssueHierarchy>();
    output::<ReportedPullRequests>();
}

#[test]
fn failure_states_missing_metadata_and_capped_provider_fields_remain_data() {
    let status: HistoricalStatuses = serde_json::from_value(json!({
        "state":"failure","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","total_count":1,
        "statuses":[{"state":"error","context":"build","description":"line\nquote\"nul\0"}]
    }))
    .unwrap();
    let stored = to_value(status).unwrap();
    assert_eq!(stored["state"], "failure");
    assert_eq!(stored["statuses"][0]["state"], "error");
    assert_eq!(stored["statuses"][0]["description"], "line\nquote\"nul\0");
    assert_eq!(stored["statuses"][0]["target_url"], serde_json::Value::Null);
    let issue: IssueRecord =
        serde_json::from_value(json!({"number":17,"title":"Issue", "state":"OPEN",
        "closing_pull_requests":[{"number":22}], "has_parent":false, "has_children":false}))
        .unwrap();
    let stored = to_value(issue).unwrap();
    assert_eq!(stored["closing_pull_requests"], json!([{"number":22}]));
    assert_eq!(stored["html_url"], serde_json::Value::Null);
    assert!(stored.get("id").is_none());
    assert!(stored.get("ready").is_none());
    assert!(stored.get("credential_continuity").is_none());
}

#[test]
fn selected_window_and_query_intent_survive_without_currentness_claims() {
    let window = HistoryWindow::new("2026-10-20T08:00:00Z", "2026-10-27T08:00:00Z").unwrap();
    assert_eq!(
        to_value(window).unwrap(),
        json!({"since":"2026-10-20T08:00:00Z","until":"2026-10-27T08:00:00Z"})
    );
    let issues = ReportedIssues {
        repository_url: "https://ghe.example.test/team/project".into(),
        query: IssueQuery::UpdatedSince("2026-10-20T08:00:00Z".into()),
        reported_total: 0,
        pages_read: 1,
        issues: vec![],
    };
    let stored = to_value(issues).unwrap();
    assert_eq!(
        stored["query"],
        json!({"UpdatedSince":"2026-10-20T08:00:00Z"})
    );
    assert_eq!(stored["issues"], json!([]));
    assert!(stored.get("complete_source_coverage").is_none());
}

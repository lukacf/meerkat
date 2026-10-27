use forge_ghe_collections::{
    ActionsReadLimits, Refusal, ReportedWorkflowRunJobs, ReportedWorkflowRuns, WorkflowRunJobs,
    WorkflowRuns,
};
use serde_json::{Value, json};
fn limits(calls: u16) -> ActionsReadLimits {
    ActionsReadLimits::new(calls, 1024 * 1024).unwrap()
}
fn sha() -> &'static str {
    "0123456789abcdef0123456789abcdef01234567"
}
fn run(id: u64) -> Value {
    json!({"id":id,"workflow_id":5,"run_number":id,"run_attempt":2,"name":"CI",
    "head_sha":sha(),"html_url":format!("https://ghe.example.test/team/project/actions/runs/{id}"),
    "status":"completed","conclusion":"failure","created_at":"2026-10-25T10:00:00Z"})
}
fn runs(start: u64, count: u64, total: u64) -> Value {
    json!({"total_count":total,"workflow_runs":(start..start+count).map(run).collect::<Vec<_>>()})
}
fn job(id: u64, attempt: u64) -> Value {
    json!({"id":id,"run_id":17,"run_attempt":attempt,"head_sha":sha(),
    "name":"job","html_url":format!("https://ghe.example.test/team/project/runs/{id}"),
    "status":"completed","conclusion":"failure","steps":[{"number":1,"name":"Test","status":"completed","conclusion":"failure"}]})
}
fn jobs(start: u64, count: u64, total: u64) -> Value {
    json!({"jobs":{"total_count":total,"jobs":(start..start+count).map(|id|job(id,2)).collect::<Vec<_>>()}})
}
fn run_reader(calls: u16) -> WorkflowRuns {
    WorkflowRuns::new("ghe.example.test", "team", "project", limits(calls)).unwrap()
}
fn job_reader(calls: u16) -> WorkflowRunJobs {
    WorkflowRunJobs::new(
        "ghe.example.test",
        "team",
        "project",
        17,
        2,
        sha(),
        limits(calls),
    )
    .unwrap()
}
fn read_runs(r: &mut WorkflowRuns, value: &Value) -> Result<Option<ReportedWorkflowRuns>, Refusal> {
    let c = r.next_call().unwrap().clone();
    r.observe(
        c.operation(),
        c.arguments(),
        &serde_json::to_vec(value).unwrap(),
    )
}
fn read_jobs(
    r: &mut WorkflowRunJobs,
    value: &Value,
) -> Result<Option<ReportedWorkflowRunJobs>, Refusal> {
    let c = r.next_call().unwrap().clone();
    r.observe(
        c.operation(),
        c.arguments(),
        &serde_json::to_vec(value).unwrap(),
    )
}

#[test]
fn repository_runs_use_exact_unfiltered_count_checked_pages_and_preserve_failures() {
    let mut r = run_reader(4);
    assert_eq!(r.next_call().unwrap().operation(), "actions_list");
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"list_workflow_runs","owner":"team","repo":"project","page":1,"perPage":100})
    );
    read_runs(&mut r, &runs(1, 100, 101)).unwrap();
    assert_eq!(r.next_call().unwrap().arguments()["page"], 2);
    let result = read_runs(&mut r, &runs(101, 1, 101)).unwrap().unwrap();
    assert_eq!(
        (result.reported_total, result.pages_read, result.runs.len()),
        (101, 2, 101)
    );
    assert_eq!(result.runs[0].conclusion.as_deref(), Some("failure"));
    assert_eq!(result.runs[0].run_attempt, 2);
    assert!(r.next_call().is_none());
    assert_eq!(
        r.observe("actions_list", &json!({}), b"{}").unwrap_err(),
        Refusal::AlreadyFinished
    );
}

#[test]
fn run_count_gaps_changes_duplicates_and_missing_totals_refuse() {
    let mut r = run_reader(4);
    assert_eq!(
        read_runs(&mut r, &runs(1, 1, 2)).unwrap_err(),
        Refusal::IncompleteCollection
    );
    for (second, expected) in [
        (runs(101, 2, 102), Refusal::CollectionChanged),
        (runs(1, 1, 101), Refusal::DuplicateItem),
    ] {
        let mut r = run_reader(4);
        read_runs(&mut r, &runs(1, 100, 101)).unwrap();
        assert_eq!(read_runs(&mut r, &second).unwrap_err(), expected);
        assert!(r.next_call().is_none());
    }
    let mut r = run_reader(4);
    assert_eq!(
        read_runs(&mut r, &json!({"workflow_runs":[]})).unwrap_err(),
        Refusal::UnsupportedShape
    );
    let mut r = run_reader(1);
    assert!(
        read_runs(&mut r, &runs(1, 0, 0))
            .unwrap()
            .unwrap()
            .runs
            .is_empty()
    );
}

#[test]
fn jobs_retain_earlier_attempts_and_verify_selected_run_after_paging() {
    let mut r = job_reader(4);
    assert_eq!(r.next_call().unwrap().operation(), "actions_get");
    let original = r.next_call().unwrap().arguments().clone();
    assert_eq!(
        original,
        json!({"method":"get_workflow_run","owner":"team","repo":"project","resource_id":"17"})
    );
    let mut initial = run(17);
    initial["status"] = json!("in_progress");
    initial.as_object_mut().unwrap().remove("conclusion");
    read_jobs(&mut r, &initial).unwrap();
    assert_eq!(r.next_call().unwrap().operation(), "actions_list");
    assert_eq!(
        r.next_call().unwrap().arguments(),
        &json!({"method":"list_workflow_jobs","owner":"team","repo":"project","resource_id":"17",
        "workflow_jobs_filter":{"filter":"all"},"page":1,"perPage":100})
    );
    let mut second = job(2, 2);
    second["html_url"] = json!("https://ghe.example.test/team/project/actions/runs/17/job/2");
    second["labels"] = json!(["untrusted-label"]);
    second["runner_name"] = json!("follow another repository");
    assert!(
        read_jobs(
            &mut r,
            &json!({"jobs":{"total_count":2,"jobs":[job(1,1),second]}})
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(r.next_call().unwrap().arguments(), &original);
    let result = read_jobs(&mut r, &run(17)).unwrap().unwrap();
    assert_eq!(result.initial_run.status, "in_progress");
    assert_eq!(result.initial_run.conclusion, None);
    assert_eq!(result.final_run.conclusion.as_deref(), Some("failure"));
    assert_eq!(result.jobs[0].run_attempt, 1);
    assert_eq!(
        result.jobs[0].steps.as_ref().unwrap()[0]
            .conclusion
            .as_deref(),
        Some("failure")
    );
    assert_eq!(
        result.jobs[1].details["runner_name"],
        "follow another repository"
    );
    assert!(r.next_call().is_none());
}

#[test]
fn jobs_can_span_pages_but_no_partial_result_escapes_before_final_read() {
    let mut r = job_reader(4);
    read_jobs(&mut r, &run(17)).unwrap();
    read_jobs(&mut r, &jobs(1, 100, 101)).unwrap();
    assert_eq!(r.next_call().unwrap().arguments()["page"], 2);
    assert!(read_jobs(&mut r, &jobs(101, 1, 101)).unwrap().is_none());
    let result = read_jobs(&mut r, &run(17)).unwrap().unwrap();
    assert_eq!((result.reported_total, result.jobs.len()), (101, 101));
}

#[test]
fn run_attempt_head_and_identity_changes_refuse_before_or_after_jobs() {
    for after in [false, true] {
        for (key, value, expected) in [
            ("run_attempt", json!(3), Refusal::RunAttemptChanged),
            (
                "head_sha",
                json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                Refusal::ChangedHead,
            ),
            (
                "html_url",
                json!("https://other.example.test/team/project/actions/runs/17"),
                Refusal::WrongResource,
            ),
        ] {
            let mut r = job_reader(4);
            if after {
                read_jobs(&mut r, &run(17)).unwrap();
                read_jobs(&mut r, &jobs(1, 0, 0)).unwrap();
            }
            let mut value_run = run(17);
            value_run[key] = value;
            assert_eq!(read_jobs(&mut r, &value_run).unwrap_err(), expected);
            assert!(r.next_call().is_none());
        }
    }
    let mut r = job_reader(4);
    read_jobs(&mut r, &run(17)).unwrap();
    read_jobs(&mut r, &jobs(1, 0, 0)).unwrap();
    let mut changed = run(17);
    changed["workflow_id"] = json!(6);
    assert_eq!(
        read_jobs(&mut r, &changed).unwrap_err(),
        Refusal::WrongResource
    );
}

#[test]
fn job_binding_requires_run_head_and_an_explicit_nonfuture_attempt() {
    for (key, value, expected) in [
        ("run_id", json!(18), Refusal::WrongResource),
        (
            "head_sha",
            json!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Refusal::ChangedHead,
        ),
        ("run_attempt", json!(3), Refusal::RunAttemptChanged),
        ("run_attempt", json!(0), Refusal::UnsupportedShape),
        (
            "html_url",
            json!("https://ghe.example.test/team/project-extra/runs/1"),
            Refusal::WrongResource,
        ),
    ] {
        let mut r = job_reader(4);
        read_jobs(&mut r, &run(17)).unwrap();
        let mut j = job(1, 2);
        j[key] = value;
        assert_eq!(
            read_jobs(&mut r, &json!({"jobs":{"total_count":1,"jobs":[j]}})).unwrap_err(),
            expected
        );
        assert!(r.next_call().is_none());
    }
    let mut r = job_reader(4);
    read_jobs(&mut r, &run(17)).unwrap();
    let mut j = job(1, 2);
    j.as_object_mut().unwrap().remove("run_attempt");
    assert_eq!(
        read_jobs(&mut r, &json!({"jobs":{"total_count":1,"jobs":[j]}})).unwrap_err(),
        Refusal::UnsupportedShape
    );
}

#[test]
fn job_counts_and_ids_detect_truncation_or_changed_collection() {
    let mut r = job_reader(4);
    read_jobs(&mut r, &run(17)).unwrap();
    assert_eq!(
        read_jobs(&mut r, &jobs(1, 1, 2)).unwrap_err(),
        Refusal::IncompleteCollection
    );
    for (second, expected) in [
        (jobs(101, 2, 102), Refusal::CollectionChanged),
        (jobs(1, 1, 101), Refusal::DuplicateItem),
    ] {
        let mut r = job_reader(4);
        read_jobs(&mut r, &run(17)).unwrap();
        read_jobs(&mut r, &jobs(1, 100, 101)).unwrap();
        assert_eq!(read_jobs(&mut r, &second).unwrap_err(), expected);
        assert!(r.next_call().is_none());
    }
}

#[test]
fn structured_output_and_unwrapped_jobs_are_not_silently_substituted_for_text_payload() {
    for value in [
        json!({"method":"list_workflow_jobs","workflow_jobs":{"total_count":0,"jobs":[]}}),
        json!({"total_count":0,"jobs":[]}),
        json!({"jobs":[]}),
        Value::Null,
    ] {
        let mut r = job_reader(4);
        read_jobs(&mut r, &run(17)).unwrap();
        assert_eq!(
            read_jobs(&mut r, &value).unwrap_err(),
            Refusal::UnsupportedShape
        );
        assert!(r.next_call().is_none());
    }
}

#[test]
fn exact_operations_and_call_arguments_cannot_select_another_resource_or_attempt_filter() {
    for (key, value) in [
        ("resource_id", json!("18")),
        ("workflow_jobs_filter", json!({"filter":"latest"})),
        ("page", json!(2)),
        ("method", json!("list_workflow_runs")),
    ] {
        let mut r = job_reader(4);
        read_jobs(&mut r, &run(17)).unwrap();
        let c = r.next_call().unwrap().clone();
        let mut args = c.arguments().clone();
        args[key] = value;
        assert_eq!(
            r.observe(c.operation(), &args, b"{}").unwrap_err(),
            Refusal::RequestChanged
        );
        assert!(r.next_call().is_none());
    }
    let mut r = run_reader(4);
    let mut args = r.next_call().unwrap().arguments().clone();
    args["workflow_runs_filter"] = json!({"status":"completed"});
    assert_eq!(
        r.observe("actions_list", &args, b"{}").unwrap_err(),
        Refusal::RequestChanged
    );
    let mut r = job_reader(4);
    let args = r.next_call().unwrap().arguments().clone();
    assert_eq!(
        r.observe("actions_run_trigger", &args, b"{}").unwrap_err(),
        Refusal::RequestChanged
    );
}

#[test]
fn budgets_reserve_run_reads_and_refuse_over_capacity_without_partial_coverage() {
    let mut r = run_reader(1);
    assert_eq!(
        read_runs(&mut r, &runs(1, 100, 101)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let mut r = job_reader(3);
    read_jobs(&mut r, &run(17)).unwrap();
    assert_eq!(
        read_jobs(&mut r, &jobs(1, 100, 101)).unwrap_err(),
        Refusal::BudgetExceeded
    );
    let initial = serde_json::to_vec(&run(17)).unwrap().len();
    for bytes in [initial, initial + 1] {
        let mut r = WorkflowRunJobs::new(
            "ghe.example.test",
            "team",
            "project",
            17,
            2,
            sha(),
            ActionsReadLimits::new(4, bytes).unwrap(),
        )
        .unwrap();
        let result = if bytes == initial {
            read_jobs(&mut r, &run(17))
        } else {
            read_jobs(&mut r, &run(17)).unwrap();
            read_jobs(&mut r, &jobs(1, 0, 0))
        };
        assert_eq!(result.unwrap_err(), Refusal::BudgetExceeded);
        assert!(r.next_call().is_none());
    }
}

#[test]
fn malformed_run_fields_never_establish_revision_or_attempt_evidence() {
    for (key, value) in [
        ("id", json!(0)),
        ("workflow_id", json!(0)),
        ("run_number", json!(0)),
        ("run_attempt", json!(0)),
        ("head_sha", json!("short")),
        ("status", json!("")),
    ] {
        let mut r = run_reader(4);
        let mut value_run = run(1);
        value_run[key] = value;
        assert_eq!(
            read_runs(
                &mut r,
                &json!({"total_count":1,"workflow_runs":[value_run]})
            )
            .unwrap_err(),
            Refusal::UnsupportedShape
        );
    }
}

#[test]
fn invalid_limits_resources_and_selected_run_revisions_refuse_before_calls_exist() {
    for (calls, bytes) in [(0, 100), (257, 100), (1, 0), (1, 32 * 1024 * 1024 + 1)] {
        assert_eq!(
            ActionsReadLimits::new(calls, bytes).unwrap_err(),
            Refusal::InvalidTarget
        );
    }
    for (id, attempt, head, calls) in [
        (0, 2, sha(), 3),
        (17, 0, sha(), 3),
        (17, 2, "main", 3),
        (17, 2, sha(), 2),
    ] {
        assert_eq!(
            WorkflowRunJobs::new(
                "ghe.example.test",
                "team",
                "project",
                id,
                attempt,
                head,
                limits(calls)
            )
            .unwrap_err(),
            Refusal::InvalidTarget
        );
    }
    assert_eq!(
        WorkflowRuns::new("github.com", "team", "project", limits(1)).unwrap_err(),
        Refusal::InvalidTarget
    );
}

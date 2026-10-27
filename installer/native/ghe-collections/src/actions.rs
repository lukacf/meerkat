//! Count-checked workflow runs and jobs. These are historical provider results,
//! never execution success, current CI readiness or a native approval decision.
//! Observe consumes the pinned successful textual JSON payload, not the MCP
//! envelope or its differently wrapped structuredContent projection.

use super::{ReadCall, Refusal, commit_id, decode, enterprise_host, segment};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const PAGE_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy)]
pub struct ActionsReadLimits {
    calls: u16,
    result_bytes: usize,
}
impl ActionsReadLimits {
    pub fn new(calls: u16, result_bytes: usize) -> Result<Self, Refusal> {
        if !(1..=256).contains(&calls) || !(1..=32 * 1024 * 1024).contains(&result_bytes) {
            return Err(Refusal::InvalidTarget);
        }
        Ok(Self {
            calls,
            result_bytes,
        })
    }
}

#[derive(Debug)]
struct Budget {
    limits: ActionsReadLimits,
    calls: u16,
    bytes: usize,
}
impl Budget {
    fn new(limits: ActionsReadLimits) -> Self {
        Self {
            limits,
            calls: 0,
            bytes: 0,
        }
    }
    fn consume(&mut self, payload: &[u8]) -> Result<(), Refusal> {
        if payload.len() > 32 * 1024 * 1024 {
            return Err(Refusal::ResultTooLarge);
        }
        self.calls += 1;
        self.bytes += payload.len();
        if self.calls > self.limits.calls || self.bytes > self.limits.result_bytes {
            return Err(Refusal::BudgetExceeded);
        }
        Ok(())
    }
    fn next(&self) -> Result<(), Refusal> {
        if self.calls >= self.limits.calls || self.bytes >= self.limits.result_bytes {
            Err(Refusal::BudgetExceeded)
        } else {
            Ok(())
        }
    }
}

fn scope(host: &str, owner: &str, repo: &str) -> Result<String, Refusal> {
    if !enterprise_host(host) || !segment(owner) || !segment(repo) {
        return Err(Refusal::InvalidTarget);
    }
    Ok(format!("https://{host}/{owner}/{repo}"))
}
fn valid_id(id: u64) -> bool {
    id > 0 && id <= i64::MAX as u64
}
fn valid_status(status: &str) -> bool {
    !status.is_empty() && status.len() <= 128 && !status.chars().any(char::is_control)
}
fn matching(call: &ReadCall, operation: &str, arguments: &Value) -> Result<(), Refusal> {
    if operation != call.operation() || arguments != call.arguments() {
        Err(Refusal::RequestChanged)
    } else {
        Ok(())
    }
}
fn check_page(
    total: &mut Option<u64>,
    expected: u64,
    observed: usize,
    received: usize,
    capacity: u64,
) -> Result<bool, Refusal> {
    if total.is_some_and(|prior| prior != expected) {
        return Err(Refusal::CollectionChanged);
    }
    *total = Some(expected);
    if expected > capacity {
        return Err(Refusal::BudgetExceeded);
    }
    if observed as u64 > expected || received as u64 != (expected - observed as u64).min(PAGE_SIZE)
    {
        return Err(Refusal::IncompleteCollection);
    }
    Ok(observed as u64 + received as u64 == expected)
}

#[derive(Debug)]
pub struct WorkflowRuns {
    repository_url: String,
    pending: Option<(ReadCall, u16)>,
    budget: Budget,
    total: Option<u64>,
    ids: BTreeSet<u64>,
    runs: Vec<WorkflowRun>,
}
impl WorkflowRuns {
    /// Enumerate the unfiltered repository run list. The pinned source has no
    /// created/head-SHA filter; exceeding this bounded enumeration must refuse.
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        limits: ActionsReadLimits,
    ) -> Result<Self, Refusal> {
        Ok(Self {
            repository_url: scope(host, owner, repo)?,
            pending: Some((
                ReadCall {
                    operation: "actions_list",
                    arguments: json!({"method":"list_workflow_runs","owner":owner,"repo":repo,"page":1,"perPage":PAGE_SIZE}),
                },
                1,
            )),
            budget: Budget::new(limits),
            total: None,
            ids: BTreeSet::new(),
            runs: Vec::new(),
        })
    }
    pub fn next_call(&self) -> Option<&ReadCall> {
        self.pending.as_ref().map(|(call, _)| call)
    }
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedWorkflowRuns>, Refusal> {
        let (mut call, page) = self.pending.take().ok_or(Refusal::AlreadyFinished)?;
        matching(&call, operation, arguments)?;
        self.budget.consume(payload)?;
        let result: RunsPage = decode(payload)?;
        let done = check_page(
            &mut self.total,
            result.total_count,
            self.runs.len(),
            result.workflow_runs.len(),
            u64::from(self.budget.limits.calls) * PAGE_SIZE,
        )?;
        for run in &result.workflow_runs {
            run.check(&self.repository_url)?;
            if !self.ids.insert(run.id) {
                return Err(Refusal::DuplicateItem);
            }
        }
        self.runs.extend(result.workflow_runs);
        if done {
            Ok(Some(ReportedWorkflowRuns {
                reported_total: result.total_count,
                pages_read: page,
                runs: std::mem::take(&mut self.runs),
            }))
        } else {
            self.budget.next()?;
            call.arguments["page"] = json!(page + 1);
            self.pending = Some((call, page + 1));
            Ok(None)
        }
    }
}

#[derive(Debug)]
enum JobPhase {
    Initial(ReadCall),
    Jobs { call: ReadCall, page: u16 },
    Verify(ReadCall),
    Finished,
}

#[derive(Debug)]
pub struct WorkflowRunJobs {
    repository_url: String,
    run_id: u64,
    attempt: u64,
    head: String,
    initial_call: ReadCall,
    phase: JobPhase,
    budget: Budget,
    initial_run: Option<WorkflowRun>,
    total: Option<u64>,
    ids: BTreeSet<u64>,
    jobs: Vec<WorkflowJob>,
}
impl WorkflowRunJobs {
    /// Preserve the run/head/attempt selected in the retained native input.
    /// Jobs from all earlier attempts are retained, never silently replaced by
    /// only the latest attempt. This constructor does not authorize the reads.
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        run_id: u64,
        attempt: u64,
        head: &str,
        limits: ActionsReadLimits,
    ) -> Result<Self, Refusal> {
        if !valid_id(run_id) || !valid_id(attempt) || !commit_id(head) || limits.calls < 3 {
            return Err(Refusal::InvalidTarget);
        }
        let call = ReadCall {
            operation: "actions_get",
            arguments: json!({"method":"get_workflow_run","owner":owner,"repo":repo,"resource_id":run_id.to_string()}),
        };
        Ok(Self {
            repository_url: scope(host, owner, repo)?,
            run_id,
            attempt,
            head: head.to_owned(),
            initial_call: call.clone(),
            phase: JobPhase::Initial(call),
            budget: Budget::new(limits),
            initial_run: None,
            total: None,
            ids: BTreeSet::new(),
            jobs: Vec::new(),
        })
    }
    pub fn next_call(&self) -> Option<&ReadCall> {
        match &self.phase {
            JobPhase::Initial(call) | JobPhase::Verify(call) | JobPhase::Jobs { call, .. } => {
                Some(call)
            }
            JobPhase::Finished => None,
        }
    }
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedWorkflowRunJobs>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, JobPhase::Finished);
        let call = match &phase {
            JobPhase::Initial(call) | JobPhase::Verify(call) | JobPhase::Jobs { call, .. } => call,
            JobPhase::Finished => return Err(Refusal::AlreadyFinished),
        };
        matching(call, operation, arguments)?;
        self.budget.consume(payload)?;
        match phase {
            JobPhase::Initial(mut call) => {
                let run: WorkflowRun = decode(payload)?;
                self.check_run(&run)?;
                self.initial_run = Some(run);
                self.budget.next()?;
                call.operation = "actions_list";
                call.arguments["method"] = json!("list_workflow_jobs");
                call.arguments["workflow_jobs_filter"] = json!({"filter":"all"});
                call.arguments["page"] = json!(1);
                call.arguments["perPage"] = json!(PAGE_SIZE);
                self.phase = JobPhase::Jobs { call, page: 1 };
                Ok(None)
            }
            JobPhase::Jobs { mut call, page } => {
                // Successful text has {"jobs":{"total_count":...,"jobs":[]}}.
                // Structured MCP output uses a different, method-tagged wrapper.
                let result: TextJobsOutput = decode(payload)?;
                let result = result.jobs;
                let done = check_page(
                    &mut self.total,
                    result.total_count,
                    self.jobs.len(),
                    result.jobs.len(),
                    u64::from(self.budget.limits.calls - 2) * PAGE_SIZE,
                )?;
                for job in &result.jobs {
                    if !valid_id(job.id) || !valid_id(job.run_attempt) || !valid_status(&job.status)
                    {
                        return Err(Refusal::UnsupportedShape);
                    }
                    if job.run_id != self.run_id
                        || !job
                            .html_url
                            .starts_with(&format!("{}/", self.repository_url))
                    {
                        return Err(Refusal::WrongResource);
                    }
                    if job.head_sha != self.head {
                        return Err(Refusal::ChangedHead);
                    }
                    if job.run_attempt > self.attempt {
                        return Err(Refusal::RunAttemptChanged);
                    }
                    if !self.ids.insert(job.id) {
                        return Err(Refusal::DuplicateItem);
                    }
                }
                self.jobs.extend(result.jobs);
                self.budget.next()?;
                self.phase = if done {
                    JobPhase::Verify(self.initial_call.clone())
                } else {
                    call.arguments["page"] = json!(page + 1);
                    JobPhase::Jobs {
                        call,
                        page: page + 1,
                    }
                };
                Ok(None)
            }
            JobPhase::Verify(_) => {
                let run: WorkflowRun = decode(payload)?;
                self.check_run(&run)?;
                let initial = self.initial_run.take().ok_or(Refusal::UnsupportedShape)?;
                if initial.workflow_id != run.workflow_id {
                    return Err(Refusal::WrongResource);
                }
                Ok(Some(ReportedWorkflowRunJobs {
                    initial_run: initial,
                    final_run: run,
                    reported_total: self.total.ok_or(Refusal::UnsupportedShape)?,
                    jobs: std::mem::take(&mut self.jobs),
                }))
            }
            JobPhase::Finished => Err(Refusal::AlreadyFinished),
        }
    }
    fn check_run(&self, run: &WorkflowRun) -> Result<(), Refusal> {
        run.check(&self.repository_url)?;
        if run.id != self.run_id {
            return Err(Refusal::WrongResource);
        }
        if run.head_sha != self.head {
            return Err(Refusal::ChangedHead);
        }
        if run.run_attempt != self.attempt {
            return Err(Refusal::RunAttemptChanged);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct RunsPage {
    total_count: u64,
    workflow_runs: Vec<WorkflowRun>,
}
#[derive(Deserialize)]
struct TextJobsOutput {
    jobs: JobsPage,
}
#[derive(Deserialize)]
struct JobsPage {
    total_count: u64,
    jobs: Vec<WorkflowJob>,
}

/// Count-matching reported run pages, not an atomic repository snapshot or a
/// current CI verdict. All state values, including failures, remain data.
#[derive(Debug, serde::Serialize)]
pub struct ReportedWorkflowRuns {
    pub reported_total: u64,
    pub pages_read: u16,
    pub runs: Vec<WorkflowRun>,
}

/// All reported job attempts through the selected attempt, with matching run
/// identity/head/attempt before and after paging. State changes are preserved;
/// those two reads do not prove an atomic snapshot or execution success.
#[derive(Debug, serde::Serialize)]
pub struct ReportedWorkflowRunJobs {
    pub initial_run: WorkflowRun,
    pub final_run: WorkflowRun,
    pub reported_total: u64,
    pub jobs: Vec<WorkflowJob>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub workflow_id: u64,
    pub run_number: u64,
    pub run_attempt: u64,
    pub name: String,
    pub head_sha: String,
    pub html_url: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub run_started_at: Option<String>,
    /// Other pinned provider fields, including event/branch/actors and reusable
    /// workflows. Their contents never schedule calls or select credentials.
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}
impl WorkflowRun {
    fn check(&self, repo: &str) -> Result<(), Refusal> {
        if !valid_id(self.id)
            || !valid_id(self.workflow_id)
            || !valid_id(self.run_number)
            || !valid_id(self.run_attempt)
            || !commit_id(&self.head_sha)
            || !valid_status(&self.status)
        {
            return Err(Refusal::UnsupportedShape);
        }
        if self.html_url != format!("{repo}/actions/runs/{}", self.id) {
            return Err(Refusal::WrongResource);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct WorkflowJob {
    pub id: u64,
    pub run_id: u64,
    pub run_attempt: u64,
    pub head_sha: String,
    pub html_url: String,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub steps: Option<Vec<WorkflowStep>>,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct WorkflowStep {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub number: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

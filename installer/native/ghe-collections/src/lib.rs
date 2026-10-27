//! Forge-owned, bounded GitHub Enterprise result programs.
//!
//! This library performs no I/O and supplies no authentication, native receipt,
//! credential coverage or installed-service qualification. The eventual Connect
//! adapter must first check the installed endpoint/account and feed only its
//! matching CommittedCollectionResult after native settlement and joined cleanup.
//! JSON supplied to these pure functions is never proof of those conditions.

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

mod projection;
pub use projection::{GithubPayload, PayloadOrigin, ProjectedBlock, projected_github_payload};
mod pull_requests;
pub use pull_requests::{
    PullRequestBranch, PullRequestBranchRepository, PullRequestListLimits, PullRequestQuery,
    PullRequestSummary, ReportedPullRequests, RepositoryPullRequests,
};
mod issues;
pub use issues::{
    IssueComments, IssueQuery, IssueReadLimits, IssueRecord, ReportedIssueComments, ReportedIssues,
    RepositoryIssues,
};
mod actions;
pub use actions::{
    ActionsReadLimits, ReportedWorkflowRunJobs, ReportedWorkflowRuns, WorkflowJob, WorkflowRun,
    WorkflowRunJobs, WorkflowRuns, WorkflowStep,
};
mod history;
pub use history::{
    HistoryAnchor, HistoryReadLimits, HistoryWindow, ReportedCommitHistory, RepositoryCommit,
    RepositoryCommitInfo, RepositoryHistory,
};
mod changes;
pub use changes::{
    ChangesReadLimits, CommitAuthor, PullRequestChanges, PullRequestCommit, PullRequestFile,
    ReportedPullRequestChanges,
};
mod discussion;
pub use discussion::{
    CommentReactions, DiscussionKind, DiscussionReadLimits, DiscussionRows, DiscussionUser,
    PullRequestComment, PullRequestDiscussion, PullRequestReview, ReportedDiscussionPages,
};
mod review_threads;
pub use review_threads::{
    PullRequestReviewThreads, ReportedReviewThreads, ReviewComment, ReviewReadLimits, ReviewThread,
};

pub const RESULT_LIMIT: usize = 1024 * 1024;
pub const MCP_SOURCE: &str = "b98d115bbb080e9555466a96656ec4df605aea21";

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    InvalidTarget,
    RequestChanged,
    UnsupportedShape,
    WrongResource,
    ChangedHead,
    IncompleteStatusPage,
    DuplicateContext,
    ResultTooLarge,
    AlreadyFinished,
    BudgetExceeded,
    CursorRepeated,
    DuplicateItem,
    CollectionChanged,
    IncompleteCollection,
    IncompleteThread,
    RunAttemptChanged,
}

#[derive(Debug, Clone)]
pub struct ReadCall {
    operation: &'static str,
    arguments: Value,
}

impl ReadCall {
    pub fn operation(&self) -> &'static str {
        self.operation
    }

    pub fn arguments(&self) -> &Value {
        &self.arguments
    }
}

#[derive(Debug)]
enum Phase {
    PullRequest(ReadCall),
    Statuses { call: ReadCall, head: String },
    Finished,
}

/// A two-call historical status read for exactly one enterprise PR.
///
/// `get_status` in the pinned server does not expose status pagination. A page
/// with fewer statuses than total_count must refuse, never claim full coverage.
/// A changed PR head between calls also refuses. A completed result describes
/// only the returned commit; it does not assert the PR still has that head.
#[derive(Debug)]
pub struct PullRequestStatuses {
    phase: Phase,
    scope: PullRequestScope,
    expected_head: Option<String>,
}

#[derive(Debug)]
struct PullRequestScope {
    repository: String,
    pull_url: String,
    pull_number: u64,
}

impl PullRequestScope {
    fn new(host: &str, owner: &str, repo: &str, pull_number: u64) -> Result<Self, Refusal> {
        if !enterprise_host(host)
            || !segment(owner)
            || !segment(repo)
            || pull_number == 0
            || pull_number > i64::MAX as u64
        {
            return Err(Refusal::InvalidTarget);
        }
        Ok(Self {
            repository: format!("{owner}/{repo}"),
            pull_url: format!("https://{host}/{owner}/{repo}/pull/{pull_number}"),
            pull_number,
        })
    }

    fn check(&self, pr: &PullRequest) -> Result<(), Refusal> {
        if pr.number != self.pull_number
            || pr.html_url != self.pull_url
            || pr.base.repo.full_name != self.repository
        {
            return Err(Refusal::WrongResource);
        }
        if !commit_id(&pr.head.sha) {
            return Err(Refusal::UnsupportedShape);
        }
        Ok(())
    }
}

impl PullRequestStatuses {
    pub fn new(host: &str, owner: &str, repo: &str, pull_number: u64) -> Result<Self, Refusal> {
        let scope = PullRequestScope::new(host, owner, repo, pull_number)?;
        Ok(Self {
            phase: Phase::PullRequest(ReadCall {
                operation: "pull_request_read",
                arguments: json!({"method": "get", "owner": owner, "repo": repo, "pullNumber": pull_number}),
            }),
            scope,
            expected_head: None,
        })
    }

    /// Preserve a revision already selected by the native requirement owner.
    /// The server can read only current PR HEAD, so an older head must refuse.
    pub fn at_head(
        host: &str,
        owner: &str,
        repo: &str,
        pull_number: u64,
        head: &str,
    ) -> Result<Self, Refusal> {
        if !commit_id(head) {
            return Err(Refusal::InvalidTarget);
        }
        let mut read = Self::new(host, owner, repo, pull_number)?;
        read.expected_head = Some(head.to_owned());
        Ok(read)
    }

    pub fn next_call(&self) -> Option<&ReadCall> {
        match &self.phase {
            Phase::PullRequest(call) | Phase::Statuses { call, .. } => Some(call),
            Phase::Finished => None,
        }
    }

    /// Validate one exact call's JSON payload. A refusal permanently ends this
    /// cursor; retries require a separately admitted native attempt and cursor.
    /// The caller must handle native tool errors before invoking this method.
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<HistoricalStatuses>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        let call = match &phase {
            Phase::PullRequest(call) | Phase::Statuses { call, .. } => call,
            Phase::Finished => return Err(Refusal::AlreadyFinished),
        };
        if operation != call.operation() || arguments != call.arguments() {
            return Err(Refusal::RequestChanged);
        }
        if payload.len() > RESULT_LIMIT {
            return Err(Refusal::ResultTooLarge);
        }
        match phase {
            Phase::PullRequest(mut call) => {
                let pr: PullRequest = decode(payload)?;
                self.scope.check(&pr)?;
                if self
                    .expected_head
                    .as_ref()
                    .is_some_and(|head| head != &pr.head.sha)
                {
                    return Err(Refusal::ChangedHead);
                }
                call.arguments["method"] = json!("get_status");
                self.phase = Phase::Statuses {
                    call,
                    head: pr.head.sha,
                };
                Ok(None)
            }
            Phase::Statuses { head, .. } => {
                let status: HistoricalStatuses = decode(payload)?;
                if status.sha != head {
                    return Err(Refusal::ChangedHead);
                }
                if status.total_count != status.statuses.len() as u64 {
                    return Err(Refusal::IncompleteStatusPage);
                }
                let mut contexts = BTreeSet::new();
                for row in &status.statuses {
                    if row.context.is_empty() || row.context.contains('\0') {
                        return Err(Refusal::UnsupportedShape);
                    }
                    if !contexts.insert(&row.context) {
                        return Err(Refusal::DuplicateContext);
                    }
                }
                Ok(Some(status))
            }
            Phase::Finished => Err(Refusal::AlreadyFinished),
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Refusal> {
    serde_json::from_slice(bytes).map_err(|_| Refusal::UnsupportedShape)
}

fn commit_id(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

// Accept only explicit canonical DNS hosts, without userinfo, paths or ports.
// No public-GitHub alias can silently substitute for the enterprise endpoint.
fn enterprise_host(value: &str) -> bool {
    value.contains('.')
        && value.len() <= 253
        && value != "github.com"
        && !value.ends_with(".github.com")
        && value != "githubusercontent.com"
        && !value.ends_with(".githubusercontent.com")
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

#[derive(Deserialize)]
struct PullRequest {
    number: u64,
    html_url: String,
    head: Head,
    base: Base,
}

#[derive(Deserialize)]
struct Head {
    sha: String,
}

#[derive(Deserialize)]
struct Base {
    repo: Repository,
}

#[derive(Deserialize)]
struct Repository {
    full_name: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Error,
    Failure,
    Pending,
    Success,
}

/// Provider content only. Failure/error statuses are valid observed data, not
/// failed collection calls. These values never authorize another operation.
#[derive(Debug, Deserialize)]
pub struct RepositoryStatus {
    pub state: StatusState,
    pub context: String,
    pub description: Option<String>,
    pub target_url: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Historical provider data, deliberately not a receipt or readiness flag.
#[derive(Debug, Deserialize)]
pub struct HistoricalStatuses {
    pub state: StatusState,
    pub sha: String,
    pub total_count: u64,
    pub statuses: Vec<RepositoryStatus>,
}

//! Repository PR discovery using the pinned textual list payload. The provider
//! discards pagination headers and nil rows; a short page is an observation,
//! not independent proof of exhaustive history or an atomic snapshot.

use super::{ReadCall, Refusal, commit_id, decode, enterprise_host, segment};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum PullRequestQuery {
    All,
    Open,
    Closed,
}
impl PullRequestQuery {
    fn state(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct PullRequestListLimits {
    calls: u16,
    result_bytes: usize,
}
impl PullRequestListLimits {
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
pub struct RepositoryPullRequests {
    repository_url: String,
    repository_name: String,
    query: PullRequestQuery,
    pending: Option<ReadCall>,
    limits: PullRequestListLimits,
    calls: u16,
    result_bytes: usize,
    numbers: BTreeSet<u64>,
    rows: Vec<PullRequestSummary>,
}
impl RepositoryPullRequests {
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        query: PullRequestQuery,
        limits: PullRequestListLimits,
    ) -> Result<Self, Refusal> {
        if !enterprise_host(host) || !segment(owner) || !segment(repo) {
            return Err(Refusal::InvalidTarget);
        }
        Ok(Self {
            repository_url: format!("https://{host}/{owner}/{repo}"),
            repository_name: format!("{owner}/{repo}"),
            query,
            pending: Some(ReadCall {
                operation: "list_pull_requests",
                arguments: json!({"owner":owner,"repo":repo,"state":query.state(),"sort":"created","direction":"asc","page":1,"perPage":PAGE_SIZE}),
            }),
            limits,
            calls: 0,
            result_bytes: 0,
            numbers: BTreeSet::new(),
            rows: Vec::new(),
        })
    }
    pub fn next_call(&self) -> Option<&ReadCall> {
        self.pending.as_ref()
    }
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedPullRequests>, Refusal> {
        let mut call = self.pending.take().ok_or(Refusal::AlreadyFinished)?;
        if operation != call.operation() || arguments != call.arguments() {
            return Err(Refusal::RequestChanged);
        }
        if payload.len() > 32 * 1024 * 1024 {
            return Err(Refusal::ResultTooLarge);
        }
        self.calls += 1;
        self.result_bytes += payload.len();
        if self.calls > self.limits.calls || self.result_bytes > self.limits.result_bytes {
            return Err(Refusal::BudgetExceeded);
        }
        let rows: Vec<PullRequestSummary> = decode(payload)?;
        if rows.len() > PAGE_SIZE {
            return Err(Refusal::UnsupportedShape);
        }
        for row in &rows {
            if row.number == 0
                || row.number > i64::MAX as u64
                || !commit_id(&row.head.sha)
                || !commit_id(&row.base.sha)
                || row.state.is_empty()
                || row.state.len() > 128
                || row.state.chars().any(char::is_control)
            {
                return Err(Refusal::UnsupportedShape);
            }
            if row.html_url != format!("{}/pull/{}", self.repository_url, row.number)
                || row.base.repo.as_ref().map(|r| r.full_name.as_str())
                    != Some(self.repository_name.as_str())
            {
                return Err(Refusal::WrongResource);
            }
            if self.query != PullRequestQuery::All && row.state != self.query.state() {
                return Err(Refusal::CollectionChanged);
            }
            if !self.numbers.insert(row.number) {
                return Err(Refusal::DuplicateItem);
            }
        }
        let received = rows.len();
        self.rows.extend(rows);
        if received < PAGE_SIZE {
            Ok(Some(ReportedPullRequests {
                repository_url: self.repository_url.clone(),
                query: self.query,
                pages_read: self.calls,
                terminal_page_rows: received,
                pull_requests: std::mem::take(&mut self.rows),
            }))
        } else {
            if self.calls >= self.limits.calls || self.result_bytes >= self.limits.result_bytes {
                return Err(Refusal::BudgetExceeded);
            }
            call.arguments["page"] = json!(self.calls + 1);
            self.pending = Some(call);
            Ok(None)
        }
    }
}

/// No total is supplied by the pinned list source. Completion here describes a
/// short page, not an independently proven exhaustive or still-current set.
#[derive(Debug, serde::Serialize)]
pub struct ReportedPullRequests {
    pub repository_url: String,
    pub query: PullRequestQuery,
    pub pages_read: u16,
    pub terminal_page_rows: usize,
    pub pull_requests: Vec<PullRequestSummary>,
}
#[derive(Debug, Deserialize, serde::Serialize)]
pub struct PullRequestSummary {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub html_url: String,
    pub head: PullRequestBranch,
    pub base: PullRequestBranch,
    /// Includes dates, labels and provider-converted flags. In particular,
    /// upstream absent merged/draft values can become false; no verdict follows.
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}
#[derive(Debug, Deserialize, serde::Serialize)]
pub struct PullRequestBranch {
    #[serde(rename = "ref")]
    pub reference: String,
    pub sha: String,
    /// A fork head may have no repository after deletion. Base must match the
    /// selected repository; head repository metadata is never a routing input.
    pub repo: Option<PullRequestBranchRepository>,
}
#[derive(Debug, Deserialize, serde::Serialize)]
pub struct PullRequestBranchRepository {
    pub full_name: String,
    pub description: Option<String>,
}

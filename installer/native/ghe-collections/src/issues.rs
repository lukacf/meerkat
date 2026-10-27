//! Issue discovery and discussion over the pinned successful textual payload.
//! The native adapter owns source/account provenance: list rows have no URL.
//! Nested labels/relationships/fields are partial provider data, never grants.

use super::{PullRequestComment, ReadCall, Refusal, decode, enterprise_host, segment};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

mod hierarchy;
pub use hierarchy::{IssueHierarchy, IssueParentReference, ReportedIssueHierarchy};

const PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueQuery {
    All,
    Open,
    /// Provider-side lower bound, with no upper-bound or snapshot guarantee.
    /// Use a separate Open query to include older outstanding issues.
    UpdatedSince(String),
}

#[derive(Debug, Clone, Copy)]
pub struct IssueReadLimits {
    calls: u16,
    result_bytes: usize,
}
impl IssueReadLimits {
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
    limits: IssueReadLimits,
    calls: u16,
    bytes: usize,
}
impl Budget {
    fn new(limits: IssueReadLimits) -> Self {
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
fn repository(host: &str, owner: &str, repo: &str) -> Result<String, Refusal> {
    if !enterprise_host(host) || !segment(owner) || !segment(repo) {
        return Err(Refusal::InvalidTarget);
    }
    Ok(format!("https://{host}/{owner}/{repo}"))
}
fn matching(call: &ReadCall, operation: &str, arguments: &Value) -> Result<(), Refusal> {
    if call.operation() != operation || call.arguments() != arguments {
        Err(Refusal::RequestChanged)
    } else {
        Ok(())
    }
}
fn cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

#[derive(Debug)]
pub struct RepositoryIssues {
    repository_url: String,
    query: IssueQuery,
    pending: Option<ReadCall>,
    budget: Budget,
    total: Option<u64>,
    numbers: BTreeSet<u64>,
    cursors: BTreeSet<String>,
    issues: Vec<IssueRecord>,
}
impl RepositoryIssues {
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        query: IssueQuery,
        limits: IssueReadLimits,
    ) -> Result<Self, Refusal> {
        let mut arguments = json!({"owner":owner,"repo":repo,"orderBy":"CREATED_AT","direction":"ASC","perPage":PAGE_SIZE});
        match &query {
            IssueQuery::All => {}
            IssueQuery::Open => {
                arguments["state"] = json!("OPEN");
            }
            IssueQuery::UpdatedSince(since) => {
                if super::history::utc_seconds(since).is_none() {
                    return Err(Refusal::InvalidTarget);
                }
                arguments["since"] = json!(since);
            }
        }
        Ok(Self {
            repository_url: repository(host, owner, repo)?,
            query,
            pending: Some(ReadCall {
                operation: "list_issues",
                arguments,
            }),
            budget: Budget::new(limits),
            total: None,
            numbers: BTreeSet::new(),
            cursors: BTreeSet::new(),
            issues: Vec::new(),
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
    ) -> Result<Option<ReportedIssues>, Refusal> {
        let mut call = self.pending.take().ok_or(Refusal::AlreadyFinished)?;
        matching(&call, operation, arguments)?;
        self.budget.consume(payload)?;
        let page: IssuesPage = decode(payload)?;
        if self.total.is_some_and(|total| total != page.total_count) {
            return Err(Refusal::CollectionChanged);
        }
        self.total = Some(page.total_count);
        if page.total_count > u64::from(self.budget.limits.calls) * PAGE_SIZE as u64 {
            return Err(Refusal::BudgetExceeded);
        }
        let received = page.issues.len();
        let observed = self.issues.len() as u64;
        if received > PAGE_SIZE || observed + received as u64 > page.total_count {
            return Err(Refusal::IncompleteCollection);
        }
        for issue in &page.issues {
            issue.check(&self.repository_url, false)?;
            if self.query == IssueQuery::Open && issue.state != "OPEN" {
                return Err(Refusal::CollectionChanged);
            }
            if !self.numbers.insert(issue.number) {
                return Err(Refusal::DuplicateItem);
            }
        }
        self.issues.extend(page.issues);
        if page.page_info.has_next_page {
            if received == 0 || self.issues.len() as u64 >= page.total_count {
                return Err(Refusal::IncompleteCollection);
            }
            let next = page.page_info.end_cursor.ok_or(Refusal::UnsupportedShape)?;
            if !cursor(&next) {
                return Err(Refusal::UnsupportedShape);
            }
            if !self.cursors.insert(next.clone()) {
                return Err(Refusal::CursorRepeated);
            }
            self.budget.next()?;
            call.arguments["after"] = json!(next);
            self.pending = Some(call);
            Ok(None)
        } else {
            if self.issues.len() as u64 != page.total_count {
                return Err(Refusal::IncompleteCollection);
            }
            Ok(Some(ReportedIssues {
                repository_url: self.repository_url.clone(),
                query: self.query.clone(),
                reported_total: page.total_count,
                pages_read: self.budget.calls,
                issues: std::mem::take(&mut self.issues),
            }))
        }
    }
}

#[derive(Debug)]
enum Phase {
    Initial(ReadCall),
    Comments { call: ReadCall, page: u16 },
    Final(ReadCall),
    Finished,
}
#[derive(Debug)]
pub struct IssueComments {
    repository_url: String,
    issue_url: String,
    number: u64,
    initial_call: ReadCall,
    initial: Option<IssueRecord>,
    phase: Phase,
    budget: Budget,
    ids: BTreeSet<u64>,
    comments: Vec<PullRequestComment>,
    pages_read: u16,
    terminal_page_rows: usize,
}
impl IssueComments {
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        number: u64,
        limits: IssueReadLimits,
    ) -> Result<Self, Refusal> {
        if number == 0 || number > i32::MAX as u64 || limits.calls < 3 {
            return Err(Refusal::InvalidTarget);
        }
        let repository_url = repository(host, owner, repo)?;
        let call = ReadCall {
            operation: "issue_read",
            arguments: json!({"method":"get","owner":owner,"repo":repo,"issue_number":number}),
        };
        Ok(Self {
            issue_url: format!("{repository_url}/issues/{number}"),
            repository_url,
            number,
            initial_call: call.clone(),
            initial: None,
            phase: Phase::Initial(call),
            budget: Budget::new(limits),
            ids: BTreeSet::new(),
            comments: Vec::new(),
            pages_read: 0,
            terminal_page_rows: 0,
        })
    }
    pub fn next_call(&self) -> Option<&ReadCall> {
        match &self.phase {
            Phase::Initial(call) | Phase::Final(call) | Phase::Comments { call, .. } => Some(call),
            Phase::Finished => None,
        }
    }
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedIssueComments>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        let call = match &phase {
            Phase::Initial(call) | Phase::Final(call) | Phase::Comments { call, .. } => call,
            Phase::Finished => return Err(Refusal::AlreadyFinished),
        };
        matching(call, operation, arguments)?;
        self.budget.consume(payload)?;
        match phase {
            Phase::Initial(mut call) => {
                let issue: IssueRecord = decode(payload)?;
                self.check_issue(&issue)?;
                // Reserve initial/final details and a short terminal page, even
                // when the explicit count is an exact multiple of PAGE_SIZE.
                if issue.comments.is_some_and(|total| {
                    total / PAGE_SIZE as u64 + 1 > u64::from(self.budget.limits.calls - 2)
                }) {
                    return Err(Refusal::BudgetExceeded);
                }
                self.initial = Some(issue);
                self.budget.next()?;
                call.arguments["method"] = json!("get_comments");
                call.arguments["page"] = json!(1);
                call.arguments["perPage"] = json!(PAGE_SIZE);
                self.phase = Phase::Comments { call, page: 1 };
                Ok(None)
            }
            Phase::Comments { mut call, page } => {
                let rows: Vec<PullRequestComment> = decode(payload)?;
                if rows.len() > PAGE_SIZE {
                    return Err(Refusal::UnsupportedShape);
                }
                for row in &rows {
                    if row.id == 0 || row.id > i64::MAX as u64 {
                        return Err(Refusal::UnsupportedShape);
                    }
                    if row.html_url != format!("{}#issuecomment-{}", self.issue_url, row.id) {
                        return Err(Refusal::WrongResource);
                    }
                    if !self.ids.insert(row.id) {
                        return Err(Refusal::DuplicateItem);
                    }
                }
                let count = rows.len();
                self.comments.extend(rows);
                let initial = self.initial.as_ref().ok_or(Refusal::UnsupportedShape)?;
                if initial.comments.is_some_and(|total| {
                    self.comments.len() as u64 > total
                        || (count < PAGE_SIZE && self.comments.len() as u64 != total)
                }) {
                    return Err(Refusal::IncompleteCollection);
                }
                self.budget.next()?;
                if count < PAGE_SIZE {
                    self.pages_read = page;
                    self.terminal_page_rows = count;
                    self.phase = Phase::Final(self.initial_call.clone());
                } else {
                    if self.budget.calls + 1 >= self.budget.limits.calls {
                        return Err(Refusal::BudgetExceeded);
                    }
                    call.arguments["page"] = json!(page + 1);
                    self.phase = Phase::Comments {
                        call,
                        page: page + 1,
                    };
                }
                Ok(None)
            }
            Phase::Final(_) => {
                let issue: IssueRecord = decode(payload)?;
                self.check_issue(&issue)?;
                let initial = self.initial.take().ok_or(Refusal::UnsupportedShape)?;
                if initial.updated_at != issue.updated_at
                    || initial.comments != issue.comments
                    || initial.state != issue.state
                {
                    return Err(Refusal::CollectionChanged);
                }
                Ok(Some(ReportedIssueComments {
                    initial_issue: initial,
                    final_issue: issue,
                    pages_read: self.pages_read,
                    terminal_page_rows: self.terminal_page_rows,
                    comments: std::mem::take(&mut self.comments),
                }))
            }
            Phase::Finished => Err(Refusal::AlreadyFinished),
        }
    }
    fn check_issue(&self, issue: &IssueRecord) -> Result<(), Refusal> {
        issue.check(&self.repository_url, true)?;
        if issue.number != self.number {
            return Err(Refusal::WrongResource);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct IssuesPage {
    issues: Vec<IssueRecord>,
    #[serde(rename = "totalCount")]
    total_count: u64,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}
#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

/// Discovery results are bound to a query, not independently to a repository
/// returned by each row. The pinned GraphQL list omits html_url/database ID.
#[derive(Debug)]
pub struct ReportedIssues {
    pub repository_url: String,
    pub query: IssueQuery,
    pub reported_total: u64,
    pub pages_read: u16,
    pub issues: Vec<IssueRecord>,
}
/// A short-page observation between matching identity/state/count/update reads.
/// Native unfiltered-mode qualification remains required for full coverage.
#[derive(Debug)]
pub struct ReportedIssueComments {
    pub initial_issue: IssueRecord,
    pub final_issue: IssueRecord,
    pub pages_read: u16,
    pub terminal_page_rows: usize,
    pub comments: Vec<PullRequestComment>,
}

#[derive(Debug, Deserialize)]
pub struct IssueRecord {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub state: String,
    pub html_url: Option<String>,
    pub comments: Option<u64>,
    pub updated_at: Option<String>,
    /// Includes optional enrichment and capped relationship/label/field data.
    /// Missing data stays missing; references never schedule another read.
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}
impl IssueRecord {
    fn check(&self, repo: &str, require_url: bool) -> Result<(), Refusal> {
        if self.number == 0
            || self.number > i32::MAX as u64
            || self.state.is_empty()
            || self.state.len() > 128
            || self.state.chars().any(char::is_control)
        {
            return Err(Refusal::UnsupportedShape);
        }
        if require_url && self.html_url.is_none() {
            return Err(Refusal::UnsupportedShape);
        }
        if self
            .html_url
            .as_ref()
            .is_some_and(|url| url != &format!("{repo}/issues/{}", self.number))
        {
            return Err(Refusal::WrongResource);
        }
        Ok(())
    }
}

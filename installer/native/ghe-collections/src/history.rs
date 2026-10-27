//! Bounded repository history with an exact starting commit and fixed UTC
//! window. The provider performs date filtering. This program never assumes
//! commit dates are monotonic or reconstructs ancestry from missing parents.

use super::{
    CommitAuthor, DiscussionUser, ReadCall, Refusal, commit_id, decode, enterprise_host, segment,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const PAGE_SIZE: usize = 100;

#[derive(Debug, Clone)]
pub struct HistoryWindow {
    since: String,
    until: String,
}

impl HistoryWindow {
    /// An explicit interval of at most seven days. The native requirement owns
    /// the chosen endpoints; this library never reads the clock or widens them.
    pub fn new(since: &str, until: &str) -> Result<Self, Refusal> {
        let start = utc_seconds(since).ok_or(Refusal::InvalidTarget)?;
        let end = utc_seconds(until).ok_or(Refusal::InvalidTarget)?;
        if end <= start || end - start > 7 * 24 * 60 * 60 {
            return Err(Refusal::InvalidTarget);
        }
        Ok(Self {
            since: since.to_owned(),
            until: until.to_owned(),
        })
    }
    pub fn since(&self) -> &str {
        &self.since
    }
    pub fn until(&self) -> &str {
        &self.until
    }
}

// Canonical whole-second UTC only. Gregorian validation prevents impossible
// dates from becoming silently normalized query bounds; offsets are not guessed.
fn utc_seconds(value: &str) -> Option<u64> {
    let b = value.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let number = |start: usize, end: usize| -> Option<u64> {
        b[start..end].iter().try_fold(0u64, |n, c| {
            c.is_ascii_digit().then(|| n * 10 + u64::from(*c - b'0'))
        })
    };
    let (year, month, day, hour, minute, second) = (
        number(0, 4)?,
        number(5, 7)?,
        number(8, 10)?,
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
    );
    if year < 1970 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let mut months = [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if leap {
        months[1] = 29;
    }
    if day == 0 || day > months[month as usize - 1] {
        return None;
    }
    let prior = year - 1;
    let days = 365 * prior + prior / 4 - prior / 100
        + prior / 400
        + months[..month as usize - 1].iter().sum::<u64>()
        + day
        - 1;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

#[derive(Debug, Clone, Copy)]
pub struct HistoryReadLimits {
    calls: u16,
    result_bytes: usize,
}
impl HistoryReadLimits {
    pub fn new(calls: u16, result_bytes: usize) -> Result<Self, Refusal> {
        if !(2..=256).contains(&calls) || !(1..=32 * 1024 * 1024).contains(&result_bytes) {
            return Err(Refusal::InvalidTarget);
        }
        Ok(Self {
            calls,
            result_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryAnchor {
    /// First result of an unfiltered one-row default-branch list read. The
    /// branch may move afterward; all subsequent calls use the returned SHA.
    DefaultBranchObservation,
    /// Exact commit supplied by the retained native requirement and checked by
    /// get_commit before the date-filtered list is requested.
    SelectedCommit,
}

#[derive(Debug)]
enum Phase {
    Anchor(ReadCall),
    Page { call: ReadCall, page: u16 },
    Finished,
}

#[derive(Debug)]
pub struct RepositoryHistory {
    repository_url: String,
    phase: Phase,
    anchor_kind: HistoryAnchor,
    expected_head: Option<String>,
    anchor: Option<RepositoryCommit>,
    window: HistoryWindow,
    limits: HistoryReadLimits,
    calls: u16,
    result_bytes: usize,
    ids: BTreeSet<String>,
    commits: Vec<RepositoryCommit>,
}

impl RepositoryHistory {
    pub fn default_branch(
        host: &str,
        owner: &str,
        repo: &str,
        window: HistoryWindow,
        limits: HistoryReadLimits,
    ) -> Result<Self, Refusal> {
        Self::build(host, owner, repo, None, window, limits)
    }
    pub fn at_head(
        host: &str,
        owner: &str,
        repo: &str,
        head: &str,
        window: HistoryWindow,
        limits: HistoryReadLimits,
    ) -> Result<Self, Refusal> {
        if !commit_id(head) {
            return Err(Refusal::InvalidTarget);
        }
        Self::build(host, owner, repo, Some(head.to_owned()), window, limits)
    }
    fn build(
        host: &str,
        owner: &str,
        repo: &str,
        head: Option<String>,
        window: HistoryWindow,
        limits: HistoryReadLimits,
    ) -> Result<Self, Refusal> {
        if !enterprise_host(host) || !segment(owner) || !segment(repo) {
            return Err(Refusal::InvalidTarget);
        }
        let (kind, call) = if let Some(head) = &head {
            (
                HistoryAnchor::SelectedCommit,
                ReadCall {
                    operation: "get_commit",
                    arguments: json!({"owner":owner,"repo":repo,"sha":head,"detail":"none"}),
                },
            )
        } else {
            (
                HistoryAnchor::DefaultBranchObservation,
                ReadCall {
                    operation: "list_commits",
                    arguments: json!({"owner":owner,"repo":repo,"page":1,"perPage":1}),
                },
            )
        };
        Ok(Self {
            repository_url: format!("https://{host}/{owner}/{repo}"),
            phase: Phase::Anchor(call),
            anchor_kind: kind,
            expected_head: head,
            anchor: None,
            window,
            limits,
            calls: 0,
            result_bytes: 0,
            ids: BTreeSet::new(),
            commits: Vec::new(),
        })
    }
    pub fn next_call(&self) -> Option<&ReadCall> {
        match &self.phase {
            Phase::Anchor(call) | Phase::Page { call, .. } => Some(call),
            Phase::Finished => None,
        }
    }

    /// Matching committed native results only, after tool-error handling and
    /// native settlement/join. Pure JSON never establishes those conditions.
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedCommitHistory>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        let call = match &phase {
            Phase::Anchor(call) | Phase::Page { call, .. } => call,
            Phase::Finished => return Err(Refusal::AlreadyFinished),
        };
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
        match phase {
            Phase::Anchor(call) => {
                let anchor: RepositoryCommit = match self.anchor_kind {
                    HistoryAnchor::SelectedCommit => decode(payload)?,
                    HistoryAnchor::DefaultBranchObservation => {
                        let mut rows: Vec<RepositoryCommit> = decode(payload)?;
                        if rows.len() != 1 {
                            return Err(Refusal::UnsupportedShape);
                        }
                        rows.remove(0)
                    }
                };
                check_commit(&self.repository_url, &anchor)?;
                if self
                    .expected_head
                    .as_ref()
                    .is_some_and(|sha| sha != &anchor.sha)
                {
                    return Err(Refusal::ChangedHead);
                }
                self.require_next()?;
                let arguments = json!({"owner":call.arguments["owner"],"repo":call.arguments["repo"],"sha":anchor.sha,
                    "since":self.window.since,"until":self.window.until,"page":1,"perPage":PAGE_SIZE});
                self.anchor = Some(anchor);
                self.phase = Phase::Page {
                    call: ReadCall {
                        operation: "list_commits",
                        arguments,
                    },
                    page: 1,
                };
                Ok(None)
            }
            Phase::Page { mut call, page } => {
                let commits: Vec<RepositoryCommit> = decode(payload)?;
                if commits.len() > PAGE_SIZE {
                    return Err(Refusal::UnsupportedShape);
                }
                for commit in &commits {
                    check_commit(&self.repository_url, commit)?;
                    if !self.ids.insert(commit.sha.clone()) {
                        return Err(Refusal::DuplicateItem);
                    }
                }
                let count = commits.len();
                self.commits.extend(commits);
                if count < PAGE_SIZE {
                    Ok(Some(ReportedCommitHistory {
                        anchor_kind: self.anchor_kind,
                        anchor: self.anchor.take().ok_or(Refusal::UnsupportedShape)?,
                        window: self.window.clone(),
                        pages_read: page,
                        terminal_page_rows: count,
                        commits: std::mem::take(&mut self.commits),
                    }))
                } else {
                    self.require_next()?;
                    call.arguments["page"] = json!(page + 1);
                    self.phase = Phase::Page {
                        call,
                        page: page + 1,
                    };
                    Ok(None)
                }
            }
            Phase::Finished => Err(Refusal::AlreadyFinished),
        }
    }
    fn require_next(&self) -> Result<(), Refusal> {
        if self.calls >= self.limits.calls || self.result_bytes >= self.limits.result_bytes {
            Err(Refusal::BudgetExceeded)
        } else {
            Ok(())
        }
    }
}

fn check_commit(repository_url: &str, commit: &RepositoryCommit) -> Result<(), Refusal> {
    if !commit_id(&commit.sha) {
        return Err(Refusal::UnsupportedShape);
    }
    if commit.html_url != format!("{repository_url}/commit/{}", commit.sha) {
        return Err(Refusal::WrongResource);
    }
    Ok(())
}

/// Observed source pages rooted at the retained commit and queried window.
/// This is not a claim that the default branch still points there, an ancestry
/// proof, file/patch coverage, native readiness or credential usability.
#[derive(Debug)]
pub struct ReportedCommitHistory {
    pub anchor_kind: HistoryAnchor,
    pub anchor: RepositoryCommit,
    pub window: HistoryWindow,
    pub pages_read: u16,
    pub terminal_page_rows: usize,
    pub commits: Vec<RepositoryCommit>,
}

#[derive(Debug, Deserialize)]
pub struct RepositoryCommit {
    pub sha: String,
    pub html_url: String,
    pub commit: Option<RepositoryCommitInfo>,
    pub author: Option<DiscussionUser>,
    pub committer: Option<DiscussionUser>,
}

#[derive(Debug, Deserialize)]
pub struct RepositoryCommitInfo {
    pub message: String,
    pub author: Option<CommitAuthor>,
    pub committer: Option<CommitAuthor>,
}

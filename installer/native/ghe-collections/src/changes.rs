//! Count-checked PR file and commit pages. The pinned source does not attach
//! revisions to file pages. Matching initial/final PR reads detect some drift,
//! but cannot establish an atomic snapshot or rule out an intervening change.

use super::{PullRequest, PullRequestScope, ReadCall, Refusal, commit_id, decode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const PAGE_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy)]
pub struct ChangesReadLimits {
    calls: u16,
    result_bytes: usize,
}

impl ChangesReadLimits {
    /// Bounds include the initial/final PR reads and both collections. Native
    /// execution limits and authorization remain independent of this program.
    pub fn new(calls: u16, result_bytes: usize) -> Result<Self, Refusal> {
        if !(4..=256).contains(&calls) || !(1..=32 * 1024 * 1024).contains(&result_bytes) {
            return Err(Refusal::InvalidTarget);
        }
        Ok(Self {
            calls,
            result_bytes,
        })
    }
}

#[derive(Debug)]
enum Phase {
    Initial(ReadCall),
    Files { call: ReadCall, page: u16 },
    Commits { call: ReadCall, page: u16 },
    Verify(ReadCall),
    Finished,
}

#[derive(Debug)]
struct Snapshot {
    head: String,
    files: u64,
    commits: u64,
}

#[derive(Debug)]
pub struct PullRequestChanges {
    scope: PullRequestScope,
    initial_call: ReadCall,
    phase: Phase,
    limits: ChangesReadLimits,
    calls: u16,
    result_bytes: usize,
    expected_head: Option<String>,
    snapshot: Option<Snapshot>,
    filenames: BTreeSet<String>,
    commit_ids: BTreeSet<String>,
    files: Vec<PullRequestFile>,
    commits: Vec<PullRequestCommit>,
}

impl PullRequestChanges {
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        pull_number: u64,
        limits: ChangesReadLimits,
    ) -> Result<Self, Refusal> {
        let scope = PullRequestScope::new(host, owner, repo, pull_number)?;
        let call = ReadCall {
            arguments: json!({"method":"get","owner":owner,"repo":repo,"pullNumber":pull_number}),
        };
        Ok(Self {
            scope,
            initial_call: call.clone(),
            phase: Phase::Initial(call),
            limits,
            calls: 0,
            result_bytes: 0,
            expected_head: None,
            snapshot: None,
            filenames: BTreeSet::new(),
            commit_ids: BTreeSet::new(),
            files: Vec::new(),
            commits: Vec::new(),
        })
    }

    /// Preserve a revision already selected by the native requirement. This
    /// does not grant revision-specific semantics to the provider's file pages.
    pub fn at_head(
        host: &str,
        owner: &str,
        repo: &str,
        pull_number: u64,
        head: &str,
        limits: ChangesReadLimits,
    ) -> Result<Self, Refusal> {
        if !commit_id(head) {
            return Err(Refusal::InvalidTarget);
        }
        let mut read = Self::new(host, owner, repo, pull_number, limits)?;
        read.expected_head = Some(head.to_owned());
        Ok(read)
    }

    pub fn next_call(&self) -> Option<&ReadCall> {
        match &self.phase {
            Phase::Initial(call) | Phase::Verify(call) => Some(call),
            Phase::Files { call, .. } | Phase::Commits { call, .. } => Some(call),
            Phase::Finished => None,
        }
    }

    /// The native adapter must first reject tool errors and establish that the
    /// payload belongs to this pending call's committed, settled native result.
    /// Failure or completion seals the cursor. Partial data are never returned.
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedPullRequestChanges>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        let call = match &phase {
            Phase::Initial(call) | Phase::Verify(call) => call,
            Phase::Files { call, .. } | Phase::Commits { call, .. } => call,
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
            Phase::Initial(_) => {
                let pr: CountedPullRequest = decode(payload)?;
                self.scope.check(&pr.pr)?;
                if self
                    .expected_head
                    .as_ref()
                    .is_some_and(|head| head != &pr.pr.head.sha)
                {
                    return Err(Refusal::ChangedHead);
                }
                // Reserve room for both collection reads, including explicit
                // empty pages, and the final PR read before scheduling work.
                let pages = pr.changed_files.div_ceil(PAGE_SIZE).max(1)
                    + pr.commits.div_ceil(PAGE_SIZE).max(1);
                if pages > u64::from(self.limits.calls - 2) {
                    return Err(Refusal::BudgetExceeded);
                }
                self.snapshot = Some(Snapshot {
                    head: pr.pr.head.sha,
                    files: pr.changed_files,
                    commits: pr.commits,
                });
                self.require_next()?;
                self.phase = Phase::Files {
                    call: self.page_call("get_files", 1),
                    page: 1,
                };
                Ok(None)
            }
            Phase::Files { page, .. } => {
                let files: Vec<PullRequestFile> = decode(payload)?;
                let expected = self
                    .snapshot
                    .as_ref()
                    .ok_or(Refusal::UnsupportedShape)?
                    .files;
                let done = check_page(self.files.len(), files.len(), expected)?;
                for file in &files {
                    // Filenames and patches are untrusted data, never local
                    // paths to open or instructions to follow.
                    if file.filename.is_empty() || file.filename.len() > 4096 {
                        return Err(Refusal::UnsupportedShape);
                    }
                    if !self.filenames.insert(file.filename.clone()) {
                        return Err(Refusal::DuplicateItem);
                    }
                }
                self.files.extend(files);
                self.require_next()?;
                self.phase = if done {
                    Phase::Commits {
                        call: self.page_call("get_commits", 1),
                        page: 1,
                    }
                } else {
                    Phase::Files {
                        call: self.page_call("get_files", page + 1),
                        page: page + 1,
                    }
                };
                Ok(None)
            }
            Phase::Commits { page, .. } => {
                let commits: Vec<PullRequestCommit> = decode(payload)?;
                let snapshot = self.snapshot.as_ref().ok_or(Refusal::UnsupportedShape)?;
                let done = check_page(self.commits.len(), commits.len(), snapshot.commits)?;
                let repo_url = self
                    .scope
                    .pull_url
                    .rsplit_once("/pull/")
                    .ok_or(Refusal::InvalidTarget)?
                    .0;
                for commit in &commits {
                    if !commit_id(&commit.sha) {
                        return Err(Refusal::UnsupportedShape);
                    }
                    if commit
                        .html_url
                        .as_ref()
                        .is_some_and(|url| url != &format!("{repo_url}/commit/{}", commit.sha))
                    {
                        return Err(Refusal::WrongResource);
                    }
                    if !self.commit_ids.insert(commit.sha.clone()) {
                        return Err(Refusal::DuplicateItem);
                    }
                }
                self.commits.extend(commits);
                if done && !self.commit_ids.contains(&snapshot.head) {
                    return Err(Refusal::IncompleteCollection);
                }
                self.require_next()?;
                self.phase = if done {
                    Phase::Verify(self.initial_call.clone())
                } else {
                    Phase::Commits {
                        call: self.page_call("get_commits", page + 1),
                        page: page + 1,
                    }
                };
                Ok(None)
            }
            Phase::Verify(_) => {
                let pr: CountedPullRequest = decode(payload)?;
                self.scope.check(&pr.pr)?;
                let snapshot = self.snapshot.as_ref().ok_or(Refusal::UnsupportedShape)?;
                if pr.pr.head.sha != snapshot.head {
                    return Err(Refusal::ChangedHead);
                }
                if pr.changed_files != snapshot.files || pr.commits != snapshot.commits {
                    return Err(Refusal::CollectionChanged);
                }
                Ok(Some(ReportedPullRequestChanges {
                    observed_pr_head: snapshot.head.clone(),
                    reported_file_count: snapshot.files,
                    reported_commit_count: snapshot.commits,
                    calls_read: self.calls,
                    files: std::mem::take(&mut self.files),
                    commits: std::mem::take(&mut self.commits),
                }))
            }
            Phase::Finished => Err(Refusal::AlreadyFinished),
        }
    }

    fn page_call(&self, method: &str, page: u16) -> ReadCall {
        let mut call = self.initial_call.clone();
        call.arguments["method"] = json!(method);
        call.arguments["page"] = json!(page);
        call.arguments["perPage"] = json!(PAGE_SIZE);
        call
    }

    fn require_next(&self) -> Result<(), Refusal> {
        if self.calls >= self.limits.calls || self.result_bytes >= self.limits.result_bytes {
            Err(Refusal::BudgetExceeded)
        } else {
            Ok(())
        }
    }
}

fn check_page(observed: usize, received: usize, expected: u64) -> Result<bool, Refusal> {
    if received > PAGE_SIZE as usize || observed as u64 > expected {
        return Err(Refusal::IncompleteCollection);
    }
    let remaining = expected - observed as u64;
    if received as u64 != remaining.min(PAGE_SIZE) {
        return Err(Refusal::IncompleteCollection);
    }
    Ok(received as u64 == remaining)
}

#[derive(Deserialize)]
struct CountedPullRequest {
    #[serde(flatten)]
    pr: PullRequest,
    // The pinned source omits zero counts and also discards upstream absence.
    // Missing is therefore not sufficient evidence for a complete empty set.
    changed_files: u64,
    commits: u64,
}

/// Count-matching provider pages with matching initial/final PR reads. This is
/// not an atomic snapshot, full diff, native receipt or readiness assertion.
#[derive(Debug)]
pub struct ReportedPullRequestChanges {
    pub observed_pr_head: String,
    pub reported_file_count: u64,
    pub reported_commit_count: u64,
    pub calls_read: u16,
    pub files: Vec<PullRequestFile>,
    pub commits: Vec<PullRequestCommit>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestFile {
    pub filename: String,
    pub status: Option<String>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub changes: Option<u64>,
    pub patch: Option<String>,
    pub previous_filename: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestCommit {
    pub sha: String,
    pub html_url: Option<String>,
    pub message: Option<String>,
    pub author: Option<CommitAuthor>,
}

#[derive(Debug, Deserialize)]
pub struct CommitAuthor {
    pub name: Option<String>,
    pub email: Option<String>,
    pub date: Option<String>,
}

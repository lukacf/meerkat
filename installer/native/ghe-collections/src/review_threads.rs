//! Cursor paging of provider-reported PR review threads. These results do not
//! prove unfiltered discussion coverage: the pinned server's lockdown mode can
//! remove comments and replace their counts. The native adapter must separately
//! check that behavior on the actual installed invocation before qualifying it.

use super::{PullRequest, PullRequestScope, ReadCall, Refusal, decode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, Copy)]
pub struct ReviewReadLimits {
    calls: u16,
    result_bytes: usize,
}

impl ReviewReadLimits {
    /// Bounds include the initial PR read. Native execution has its own
    /// independent limits; this object supplies no permission to make calls.
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

#[derive(Debug)]
enum Phase {
    PullRequest(ReadCall),
    Threads(ReadCall),
    Finished,
}

#[derive(Debug)]
pub struct PullRequestReviewThreads {
    scope: PullRequestScope,
    phase: Phase,
    limits: ReviewReadLimits,
    calls: u16,
    result_bytes: usize,
    total: Option<u64>,
    thread_ids: BTreeSet<String>,
    comment_urls: BTreeSet<String>,
    cursors: BTreeSet<String>,
    threads: Vec<ReviewThread>,
}

impl PullRequestReviewThreads {
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        pull_number: u64,
        limits: ReviewReadLimits,
    ) -> Result<Self, Refusal> {
        // The pinned implementation casts this value to GraphQL's signed Int.
        if pull_number > i32::MAX as u64 {
            return Err(Refusal::InvalidTarget);
        }
        Ok(Self {
            scope: PullRequestScope::new(host, owner, repo, pull_number)?,
            phase: Phase::PullRequest(ReadCall {
                operation: "pull_request_read",
                arguments: json!({"method":"get","owner":owner,"repo":repo,"pullNumber":pull_number}),
            }),
            limits,
            calls: 0,
            result_bytes: 0,
            total: None,
            thread_ids: BTreeSet::new(),
            comment_urls: BTreeSet::new(),
            cursors: BTreeSet::new(),
            threads: Vec::new(),
        })
    }

    pub fn next_call(&self) -> Option<&ReadCall> {
        match &self.phase {
            Phase::PullRequest(call) | Phase::Threads(call) => Some(call),
            Phase::Finished => None,
        }
    }

    /// Consume only the exact pending call's provider JSON. Errors end this
    /// cursor permanently. The native adapter must reject tool errors and
    /// retain the original native observation, login and settlement separately.
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedReviewThreads>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        let call = match &phase {
            Phase::PullRequest(call) | Phase::Threads(call) => call,
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
            Phase::PullRequest(mut call) => {
                self.scope.check(&decode::<PullRequest>(payload)?)?;
                if self.result_bytes >= self.limits.result_bytes {
                    return Err(Refusal::BudgetExceeded);
                }
                call.arguments["method"] = json!("get_review_comments");
                call.arguments["perPage"] = json!(PAGE_SIZE);
                self.phase = Phase::Threads(call);
                Ok(None)
            }
            Phase::Threads(mut call) => {
                let page: ReviewPage = decode(payload)?;
                if page.review_threads.len() > PAGE_SIZE {
                    return Err(Refusal::UnsupportedShape);
                }
                if self.total.is_some_and(|total| total != page.total_count) {
                    return Err(Refusal::CollectionChanged);
                }
                self.total = Some(page.total_count);
                if page.total_count > u64::from(self.limits.calls - 1) * PAGE_SIZE as u64 {
                    return Err(Refusal::BudgetExceeded);
                }
                for thread in &page.review_threads {
                    if !opaque(&thread.id) {
                        return Err(Refusal::UnsupportedShape);
                    }
                    if !self.thread_ids.insert(thread.id.clone()) {
                        return Err(Refusal::DuplicateItem);
                    }
                    if thread.comments.len() > PAGE_SIZE
                        || thread.total_count != thread.comments.len() as u64
                    {
                        return Err(Refusal::IncompleteThread);
                    }
                    for comment in &thread.comments {
                        let prefix = format!("{}#discussion_r", self.scope.pull_url);
                        let id = comment.html_url.strip_prefix(&prefix).unwrap_or("");
                        if id.is_empty()
                            || id.starts_with('0')
                            || !id.bytes().all(|c| c.is_ascii_digit())
                        {
                            return Err(Refusal::WrongResource);
                        }
                        if !self.comment_urls.insert(comment.html_url.clone()) {
                            return Err(Refusal::DuplicateItem);
                        }
                    }
                }
                self.threads.extend(page.review_threads);
                let observed = self.threads.len() as u64;
                if observed > page.total_count {
                    return Err(Refusal::IncompleteCollection);
                }
                if page.page_info.has_next_page {
                    if observed == page.total_count {
                        return Err(Refusal::IncompleteCollection);
                    }
                    let cursor = page
                        .page_info
                        .end_cursor
                        .filter(|cursor| opaque(cursor))
                        .ok_or(Refusal::UnsupportedShape)?;
                    if !self.cursors.insert(cursor.clone()) {
                        return Err(Refusal::CursorRepeated);
                    }
                    if self.calls >= self.limits.calls
                        || self.result_bytes >= self.limits.result_bytes
                    {
                        return Err(Refusal::BudgetExceeded);
                    }
                    call.arguments["after"] = json!(cursor);
                    self.phase = Phase::Threads(call);
                    Ok(None)
                } else {
                    if observed != page.total_count {
                        return Err(Refusal::IncompleteCollection);
                    }
                    Ok(Some(ReportedReviewThreads {
                        reported_total: page.total_count,
                        pages_read: self.calls - 1,
                        threads: std::mem::take(&mut self.threads),
                    }))
                }
            }
            Phase::Finished => Err(Refusal::AlreadyFinished),
        }
    }
}

fn opaque(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

/// A completed set of provider-reported pages, not an unfiltered conversation
/// or atomic revision snapshot. Lockdown filtering can rewrite nested counts.
#[derive(Debug, serde::Serialize)]
pub struct ReportedReviewThreads {
    pub reported_total: u64,
    pub pages_read: u16,
    pub threads: Vec<ReviewThread>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct ReviewThread {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub is_collapsed: bool,
    pub comments: Vec<ReviewComment>,
    pub total_count: u64,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct ReviewComment {
    pub body: Option<String>,
    pub path: String,
    pub line: Option<i64>,
    pub original_line: Option<i64>,
    pub start_line: Option<i64>,
    pub original_start_line: Option<i64>,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub html_url: String,
}

#[derive(Deserialize)]
struct ReviewPage {
    review_threads: Vec<ReviewThread>,
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

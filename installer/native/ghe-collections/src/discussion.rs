//! Provider-reported PR comments and review summaries. Both pinned methods can
//! filter rows after pagination. The native adapter must establish unfiltered
//! installed behavior before treating a short page as complete source coverage.

use super::{PullRequest, PullRequestScope, ReadCall, Refusal, commit_id, decode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

const PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscussionKind {
    Comments,
    Reviews,
}

impl DiscussionKind {
    fn method(self) -> &'static str {
        match self {
            Self::Comments => "get_comments",
            Self::Reviews => "get_reviews",
        }
    }
    fn fragment(self) -> &'static str {
        match self {
            Self::Comments => "issuecomment-",
            Self::Reviews => "pullrequestreview-",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiscussionReadLimits {
    calls: u16,
    result_bytes: usize,
}

impl DiscussionReadLimits {
    /// Bounds include the initial PR read and any empty terminal page. These
    /// limits are not grants or substitutes for native execution limits.
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
    Initial(ReadCall),
    Page { call: ReadCall, page: u16 },
    Finished,
}

#[derive(Debug)]
pub struct PullRequestDiscussion {
    scope: PullRequestScope,
    kind: DiscussionKind,
    phase: Phase,
    limits: DiscussionReadLimits,
    calls: u16,
    result_bytes: usize,
    initial_comment_count: Option<u64>,
    ids: BTreeSet<u64>,
    rows: DiscussionRows,
}

impl PullRequestDiscussion {
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        pull_number: u64,
        kind: DiscussionKind,
        limits: DiscussionReadLimits,
    ) -> Result<Self, Refusal> {
        Ok(Self {
            scope: PullRequestScope::new(host, owner, repo, pull_number)?,
            kind,
            phase: Phase::Initial(ReadCall {
                arguments: json!({"method":"get","owner":owner,"repo":repo,"pullNumber":pull_number}),
            }),
            limits,
            calls: 0,
            result_bytes: 0,
            initial_comment_count: None,
            ids: BTreeSet::new(),
            rows: DiscussionRows::empty(kind),
        })
    }

    pub fn next_call(&self) -> Option<&ReadCall> {
        match &self.phase {
            Phase::Initial(call) | Phase::Page { call, .. } => Some(call),
            Phase::Finished => None,
        }
    }

    /// Consume only matching committed provider JSON after the native adapter
    /// handles tool errors and retains original native authority and settlement.
    /// A returned short page is data, not proof of unfiltered coverage. No
    /// native readiness, credential or approval decision is produced here.
    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedDiscussionPages>, Refusal> {
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        let call = match &phase {
            Phase::Initial(call) | Phase::Page { call, .. } => call,
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
            Phase::Initial(mut call) => {
                let pr: DiscussionPullRequest = decode(payload)?;
                self.scope.check(&pr.pr)?;
                if self.kind == DiscussionKind::Comments {
                    self.initial_comment_count = pr.comments;
                    // Even an exact multiple requires another (empty) page to
                    // observe the unfiltered endpoint's short-page condition.
                    if pr.comments.is_some_and(|count| {
                        count / PAGE_SIZE as u64 + 1 > u64::from(self.limits.calls - 1)
                    }) {
                        return Err(Refusal::BudgetExceeded);
                    }
                }
                self.require_next()?;
                call.arguments["method"] = json!(self.kind.method());
                call.arguments["page"] = json!(1);
                call.arguments["perPage"] = json!(PAGE_SIZE);
                self.phase = Phase::Page { call, page: 1 };
                Ok(None)
            }
            Phase::Page { mut call, page } => {
                let count = match &mut self.rows {
                    DiscussionRows::Comments(rows) => {
                        let incoming: Vec<PullRequestComment> = decode(payload)?;
                        if incoming.len() > PAGE_SIZE {
                            return Err(Refusal::UnsupportedShape);
                        }
                        for row in &incoming {
                            check_identity(
                                &self.scope,
                                self.kind,
                                row.id,
                                &row.html_url,
                                &mut self.ids,
                            )?;
                        }
                        let count = incoming.len();
                        rows.extend(incoming);
                        count
                    }
                    DiscussionRows::Reviews(rows) => {
                        let incoming: Vec<PullRequestReview> = decode(payload)?;
                        if incoming.len() > PAGE_SIZE {
                            return Err(Refusal::UnsupportedShape);
                        }
                        for row in &incoming {
                            check_identity(
                                &self.scope,
                                self.kind,
                                row.id,
                                &row.html_url,
                                &mut self.ids,
                            )?;
                            if row.state.is_empty()
                                || row.state.len() > 128
                                || row.state.chars().any(char::is_control)
                                || row.commit_id.as_deref().is_some_and(|sha| !commit_id(sha))
                            {
                                return Err(Refusal::UnsupportedShape);
                            }
                        }
                        let count = incoming.len();
                        rows.extend(incoming);
                        count
                    }
                };
                let observed = self.ids.len() as u64;
                if self.initial_comment_count.is_some_and(|total| {
                    observed > total || (count < PAGE_SIZE && observed != total)
                }) {
                    return Err(Refusal::IncompleteCollection);
                }
                if count < PAGE_SIZE {
                    Ok(Some(ReportedDiscussionPages {
                        initial_comment_count: self.initial_comment_count,
                        pages_read: page,
                        terminal_page_rows: count,
                        rows: std::mem::replace(&mut self.rows, DiscussionRows::empty(self.kind)),
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

fn check_identity(
    scope: &PullRequestScope,
    kind: DiscussionKind,
    id: u64,
    url: &str,
    ids: &mut BTreeSet<u64>,
) -> Result<(), Refusal> {
    if id == 0 || id > i64::MAX as u64 {
        return Err(Refusal::UnsupportedShape);
    }
    if url != format!("{}#{}{id}", scope.pull_url, kind.fragment()) {
        return Err(Refusal::WrongResource);
    }
    if !ids.insert(id) {
        return Err(Refusal::DuplicateItem);
    }
    Ok(())
}

#[derive(Deserialize)]
struct DiscussionPullRequest {
    #[serde(flatten)]
    pr: PullRequest,
    // Missing is preserved, never interpreted as an explicit zero total.
    comments: Option<u64>,
}

#[derive(Debug)]
pub enum DiscussionRows {
    Comments(Vec<PullRequestComment>),
    Reviews(Vec<PullRequestReview>),
}

impl DiscussionRows {
    fn empty(kind: DiscussionKind) -> Self {
        match kind {
            DiscussionKind::Comments => Self::Comments(Vec::new()),
            DiscussionKind::Reviews => Self::Reviews(Vec::new()),
        }
    }
}

/// A bounded sequence ending in a provider-reported short page. Both methods
/// can filter rows before returning, so this is not complete source coverage
/// without native evidence of the actual installed invocation's filtering mode.
/// It is also not an atomic snapshot or the current effective review decision.
#[derive(Debug)]
pub struct ReportedDiscussionPages {
    pub initial_comment_count: Option<u64>,
    pub pages_read: u16,
    pub terminal_page_rows: usize,
    pub rows: DiscussionRows,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestComment {
    pub id: u64,
    pub html_url: String,
    pub body: Option<String>,
    pub user: Option<DiscussionUser>,
    pub author_association: Option<String>,
    pub reactions: Option<CommentReactions>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequestReview {
    pub id: u64,
    pub html_url: String,
    pub state: String,
    pub body: Option<String>,
    pub user: Option<DiscussionUser>,
    pub commit_id: Option<String>,
    pub submitted_at: Option<String>,
    pub author_association: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DiscussionUser {
    pub login: String,
    pub id: Option<u64>,
    pub profile_url: Option<String>,
    pub avatar_url: Option<String>,
    pub details: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct CommentReactions {
    pub total_count: u64,
    #[serde(rename = "+1")]
    pub plus_one: u64,
    #[serde(rename = "-1")]
    pub minus_one: u64,
    pub laugh: u64,
    pub confused: u64,
    pub heart: u64,
    pub hooray: u64,
    pub rocket: u64,
    pub eyes: u64,
}

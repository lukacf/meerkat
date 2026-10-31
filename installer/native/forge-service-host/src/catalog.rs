use forge_ghe_collections::*;
use serde_json::{Value, json};

/// Supplied by trusted application composition, not extracted from result text.
#[derive(Clone)]
pub struct GheRepository {
    pub host: String,
    pub owner: String,
    pub repository: String,
}
#[derive(Clone, Copy)]
pub struct ProgramLimits {
    pub calls: u16,
    pub result_bytes: usize,
}

/// Fixed compiled programs. There is deliberately no Deserialize implementation
/// or arbitrary predicate/operation/argument program accepted from a wire.
#[derive(Clone)]
pub enum ProgramSpec {
    PullRequestStatuses {
        number: u64,
        head: Option<String>,
    },
    PullRequestThreads {
        number: u64,
    },
    PullRequestChanges {
        number: u64,
        head: Option<String>,
    },
    PullRequestDiscussion {
        number: u64,
        kind: DiscussionKind,
    },
    History {
        head: Option<String>,
        since: String,
        until: String,
    },
    WorkflowRuns,
    WorkflowRunJobs {
        run_id: u64,
        attempt: u64,
        head: String,
    },
    Issues {
        query: IssueQuery,
    },
    IssueComments {
        number: u64,
    },
    IssueHierarchy {
        number: u64,
    },
    PullRequests {
        query: PullRequestQuery,
    },
}

pub(crate) enum Engine {
    Statuses(PullRequestStatuses),
    Threads(PullRequestReviewThreads),
    Changes(PullRequestChanges),
    Discussion(PullRequestDiscussion),
    History(RepositoryHistory),
    Runs(WorkflowRuns),
    Jobs(WorkflowRunJobs),
    Issues(RepositoryIssues),
    Comments(IssueComments),
    Hierarchy(IssueHierarchy),
    PullRequests(RepositoryPullRequests),
}

impl ProgramSpec {
    /// Decode only the compiled catalog's exact resource schema. This supplies
    /// intent, never an installed connection, authenticated target or receipt.
    /// Protected composition must still bind the actual installed configuration.
    pub fn from_resource(
        program_id: &str,
        resource: &Value,
    ) -> Result<(GheRepository, Self, ProgramLimits), crate::AdapterError> {
        use crate::AdapterError::InvalidSelection as bad;
        fn fields(value: &Value, keys: &[&str]) -> Result<(), crate::AdapterError> {
            let object = value.as_object().ok_or(bad)?;
            if object.len() != keys.len() || keys.iter().any(|key| !object.contains_key(*key)) {
                return Err(bad);
            }
            Ok(())
        }
        fn text(value: &Value) -> Result<String, crate::AdapterError> {
            value.as_str().map(str::to_owned).ok_or(bad)
        }
        fn optional_text(value: &Value) -> Result<Option<String>, crate::AdapterError> {
            if value.is_null() {
                Ok(None)
            } else {
                text(value).map(Some)
            }
        }
        fn number(value: &Value) -> Result<u64, crate::AdapterError> {
            value.as_u64().ok_or(bad)
        }
        fields(
            resource,
            &[
                "program",
                "host",
                "owner",
                "repository",
                "selector",
                "limits",
            ],
        )?;
        if resource["program"].as_str() != Some(program_id) {
            return Err(bad);
        }
        fields(&resource["limits"], &["calls", "result_bytes"])?;
        let limits = ProgramLimits {
            calls: u16::try_from(number(&resource["limits"]["calls"])?).map_err(|_| bad)?,
            result_bytes: usize::try_from(number(&resource["limits"]["result_bytes"])?)
                .map_err(|_| bad)?,
        };
        let repo = GheRepository {
            host: text(&resource["host"])?,
            owner: text(&resource["owner"])?,
            repository: text(&resource["repository"])?,
        };
        let s = &resource["selector"];
        let spec = match program_id {
            "ghe.pr_statuses.v1" | "ghe.pr_changes.v1" => {
                fields(s, &["number", "head"])?;
                let number = number(&s["number"])?;
                let head = optional_text(&s["head"])?;
                if program_id == "ghe.pr_statuses.v1" {
                    Self::PullRequestStatuses { number, head }
                } else {
                    Self::PullRequestChanges { number, head }
                }
            }
            "ghe.pr_threads.v1"
            | "ghe.pr_comments.v1"
            | "ghe.pr_reviews.v1"
            | "ghe.issue_comments.v1"
            | "ghe.issue_hierarchy.v1" => {
                fields(s, &["number"])?;
                let number = number(&s["number"])?;
                match program_id {
                    "ghe.pr_threads.v1" => Self::PullRequestThreads { number },
                    "ghe.pr_comments.v1" => Self::PullRequestDiscussion {
                        number,
                        kind: DiscussionKind::Comments,
                    },
                    "ghe.pr_reviews.v1" => Self::PullRequestDiscussion {
                        number,
                        kind: DiscussionKind::Reviews,
                    },
                    "ghe.issue_comments.v1" => Self::IssueComments { number },
                    _ => Self::IssueHierarchy { number },
                }
            }
            "ghe.history.v1" => {
                fields(s, &["head", "since", "until"])?;
                Self::History {
                    head: optional_text(&s["head"])?,
                    since: text(&s["since"])?,
                    until: text(&s["until"])?,
                }
            }
            "ghe.workflow_runs.v1" => {
                fields(s, &[])?;
                Self::WorkflowRuns
            }
            "ghe.workflow_jobs.v1" => {
                fields(s, &["run_id", "attempt", "head"])?;
                Self::WorkflowRunJobs {
                    run_id: number(&s["run_id"])?,
                    attempt: number(&s["attempt"])?,
                    head: text(&s["head"])?,
                }
            }
            "ghe.issues.v1" => {
                let query = match s["query"].as_str() {
                    Some("all") => {
                        fields(s, &["query"])?;
                        IssueQuery::All
                    }
                    Some("open") => {
                        fields(s, &["query"])?;
                        IssueQuery::Open
                    }
                    Some("updated_since") => {
                        fields(s, &["query", "since"])?;
                        IssueQuery::UpdatedSince(text(&s["since"])?)
                    }
                    _ => return Err(bad),
                };
                Self::Issues { query }
            }
            "ghe.pull_requests.v1" => {
                fields(s, &["query"])?;
                Self::PullRequests {
                    query: match s["query"].as_str() {
                        Some("all") => PullRequestQuery::All,
                        Some("open") => PullRequestQuery::Open,
                        Some("closed") => PullRequestQuery::Closed,
                        _ => return Err(bad),
                    },
                }
            }
            _ => return Err(bad),
        };
        // Existing constructors validate host, identities, time bounds, counts,
        // revisions and budgets. Exact equality also rejects ignored fields.
        spec.build(&repo, limits).map_err(|_| bad)?;
        if spec.resource(&repo, limits) != *resource {
            return Err(bad);
        }
        Ok((repo, spec, limits))
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::PullRequestStatuses { .. } => "ghe.pr_statuses.v1",
            Self::PullRequestThreads { .. } => "ghe.pr_threads.v1",
            Self::PullRequestChanges { .. } => "ghe.pr_changes.v1",
            Self::PullRequestDiscussion {
                kind: DiscussionKind::Comments,
                ..
            } => "ghe.pr_comments.v1",
            Self::PullRequestDiscussion {
                kind: DiscussionKind::Reviews,
                ..
            } => "ghe.pr_reviews.v1",
            Self::History { .. } => "ghe.history.v1",
            Self::WorkflowRuns => "ghe.workflow_runs.v1",
            Self::WorkflowRunJobs { .. } => "ghe.workflow_jobs.v1",
            Self::Issues { .. } => "ghe.issues.v1",
            Self::IssueComments { .. } => "ghe.issue_comments.v1",
            Self::IssueHierarchy { .. } => "ghe.issue_hierarchy.v1",
            Self::PullRequests { .. } => "ghe.pull_requests.v1",
        }
    }
    pub(crate) fn operations(&self) -> Vec<String> {
        match self {
            Self::PullRequestStatuses { .. }
            | Self::PullRequestThreads { .. }
            | Self::PullRequestChanges { .. }
            | Self::PullRequestDiscussion { .. } => vec!["pull_request_read"],
            Self::History { head: Some(_), .. } => vec!["get_commit", "list_commits"],
            Self::History { head: None, .. } => vec!["list_commits"],
            Self::WorkflowRuns => vec!["actions_list"],
            Self::WorkflowRunJobs { .. } => vec!["actions_get", "actions_list"],
            Self::Issues { .. } => vec!["list_issues"],
            Self::IssueComments { .. } | Self::IssueHierarchy { .. } => vec!["issue_read"],
            Self::PullRequests { .. } => vec!["list_pull_requests"],
        }
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
    pub(crate) fn resource(&self, repo: &GheRepository, limits: ProgramLimits) -> Value {
        let selector = match self {
            Self::PullRequestStatuses { number, head }
            | Self::PullRequestChanges { number, head } => json!({"number":number,"head":head}),
            Self::PullRequestThreads { number }
            | Self::PullRequestDiscussion { number, .. }
            | Self::IssueComments { number }
            | Self::IssueHierarchy { number } => json!({"number":number}),
            Self::History { head, since, until } => {
                json!({"head":head,"since":since,"until":until})
            }
            Self::WorkflowRuns => json!({}),
            Self::WorkflowRunJobs {
                run_id,
                attempt,
                head,
            } => json!({"run_id":run_id,"attempt":attempt,"head":head}),
            Self::Issues { query } => match query {
                IssueQuery::All => json!({"query":"all"}),
                IssueQuery::Open => json!({"query":"open"}),
                IssueQuery::UpdatedSince(since) => json!({"query":"updated_since","since":since}),
            },
            Self::PullRequests { query } => {
                json!({"query":match query {PullRequestQuery::All=>"all",PullRequestQuery::Open=>"open",PullRequestQuery::Closed=>"closed"}})
            }
        };
        json!({"program":self.id(),"host":repo.host,"owner":repo.owner,"repository":repo.repository,
            "selector":selector,"limits":{"calls":limits.calls,"result_bytes":limits.result_bytes}})
    }
    pub(crate) fn build(
        &self,
        repo: &GheRepository,
        limits: ProgramLimits,
    ) -> Result<Engine, Refusal> {
        if limits.calls == 0
            || limits.calls > 256
            || limits.result_bytes == 0
            || limits.result_bytes > 32 * 1024 * 1024
        {
            return Err(Refusal::InvalidTarget);
        }
        let (h, o, r) = (
            repo.host.as_str(),
            repo.owner.as_str(),
            repo.repository.as_str(),
        );
        Ok(match self {
            Self::PullRequestStatuses { number, head } => {
                if limits.calls < 2 {
                    return Err(Refusal::InvalidTarget);
                }
                Engine::Statuses(match head {
                    Some(head) => PullRequestStatuses::at_head(h, o, r, *number, head)?,
                    None => PullRequestStatuses::new(h, o, r, *number)?,
                })
            }
            Self::PullRequestThreads { number } => Engine::Threads(PullRequestReviewThreads::new(
                h,
                o,
                r,
                *number,
                ReviewReadLimits::new(limits.calls, limits.result_bytes)?,
            )?),
            Self::PullRequestChanges { number, head } => {
                let limits = ChangesReadLimits::new(limits.calls, limits.result_bytes)?;
                Engine::Changes(match head {
                    Some(head) => PullRequestChanges::at_head(h, o, r, *number, head, limits)?,
                    None => PullRequestChanges::new(h, o, r, *number, limits)?,
                })
            }
            Self::PullRequestDiscussion { number, kind } => {
                Engine::Discussion(PullRequestDiscussion::new(
                    h,
                    o,
                    r,
                    *number,
                    *kind,
                    DiscussionReadLimits::new(limits.calls, limits.result_bytes)?,
                )?)
            }
            Self::History { head, since, until } => {
                let window = HistoryWindow::new(since, until)?;
                let limits = HistoryReadLimits::new(limits.calls, limits.result_bytes)?;
                Engine::History(match head {
                    Some(head) => RepositoryHistory::at_head(h, o, r, head, window, limits)?,
                    None => RepositoryHistory::default_branch(h, o, r, window, limits)?,
                })
            }
            Self::WorkflowRuns => Engine::Runs(WorkflowRuns::new(
                h,
                o,
                r,
                ActionsReadLimits::new(limits.calls, limits.result_bytes)?,
            )?),
            Self::WorkflowRunJobs {
                run_id,
                attempt,
                head,
            } => Engine::Jobs(WorkflowRunJobs::new(
                h,
                o,
                r,
                *run_id,
                *attempt,
                head,
                ActionsReadLimits::new(limits.calls, limits.result_bytes)?,
            )?),
            Self::Issues { query } => Engine::Issues(RepositoryIssues::new(
                h,
                o,
                r,
                query.clone(),
                IssueReadLimits::new(limits.calls, limits.result_bytes)?,
            )?),
            Self::IssueComments { number } => Engine::Comments(IssueComments::new(
                h,
                o,
                r,
                *number,
                IssueReadLimits::new(limits.calls, limits.result_bytes)?,
            )?),
            Self::IssueHierarchy { number } => Engine::Hierarchy(IssueHierarchy::new(
                h,
                o,
                r,
                *number,
                IssueReadLimits::new(limits.calls, limits.result_bytes)?,
            )?),
            Self::PullRequests { query } => Engine::PullRequests(RepositoryPullRequests::new(
                h,
                o,
                r,
                *query,
                PullRequestListLimits::new(limits.calls, limits.result_bytes)?,
            )?),
        })
    }
}

impl Engine {
    pub(crate) fn next(&self) -> Option<&ReadCall> {
        match self {
            Self::Statuses(r) => r.next_call(),
            Self::Threads(r) => r.next_call(),
            Self::Changes(r) => r.next_call(),
            Self::Discussion(r) => r.next_call(),
            Self::History(r) => r.next_call(),
            Self::Runs(r) => r.next_call(),
            Self::Jobs(r) => r.next_call(),
            Self::Issues(r) => r.next_call(),
            Self::Comments(r) => r.next_call(),
            Self::Hierarchy(r) => r.next_call(),
            Self::PullRequests(r) => r.next_call(),
        }
    }
    pub(crate) fn observe(
        &mut self,
        call: &ReadCall,
        payload: &[u8],
    ) -> Result<Option<crate::HistoricalCollection>, Refusal> {
        use crate::HistoricalCollection as H;
        let (op, args) = (call.operation(), call.arguments());
        Ok(match self {
            Self::Statuses(r) => r.observe(op, args, payload)?.map(H::Statuses),
            Self::Threads(r) => r.observe(op, args, payload)?.map(H::Threads),
            Self::Changes(r) => r.observe(op, args, payload)?.map(H::Changes),
            Self::Discussion(r) => r.observe(op, args, payload)?.map(H::Discussion),
            Self::History(r) => r.observe(op, args, payload)?.map(H::Commits),
            Self::Runs(r) => r.observe(op, args, payload)?.map(H::Runs),
            Self::Jobs(r) => r.observe(op, args, payload)?.map(H::Jobs),
            Self::Issues(r) => r.observe(op, args, payload)?.map(H::Issues),
            Self::Comments(r) => r.observe(op, args, payload)?.map(H::IssueComments),
            Self::Hierarchy(r) => r.observe(op, args, payload)?.map(H::IssueHierarchy),
            Self::PullRequests(r) => r.observe(op, args, payload)?.map(H::PullRequests),
        })
    }
}

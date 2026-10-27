//! Bounded direct hierarchy reads; returned links never expand the read scope.
use super::*;

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct IssueParentReference {
    pub number: u64,
    pub repository: String,
    pub state: String,
    pub title: String,
    pub url: String,
}
impl IssueParentReference {
    fn check(&self, host: &str, source: &str) -> Result<(), Refusal> {
        let Some((owner, repo)) = self.repository.split_once('/') else {
            return Err(Refusal::WrongResource);
        };
        if !segment(owner) || !segment(repo) || self.number == 0 || self.number > i32::MAX as u64 {
            return Err(Refusal::WrongResource);
        }
        if self.url != format!("https://{host}/{owner}/{repo}/issues/{}", self.number)
            || self.url == source
        {
            return Err(Refusal::WrongResource);
        }
        if !matches!(self.state.as_str(), "OPEN" | "CLOSED") {
            return Err(Refusal::UnsupportedShape);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ChildSummary {
    total: u64,
    completed: u64,
    percent_completed: u64,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Relationships {
    has_parent: bool,
    has_children: bool,
    sub_issues_summary: Option<ChildSummary>,
    parent: Option<IssueParentReference>,
}
impl Relationships {
    fn read(issue: &IssueRecord, host: &str, source: &str) -> Result<Self, Refusal> {
        // Required bool fields distinguish failed enrichment from explicit absence.
        let value: Self =
            serde_json::from_value(json!(issue.details)).map_err(|_| Refusal::UnsupportedShape)?;
        match (&value.sub_issues_summary, value.has_children) {
            (Some(summary), true)
                if summary.total > 0
                    && summary.completed <= summary.total
                    && summary.percent_completed <= 100 => {}
            (None, false) => {}
            _ => return Err(Refusal::UnsupportedShape),
        }
        if let Some(parent) = &value.parent {
            if !value.has_parent {
                return Err(Refusal::UnsupportedShape);
            }
            parent.check(host, source)?;
        }
        Ok(value)
    }
    fn count(&self) -> u64 {
        self.sub_issues_summary.as_ref().map_or(0, |s| s.total)
    }
}

#[derive(Debug)]
enum Phase {
    Initial,
    Parent,
    Children(u16),
    Final,
    Finished,
}

#[derive(Debug)]
pub struct IssueHierarchy {
    host: String,
    repository_url: String,
    issue_url: String,
    number: u64,
    initial_call: ReadCall,
    pending: Option<ReadCall>,
    phase: Phase,
    budget: Budget,
    initial: Option<IssueRecord>,
    relationships: Option<Relationships>,
    parent: Option<IssueParentReference>,
    children: Vec<IssueRecord>,
    ids: BTreeSet<u64>,
    urls: BTreeSet<String>,
    pages_read: u16,
    terminal_page_rows: usize,
}
impl IssueHierarchy {
    pub fn new(
        host: &str,
        owner: &str,
        repo: &str,
        number: u64,
        limits: IssueReadLimits,
    ) -> Result<Self, Refusal> {
        if number == 0 || number > i32::MAX as u64 || limits.calls < 4 {
            return Err(Refusal::InvalidTarget);
        }
        let repository_url = repository(host, owner, repo)?;
        let call = ReadCall {
            operation: "issue_read",
            arguments: json!({"method":"get","owner":owner,"repo":repo,"issue_number":number}),
        };
        Ok(Self {
            host: host.into(),
            issue_url: format!("{repository_url}/issues/{number}"),
            repository_url,
            number,
            initial_call: call.clone(),
            pending: Some(call),
            phase: Phase::Initial,
            budget: Budget::new(limits),
            initial: None,
            relationships: None,
            parent: None,
            children: Vec::new(),
            ids: BTreeSet::new(),
            urls: BTreeSet::new(),
            pages_read: 0,
            terminal_page_rows: 0,
        })
    }
    pub fn next_call(&self) -> Option<&ReadCall> {
        self.pending.as_ref()
    }

    fn check_issue(&self, issue: &IssueRecord) -> Result<(), Refusal> {
        issue.check(&self.repository_url, true)?;
        if issue.number != self.number {
            return Err(Refusal::WrongResource);
        }
        Ok(())
    }

    pub fn observe(
        &mut self,
        operation: &str,
        arguments: &Value,
        payload: &[u8],
    ) -> Result<Option<ReportedIssueHierarchy>, Refusal> {
        let call = self.pending.take().ok_or(Refusal::AlreadyFinished)?;
        matching(&call, operation, arguments)?;
        self.budget.consume(payload)?;
        let phase = std::mem::replace(&mut self.phase, Phase::Finished);
        let mut next = self.initial_call.clone();
        match phase {
            Phase::Initial => {
                let issue: IssueRecord = decode(payload)?;
                self.check_issue(&issue)?;
                let relationships = Relationships::read(&issue, &self.host, &self.issue_url)?;
                // Include one short terminal page, even at an exact multiple of 100.
                if relationships.count() / PAGE_SIZE as u64 + 1
                    > u64::from(self.budget.limits.calls - 3)
                {
                    return Err(Refusal::BudgetExceeded);
                }
                self.initial = Some(issue);
                self.relationships = Some(relationships);
                next.arguments["method"] = json!("get_parent");
                self.phase = Phase::Parent;
            }
            Phase::Parent => {
                // A missing key is malformed, not the provider's explicit null.
                let value: Value = decode(payload)?;
                let parent = value
                    .as_object()
                    .and_then(|v| v.get("parent"))
                    .ok_or(Refusal::UnsupportedShape)?;
                let parent: Option<IssueParentReference> =
                    serde_json::from_value(parent.clone())
                        .map_err(|_| Refusal::UnsupportedShape)?;
                let initial = self
                    .relationships
                    .as_ref()
                    .ok_or(Refusal::UnsupportedShape)?;
                if initial.has_parent != parent.is_some() {
                    // Null can also mean lockdown filtering. Never report no parent
                    // when the independent structural signal says one exists.
                    return Err(Refusal::IncompleteCollection);
                }
                if let Some(parent) = &parent {
                    parent.check(&self.host, &self.issue_url)?;
                }
                if initial
                    .parent
                    .as_ref()
                    .is_some_and(|p| Some(p) != parent.as_ref())
                {
                    return Err(Refusal::CollectionChanged);
                }
                self.parent = parent;
                next.arguments["method"] = json!("get_sub_issues");
                next.arguments["page"] = json!(1);
                next.arguments["perPage"] = json!(PAGE_SIZE);
                self.phase = Phase::Children(1);
            }
            Phase::Children(page) => {
                // Go's nil slice encodes as null. It is an empty page, but the
                // independent structural count must still agree before finishing.
                let rows: Option<Vec<IssueRecord>> = decode(payload)?;
                let rows = rows.unwrap_or_default();
                if rows.len() > PAGE_SIZE {
                    return Err(Refusal::UnsupportedShape);
                }
                for row in &rows {
                    let url = row.html_url.as_ref().ok_or(Refusal::UnsupportedShape)?;
                    let Some(path) = url.strip_prefix(&format!("https://{}/", self.host)) else {
                        return Err(Refusal::WrongResource);
                    };
                    let parts: Vec<_> = path.split('/').collect();
                    if parts.len() != 4
                        || parts[2] != "issues"
                        || !segment(parts[0])
                        || !segment(parts[1])
                        || url == &self.issue_url
                    {
                        return Err(Refusal::WrongResource);
                    }
                    row.check(
                        &format!("https://{}/{}/{}", self.host, parts[0], parts[1]),
                        true,
                    )?;
                    // The pinned REST representation can omit database IDs.
                    // Canonical repository/number URLs still identify each row.
                    if let Some(value) = row.details.get("id") {
                        let id = value.as_u64().ok_or(Refusal::UnsupportedShape)?;
                        if id == 0 || id > i64::MAX as u64 {
                            return Err(Refusal::UnsupportedShape);
                        }
                        if !self.ids.insert(id) {
                            return Err(Refusal::DuplicateItem);
                        }
                    }
                    if !self.urls.insert(url.clone()) {
                        return Err(Refusal::DuplicateItem);
                    }
                }
                let count = rows.len();
                self.children.extend(rows);
                let total = self
                    .relationships
                    .as_ref()
                    .ok_or(Refusal::UnsupportedShape)?
                    .count();
                if self.children.len() as u64 > total
                    || (count < PAGE_SIZE && self.children.len() as u64 != total)
                {
                    return Err(Refusal::IncompleteCollection);
                }
                if count < PAGE_SIZE {
                    self.pages_read = page;
                    self.terminal_page_rows = count;
                    self.phase = Phase::Final;
                } else {
                    // A next children page must leave room for the final detail.
                    if self.budget.calls + 1 >= self.budget.limits.calls {
                        return Err(Refusal::BudgetExceeded);
                    }
                    next.arguments["method"] = json!("get_sub_issues");
                    next.arguments["page"] = json!(page + 1);
                    next.arguments["perPage"] = json!(PAGE_SIZE);
                    self.phase = Phase::Children(page + 1);
                }
            }
            Phase::Final => {
                let issue: IssueRecord = decode(payload)?;
                self.check_issue(&issue)?;
                let final_relationships = Relationships::read(&issue, &self.host, &self.issue_url)?;
                let initial = self.initial.take().ok_or(Refusal::UnsupportedShape)?;
                let relationships = self.relationships.take().ok_or(Refusal::UnsupportedShape)?;
                if initial.updated_at != issue.updated_at
                    || initial.state != issue.state
                    || relationships != final_relationships
                    || final_relationships
                        .parent
                        .as_ref()
                        .is_some_and(|p| Some(p) != self.parent.as_ref())
                {
                    return Err(Refusal::CollectionChanged);
                }
                return Ok(Some(ReportedIssueHierarchy {
                    initial_issue: initial,
                    final_issue: issue,
                    parent: self.parent.take(),
                    children: std::mem::take(&mut self.children),
                    reported_children: relationships.count(),
                    pages_read: self.pages_read,
                    terminal_page_rows: self.terminal_page_rows,
                }));
            }
            Phase::Finished => return Err(Refusal::AlreadyFinished),
        }
        self.budget.next()?;
        self.pending = Some(next);
        Ok(None)
    }
}

/// Historical direct links between matching detail/count/routing observations.
/// A None parent means explicit has_parent=false plus a null parent response.
/// Missing enrichment, filtered references and incomplete children refuse.
/// This is not an atomic snapshot, transitive closure or permission to follow
/// returned links. Closing-PR references remain capped optional IssueRecord data.
#[derive(Debug)]
pub struct ReportedIssueHierarchy {
    pub initial_issue: IssueRecord,
    pub final_issue: IssueRecord,
    pub parent: Option<IssueParentReference>,
    pub children: Vec<IssueRecord>,
    pub reported_children: u64,
    pub pages_read: u16,
    pub terminal_page_rows: usize,
}

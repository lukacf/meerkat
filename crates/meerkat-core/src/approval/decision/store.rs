//! The durable decision store contract and its in-memory store.
//!
//! A store keeps one row per decision and answers lazy reads: one row by id,
//! or one page of an index. Nothing is read at open and nothing scans every
//! row: the queries a host's owner runs when it starts (owed executions and
//! notices, due expiries) grow with the rows that owe work, never with all
//! rows.

use std::collections::BTreeMap;
use std::ops::Bound;

use parking_lot::RwLock;

use super::{
    ApproverId, DecisionId, DecisionKind, DecisionNotice, DecisionPhase, DecisionPrincipalKey,
    StoredDecision,
};
use crate::approval::ApprovalStoreError;

/// Most rows one page holds; a larger limit is read as this one.
pub const DECISION_PAGE_MAX: usize = 500;

/// A row as a store read it: decoded, or kept unreadable for its decision
/// only. An unreadable row is listed as such and never executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionRow {
    Readable(Box<StoredDecision>),
    Unreadable(UnreadableDecision),
}

impl DecisionRow {
    /// The id the row is stored under.
    #[must_use]
    pub fn decision_id(&self) -> &str {
        match self {
            Self::Readable(row) => row.decision_id.as_str(),
            Self::Unreadable(row) => &row.decision_id,
        }
    }
}

/// A stored row this binary cannot read: undecodable, or torn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableDecision {
    /// The id the row is stored under.
    pub decision_id: String,
    pub reason: String,
}

/// One page of rows, in decision id order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionPage {
    pub rows: Vec<DecisionRow>,
    /// The id to read the next page after, when there may be more rows.
    pub next_after: Option<String>,
}

/// Which rows a listing reads. Every set field must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionListFilter {
    pub kind: Option<DecisionKind>,
    pub phase: Option<DecisionPhase>,
    pub approver_id: Option<ApproverId>,
    pub requester: Option<DecisionPrincipalKey>,
}

impl DecisionListFilter {
    /// Whether `row` matches every field this filter sets.
    #[must_use]
    pub fn matches(&self, row: &StoredDecision) -> bool {
        self.kind
            .as_ref()
            .is_none_or(|kind| *kind == row.descriptor.kind)
            && self.phase.is_none_or(|phase| phase == row.state.phase)
            && self
                .approver_id
                .as_ref()
                .is_none_or(|approver_id| *approver_id == row.descriptor.approver_id)
            && self
                .requester
                .as_ref()
                .is_none_or(|requester| *requester == row.descriptor.requester)
    }
}

/// Whether `row` owes its owner work: an approved decision to execute (or
/// expire), or a notice to deliver.
#[must_use]
pub fn decision_owes_work(row: &StoredDecision) -> bool {
    row.state.phase == DecisionPhase::Approved || row.state.notice == DecisionNotice::Owed
}

/// Whether `row` is due an expiry observation at `now_ms`: open or approved,
/// with its deadline at or before `now_ms`.
#[must_use]
pub fn decision_is_due(row: &StoredDecision, now_ms: u64) -> bool {
    matches!(
        row.state.phase,
        DecisionPhase::Open | DecisionPhase::Approved
    ) && row.descriptor.deadline_at_ms <= now_ms
}

/// The store of durable decision rows.
///
/// Stores persist whole rows and decide no lifecycle legality: the owner
/// service rebuilds each row's generated machine and writes the state the
/// machine left. Every write is conditional, which makes the store the one
/// commit domain for every owner of the same backing, and never waits for
/// another owner: a held backing is [`ApprovalStoreError::Contended`] and
/// nothing is written.
pub trait DecisionStore: Send + Sync {
    /// The row of one decision.
    ///
    /// # Errors
    /// Store failures; an undecodable row is [`DecisionRow::Unreadable`].
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError>;

    /// Insert `row` if no row has its id. A row with its id is
    /// [`ApprovalStoreError::Conflict`], and nothing is written.
    ///
    /// # Errors
    /// The commitment outcome of a write that did not commit.
    fn insert_if_absent(&self, row: &StoredDecision) -> Result<(), ApprovalStoreError>;

    /// Replace the row of `row.decision_id` if its stored revision is
    /// `expected_revision`. Any other stored row (or none) is
    /// [`ApprovalStoreError::Conflict`], and nothing is written.
    ///
    /// # Errors
    /// The commitment outcome of a write that did not commit.
    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), ApprovalStoreError>;

    /// One page of at most `limit` rows that match `filter`, after the id
    /// `after`.
    ///
    /// # Errors
    /// Store failures.
    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError>;

    /// One page of the rows that owe work ([`decision_owes_work`]), after
    /// `after`.
    ///
    /// # Errors
    /// Store failures.
    fn owed(&self, after: Option<&str>, limit: usize) -> Result<DecisionPage, ApprovalStoreError>;

    /// One page of the rows due an expiry observation at `now_ms`
    /// ([`decision_is_due`]), after `after`.
    ///
    /// # Errors
    /// Store failures.
    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError>;

    /// How many rows match `filter`.
    ///
    /// # Errors
    /// Store failures.
    fn count(&self, filter: &DecisionListFilter) -> Result<u64, ApprovalStoreError>;

    /// Whether rows survive a restart.
    fn is_persistent(&self) -> bool;
}

/// In-memory decision store for tests and process-local hosts.
#[derive(Debug, Default)]
pub struct InMemoryDecisionStore {
    rows: RwLock<BTreeMap<String, StoredDecision>>,
}

impl InMemoryDecisionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn page(
        &self,
        after: Option<&str>,
        limit: usize,
        keep: impl Fn(&StoredDecision) -> bool,
    ) -> DecisionPage {
        let limit = limit.clamp(1, DECISION_PAGE_MAX);
        let rows = self.rows.read();
        let start = after.map_or(Bound::Unbounded, |after| Bound::Excluded(after.to_owned()));
        let mut page = DecisionPage::default();
        for row in rows.range((start, Bound::Unbounded)).map(|(_, row)| row) {
            if !keep(row) {
                continue;
            }
            if page.rows.len() == limit {
                page.next_after = page.rows.last().map(|last| last.decision_id().to_owned());
                break;
            }
            page.rows.push(DecisionRow::Readable(Box::new(row.clone())));
        }
        page
    }
}

impl DecisionStore for InMemoryDecisionStore {
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError> {
        Ok(self
            .rows
            .read()
            .get(decision_id.as_str())
            .map(|row| DecisionRow::Readable(Box::new(row.clone()))))
    }

    /// Never waits: a map held by another owner is
    /// [`ApprovalStoreError::Contended`].
    fn insert_if_absent(&self, row: &StoredDecision) -> Result<(), ApprovalStoreError> {
        let Some(mut rows) = self.rows.try_write() else {
            return Err(ApprovalStoreError::Contended);
        };
        if rows.contains_key(row.decision_id.as_str()) {
            return Err(ApprovalStoreError::Conflict);
        }
        rows.insert(row.decision_id.as_str().to_owned(), row.clone());
        Ok(())
    }

    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), ApprovalStoreError> {
        let Some(mut rows) = self.rows.try_write() else {
            return Err(ApprovalStoreError::Contended);
        };
        match rows.get_mut(row.decision_id.as_str()) {
            Some(stored) if stored.revision == expected_revision => {
                *stored = row.clone();
                Ok(())
            }
            _ => Err(ApprovalStoreError::Conflict),
        }
    }

    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        Ok(self.page(after, limit, |row| filter.matches(row)))
    }

    fn owed(&self, after: Option<&str>, limit: usize) -> Result<DecisionPage, ApprovalStoreError> {
        Ok(self.page(after, limit, decision_owes_work))
    }

    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        Ok(self.page(after, limit, |row| decision_is_due(row, now_ms)))
    }

    fn count(&self, filter: &DecisionListFilter) -> Result<u64, ApprovalStoreError> {
        let matching = self
            .rows
            .read()
            .values()
            .filter(|row| filter.matches(row))
            .count();
        Ok(u64::try_from(matching).unwrap_or(u64::MAX))
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

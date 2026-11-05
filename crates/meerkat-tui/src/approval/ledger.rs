//! Durable record of human escalations.
//!
//! Every escalation the engine puts to the human is first requested from
//! Meerkat's [`ApprovalService`] over a [`FileApprovalStore`]
//! (`<realm>/tool-approvals.json`): owner = the session, resource = the tool
//! call, risk by action kind, expiry = the escalation timeout. The human's
//! answer (or a timeout, or the call being abandoned) decides the record.
//!
//! A record never authorizes anything by itself: only the exact waiting
//! dispatch consumes the human's answer. A process that ends while a record
//! is pending cannot resume that dispatch, so [`EscalationLedger::open`]
//! denies every record still pending from an earlier process.
//!
//! "Allow always" rules do not live here: they go to the realm rules file
//! (`tui.toml`), as before.

use std::path::Path;
use std::time::{Duration, SystemTime};

use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalId, ApprovalListFilter, ApprovalOwnerRef,
    ApprovalPrincipalId, ApprovalProposedAction, ApprovalRecord, ApprovalRequest,
    ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalService,
    ApprovalStatus, SessionId, SurfaceMetadata,
};
use meerkat_store::FileApprovalStore;
use serde_json::Value;

use super::policy::{Action, ActionKind};

/// File in the realm directory holding the escalation records.
pub const LEDGER_FILE: &str = "tool-approvals.json";

/// Principal that requests escalations (the TUI's approval gate).
const REQUESTER: &str = "rkat-tui-approval-gate";
/// Principal of decisions the human made.
const HUMAN: &str = "rkat-tui-user";
/// Principal of decisions made without the human (timeouts, abandoned
/// calls, restarts).
const RUNTIME: &str = "rkat-tui-runtime";

/// Failures of the escalation ledger.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The store file could not be opened.
    #[error("cannot open the approval ledger: {0}")]
    Store(#[from] meerkat_core::ApprovalStoreError),
    /// The approval service refused an operation.
    #[error("approval ledger: {0}")]
    Service(#[from] meerkat_core::ApprovalError),
}

/// Who decided an escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decider {
    /// The human answered.
    Human,
    /// The TUI decided without the human (timeout, abandoned call, restart).
    Runtime,
}

impl Decider {
    fn principal(self) -> Result<ApprovalPrincipalId, LedgerError> {
        let name = match self {
            Self::Human => HUMAN,
            Self::Runtime => RUNTIME,
        };
        Ok(ApprovalPrincipalId::new(name)?)
    }
}

/// One escalation to record.
#[derive(Debug, Clone, Copy)]
pub struct EscalationEntry<'a> {
    /// Session that made the call.
    pub session_id: &'a SessionId,
    /// Tool-use id of the call.
    pub tool_use_id: &'a str,
    /// The parsed call.
    pub action: &'a Action<'a>,
    /// How long the human has to answer.
    pub timeout: Duration,
    /// The classifier's opinion shown with the escalation, if any.
    pub hint: Option<&'a str>,
}

/// The escalation records of one realm.
#[derive(Clone)]
pub struct EscalationLedger {
    service: ApprovalService,
}

impl std::fmt::Debug for EscalationLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EscalationLedger")
            .field("persistent", &self.service.is_persistent())
            .finish()
    }
}

impl EscalationLedger {
    /// Open the ledger in `realm_root`, denying every record an earlier
    /// process left pending.
    ///
    /// # Errors
    ///
    /// [`LedgerError`] when the store cannot be opened or read.
    pub fn open(realm_root: &Path) -> Result<Self, LedgerError> {
        let store = FileApprovalStore::open(realm_root.join(LEDGER_FILE))?;
        let ledger = Self {
            service: ApprovalService::with_store(std::sync::Arc::new(store))?,
        };
        let pending = ledger.service.list(ApprovalListFilter {
            status: Some(ApprovalStatus::Pending),
        })?;
        for record in pending {
            ledger.decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                Decider::Runtime,
                "rkat-tui ended before this request was answered; the tool call did not run",
            )?;
        }
        Ok(ledger)
    }

    /// A ledger kept in memory only (tests, or a realm without a directory).
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            service: ApprovalService::new(),
        }
    }

    /// Record a pending escalation.
    ///
    /// # Errors
    ///
    /// [`LedgerError`] when the service refuses the request.
    pub fn request(&self, entry: EscalationEntry<'_>) -> Result<ApprovalId, LedgerError> {
        let (kind, risk) = classify(entry.action);
        let request = ApprovalRequest {
            requester: ApprovalPrincipalId::new(REQUESTER)?,
            owner: ApprovalOwnerRef::Session {
                session_id: entry.session_id.clone(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::ToolCall,
                id: ApprovalResourceId::new(entry.tool_use_id),
            },
            proposed_action: ApprovalProposedAction {
                kind,
                summary: format!("Run {}", entry.action.name),
                body: Some(serde_json::json!({
                    "tool": entry.action.name,
                    "arguments": entry.action.args,
                })),
            },
            risk,
            request_body: entry.hint.map(|hint| Value::String(hint.to_string())),
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect(),
            expires_at: SystemTime::now().checked_add(entry.timeout).map(Into::into),
            metadata: SurfaceMetadata::default(),
            request_provenance: None,
        };
        Ok(self.service.request(request)?.approval_id)
    }

    /// Decide a pending record.
    ///
    /// # Errors
    ///
    /// [`LedgerError`] when the record is unknown or already decided.
    pub fn decide(
        &self,
        id: &ApprovalId,
        decision: ApprovalDecision,
        by: Decider,
        reason: &str,
    ) -> Result<ApprovalRecord, LedgerError> {
        Ok(self.service.decide(
            id,
            decision,
            by.principal()?,
            Some(reason.to_string()),
            None,
        )?)
    }

    /// Every record, oldest first.
    ///
    /// # Errors
    ///
    /// [`LedgerError`] when the store cannot be read.
    pub fn records(&self) -> Result<Vec<ApprovalRecord>, LedgerError> {
        let mut records = self.service.list(ApprovalListFilter::default())?;
        records.sort_by_key(|record| record.created_at);
        Ok(records)
    }
}

/// Action kind and risk of an escalated call: shell commands are high risk
/// (they can do anything the user can), file writes medium, anything else
/// (unknown tools, malformed arguments) medium.
fn classify(action: &Action<'_>) -> (ApprovalActionKind, ApprovalRisk) {
    match action.kind {
        ActionKind::Shell { .. } => (ApprovalActionKind::ShellCommand, ApprovalRisk::High),
        ActionKind::FileWrite { .. } => (ApprovalActionKind::FileWrite, ApprovalRisk::Medium),
        _ => (ApprovalActionKind::ToolCall, ApprovalRisk::Medium),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_requests_and_decisions_and_denies_leftovers_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let args = json!({"command": "git push --force"});
        let action = Action::parse("shell", &args);
        let entry = |id| EscalationEntry {
            session_id: &session,
            tool_use_id: id,
            action: &action,
            timeout: Duration::from_mins(5),
            hint: Some("classifier: BLOCK [D2]"),
        };

        let ledger = EscalationLedger::open(dir.path()).unwrap();
        let answered = ledger.request(entry("call_1")).unwrap();
        let left = ledger.request(entry("call_2")).unwrap();
        let record = ledger
            .decide(
                &answered,
                ApprovalDecision::Approve,
                Decider::Human,
                "allowed once",
            )
            .unwrap();
        assert_eq!(record.status, ApprovalStatus::Approved);
        assert_eq!(record.risk, ApprovalRisk::High);
        assert!(record.expires_at.is_some());
        assert!(matches!(
            &record.owner,
            ApprovalOwnerRef::Session { session_id } if session_id == &session
        ));
        drop(ledger);

        // A new process: the record left pending is denied, the decided one kept.
        let reopened = EscalationLedger::open(dir.path()).unwrap();
        let records = reopened.records().unwrap();
        assert_eq!(records.len(), 2);
        let status = |id: &ApprovalId| {
            records
                .iter()
                .find(|r| &r.approval_id == id)
                .map(|r| r.status)
        };
        assert_eq!(status(&answered), Some(ApprovalStatus::Approved));
        assert_eq!(status(&left), Some(ApprovalStatus::Denied));
    }

    #[test]
    fn writes_are_medium_risk() {
        let args = json!({"path": "/etc/hosts", "content": "x"});
        let (kind, risk) = classify(&Action::parse("write_file", &args));
        assert_eq!(
            (kind, risk),
            (ApprovalActionKind::FileWrite, ApprovalRisk::Medium)
        );
    }
}

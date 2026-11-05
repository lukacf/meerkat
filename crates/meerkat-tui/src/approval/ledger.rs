//! Durable record of human escalations.
//!
//! Every escalation the engine puts to the human is first requested from
//! the realm's one approval issuer: the [`ApprovalService`] the persistence
//! bundle opened over the realm's canonical `approvals.json`
//! (`PersistenceBundle::approval_issuer`, selected once per host by
//! `meerkat::ApprovalIssuerSelection`; the TUI never opens a second owner
//! of that file). Owner = the session, resource = the tool call, requester
//! = the TUI's gate, risk by action kind, expiry = the escalation timeout.
//! The human's answer (or a timeout, or the call being abandoned) decides
//! the record.
//!
//! A record never authorizes anything by itself: only the exact waiting
//! dispatch consumes the human's answer. A process that ends while a record
//! is pending cannot resume that dispatch, so when this process opens a
//! session, [`EscalationLedger::deny_leftovers`] denies that session's
//! records the TUI's gate left pending, and nothing else in the realm.
//!
//! The engine fails closed on the ledger: an escalation whose request
//! cannot be recorded, or whose answer cannot be recorded, is a denial.
//!
//! "Allow always" rules do not live here: they go to the realm rules file
//! (`tui.toml`), as before.


use std::time::{Duration, SystemTime};

use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalId, ApprovalListFilter, ApprovalOwnerRef,
    ApprovalPrincipalId, ApprovalProposedAction, ApprovalRecord, ApprovalRequest,
    ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalService,
    ApprovalStatus, SessionId, SurfaceMetadata,
};
use serde_json::Value;

use super::policy::{Action, ActionKind};

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
    /// The ledger over the realm's approval issuer.
    #[must_use]
    pub fn new(service: ApprovalService) -> Self {
        Self { service }
    }

    /// Deny every record the TUI's gate left pending for `session_id` (an
    /// earlier process ended before its human answered; the dispatch it
    /// waited for is gone). Records of other sessions, and records other
    /// requesters made, are left alone. Returns how many were denied.
    ///
    /// # Errors
    ///
    /// [`LedgerError`] when the records cannot be read or decided.
    pub fn deny_leftovers(&self, session_id: &SessionId) -> Result<usize, LedgerError> {
        let requester = ApprovalPrincipalId::new(REQUESTER)?;
        let pending = self.service.list(ApprovalListFilter {
            status: Some(ApprovalStatus::Pending),
        })?;
        let mut denied = 0;
        for record in pending {
            let ours = record.requester == requester
                && matches!(&record.owner, ApprovalOwnerRef::Session { session_id: owner } if owner == session_id);
            if !ours {
                continue;
            }
            self.decide(
                &record.approval_id,
                ApprovalDecision::Deny,
                Decider::Runtime,
                "rkat-tui ended before this request was answered; the tool call did not run",
            )?;
            denied += 1;
        }
        Ok(denied)
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
        self.submit(Self::prepare(entry)?)
    }

    /// The approval request for `entry` (no I/O: [`Self::submit`] it, off
    /// the async executor when the store is a file).
    ///
    /// # Errors
    ///
    /// [`LedgerError`] when a principal id is invalid.
    pub fn prepare(entry: EscalationEntry<'_>) -> Result<ApprovalRequest, LedgerError> {
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
                    "arguments": serde_json::from_str::<Value>(entry.action.args.get())
                        .unwrap_or_else(|_| Value::String(entry.action.args.get().to_string())),
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
        Ok(request)
    }

    /// Record a prepared request.
    ///
    /// # Errors
    ///
    /// [`LedgerError`] when the service refuses the request.
    pub fn submit(&self, request: ApprovalRequest) -> Result<ApprovalId, LedgerError> {
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
    use crate::approval::catalog::ToolOrigin;
    use meerkat_core::ToolSourceKind;
    use serde_json::json;
    use serde_json::value::RawValue;

    fn raw(value: &Value) -> Box<RawValue> {
        serde_json::value::to_raw_value(value).unwrap()
    }

    fn service(path: &std::path::Path) -> ApprovalService {
        meerkat::ApprovalIssuerSelection::Persistent {
            store_path: path.to_path_buf(),
        }
        .open()
    }

    #[test]
    fn records_requests_and_decisions_and_denies_only_its_own_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let other_session = SessionId::new();
        let args = raw(&json!({"command": "git push --force"}));
        let origin = ToolOrigin::native(ToolSourceKind::Shell);
        let action = Action::parse("shell", &args, Some(&origin));
        let entry = |session_id, id| EscalationEntry {
            session_id,
            tool_use_id: id,
            action: &action,
            timeout: Duration::from_mins(5),
            hint: Some("classifier: BLOCK [D2]"),
        };

        let ledger = EscalationLedger::new(service(dir.path()));
        let answered = ledger.request(entry(&session, "call_1")).unwrap();
        let left = ledger.request(entry(&session, "call_2")).unwrap();
        let elsewhere = ledger.request(entry(&other_session, "call_3")).unwrap();
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
        drop(ledger);

        // A record another requester (another host) left pending in the realm.
        let service = service(dir.path());
        let foreign = service
            .request(ApprovalRequest {
                requester: ApprovalPrincipalId::new("some-other-host").unwrap(),
                owner: ApprovalOwnerRef::Session {
                    session_id: session.clone(),
                },
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::ToolCall,
                    id: ApprovalResourceId::new("call_9"),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::ToolCall,
                    summary: "x".into(),
                    body: None,
                },
                risk: ApprovalRisk::Low,
                request_body: None,
                allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                    .into_iter()
                    .collect(),
                expires_at: None,
                metadata: SurfaceMetadata::default(),
                request_provenance: None,
            })
            .unwrap()
            .approval_id;

        // A new process opens `session`: only its own gate's leftover is denied.
        let reopened = EscalationLedger::new(service);
        assert_eq!(reopened.deny_leftovers(&session).unwrap(), 1);
        assert_eq!(reopened.deny_leftovers(&session).unwrap(), 0);
        let records = reopened.records().unwrap();
        let status = |id: &ApprovalId| {
            records
                .iter()
                .find(|r| &r.approval_id == id)
                .map(|r| r.status)
        };
        assert_eq!(status(&answered), Some(ApprovalStatus::Approved));
        assert_eq!(status(&left), Some(ApprovalStatus::Denied));
        assert_eq!(status(&elsewhere), Some(ApprovalStatus::Pending));
        assert_eq!(status(&foreign), Some(ApprovalStatus::Pending));
        // The records live in the realm's canonical approval store.
        assert!(dir.path().join("approvals.json").exists());
        assert!(!dir.path().join("tool-approvals.json").exists());
    }

    #[test]
    fn writes_are_medium_risk() {
        let args = raw(&json!({"path": "/etc/hosts", "content": "x"}));
        let origin = ToolOrigin::tui(crate::tools::FILE_TOOLS_SOURCE, "write_file");
        let (kind, risk) = classify(&Action::parse("write_file", &args, Some(&origin)));
        assert_eq!(
            (kind, risk),
            (ApprovalActionKind::FileWrite, ApprovalRisk::Medium)
        );
    }
}

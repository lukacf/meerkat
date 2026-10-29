//! Projection of scoped tool grants onto their wire contracts. It lives in
//! the runtime, beside the grant service, so every surface reports a grant,
//! a listing and an audit row the same way.

use meerkat_contracts::{
    WireToolGrant, WireToolGrantAuditEvent, WireToolGrantAuditRow, WireToolGrantExpiryDriver,
    WireToolGrantIssuer, WireToolGrantRefusalKind, WireToolGrantStatus, WireToolGrantSubject,
    WireToolGrantTarget, WireUnreadableToolGrant,
};
use meerkat_core::tool_grant::{ToolGrantIssuer, ToolGrantSubject, ToolGrantTarget};

use super::audit::{
    ToolGrantAuditEvent, ToolGrantAuditRecord, ToolGrantExpiryDriver, ToolGrantRefusalKind,
};
use super::service::{StoredToolGrant, ToolGrantStatus, UnreadableToolGrant};

/// One grant as its row stands.
#[must_use]
pub fn wire_tool_grant(grant: &StoredToolGrant) -> WireToolGrant {
    WireToolGrant {
        grant_id: grant.record.grant_id.to_string(),
        subject: wire_tool_grant_subject(&grant.record.subject),
        target: wire_tool_grant_target(&grant.record.target),
        issuer: wire_tool_grant_issuer(&grant.record.issuer),
        reason: grant.record.reason.clone(),
        status: wire_tool_grant_status(grant.status),
        issued_at_ms: grant.issued_at_ms,
        expires_at_ms: grant.expires_at_ms,
        ended_at_ms: grant.ended_at_ms,
    }
}

/// A grant's subject. An identity subject names its mob and identity; the
/// mob incarnation it was issued in stays a server fact.
#[must_use]
pub fn wire_tool_grant_subject(subject: &ToolGrantSubject) -> WireToolGrantSubject {
    match subject {
        ToolGrantSubject::Session { session_id } => WireToolGrantSubject::Session {
            session_id: session_id.to_string(),
        },
        ToolGrantSubject::Identity {
            mob_id, identity, ..
        } => WireToolGrantSubject::Identity {
            mob_id: mob_id.clone(),
            identity: identity.clone(),
        },
    }
}

/// A grant's target. A family target carries the names its grant took, in
/// name order.
#[must_use]
pub fn wire_tool_grant_target(target: &ToolGrantTarget) -> WireToolGrantTarget {
    match target {
        ToolGrantTarget::Tool { name } => WireToolGrantTarget::Tool {
            name: name.to_string(),
        },
        ToolGrantTarget::Family { family, names } => WireToolGrantTarget::Family {
            family: family.clone(),
            names: names.iter().map(ToString::to_string).collect(),
        },
    }
}

/// Who issued a grant.
#[must_use]
pub fn wire_tool_grant_issuer(issuer: &ToolGrantIssuer) -> WireToolGrantIssuer {
    match issuer {
        ToolGrantIssuer::Operator { caller } => WireToolGrantIssuer::Operator {
            caller: caller.clone(),
        },
        ToolGrantIssuer::Approval { approval_id } => WireToolGrantIssuer::Approval {
            approval_id: approval_id.clone(),
        },
    }
}

/// A grant's status, as its row records it.
#[must_use]
pub fn wire_tool_grant_status(status: ToolGrantStatus) -> WireToolGrantStatus {
    match status {
        ToolGrantStatus::Active => WireToolGrantStatus::Active,
        ToolGrantStatus::Expired => WireToolGrantStatus::Expired,
        ToolGrantStatus::Revoked => WireToolGrantStatus::Revoked,
    }
}

/// A grant row this binary cannot read.
#[must_use]
pub fn wire_unreadable_tool_grant(unreadable: &UnreadableToolGrant) -> WireUnreadableToolGrant {
    WireUnreadableToolGrant {
        grant_id: unreadable.grant_id.clone(),
        detail: unreadable.detail.clone(),
    }
}

/// One audit row with its place in its subject's log.
#[must_use]
pub fn wire_tool_grant_audit_row(record: &ToolGrantAuditRecord) -> WireToolGrantAuditRow {
    WireToolGrantAuditRow {
        sequence: record.sequence,
        at_ms: record.row.at_ms,
        event: wire_tool_grant_audit_event(&record.row.event),
    }
}

fn wire_tool_grant_audit_event(event: &ToolGrantAuditEvent) -> WireToolGrantAuditEvent {
    match event {
        ToolGrantAuditEvent::Granted {
            grant_id,
            target,
            issuer,
            reason,
            expires_at_ms,
        } => WireToolGrantAuditEvent::Granted {
            grant_id: grant_id.to_string(),
            target: wire_tool_grant_target(target),
            issuer: wire_tool_grant_issuer(issuer),
            reason: reason.clone(),
            expires_at_ms: *expires_at_ms,
        },
        ToolGrantAuditEvent::Expired {
            grant_id,
            expires_at_ms,
            observed_at_ms,
            driver,
        } => WireToolGrantAuditEvent::Expired {
            grant_id: grant_id.to_string(),
            expires_at_ms: *expires_at_ms,
            observed_at_ms: *observed_at_ms,
            driver: wire_tool_grant_expiry_driver(*driver),
        },
        ToolGrantAuditEvent::Revoked {
            grant_id,
            observed_at_ms,
            actor,
            reason,
        } => WireToolGrantAuditEvent::Revoked {
            grant_id: grant_id.to_string(),
            observed_at_ms: *observed_at_ms,
            actor: actor.clone(),
            reason: reason.clone(),
        },
        ToolGrantAuditEvent::Refused {
            tool,
            reason,
            grant_id,
            call_id,
            run_id,
        } => WireToolGrantAuditEvent::Refused {
            tool: tool.to_string(),
            reason: wire_tool_grant_refusal_kind(*reason),
            grant_id: grant_id.as_ref().map(ToString::to_string),
            call_id: call_id.clone(),
            run_id: run_id.clone(),
        },
    }
}

fn wire_tool_grant_expiry_driver(driver: ToolGrantExpiryDriver) -> WireToolGrantExpiryDriver {
    match driver {
        ToolGrantExpiryDriver::Timer => WireToolGrantExpiryDriver::Timer,
        ToolGrantExpiryDriver::Boundary => WireToolGrantExpiryDriver::Boundary,
        ToolGrantExpiryDriver::Dispatch => WireToolGrantExpiryDriver::Dispatch,
        ToolGrantExpiryDriver::Read => WireToolGrantExpiryDriver::Read,
        ToolGrantExpiryDriver::Revoke => WireToolGrantExpiryDriver::Revoke,
    }
}

fn wire_tool_grant_refusal_kind(kind: ToolGrantRefusalKind) -> WireToolGrantRefusalKind {
    match kind {
        ToolGrantRefusalKind::Required => WireToolGrantRefusalKind::Required,
        ToolGrantRefusalKind::Expired => WireToolGrantRefusalKind::Expired,
        ToolGrantRefusalKind::Revoked => WireToolGrantRefusalKind::Revoked,
        ToolGrantRefusalKind::Unavailable => WireToolGrantRefusalKind::Unavailable,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::tool_grant::{ToolGrantId, ToolGrantRecord};
    use meerkat_core::types::{SessionId, ToolName};
    use std::collections::BTreeSet;

    use crate::tool_grant::ToolGrantAuditRow;

    #[test]
    fn a_stored_grant_and_its_audit_rows_project_onto_their_wire_contracts() {
        let grant_id = ToolGrantId::mint();
        let session_id = SessionId::new();
        let grant = StoredToolGrant {
            record: ToolGrantRecord {
                grant_id: grant_id.clone(),
                subject: ToolGrantSubject::session(session_id.clone()),
                target: ToolGrantTarget::Family {
                    family: "shell".to_string(),
                    names: BTreeSet::from([ToolName::from("shell_jobs"), ToolName::from("shell")]),
                },
                issuer: ToolGrantIssuer::Operator {
                    caller: Some("ops".to_string()),
                },
                reason: Some("incident".to_string()),
            },
            status: ToolGrantStatus::Revoked,
            issued_at_ms: 1_000,
            expires_at_ms: 61_000,
            ended_at_ms: Some(2_000),
        };
        assert_eq!(
            serde_json::to_value(wire_tool_grant(&grant)).unwrap(),
            serde_json::json!({
                "grant_id": grant_id.to_string(),
                "subject": {"kind": "session", "session_id": session_id.to_string()},
                "target": {"kind": "family", "family": "shell", "names": ["shell", "shell_jobs"]},
                "issuer": {"kind": "operator", "caller": "ops"},
                "reason": "incident",
                "status": "revoked",
                "issued_at_ms": 1000,
                "expires_at_ms": 61000,
                "ended_at_ms": 2000,
            })
        );

        let row = ToolGrantAuditRecord {
            sequence: 3,
            row: ToolGrantAuditRow {
                at_ms: 70_000,
                event: ToolGrantAuditEvent::Refused {
                    tool: ToolName::from("shell"),
                    reason: ToolGrantRefusalKind::Expired,
                    grant_id: Some(grant_id.clone()),
                    call_id: "call-1".to_string(),
                    run_id: None,
                },
            },
        };
        assert_eq!(
            serde_json::to_value(wire_tool_grant_audit_row(&row)).unwrap(),
            serde_json::json!({
                "sequence": 3,
                "at_ms": 70000,
                "event": {
                    "kind": "refused",
                    "tool": "shell",
                    "reason": "expired",
                    "grant_id": grant_id.to_string(),
                    "call_id": "call-1",
                },
            })
        );
    }
}
